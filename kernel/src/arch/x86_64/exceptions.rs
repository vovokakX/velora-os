//! CPU exception handling and the fatal exception report.

use super::cpu;
use super::idt::InterruptFrame;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const NAMES: [(&str, &str); 32] = [
    ("#DE", "Divide error"),
    ("#DB", "Debug"),
    ("NMI", "Non-maskable interrupt"),
    ("#BP", "Breakpoint"),
    ("#OF", "Overflow"),
    ("#BR", "Bound range exceeded"),
    ("#UD", "Invalid opcode"),
    ("#NM", "Device not available"),
    ("#DF", "Double fault"),
    ("---", "Coprocessor segment overrun"),
    ("#TS", "Invalid TSS"),
    ("#NP", "Segment not present"),
    ("#SS", "Stack-segment fault"),
    ("#GP", "General protection fault"),
    ("#PF", "Page fault"),
    ("---", "Reserved"),
    ("#MF", "x87 floating-point error"),
    ("#AC", "Alignment check"),
    ("#MC", "Machine check"),
    ("#XM", "SIMD floating-point error"),
    ("#VE", "Virtualization exception"),
    ("#CP", "Control protection"),
    ("---", "Reserved"),
    ("---", "Reserved"),
    ("---", "Reserved"),
    ("---", "Reserved"),
    ("---", "Reserved"),
    ("---", "Reserved"),
    ("#HV", "Hypervisor injection"),
    ("#VC", "VMM communication"),
    ("#SX", "Security exception"),
    ("---", "Reserved"),
];

pub const DIVIDE_ERROR: u64 = 0;
pub const BREAKPOINT: u64 = 3;
pub const INVALID_OPCODE: u64 = 6;
pub const GENERAL_PROTECTION: u64 = 13;
pub const PAGE_FAULT: u64 = 14;

pub fn name(vector: u64) -> (&'static str, &'static str) {
    NAMES
        .get(vector as usize)
        .copied()
        .unwrap_or(("???", "Unknown"))
}

static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);

/// Number of `int3` breakpoints handled so far.
pub fn breakpoint_count() -> u64 {
    BREAKPOINTS.load(Ordering::SeqCst)
}

// --- Recovery hook ---------------------------------------------------------
// Lets kernel tests provoke an exception on purpose and continue at a known
// address. Later this becomes the mechanism behind safe user-memory copies.

const NONE: u64 = u64::MAX;
static RECOVER_VECTOR: AtomicU64 = AtomicU64::new(NONE);
static RECOVER_RIP: AtomicU64 = AtomicU64::new(0);
static CAUGHT_VECTOR: AtomicU64 = AtomicU64::new(NONE);
static CAUGHT_ERROR: AtomicU64 = AtomicU64::new(0);
static CAUGHT_CR2: AtomicU64 = AtomicU64::new(0);

/// Arms a one-shot recovery: the next exception with `vector` resumes at
/// `resume_rip` instead of being fatal.
pub fn arm_recovery(vector: u64, resume_rip: u64) {
    CAUGHT_VECTOR.store(NONE, Ordering::SeqCst);
    RECOVER_RIP.store(resume_rip, Ordering::SeqCst);
    RECOVER_VECTOR.store(vector, Ordering::SeqCst);
}

/// Address of the resume-RIP slot, so inline assembly can store a local
/// label into it right before the faulting instruction.
pub fn recover_rip_slot() -> *mut u64 {
    RECOVER_RIP.as_ptr()
}

/// Disarms the hook and returns (vector, error code, CR2) if it fired.
pub fn take_caught() -> Option<(u64, u64, u64)> {
    RECOVER_VECTOR.store(NONE, Ordering::SeqCst);
    let v = CAUGHT_VECTOR.swap(NONE, Ordering::SeqCst);
    if v == NONE {
        None
    } else {
        Some((
            v,
            CAUGHT_ERROR.load(Ordering::SeqCst),
            CAUGHT_CR2.load(Ordering::SeqCst),
        ))
    }
}

static IN_FATAL: AtomicBool = AtomicBool::new(false);

pub fn handle(frame: &mut InterruptFrame) {
    let cr2 = if frame.vector == PAGE_FAULT {
        cpu::read_cr2()
    } else {
        0
    };

    if RECOVER_VECTOR.load(Ordering::SeqCst) == frame.vector {
        RECOVER_VECTOR.store(NONE, Ordering::SeqCst);
        CAUGHT_ERROR.store(frame.error_code, Ordering::SeqCst);
        CAUGHT_CR2.store(cr2, Ordering::SeqCst);
        CAUGHT_VECTOR.store(frame.vector, Ordering::SeqCst);
        frame.rip = RECOVER_RIP.load(Ordering::SeqCst);
        return;
    }

    if frame.vector == BREAKPOINT {
        BREAKPOINTS.fetch_add(1, Ordering::SeqCst);
        crate::kinfo!("breakpoint at {:#018x}", frame.rip.wrapping_sub(1));
        return;
    }

    if frame.cs & 3 == 3 {
        user_fault(frame, cr2);
    }

    fatal(frame, cr2);
}

/// A user program caused an exception: report it and end the process.
/// The kernel keeps running.
fn user_fault(frame: &InterruptFrame, cr2: u64) -> ! {
    let (mnemonic, name) = name(frame.vector);
    let pid = crate::sched::current_pid();
    let pname = crate::sched::current_name();
    if frame.vector == PAGE_FAULT {
        crate::kwarn!(
            "process {} ({}) killed: {} {} at rip {:#x}, address {:#x} ({})",
            pid,
            pname,
            mnemonic,
            name,
            frame.rip,
            cr2,
            describe_page_fault(frame.error_code)
        );
    } else {
        crate::kwarn!(
            "process {} ({}) killed: {} {} at rip {:#x}, error code {:#x}",
            pid,
            pname,
            mnemonic,
            name,
            frame.rip,
            frame.error_code
        );
    }
    crate::sched::exit_process(velora_abi::fault_exit_status(frame.vector as u8))
}

fn fatal(frame: &InterruptFrame, cr2: u64) -> ! {
    cpu::disable_interrupts();
    if IN_FATAL.swap(true, Ordering::SeqCst) {
        // A fault while printing the report: print the bare minimum.
        crate::console::emergency(|w| {
            let _ = writeln!(
                w,
                "\nnested exception {} at {:#x}, halting",
                frame.vector, frame.rip
            );
        });
        crate::arch::fatal_exit();
    }
    crate::console::emergency(|w| {
        let _ = write_report(w, frame, cr2);
    });
    crate::arch::fatal_exit();
}

/// Human-readable description of a page fault error code.
pub fn describe_page_fault(error: u64) -> &'static str {
    let present = error & 1 != 0;
    let write = error & 2 != 0;
    let user = error & 4 != 0;
    let reserved = error & 8 != 0;
    let fetch = error & 16 != 0;
    match (reserved, fetch, present, write, user) {
        (true, ..) => "reserved bit set in a page table entry",
        (false, true, false, _, false) => "instruction fetch from a non-present page (kernel)",
        (false, true, false, _, true) => "instruction fetch from a non-present page (user)",
        (false, true, true, _, _) => "instruction fetch from a no-execute page",
        (false, false, false, false, false) => "read from a non-present page (kernel)",
        (false, false, false, true, false) => "write to a non-present page (kernel)",
        (false, false, false, false, true) => "read from a non-present page (user)",
        (false, false, false, true, true) => "write to a non-present page (user)",
        (false, false, true, true, false) => "write to a read-only page (kernel)",
        (false, false, true, true, true) => "write to a read-only page (user)",
        (false, false, true, false, true) => "user access to a kernel page",
        (false, false, true, false, false) => "protection violation on read (kernel)",
    }
}

/// Writes the full exception report. Separate from the halt path so that
/// tests can render it into a buffer and check the contents.
pub fn write_report(w: &mut dyn Write, f: &InterruptFrame, cr2: u64) -> fmt::Result {
    let (mnemonic, name) = name(f.vector);
    let mode = if f.cs & 3 == 3 { "user" } else { "kernel" };
    writeln!(w)?;
    writeln!(
        w,
        "\x1b[1;31m==================== CPU EXCEPTION ====================\x1b[0m"
    )?;
    writeln!(
        w,
        "{} {} (vector {}) in {} mode",
        mnemonic, name, f.vector, mode
    )?;
    writeln!(w, "error code: {:#06x}", f.error_code)?;
    if f.vector == PAGE_FAULT {
        writeln!(w, "cause:      {}", describe_page_fault(f.error_code))?;
        writeln!(w, "address:    CR2={:#018x}", cr2)?;
    }
    writeln!(
        w,
        "RIP={:#018x} CS={:#06x} RFLAGS={:#018x}",
        f.rip, f.cs, f.rflags
    )?;
    writeln!(w, "RSP={:#018x} SS={:#06x}", f.rsp, f.ss)?;
    writeln!(
        w,
        "RAX={:#018x} RBX={:#018x} RCX={:#018x}",
        f.rax, f.rbx, f.rcx
    )?;
    writeln!(
        w,
        "RDX={:#018x} RSI={:#018x} RDI={:#018x}",
        f.rdx, f.rsi, f.rdi
    )?;
    writeln!(
        w,
        "RBP={:#018x} R8 ={:#018x} R9 ={:#018x}",
        f.rbp, f.r8, f.r9
    )?;
    writeln!(
        w,
        "R10={:#018x} R11={:#018x} R12={:#018x}",
        f.r10, f.r11, f.r12
    )?;
    writeln!(
        w,
        "R13={:#018x} R14={:#018x} R15={:#018x}",
        f.r13, f.r14, f.r15
    )?;
    writeln!(
        w,
        "CR0={:#018x} CR3={:#018x} CR4={:#018x}",
        cpu::read_cr0(),
        cpu::read_cr3(),
        cpu::read_cr4()
    )?;
    crate::panic::write_backtrace(w, f.rip, f.rbp)?;
    writeln!(w, "System halted.")
}

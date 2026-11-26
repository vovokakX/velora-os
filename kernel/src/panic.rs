//! Kernel panic handler and stack backtraces.

use crate::arch::cpu;
use core::fmt::{self, Write};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

static PANICKING: AtomicBool = AtomicBool::new(false);

const MAX_FRAMES: usize = 24;
/// Frame pointers must be in the higher half (kernel image, kernel stacks
/// or the HHDM); anything else means the chain is broken.
const KERNEL_SPACE: u64 = 0xffff_8000_0000_0000;

/// Walks the frame-pointer chain starting at `rbp`. `rip` is printed as
/// frame #0. Addresses can be resolved with
/// `addr2line -e target/x86_64-unknown-none/release/velora <addr>`.
pub fn write_backtrace(w: &mut dyn Write, rip: u64, mut rbp: u64) -> fmt::Result {
    writeln!(w, "backtrace:")?;
    writeln!(w, "  #0  {:#018x}", rip)?;
    for i in 1..MAX_FRAMES {
        if rbp < KERNEL_SPACE || !rbp.is_multiple_of(8) {
            break;
        }
        // SAFETY: rbp passed the plausibility checks above; the kernel is
        // built with frame pointers, so [rbp] = previous rbp and
        // [rbp + 8] = return address.
        let (next, ret) = unsafe { (*(rbp as *const u64), *((rbp + 8) as *const u64)) };
        if ret == 0 {
            break;
        }
        writeln!(w, "  #{:<2} {:#018x}", i, ret)?;
        if next <= rbp {
            break; // stack grows down, so the chain must go up
        }
        rbp = next;
    }
    Ok(())
}

pub fn write_report(w: &mut dyn Write, info: &PanicInfo, rbp: u64) -> fmt::Result {
    writeln!(w)?;
    writeln!(
        w,
        "\x1b[1;31m===================== KERNEL PANIC =====================\x1b[0m"
    )?;
    writeln!(w, "message:  {}", info.message())?;
    match info.location() {
        Some(l) => writeln!(w, "location: {}:{}:{}", l.file(), l.line(), l.column())?,
        None => writeln!(w, "location: unknown")?,
    }
    writeln!(w, "RSP={:#018x} RBP={:#018x}", cpu::read_rsp(), rbp)?;
    writeln!(
        w,
        "CR0={:#018x} CR2={:#018x} CR3={:#018x} CR4={:#018x}",
        cpu::read_cr0(),
        cpu::read_cr2(),
        cpu::read_cr3(),
        cpu::read_cr4()
    )?;
    // Frame #0 is the panic handler itself, so print the caller chain.
    write_backtrace(w, panic_rip(), rbp)?;
    writeln!(w, "System halted.")
}

#[inline(never)]
fn panic_rip() -> u64 {
    let rip: u64;
    // SAFETY: reads the current instruction pointer.
    unsafe {
        core::arch::asm!("lea {}, [rip]", out(reg) rip, options(nomem, nostack, preserves_flags))
    }
    rip
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    cpu::disable_interrupts();
    let rbp = cpu::read_rbp();
    if PANICKING.swap(true, Ordering::SeqCst) {
        crate::console::emergency(|w| {
            let _ = writeln!(w, "\npanic while panicking: {}", info.message());
        });
        crate::arch::fatal_exit();
    }
    crate::console::emergency(|w| {
        let _ = write_report(w, info, rbp);
    });
    crate::crashlog::save();

    #[cfg(feature = "ktest")]
    crate::ktest::on_panic();

    crate::arch::fatal_exit()
}

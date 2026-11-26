//! Interrupt Descriptor Table and the low-level entry stubs.
//!
//! Every vector has a 16-byte stub in `isr_stubs`. Each stub pushes a dummy
//! error code (if the CPU did not push one) and the vector number, then jumps
//! to `isr_common`, which saves all general-purpose registers and calls
//! [`interrupt_dispatch`] with a pointer to the resulting [`InterruptFrame`].

use super::gdt;
use crate::sync::RacyCell;
use core::arch::{asm, global_asm};
use core::mem::size_of;

/// Register state saved on interrupt entry. Field order matches the pushes
/// in `isr_common` (last pushed = lowest address = first field).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct InterruptFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error_code: u64,
    // Pushed by the CPU.
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

const STUB_SIZE: usize = 16;

global_asm!(
    r#"
    .section .text.isr, "ax", @progbits
    .balign 16
    .global isr_stubs
isr_stubs:
    .set vec, 0
    .rept 256
        .balign 16
        .set has_err, 0
        .if vec == 8
            .set has_err, 1
        .endif
        .if vec == 10
            .set has_err, 1
        .endif
        .if vec == 11
            .set has_err, 1
        .endif
        .if vec == 12
            .set has_err, 1
        .endif
        .if vec == 13
            .set has_err, 1
        .endif
        .if vec == 14
            .set has_err, 1
        .endif
        .if vec == 17
            .set has_err, 1
        .endif
        .if vec == 21
            .set has_err, 1
        .endif
        .if vec == 29
            .set has_err, 1
        .endif
        .if vec == 30
            .set has_err, 1
        .endif
        .if has_err == 0
            pushq $0
        .endif
        pushq $vec
        jmp isr_common
        .set vec, vec + 1
    .endr

isr_common:
    # Coming from ring 3 (CS at 24(%rsp) has RPL 3): switch GS to the
    # kernel's per-CPU data. Returning to ring 3 swaps it back below.
    testb $3, 24(%rsp)
    jz 1f
    swapgs
1:
    pushq %rax
    pushq %rbx
    pushq %rcx
    pushq %rdx
    pushq %rsi
    pushq %rdi
    pushq %rbp
    pushq %r8
    pushq %r9
    pushq %r10
    pushq %r11
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, %rdi
    cld
    call interrupt_dispatch
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %r11
    popq %r10
    popq %r9
    popq %r8
    popq %rbp
    popq %rdi
    popq %rsi
    popq %rdx
    popq %rcx
    popq %rbx
    popq %rax
    addq $16, %rsp
    testb $3, 8(%rsp)
    jz 2f
    swapgs
2:
    iretq
"#,
    options(att_syntax)
);

unsafe extern "C" {
    static isr_stubs: [u8; 0];
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_lo: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_hi: u32,
    _zero: u32,
}

impl IdtEntry {
    const EMPTY: Self = Self {
        offset_lo: 0,
        selector: 0,
        ist: 0,
        type_attr: 0,
        offset_mid: 0,
        offset_hi: 0,
        _zero: 0,
    };

    fn new(handler: u64, ist: u8, dpl: u8) -> Self {
        Self {
            offset_lo: handler as u16,
            selector: gdt::KERNEL_CODE,
            ist,
            // present | DPL | 64-bit interrupt gate (IF cleared on entry)
            type_attr: 0x80 | (dpl << 5) | 0x0e,
            offset_mid: (handler >> 16) as u16,
            offset_hi: (handler >> 32) as u32,
            _zero: 0,
        }
    }
}

static IDT: RacyCell<[IdtEntry; 256]> = RacyCell::new([IdtEntry::EMPTY; 256]);

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

pub fn init() {
    // SAFETY: runs once on the boot CPU with interrupts disabled.
    unsafe {
        let base = (&raw const isr_stubs) as u64;
        let idt = &mut *IDT.get();
        for (v, entry) in idt.iter_mut().enumerate() {
            let ist = match v {
                2 => gdt::IST_NMI,
                8 => gdt::IST_DOUBLE_FAULT,
                18 => gdt::IST_MACHINE_CHECK,
                _ => 0,
            };
            *entry = IdtEntry::new(base + (v * STUB_SIZE) as u64, ist, 0);
        }
    }
    load();
}

/// Loads the (shared) IDT on this CPU.
pub fn load() {
    // SAFETY: the IDT is a static filled by `init` on the boot CPU.
    unsafe {
        let idt = &*IDT.get();
        let ptr = DescriptorPointer {
            limit: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: idt.as_ptr() as u64,
        };
        asm!("lidt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));
    }
}

/// Returns the (limit, base) of the loaded IDT.
pub fn current() -> (u16, u64) {
    let mut buf = [0u8; 10];
    // SAFETY: `sidt` stores 10 bytes into the buffer.
    unsafe { asm!("sidt [{}]", in(reg) buf.as_mut_ptr(), options(nostack, preserves_flags)) }
    let limit = u16::from_le_bytes([buf[0], buf[1]]);
    let base = u64::from_le_bytes(buf[2..10].try_into().unwrap());
    (limit, base)
}

/// Called from `isr_common` for every interrupt and exception.
#[unsafe(no_mangle)]
extern "C" fn interrupt_dispatch(frame: &mut InterruptFrame) {
    // NMI, double fault and machine check never take the big kernel
    // lock: they end the system (and an NMI from a panicking CPU stops
    // this one).
    if matches!(frame.vector, 2 | 8 | 18) {
        #[cfg(feature = "ktest")]
        if frame.vector == 2 && crate::smp::answer_where(frame.rip, frame.rbp, frame.rflags) {
            return;
        }
        if crate::panic::panicking() {
            super::cpu::halt_forever();
        }
        super::exceptions::handle(frame);
        return;
    }
    #[cfg(feature = "ktest")]
    if frame.vector == 0x20 && super::percpu::index() == 0 {
        crate::ktest::watchdog();
    }
    crate::smp::bkl_enter();
    if frame.vector < 32 {
        super::exceptions::handle(frame);
    } else {
        super::irq::dispatch(frame);
    }
    if frame.cs & 3 == 3 {
        crate::sched::leave_if_killed();
    }
    crate::smp::bkl_exit();
}

//! x86_64 architecture support.

pub mod apic;
pub mod context;
pub mod cpu;
pub mod exceptions;
pub mod fpu;
pub mod gdt;
pub mod hpet;
pub mod idt;
pub mod ioapic;
pub mod irq;
pub mod percpu;
pub mod pic;
pub mod pit;
pub mod pmtimer;
pub mod port;
pub mod qemu;
pub mod serial;
pub mod syscall;

/// Early CPU setup: our own GDT/TSS and the IDT with exception handlers.
pub fn init() {
    gdt::init();
    idt::init();
    percpu::init();
    fpu::init();
    syscall::init();
}

/// Ends the system after a fatal error. Test builds report failure to QEMU
/// so the test runner does not wait for a timeout.
pub fn fatal_exit() -> ! {
    #[cfg(feature = "ktest")]
    qemu::exit(qemu::ExitCode::Failure);
    #[allow(unreachable_code)]
    cpu::halt_forever()
}

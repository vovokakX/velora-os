//! x86_64 architecture support.

pub mod cpu;
pub mod exceptions;
pub mod gdt;
pub mod idt;
pub mod port;
pub mod qemu;
pub mod serial;

/// Early CPU setup: our own GDT/TSS and the IDT with exception handlers.
pub fn init() {
    gdt::init();
    idt::init();
}

/// Ends the system after a fatal error. Test builds report failure to QEMU
/// so the test runner does not wait for a timeout.
pub fn fatal_exit() -> ! {
    #[cfg(feature = "ktest")]
    qemu::exit(qemu::ExitCode::Failure);
    #[allow(unreachable_code)]
    cpu::halt_forever()
}

//! QEMU `isa-debug-exit` device, used only by the test runner.
//!
//! QEMU is started with `-device isa-debug-exit,iobase=0xf4,iosize=0x04`.
//! Writing value `v` makes QEMU exit with status `(v << 1) | 1`.

use super::port::outl;

#[derive(Clone, Copy)]
#[repr(u32)]
pub enum ExitCode {
    /// QEMU exit status 33.
    Success = 0x10,
    /// QEMU exit status 35.
    Failure = 0x11,
}

pub fn exit(code: ExitCode) -> ! {
    // SAFETY: on real hardware port 0xf4 is unused; the write is harmless.
    unsafe { outl(0xf4, code as u32) }
    super::cpu::halt_forever()
}

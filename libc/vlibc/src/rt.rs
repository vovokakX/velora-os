//! Program startup and the panic handler.

use crate::{env, process};
use core::arch::naked_asm;
use core::panic::PanicInfo;

unsafe extern "Rust" {
    /// Defined by the program through `vlibc::main!`.
    fn __vlibc_main() -> i32;
}

/// Process entry. The kernel starts here with RSP pointing at `argc`
/// (System V layout: argc, argv[], NULL, envp[], NULL, auxv).
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        "xor ebp, ebp",  // end of the frame-pointer chain
        "mov rdi, rsp",  // pointer to argc
        "and rsp, -16",
        "call {start}",
        "ud2",
        start = sym start_rust,
    )
}

extern "C" fn start_rust(sp: *const u64) -> ! {
    // SAFETY: the kernel built this stack layout.
    unsafe { env::init(sp) };
    // SAFETY: the program defines `__vlibc_main` with `vlibc::main!`.
    let code = unsafe { __vlibc_main() };
    process::exit(code)
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::eprintln!("panic: {}", info);
    process::exit(101)
}

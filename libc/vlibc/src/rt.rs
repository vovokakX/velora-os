//! Program startup and the panic handler.

use crate::{env, process};
use core::arch::naked_asm;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicUsize, Ordering};

/// The program's panic hook (a `fn(&PanicInfo)`), 0 for none.
static PANIC_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Sets a function the panic handler calls after printing the message
/// and before the program ends with status 101, for example to write the
/// message to the program's own log.
pub fn set_panic_hook(hook: fn(&PanicInfo)) {
    PANIC_HOOK.store(hook as usize, Ordering::Relaxed);
}

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
    let hook = PANIC_HOOK.swap(0, Ordering::Relaxed);
    if hook != 0 {
        // SAFETY: only `set_panic_hook` stores here, from a
        // `fn(&PanicInfo)`; swapping to 0 runs it once even if it panics.
        let hook: fn(&PanicInfo) = unsafe { core::mem::transmute(hook) };
        hook(info);
    }
    process::exit(101)
}

//! Raw system calls. See `velora_abi` for numbers and conventions.

use core::arch::asm;

/// # Safety
/// The arguments must be valid for the given system call.
#[inline]
pub unsafe fn syscall0(nr: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall", inlateout("rax") nr as i64 => ret, out("rcx") _, out("r11") _, options(nostack))
    }
    ret
}

/// # Safety
/// See [`syscall0`].
#[inline]
pub unsafe fn syscall1(nr: u64, a0: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall", inlateout("rax") nr as i64 => ret, in("rdi") a0,
             out("rcx") _, out("r11") _, options(nostack))
    }
    ret
}

/// # Safety
/// See [`syscall0`].
#[inline]
pub unsafe fn syscall2(nr: u64, a0: u64, a1: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall", inlateout("rax") nr as i64 => ret, in("rdi") a0, in("rsi") a1,
             out("rcx") _, out("r11") _, options(nostack))
    }
    ret
}

/// # Safety
/// See [`syscall0`].
#[inline]
pub unsafe fn syscall3(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall", inlateout("rax") nr as i64 => ret, in("rdi") a0, in("rsi") a1, in("rdx") a2,
             out("rcx") _, out("r11") _, options(nostack))
    }
    ret
}

/// # Safety
/// See [`syscall0`].
#[inline]
pub unsafe fn syscall4(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall", inlateout("rax") nr as i64 => ret, in("rdi") a0, in("rsi") a1, in("rdx") a2,
             in("r10") a3, out("rcx") _, out("r11") _, options(nostack))
    }
    ret
}

/// # Safety
/// See [`syscall0`].
#[inline]
pub unsafe fn syscall6(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall", inlateout("rax") nr as i64 => ret, in("rdi") a0, in("rsi") a1, in("rdx") a2,
             in("r10") a3, in("r8") a4, in("r9") a5, out("rcx") _, out("r11") _, options(nostack))
    }
    ret
}

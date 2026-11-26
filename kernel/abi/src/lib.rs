//! The Velora native ABI, shared by the kernel and user space (vlibc).
//!
//! Calling convention: `syscall` with the number in `rax`, arguments in
//! `rdi, rsi, rdx, r10, r8, r9`. The result comes back in `rax`: a value
//! `>= 0` on success or `-errno` on failure. `rcx` and `r11` are clobbered.
//! See docs/syscalls.md.

#![cfg_attr(not(test), no_std)]

/// System call numbers.
pub mod nr {
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const YIELD: u64 = 3;
    pub const GETPID: u64 = 4;
    /// spawn(path, path_len, argv: *const StrRef, argc, envp: *const StrRef, envc) -> pid
    pub const SPAWN: u64 = 5;
    /// wait(pid, status: *mut i64) -> pid
    pub const WAIT: u64 = 6;
    /// mmap(addr, len, prot, flags) -> addr
    pub const MMAP: u64 = 7;
    /// munmap(addr, len) -> 0
    pub const MUNMAP: u64 = 8;
    pub const CLOCK_GETTIME: u64 = 21;
    pub const SLEEP_MS: u64 = 25;
}

/// Error numbers (returned negated).
pub mod errno {
    pub const EPERM: i64 = 1;
    pub const ENOENT: i64 = 2;
    pub const ESRCH: i64 = 3;
    pub const EINTR: i64 = 4;
    pub const EIO: i64 = 5;
    pub const E2BIG: i64 = 7;
    pub const ENOEXEC: i64 = 8;
    pub const EBADF: i64 = 9;
    pub const ECHILD: i64 = 10;
    pub const EAGAIN: i64 = 11;
    pub const ENOMEM: i64 = 12;
    pub const EFAULT: i64 = 14;
    pub const EEXIST: i64 = 17;
    pub const EINVAL: i64 = 22;
    pub const ENOSYS: i64 = 38;
}

/// Clock ids for `clock_gettime`.
pub mod clock {
    /// Time since boot, never goes backwards.
    pub const MONOTONIC: u64 = 1;
}

/// `mmap` protection bits. Writable and executable together is refused
/// (W^X); PROT_NONE mappings are not supported.
pub mod prot {
    pub const READ: u64 = 1;
    pub const WRITE: u64 = 2;
    pub const EXEC: u64 = 4;
}

/// `mmap` flags.
pub mod map {
    /// Map exactly at `addr`; fails with EEXIST if a page there is in use.
    pub const FIXED: u64 = 1;
}

/// A string passed to the kernel (`spawn` arguments and environment).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StrRef {
    pub ptr: u64,
    pub len: u64,
}

/// Limits for `spawn`.
pub const MAX_ARGS: usize = 256;
pub const MAX_ARG_BYTES: usize = 64 * 1024;

/// Auxiliary vector keys placed on the initial stack.
pub mod auxv {
    pub const NULL: u64 = 0;
    pub const PAGESZ: u64 = 6;
    pub const ENTRY: u64 = 9;
}

/// Standard file descriptors (until the VFS in stage 7 they are fixed:
/// 0 = keyboard, 1 and 2 = console).
pub const STDIN: u64 = 0;
pub const STDOUT: u64 = 1;
pub const STDERR: u64 = 2;

/// `struct timespec` as written by `clock_gettime`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timespec {
    pub seconds: u64,
    pub nanoseconds: u64,
}

/// Exit status of a process killed by a CPU exception: 128 + vector,
/// like shells report signals.
pub const fn fault_exit_status(vector: u8) -> i64 {
    128 + vector as i64
}

/// Lowest address user programs may use.
pub const USER_MIN: u64 = 0x0000_0000_0040_0000;
/// One past the highest user address. The last page below the canonical
/// hole stays unmapped so `sysret` never returns to a non-canonical RIP.
pub const USER_MAX: u64 = 0x0000_7fff_ffff_f000;

const _: () = assert!(USER_MAX.is_multiple_of(4096));
const _: () = assert!(USER_MIN < USER_MAX && USER_MAX < 0x0000_8000_0000_0000);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_constants() {
        assert_eq!(core::mem::size_of::<Timespec>(), 16);
        assert_eq!(fault_exit_status(14), 142);
    }

    #[test]
    fn syscall_numbers_are_unique() {
        let all = [
            nr::EXIT,
            nr::WRITE,
            nr::READ,
            nr::YIELD,
            nr::GETPID,
            nr::SPAWN,
            nr::WAIT,
            nr::MMAP,
            nr::MUNMAP,
            nr::CLOCK_GETTIME,
            nr::SLEEP_MS,
        ];
        for (i, a) in all.iter().enumerate() {
            assert!(!all[i + 1..].contains(a), "duplicate syscall number {a}");
        }
    }
}

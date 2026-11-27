//! vlibc: the runtime library for Velora user programs.
//!
//! - `_start` entry, argument/environment parsing, `main!` macro
//! - raw system calls ([`sys`]) and safe wrappers
//! - console I/O with `print!`/`println!`/`eprintln!`
//! - files and directories ([`fs`])
//! - a heap (`alloc` works) on top of `mmap`
//! - processes: `spawn`, `wait`, `exit`, `getpid`, `kill`, users, process
//!   list, system information, sleep, time
//! - the console terminal ([`term`])
//! - TCP and UDP sockets, ping, interface information ([`net`])
//! - optional C API (feature `cabi`, see `include/vlibc.h`)
//!
//! Programs are single-threaded for now; the heap is not thread-safe.

#![no_std]

extern crate alloc;

pub mod env;
pub mod fs;
pub mod heap;
pub mod io;
pub mod mem;
pub mod net;
pub mod process;
mod rt;
pub mod sys;
pub mod term;
pub mod time;

#[cfg(feature = "cabi")]
pub mod cabi;

pub use velora_abi as abi;

/// An error number from the kernel (positive, e.g. `EFAULT` = 14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Errno(pub i64);

impl Errno {
    pub fn name(self) -> &'static str {
        use velora_abi::errno::*;
        match self.0 {
            EPERM => "EPERM",
            EPIPE => "EPIPE",
            ENOENT => "ENOENT",
            ESRCH => "ESRCH",
            EINTR => "EINTR",
            EIO => "EIO",
            E2BIG => "E2BIG",
            ENOEXEC => "ENOEXEC",
            EBADF => "EBADF",
            ECHILD => "ECHILD",
            EAGAIN => "EAGAIN",
            ENOMEM => "ENOMEM",
            EFAULT => "EFAULT",
            EACCES => "EACCES",
            EBUSY => "EBUSY",
            EEXIST => "EEXIST",
            EXDEV => "EXDEV",
            ENOTDIR => "ENOTDIR",
            EISDIR => "EISDIR",
            EINVAL => "EINVAL",
            EMFILE => "EMFILE",
            EFBIG => "EFBIG",
            ENODEV => "ENODEV",
            ENOSPC => "ENOSPC",
            ESPIPE => "ESPIPE",
            EROFS => "EROFS",
            ENAMETOOLONG => "ENAMETOOLONG",
            ENOSYS => "ENOSYS",
            ENOTEMPTY => "ENOTEMPTY",
            EUCLEAN => "EUCLEAN",
            _ => "unknown error",
        }
    }
}

impl core::fmt::Display for Errno {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} ({})", self.name(), self.0)
    }
}

/// Converts a raw system call result into `Result`.
pub fn check(ret: i64) -> Result<u64, Errno> {
    if ret < 0 {
        Err(Errno(-ret))
    } else {
        Ok(ret as u64)
    }
}

/// Declares the program's `main` function: `fn main() -> i32`.
/// Arguments are available through [`env::args`].
#[macro_export]
macro_rules! main {
    ($f:path) => {
        #[unsafe(no_mangle)]
        fn __vlibc_main() -> i32 {
            $f()
        }
    };
}

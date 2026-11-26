//! The native Velora system calls.

use super::uaccess;
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::{input, kprint, sched, time};
use velora_abi::{Timespec, clock, errno, nr};

/// Largest single write; longer writes are cut (the caller sees the count).
const MAX_WRITE: u64 = 64 * 1024;

pub fn dispatch(f: &mut SyscallFrame) -> i64 {
    let (a0, a1, a2) = (f.rdi, f.rsi, f.rdx);
    match f.rax {
        nr::EXIT => sched::exit_process(a0 as i32 as i64),
        nr::WRITE => write(a0, a1, a2),
        nr::READ => read(a0, a1, a2),
        nr::YIELD => {
            sched::yield_now();
            0
        }
        nr::GETPID => sched::current_pid() as i64,
        nr::CLOCK_GETTIME => clock_gettime(a0, a1),
        nr::SLEEP_MS => {
            sched::sleep_ms(a0);
            0
        }
        _ => -errno::ENOSYS,
    }
}

fn write(fd: u64, buf: u64, len: u64) -> i64 {
    if fd != velora_abi::STDOUT && fd != velora_abi::STDERR {
        return -errno::EBADF;
    }
    let len = len.min(MAX_WRITE);
    let Some(bytes) = uaccess::read_slice(buf, len) else {
        return -errno::EFAULT;
    };
    // Invalid UTF-8 is shown as U+FFFD instead of being rejected.
    for chunk in bytes.utf8_chunks() {
        kprint!("{}", chunk.valid());
        if !chunk.invalid().is_empty() {
            kprint!("\u{fffd}");
        }
    }
    len as i64
}

/// Reads typed text from the keyboard: blocks until at least one
/// character is available, then returns as many whole UTF-8 characters
/// as fit. Line editing comes with the terminal layer (stage 8).
fn read(fd: u64, buf: u64, len: u64) -> i64 {
    if fd != velora_abi::STDIN {
        return -errno::EBADF;
    }
    if len < 4 {
        return -errno::EINVAL; // room for one UTF-8 character
    }
    let mut out = [0u8; 256];
    let cap = (len as usize).min(out.len());
    let mut n = 0;
    loop {
        let ev = if n == 0 {
            input::wait(None)
        } else {
            input::pop()
        };
        let Some(ev) = ev else { break };
        if let (true, Some(c)) = (ev.pressed, ev.ch) {
            if n + c.len_utf8() > cap {
                break;
            }
            n += c.encode_utf8(&mut out[n..]).len();
        }
    }
    if uaccess::write(buf, &out[..n]) {
        n as i64
    } else {
        -errno::EFAULT
    }
}

fn clock_gettime(id: u64, out: u64) -> i64 {
    if id != clock::MONOTONIC {
        return -errno::EINVAL;
    }
    let ns = time::uptime_ns();
    let ts = Timespec {
        seconds: ns / 1_000_000_000,
        nanoseconds: ns % 1_000_000_000,
    };
    // SAFETY: Timespec is plain data (two u64).
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&ts as *const Timespec).cast::<u8>(),
            core::mem::size_of::<Timespec>(),
        )
    };
    if uaccess::write(out, bytes) {
        0
    } else {
        -errno::EFAULT
    }
}

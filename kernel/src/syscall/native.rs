//! The native Velora system calls.

use super::uaccess;
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::exec;
use crate::mm::PAGE_SIZE;
use crate::mm::paging::UserKind;
use crate::{input, kprint, sched, time};
use alloc::string::String;
use alloc::vec::Vec;
use velora_abi::{StrRef, Timespec, clock, errno, map, nr, prot};

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
        nr::SPAWN => spawn(a0, a1, a2, f.r10, f.r8, f.r9),
        nr::WAIT => wait(a0, a1),
        nr::MMAP => mmap(a0, a1, a2, f.r10),
        nr::MUNMAP => munmap(a0, a1),
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

/// Copies a user string. Invalid UTF-8 is an error.
fn user_string(ptr: u64, len: u64) -> Result<String, i64> {
    if len as usize > velora_abi::MAX_ARG_BYTES {
        return Err(-errno::E2BIG);
    }
    let bytes = uaccess::read_slice(ptr, len).ok_or(-errno::EFAULT)?;
    core::str::from_utf8(bytes)
        .map(String::from)
        .map_err(|_| -errno::EINVAL)
}

/// Copies an array of `StrRef` and the strings it points to.
fn user_strings(ptr: u64, count: u64) -> Result<Vec<String>, i64> {
    if count as usize > velora_abi::MAX_ARGS {
        return Err(-errno::E2BIG);
    }
    let size = count * core::mem::size_of::<StrRef>() as u64;
    let raw = uaccess::read_slice(ptr, size).ok_or(-errno::EFAULT)?;
    let mut out = Vec::with_capacity(count as usize);
    for chunk in raw.as_chunks::<16>().0 {
        let p = u64::from_le_bytes(chunk[..8].try_into().unwrap());
        let l = u64::from_le_bytes(chunk[8..].try_into().unwrap());
        out.push(user_string(p, l)?);
    }
    Ok(out)
}

fn spawn(path: u64, path_len: u64, argv: u64, argc: u64, envp: u64, envc: u64) -> i64 {
    let result = (|| {
        let path = user_string(path, path_len)?;
        let argv = user_strings(argv, argc)?;
        let envp = user_strings(envp, envc)?;
        let a: Vec<&str> = argv.iter().map(String::as_str).collect();
        let e: Vec<&str> = envp.iter().map(String::as_str).collect();
        exec::spawn(&path, &a, &e).map_err(|e| -e.errno())
    })();
    match result {
        Ok(pid) => pid as i64,
        Err(e) => e,
    }
}

/// Waits for a child process to exit and stores its status.
fn wait(pid: u64, status_ptr: u64) -> i64 {
    if sched::parent_of(pid) != Some(sched::current_pid()) {
        return -errno::ECHILD;
    }
    if status_ptr != 0 && !uaccess::write(status_ptr, &[0u8; 8]) {
        return -errno::EFAULT;
    }
    let Some(status) = sched::wait_pid(pid) else {
        return -errno::ECHILD;
    };
    if status_ptr != 0 && !uaccess::write(status_ptr, &status.to_le_bytes()) {
        return -errno::EFAULT;
    }
    pid as i64
}

/// Anonymous memory. Pages are zero-filled; W+X and PROT_NONE are refused.
fn mmap(addr: u64, len: u64, protection: u64, flags: u64) -> i64 {
    if len == 0 || len > 1 << 40 || protection & !(prot::READ | prot::WRITE | prot::EXEC) != 0 {
        return -errno::EINVAL;
    }
    if protection & prot::WRITE != 0 && protection & prot::EXEC != 0 {
        return -errno::EINVAL; // W^X
    }
    let kind = if protection & prot::EXEC != 0 {
        UserKind::Code
    } else if protection & prot::WRITE != 0 {
        UserKind::Data
    } else if protection & prot::READ != 0 {
        UserKind::ReadOnly
    } else {
        return -errno::EINVAL;
    };
    let len = len.next_multiple_of(PAGE_SIZE);
    let start = if flags & map::FIXED != 0 {
        if !addr.is_multiple_of(PAGE_SIZE) || !exec::user_range_ok(addr, len) {
            return -errno::EINVAL;
        }
        addr
    } else {
        match sched::reserve_mmap_range(len) {
            Some(a) => a,
            None => return -errno::ENOMEM,
        }
    };
    let pid = sched::current_pid();
    sched::with_address_space(pid, |a| {
        let mut v = start;
        while v < start + len {
            if a.translate(v).is_some() {
                return -errno::EEXIST;
            }
            v += PAGE_SIZE;
        }
        if a.map_new(start, len / PAGE_SIZE, kind).is_err() {
            let mut v = start;
            while v < start + len {
                a.unmap(v);
                v += PAGE_SIZE;
            }
            return -errno::ENOMEM;
        }
        start as i64
    })
    .unwrap_or(-errno::ESRCH)
}

fn munmap(addr: u64, len: u64) -> i64 {
    if !addr.is_multiple_of(PAGE_SIZE) || len == 0 {
        return -errno::EINVAL;
    }
    let len = len.next_multiple_of(PAGE_SIZE);
    if !exec::user_range_ok(addr, len) {
        return -errno::EINVAL;
    }
    let pid = sched::current_pid();
    sched::with_address_space(pid, |a| {
        let mut v = addr;
        while v < addr + len {
            a.unmap(v);
            v += PAGE_SIZE;
        }
        0
    })
    .unwrap_or(-errno::ESRCH)
}

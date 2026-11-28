//! The native Velora system calls.

use super::{files, process, socket, uaccess};
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::exec;
use crate::mm::PAGE_SIZE;
use crate::mm::paging::UserKind;
use crate::{fs, sched, time, tty};
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
        nr::OPEN => files::open(a0, a1, a2, f.r10),
        nr::CLOSE => files::close(a0),
        nr::SEEK => files::seek(a0, a1 as i64, a2),
        nr::STAT => files::stat(a0, a1, a2),
        nr::FSTAT => files::fstat(a0, a1),
        nr::READDIR => files::readdir(a0, a1, a2),
        nr::MKDIR => files::mkdir(a0, a1, a2),
        nr::UNLINK => files::unlink(a0, a1),
        nr::RENAME => files::rename(a0, a1, a2, f.r10),
        nr::FTRUNCATE => files::ftruncate(a0, a1),
        nr::SYNC => files::sync(),
        nr::STATFS => files::statfs(a0, a1, a2),
        nr::PIPE => files::pipe(a0),
        nr::DUP => files::dup(a0),
        nr::DUP2 => files::dup2(a0, a1),
        nr::CHDIR => files::chdir(a0, a1),
        nr::GETCWD => files::getcwd(a0, a1),
        nr::CHMOD => files::chmod(a0, a1, a2),
        nr::CHOWN => files::chown(a0, a1, a2, f.r10),
        nr::KILL => process::kill(a0, a1),
        nr::SYSINFO => process::sysinfo(a0),
        nr::REBOOT => process::reboot(a0),
        nr::SETUID => process::setuid(a0, a1),
        nr::GETUID => process::getuid(),
        nr::PROCLIST => process::proclist(a0, a1),
        nr::TTY => process::tty(a0, a1),
        nr::GETRANDOM => process::getrandom(a0, a1),
        nr::SOCKET => socket::socket(a0),
        nr::BIND => socket::bind(a0, a1),
        nr::CONNECT => socket::connect(a0, a1, a2),
        nr::LISTEN => socket::listen(a0),
        nr::ACCEPT => socket::accept(a0),
        nr::SENDTO => socket::sendto_fd(a0, a1, a2, f.r10, f.r8),
        nr::RECVFROM => socket::recvfrom_fd(a0, a1, a2, f.r10, f.r8),
        nr::NETINFO => socket::netinfo(a0),
        nr::PING => socket::ping(a0, a1, a2),
        nr::SHUTDOWN => socket::shutdown(a0),
        nr::DISPLAY => match a0 {
            velora_abi::display::INFO => crate::display::info(a1),
            velora_abi::display::MAP => crate::display::map(),
            velora_abi::display::RELEASE => crate::display::release(),
            _ => -errno::EINVAL,
        },
        nr::DISKINFO => diskinfo(a0, a1, a2),
        nr::INPUT_WAIT => crate::input::wait_any(a0.min(60_000)) as i64,
        nr::UMOUNT => files::umount(a0, a1),
        nr::CLOCK_GETTIME => clock_gettime(a0, a1),
        nr::SLEEP_MS => {
            sched::sleep_ms(a0);
            0
        }
        _ => -errno::ENOSYS,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Console,
    Pipe,
    File,
    Socket(u32),
}

fn target(f: &fs::FileRef) -> Target {
    match f.lock().handle {
        fs::Handle::Console => Target::Console,
        fs::Handle::Pipe { .. } => Target::Pipe,
        fs::Handle::File { .. } => Target::File,
        fs::Handle::Socket { id } => Target::Socket(id),
    }
}

fn write(fd: u64, buf: u64, len: u64) -> i64 {
    let Some(f) = sched::fd_get(fd) else {
        return -errno::EBADF;
    };
    match target(&f) {
        Target::File => return files::write_file(&f, buf, len),
        Target::Pipe => return files::write_pipe(&f, buf, len),
        Target::Socket(id) => return socket::sendto(id, buf, len, 0, 0),
        Target::Console => {}
    }
    let len = len.min(MAX_WRITE);
    let Some(bytes) = uaccess::read_slice(buf, len) else {
        return -errno::EFAULT;
    };
    tty::write(bytes);
    len as i64
}

/// Reads a file, a pipe or the console terminal (see `tty`).
fn read(fd: u64, buf: u64, len: u64) -> i64 {
    let Some(f) = sched::fd_get(fd) else {
        return -errno::EBADF;
    };
    match target(&f) {
        Target::File => return files::read_file(&f, buf, len),
        Target::Pipe => return files::read_pipe(&f, buf, len),
        Target::Socket(id) => return socket::recvfrom(id, buf, len, 0, 0),
        Target::Console => {}
    }
    let data = tty::read(len.min(4096) as usize);
    if uaccess::write(buf, &data) {
        data.len() as i64
    } else {
        -errno::EFAULT
    }
}

fn clock_gettime(id: u64, out: u64) -> i64 {
    let ns = match id {
        clock::MONOTONIC => time::uptime_ns(),
        clock::REALTIME => match time::realtime_ns() {
            Some(ns) => ns,
            None => return -errno::EAGAIN,
        },
        _ => return -errno::EINVAL,
    };
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
        // Relative program paths start at the working directory.
        let path = files::user_path(path, path_len)?;
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

/// Size, kind and model of a disk or partition (`/dev/NAME`).
fn diskinfo(path: u64, len: u64, out: u64) -> i64 {
    let path = match files::user_path(path, len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(d) = path
        .strip_prefix("/dev/")
        .and_then(crate::drivers::block::find)
    else {
        return -errno::ENODEV;
    };
    let mut info = velora_abi::DiskInfo {
        size_bytes: d.size_bytes(),
        sector_size: d.sector_size() as u32,
        kind: d.kind()
            | if crate::drivers::block::boot_disk().as_deref() == Some(d.disk_name().as_str()) {
                velora_abi::disk_kind::BOOT
            } else {
                0
            },
        model: [0; 48],
    };
    let model = d.model();
    let model = model.trim();
    // Cut at a character boundary.
    let mut n = model.len().min(48);
    while !model.is_char_boundary(n) {
        n -= 1;
    }
    info.model[..n].copy_from_slice(&model.as_bytes()[..n]);
    // SAFETY: DiskInfo is plain data with no padding.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&info as *const velora_abi::DiskInfo).cast::<u8>(),
            core::mem::size_of::<velora_abi::DiskInfo>(),
        )
    };
    if uaccess::write(out, bytes) {
        0
    } else {
        -errno::EFAULT
    }
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

//! Processes: exit, pid, spawning and waiting for children.

use crate::sys::{syscall0, syscall1, syscall2, syscall6};
use crate::{Errno, check};
use alloc::vec::Vec;
use velora_abi::{StrRef, nr};

/// Ends the program with `code`.
pub fn exit(code: i32) -> ! {
    // SAFETY: exit takes no pointers and does not return.
    unsafe { syscall1(nr::EXIT, code as i64 as u64) };
    unreachable!("exit returned")
}

pub fn getpid() -> u64 {
    // SAFETY: no arguments.
    unsafe { syscall0(nr::GETPID) as u64 }
}

/// Gives the CPU to other ready programs.
pub fn yield_now() {
    // SAFETY: no arguments.
    unsafe { syscall0(nr::YIELD) };
}

fn refs(items: &[&str]) -> Vec<StrRef> {
    items
        .iter()
        .map(|s| StrRef {
            ptr: s.as_ptr() as u64,
            len: s.len() as u64,
        })
        .collect()
}

/// Starts the program at `path` with arguments and environment
/// ("NAME=value" strings). Returns the child's pid.
pub fn spawn(path: &str, args: &[&str], env: &[&str]) -> Result<u64, Errno> {
    let a = refs(args);
    let e = refs(env);
    // SAFETY: all pointers refer to live buffers for the duration of the call.
    check(unsafe {
        syscall6(
            nr::SPAWN,
            path.as_ptr() as u64,
            path.len() as u64,
            a.as_ptr() as u64,
            a.len() as u64,
            e.as_ptr() as u64,
            e.len() as u64,
        )
    })
}

/// Waits for child `pid` to exit and returns its exit status. A child
/// killed by a CPU exception has status 128 + vector (page fault = 142).
pub fn wait(pid: u64) -> Result<i64, Errno> {
    let mut status: i64 = 0;
    // SAFETY: `status` is a valid, writable i64.
    check(unsafe { syscall2(nr::WAIT, pid, &mut status as *mut i64 as u64) })?;
    Ok(status)
}

/// Ends process `pid` with exit status `status`.
pub fn kill(pid: u64, status: i32) -> Result<(), Errno> {
    // SAFETY: no pointers.
    check(unsafe { crate::sys::syscall2(nr::KILL, pid, status as i64 as u64) }).map(|_| ())
}

/// (uid, gid) of this process.
pub fn getuid() -> (u32, u32) {
    // SAFETY: no arguments.
    let r = unsafe { syscall0(nr::GETUID) } as u64;
    (r as u32, (r >> 32) as u32)
}

/// Becomes user `uid` in group `gid` (root only, cannot be undone).
pub fn setuid(uid: u32, gid: u32) -> Result<(), Errno> {
    // SAFETY: no pointers.
    check(unsafe { syscall2(nr::SETUID, uid as u64, gid as u64) }).map(|_| ())
}

/// Every process the kernel knows.
pub fn list() -> Result<Vec<velora_abi::ProcEntry>, Errno> {
    let mut v = alloc::vec![velora_abi::ProcEntry::default(); 256];
    let bytes = v.len() * size_of::<velora_abi::ProcEntry>();
    // SAFETY: the buffer is writable for `bytes` bytes.
    let n = check(unsafe { syscall2(nr::PROCLIST, v.as_mut_ptr() as u64, bytes as u64) })?;
    v.truncate(n as usize / size_of::<velora_abi::ProcEntry>());
    Ok(v)
}

pub fn sysinfo() -> Result<velora_abi::SysInfo, Errno> {
    let mut s = velora_abi::SysInfo::default();
    // SAFETY: `s` is writable and has the kernel's layout.
    check(unsafe { syscall1(nr::SYSINFO, &mut s as *mut velora_abi::SysInfo as u64) })?;
    Ok(s)
}

/// Restarts or powers off the machine (root only). Returns only on error.
pub fn reboot(cmd: u64) -> Errno {
    // SAFETY: no pointers.
    match check(unsafe { syscall1(nr::REBOOT, cmd) }) {
        Err(e) => e,
        Ok(_) => Errno(velora_abi::errno::EIO),
    }
}

/// Text of a NUL-padded name field.
pub fn field_str(b: &[u8]) -> &str {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    core::str::from_utf8(&b[..n]).unwrap_or("?")
}

/// Fills `buf` with random bytes from the kernel (RDRAND when available).
pub fn random(buf: &mut [u8]) {
    for chunk in buf.chunks_mut(256) {
        // SAFETY: the chunk is writable for its length.
        unsafe { syscall2(nr::GETRANDOM, chunk.as_mut_ptr() as u64, chunk.len() as u64) };
    }
}

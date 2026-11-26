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

//! uptime: time since boot and the number of processes.

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::{println, process};

vlibc::main!(main);

fn main() -> i32 {
    let s = match process::sysinfo() {
        Ok(s) => s,
        Err(e) => return fail("sysinfo", e),
    };
    println!(
        "up {}, {} processes, boot took {} ms",
        duration(s.uptime_ns / 1_000_000_000),
        s.processes,
        s.boot_ms
    );
    0
}

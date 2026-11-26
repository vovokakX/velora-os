//! mem: memory use.

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
    let free = s.mem_total.saturating_sub(s.mem_used);
    println!(
        "total: {:>10}  ({} KiB)",
        human(s.mem_total),
        s.mem_total / 1024
    );
    println!(
        "used:  {:>10}  ({} KiB)",
        human(s.mem_used),
        s.mem_used / 1024
    );
    println!("free:  {:>10}  ({} KiB)", human(free), free / 1024);
    0
}

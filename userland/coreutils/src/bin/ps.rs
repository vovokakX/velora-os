//! ps: lists processes.

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::println;
use vlibc::process::{self, field_str};

vlibc::main!(main);

fn main() -> i32 {
    let list = match process::list() {
        Ok(l) => l,
        Err(e) => return fail("process list", e),
    };
    println!("  PID  PPID USER     THR      MEM     TIME STATE   NAME");
    for p in list {
        // Processor time in seconds (timer ticks, 100 a second).
        let t = p.cpu_ticks;
        println!(
            "{:>5} {:>5} {:<8} {:>3} {:>8} {:>5}.{:02} {:<7} {}",
            p.pid,
            p.parent,
            user_name(p.uid),
            p.threads,
            human(p.memory),
            t / 100,
            t % 100,
            if p.state == 0 { "running" } else { "exited" },
            field_str(&p.name)
        );
    }
    0
}

//! kill [-s STATUS] PID...: ends processes. The exit status defaults to
//! 143 (128 + 15, as for SIGTERM).

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::{env, eprintln, process};

vlibc::main!(main);

fn main() -> i32 {
    let args = env::args();
    let mut status = 143;
    let mut i = 1;
    if args.get(1) == Some("-s") {
        match args.get(2).and_then(|s| s.parse().ok()) {
            Some(s) => status = s,
            None => {
                eprintln!("kill: -s needs a number");
                return 2;
            }
        }
        i = 3;
    }
    if args.len() <= i {
        eprintln!("usage: kill [-s STATUS] PID...");
        return 2;
    }
    let mut rc = 0;
    for a in args.iter().skip(i) {
        match a.parse::<u64>() {
            Ok(pid) => {
                if let Err(e) = process::kill(pid, status) {
                    rc = fail(a, e);
                }
            }
            Err(_) => {
                eprintln!("kill: {}: not a process id", a);
                rc = 1;
            }
        }
    }
    rc
}

//! touch FILE...: creates empty files that do not exist yet.

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::eprintln;
use vlibc::fs::{self, File, flags};

vlibc::main!(main);

fn main() -> i32 {
    let args = vlibc::env::args();
    if args.len() < 2 {
        eprintln!("usage: touch FILE...");
        return 2;
    }
    let mut status = 0;
    for p in args.iter().skip(1) {
        if fs::exists(p) {
            continue;
        }
        if let Err(e) = File::open_with(p, flags::WRITE | flags::CREATE, 0o644) {
            status = fail(p, e);
        }
    }
    status
}

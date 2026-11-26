//! dmesg: prints the kernel log (the last 64 KiB of kernel messages,
//! from /dev/kmsg).
//!
//! The log is read in one call: printing it on the console adds to the
//! log, so reading it piece by piece would chase its own output.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use coreutils::*;
use vlibc::fs::File;
use vlibc::io;

vlibc::main!(main);

/// The kernel keeps 64 KiB; a little more is harmless.
const LOG_SIZE: usize = 72 * 1024;

fn main() -> i32 {
    let mut f = match File::open("/dev/kmsg") {
        Ok(f) => f,
        Err(e) => return fail("/dev/kmsg", e),
    };
    let mut buf = vec![0u8; LOG_SIZE];
    match f.read(&mut buf) {
        Ok(n) => {
            if io::write_all(1, &buf[..n]).is_err() {
                return 1;
            }
            0
        }
        Err(e) => fail("/dev/kmsg", e),
    }
}

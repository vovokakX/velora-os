//! cat [FILE...]: prints files, or standard input without arguments
//! (or for "-").

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::io;

vlibc::main!(main);

fn copy(fd: u64) -> Result<(), vlibc::Errno> {
    let mut buf = [0u8; 4096];
    loop {
        let n = io::read(fd, &mut buf)?;
        if n == 0 {
            return Ok(());
        }
        io::write_all(1, &buf[..n])?;
    }
}

fn main() -> i32 {
    let args = vlibc::env::args();
    if args.len() < 2 {
        return match copy(0) {
            Ok(()) => 0,
            Err(e) => fail("standard input", e),
        };
    }
    let mut status = 0;
    for p in args.iter().skip(1) {
        let r = if p == "-" {
            copy(0)
        } else {
            vlibc::fs::File::open(p).and_then(|f| copy(f.fd()))
        };
        if let Err(e) = r {
            status = fail(p, e);
        }
    }
    status
}

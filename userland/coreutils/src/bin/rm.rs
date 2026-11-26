//! rm [-r] [-f] PATH...: removes files (and directories with -r).

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::eprintln;
use vlibc::fs;

vlibc::main!(main);

fn remove(p: &str, recursive: bool) -> Result<(), vlibc::Errno> {
    let st = fs::stat(p)?;
    if is_dir(&st) {
        if !recursive {
            return Err(vlibc::Errno(vlibc::abi::errno::EISDIR));
        }
        let names: alloc::vec::Vec<_> = fs::read_dir(p)?.flatten().map(|e| e.name).collect();
        for n in names {
            remove(&join(p, &n), true)?;
        }
    }
    fs::remove(p)
}

fn main() -> i32 {
    let (flags, ops) = match parse_args("rf") {
        Ok(x) => x,
        Err(e) => {
            eprintln!("rm: {}", e);
            return 2;
        }
    };
    let force = flags.contains(&'f');
    if ops.is_empty() && !force {
        eprintln!("usage: rm [-r] [-f] PATH...");
        return 2;
    }
    let mut status = 0;
    for p in ops {
        if let Err(e) = remove(p, flags.contains(&'r'))
            && !(force && e.0 == vlibc::abi::errno::ENOENT)
        {
            status = fail(p, e);
        }
    }
    status
}

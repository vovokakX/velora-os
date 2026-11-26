//! mkdir [-p] DIR...: creates directories (-p: with parents, no error if
//! they exist).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use coreutils::*;
use vlibc::eprintln;
use vlibc::fs;

vlibc::main!(main);

fn main() -> i32 {
    let (flags, ops) = match parse_args("p") {
        Ok(x) => x,
        Err(e) => {
            eprintln!("mkdir: {}", e);
            return 2;
        }
    };
    if ops.is_empty() {
        eprintln!("usage: mkdir [-p] DIR...");
        return 2;
    }
    let mut status = 0;
    for d in ops {
        if !flags.contains(&'p') {
            if let Err(e) = fs::mkdir(d, 0o755) {
                status = fail(d, e);
            }
            continue;
        }
        let mut path = String::new();
        if d.starts_with('/') {
            path.push('/');
        }
        for part in d.split('/').filter(|p| !p.is_empty()) {
            path.push_str(part);
            if !fs::stat(&path).is_ok_and(|s| is_dir(&s))
                && let Err(e) = fs::mkdir(&path, 0o755)
            {
                status = fail(&path, e);
                break;
            }
            path.push('/');
        }
    }
    status
}

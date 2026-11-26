//! cp [-r] SOURCE... TARGET: copies files (and directories with -r).

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::eprintln;
use vlibc::fs;

vlibc::main!(main);

fn copy(from: &str, to: &str, recursive: bool) -> i32 {
    let st = match fs::stat(from) {
        Ok(s) => s,
        Err(e) => return fail(from, e),
    };
    if !is_dir(&st) {
        return match copy_file(from, to) {
            Ok(()) => 0,
            Err(e) => fail(to, e),
        };
    }
    if !recursive {
        eprintln!("cp: {} is a directory (use -r)", from);
        return 1;
    }
    if let Err(e) = fs::mkdir(to, st.mode & 0o777)
        && !fs::exists(to)
    {
        return fail(to, e);
    }
    let mut status = 0;
    let entries = match fs::read_dir(from) {
        Ok(d) => d,
        Err(e) => return fail(from, e),
    };
    for ent in entries.flatten() {
        let r = copy(&join(from, &ent.name), &join(to, &ent.name), true);
        if r != 0 {
            status = r;
        }
    }
    status
}

fn main() -> i32 {
    let (flags, ops) = match parse_args("r") {
        Ok(x) => x,
        Err(e) => {
            eprintln!("cp: {}", e);
            return 2;
        }
    };
    if ops.len() < 2 {
        eprintln!("usage: cp [-r] SOURCE... TARGET");
        return 2;
    }
    let (sources, target) = ops.split_at(ops.len() - 1);
    let target = target[0];
    let into_dir = fs::stat(target).is_ok_and(|s| is_dir(&s));
    if sources.len() > 1 && !into_dir {
        eprintln!("cp: {} is not a directory", target);
        return 1;
    }
    let mut status = 0;
    for s in sources {
        let dst = if into_dir {
            join(target, base_name(s))
        } else {
            target.into()
        };
        let r = copy(s, &dst, flags.contains(&'r'));
        if r != 0 {
            status = r;
        }
    }
    status
}

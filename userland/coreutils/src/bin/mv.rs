//! mv SOURCE... TARGET: renames or moves files and directories. Across
//! file systems files are copied and the source removed.

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::abi::errno;
use vlibc::eprintln;
use vlibc::fs;

vlibc::main!(main);

fn main() -> i32 {
    let (_, ops) = match parse_args("") {
        Ok(x) => x,
        Err(e) => {
            eprintln!("mv: {}", e);
            return 2;
        }
    };
    if ops.len() < 2 {
        eprintln!("usage: mv SOURCE... TARGET");
        return 2;
    }
    let (sources, target) = ops.split_at(ops.len() - 1);
    let target = target[0];
    let into_dir = fs::stat(target).is_ok_and(|s| is_dir(&s));
    if sources.len() > 1 && !into_dir {
        eprintln!("mv: {} is not a directory", target);
        return 1;
    }
    let mut status = 0;
    for s in sources {
        let dst = if into_dir {
            join(target, base_name(s))
        } else {
            target.into()
        };
        match fs::rename(s, &dst) {
            Ok(()) => {}
            Err(e) if e.0 == errno::EXDEV => {
                let r = copy_file(s, &dst).and_then(|_| fs::remove(s));
                if let Err(e) = r {
                    status = fail(s, e);
                }
            }
            Err(e) => status = fail(s, e),
        }
    }
    status
}

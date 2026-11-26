//! ls [-l] [-a] [PATH...]: lists directories.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use coreutils::*;
use vlibc::fs::{self, mode};
use vlibc::{eprintln, print, println};

vlibc::main!(main);

struct Item {
    name: String,
    st: fs::Stat,
}

fn show(items: &[Item], long: bool, color: bool) {
    for it in items {
        let m = it.st.mode;
        let (pre, post) = if !color {
            ("", "")
        } else if m & mode::TYPE_MASK == mode::DIR {
            ("\x1b[1;34m", "\x1b[0m")
        } else if m & mode::TYPE_MASK != mode::FILE {
            ("\x1b[33m", "\x1b[0m")
        } else if m & 0o111 != 0 {
            ("\x1b[1;32m", "\x1b[0m")
        } else {
            ("", "")
        };
        if long {
            print!(
                "{} {:>2} {:<6} {:<6} {:>9} {} ",
                mode_string(m),
                it.st.nlink,
                user_name(it.st.uid),
                group_name(it.st.gid),
                it.st.size,
                &format_time(it.st.mtime)[..16]
            );
        }
        println!("{}{}{}", pre, it.name, post);
    }
}

fn main() -> i32 {
    let (flags, mut paths) = match parse_args("la") {
        Ok(x) => x,
        Err(e) => {
            eprintln!("ls: {}", e);
            return 2;
        }
    };
    let long = flags.contains(&'l');
    let all = flags.contains(&'a');
    let color = colors();
    if paths.is_empty() {
        paths.push(".");
    }
    let many = paths.len() > 1;
    let mut status = 0;
    for (n, p) in paths.iter().enumerate() {
        let st = match fs::stat(p) {
            Ok(s) => s,
            Err(e) => {
                status = fail(p, e);
                continue;
            }
        };
        if !is_dir(&st) {
            show(
                &[Item {
                    name: String::from(*p),
                    st,
                }],
                long,
                color,
            );
            continue;
        }
        let mut items = Vec::new();
        let dir = match fs::read_dir(p) {
            Ok(d) => d,
            Err(e) => {
                status = fail(p, e);
                continue;
            }
        };
        for ent in dir {
            let ent = match ent {
                Ok(e) => e,
                Err(e) => {
                    status = fail(p, e);
                    break;
                }
            };
            if !all && ent.name.starts_with('.') {
                continue;
            }
            match fs::stat(&join(p, &ent.name)) {
                Ok(st) => items.push(Item { name: ent.name, st }),
                Err(e) => status = fail(&ent.name, e),
            }
        }
        items.sort_by(|a, b| a.name.cmp(&b.name));
        if many {
            if n > 0 {
                println!();
            }
            println!("{}:", p);
        }
        show(&items, long, color);
    }
    status
}

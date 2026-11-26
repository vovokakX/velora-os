//! df: file systems, their size and free space.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use coreutils::*;
use vlibc::fs;
use vlibc::println;
use vlibc::process::field_str;

vlibc::main!(main);

fn main() -> i32 {
    let mut paths: Vec<String> = ["/", "/dev", "/tmp", "/home", "/root"]
        .iter()
        .map(|s| String::from(*s))
        .collect();
    if let Ok(d) = fs::read_dir("/mnt") {
        paths.extend(d.flatten().map(|e| join("/mnt", &e.name)));
    }
    println!(
        "{:<14} {:<10} {:>10} {:>10} {:>10}  {}",
        "Mounted on", "Type", "Size", "Used", "Free", ""
    );
    let root = fs::stat("/").ok();
    for p in paths {
        let (Ok(st), Ok(s)) = (fs::statfs(&p), fs::stat(&p)) else {
            continue;
        };
        // Skip directories that are not mount points (same device as /).
        if p != "/" && root.is_some_and(|r| r.dev == s.dev) {
            continue;
        }
        let size = st.blocks * st.block_size;
        let free = st.free_blocks * st.block_size;
        println!(
            "{:<14} {:<10} {:>10} {:>10} {:>10}  {}",
            p,
            field_str(&st.fs_type),
            if size > 0 {
                human(size)
            } else {
                String::from("-")
            },
            if size > 0 {
                human(size - free)
            } else {
                String::from("-")
            },
            if size > 0 {
                human(free)
            } else {
                String::from("-")
            },
            if st.read_only != 0 { "read-only" } else { "" }
        );
    }
    0
}

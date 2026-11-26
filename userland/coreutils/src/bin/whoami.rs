//! whoami: the current user name.

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::{println, process};

vlibc::main!(main);

fn main() -> i32 {
    println!("{}", user_name(process::getuid().0));
    0
}

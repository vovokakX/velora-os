//! reboot: restarts the computer (root only).

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::process;

vlibc::main!(main);

fn main() -> i32 {
    if !need_root() {
        return 1;
    }
    let e = process::reboot(vlibc::abi::reboot::RESTART);
    fail("reboot", e)
}

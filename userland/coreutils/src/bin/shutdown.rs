//! shutdown [-r]: powers the computer off (-r: restarts it). Root only.

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::{env, process};

vlibc::main!(main);

fn main() -> i32 {
    if !need_root() {
        return 1;
    }
    let cmd = if env::args().get(1) == Some("-r") {
        vlibc::abi::reboot::RESTART
    } else {
        vlibc::abi::reboot::POWER_OFF
    };
    let e = process::reboot(cmd);
    fail("shutdown", e)
}

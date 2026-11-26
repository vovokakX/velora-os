//! date: current date and time (UTC).

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::{println, time};

vlibc::main!(main);

fn main() -> i32 {
    match time::realtime() {
        Ok(t) => {
            println!("{} UTC", format_time(t.as_secs()));
            0
        }
        Err(e) => fail("clock", e),
    }
}

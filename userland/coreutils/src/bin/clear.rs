//! clear: clears the screen.

#![no_std]
#![no_main]

extern crate alloc;

use vlibc::print;

vlibc::main!(main);

fn main() -> i32 {
    print!("\x1b[2J\x1b[H");
    0
}

//! echo [-n] TEXT...: prints its arguments.

#![no_std]
#![no_main]

extern crate alloc;

use vlibc::{env, print};

vlibc::main!(main);

fn main() -> i32 {
    let args = env::args();
    let mut words: alloc::vec::Vec<&str> = args.iter().skip(1).collect();
    let newline = !(words.first() == Some(&"-n"));
    if !newline {
        words.remove(0);
    }
    print!("{}", words.join(" "));
    if newline {
        print!("\n");
    }
    0
}

//! /bin/hello: the first program loaded from a file. Prints its pid,
//! arguments and environment.

#![no_std]
#![no_main]

use vlibc::{env, println, process};

vlibc::main!(main);

fn main() -> i32 {
    println!("Hello from /bin/hello (pid {})", process::getpid());
    for (i, a) in env::args().iter().enumerate() {
        println!("  argv[{}] = {}", i, a);
    }
    for v in env::vars() {
        println!("  env: {}", v);
    }
    0
}

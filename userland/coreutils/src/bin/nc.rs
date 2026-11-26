//! nc: a simple TCP tool.
//!
//! - `nc ADDRESS PORT`: connects, sends standard input until its end,
//!   then prints what the peer sends until it closes.
//! - `nc -l PORT [REPLY]`: waits for one connection, prints what arrives
//!   until the peer closes its side, then sends REPLY (if given) and
//!   closes.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use coreutils::*;
use vlibc::net::{Ipv4, TcpListener, TcpStream};
use vlibc::{env, eprintln, io};

vlibc::main!(main);

fn copy_to_stdout(s: &mut TcpStream) -> Result<(), vlibc::Errno> {
    let mut buf = [0u8; 4096];
    loop {
        let n = s.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        io::write_all(1, &buf[..n])?;
    }
}

fn client(ip: Ipv4, port: u16) -> i32 {
    let mut s = match TcpStream::connect(ip, port) {
        Ok(s) => s,
        Err(e) => return fail(&format!("{}:{}", ip, port), e),
    };
    let mut buf = [0u8; 4096];
    loop {
        match io::read(0, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if let Err(e) = s.write_all(&buf[..n]) {
                    return fail("send", e);
                }
            }
            Err(e) => return fail("standard input", e),
        }
    }
    // Tell the peer that our data is complete, then read its answer.
    if let Err(e) = s.shutdown_write() {
        return fail("shutdown", e);
    }
    match copy_to_stdout(&mut s) {
        Ok(()) => 0,
        Err(e) => fail("receive", e),
    }
}

fn server(port: u16, reply: Option<&str>) -> i32 {
    let l = match TcpListener::bind(port) {
        Ok(l) => l,
        Err(e) => return fail(&format!("port {}", port), e),
    };
    let mut s = match l.accept() {
        Ok(s) => s,
        Err(e) => return fail("accept", e),
    };
    drop(l);
    if let Err(e) = copy_to_stdout(&mut s) {
        return fail("receive", e);
    }
    if let Some(r) = reply
        && let Err(e) = s.write_all(format!("{}\n", r).as_bytes())
    {
        return fail("send", e);
    }
    0
}

fn main() -> i32 {
    let args = env::args();
    let usage = || {
        eprintln!("usage: nc ADDRESS PORT | nc -l PORT [REPLY]");
        2
    };
    if args.get(1) == Some("-l") {
        let Some(port) = args.get(2).and_then(|p| p.parse().ok()) else {
            return usage();
        };
        return server(port, args.get(3));
    }
    let (Some(a), Some(p)) = (args.get(1), args.get(2)) else {
        return usage();
    };
    let Some(ip) = Ipv4::parse(a) else {
        eprintln!(
            "nc: {}: not an IPv4 address (names are not resolved yet)",
            a
        );
        return 2;
    };
    let Some(port) = p.parse().ok() else {
        return usage();
    };
    client(ip, port)
}

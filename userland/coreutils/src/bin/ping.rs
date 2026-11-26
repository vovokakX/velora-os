//! ping [-c COUNT] ADDRESS: ICMP echo requests, one per second.

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::net::{self, Ipv4};
use vlibc::{env, eprintln, println, time};

vlibc::main!(main);

fn main() -> i32 {
    let args = env::args();
    let mut count = 4u16;
    let mut target = None;
    let mut i = 1;
    while let Some(a) = args.get(i) {
        if a == "-c" {
            match args.get(i + 1).and_then(|s| s.parse().ok()) {
                Some(n) if n > 0 => count = n,
                _ => {
                    eprintln!("ping: -c needs a positive number");
                    return 2;
                }
            }
            i += 2;
        } else {
            target = Some(a);
            i += 1;
        }
    }
    let Some(t) = target else {
        eprintln!("usage: ping [-c COUNT] ADDRESS");
        return 2;
    };
    let Some(ip) = Ipv4::parse(t) else {
        eprintln!(
            "ping: {}: not an IPv4 address (names are not resolved yet)",
            t
        );
        return 2;
    };
    println!("PING {}", ip);
    let mut received = 0;
    for seq in 1..=count {
        match net::ping(ip, seq, 2000) {
            Ok(us) => {
                received += 1;
                println!(
                    "reply from {}: seq={} time={}.{:03} ms",
                    ip,
                    seq,
                    us / 1000,
                    us % 1000
                );
            }
            Err(e) => println!("no reply from {}: seq={} ({})", ip, seq, describe(e)),
        }
        if seq < count {
            time::sleep_ms(1000);
        }
    }
    println!("{} packets transmitted, {} received", count, received);
    if received > 0 { 0 } else { 1 }
}

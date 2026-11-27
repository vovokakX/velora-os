//! host NAME [SERVER[:PORT]]: looks up the IPv4 address of a name in DNS
//! (the DNS server from DHCP, or SERVER).

#![no_std]
#![no_main]

extern crate alloc;

use vlibc::net::{self, Ipv4};
use vlibc::{env, eprintln, println};

vlibc::main!(main);

fn main() -> i32 {
    let args = env::args();
    let (Some(name), extra) = (args.get(1), args.get(2)) else {
        eprintln!("usage: host NAME [SERVER[:PORT]]");
        return 2;
    };
    let server = match extra {
        None => None,
        Some(s) => {
            let (ip, port) = match s.split_once(':') {
                Some((i, p)) => (i, p.parse::<u16>().ok()),
                None => (s, Some(53)),
            };
            match (Ipv4::parse(ip), port) {
                (Some(ip), Some(port)) => Some((ip, port)),
                _ => {
                    eprintln!("host: {}: not ADDRESS[:PORT]", s);
                    return 2;
                }
            }
        }
    };
    match net::resolve_with(name, server) {
        Ok(ip) => {
            println!("{} has address {}", name, ip);
            0
        }
        Err(e) => {
            eprintln!("host: {}: {}", name, e.describe());
            1
        }
    }
}

//! ip: the network interface and its address.

#![no_std]
#![no_main]

extern crate alloc;

use coreutils::*;
use vlibc::net::{self, Ipv4};
use vlibc::println;

vlibc::main!(main);

fn main() -> i32 {
    let i = match net::netinfo() {
        Ok(i) => i,
        Err(e) => return fail("netinfo", e),
    };
    if i.flags & 1 == 0 {
        println!("no network card");
        return 1;
    }
    println!("net0: MAC {}", net::format_mac(&i.mac));
    if i.flags & 2 == 0 {
        println!("  no address yet (waiting for DHCP)");
    } else {
        println!("  inet {}/{}", Ipv4(i.ip), i.netmask.count_ones());
        println!("  gateway {}", Ipv4(i.gateway));
        println!("  dns {}", Ipv4(i.dns));
    }
    println!("  frames received {}, sent {}", i.rx_frames, i.tx_frames);
    0
}

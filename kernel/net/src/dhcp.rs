//! DHCP client (RFC 2131): DISCOVER, OFFER, REQUEST, ACK. The lease is
//! taken as permanent: renewal is NOT IMPLEMENTED (QEMU and most home
//! routers give leases of a day or more).

use crate::Config;
use crate::wire::{Ipv4, Mac, be32};
use alloc::vec::Vec;

pub const CLIENT_PORT: u16 = 68;
pub const SERVER_PORT: u16 = 67;
const MAGIC: [u8; 4] = [99, 130, 83, 99];
const RETRY_MS: u64 = 2000;

const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const ACK: u8 = 5;
const NAK: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Init,
    Selecting,
    Requesting { offered: Ipv4, server: Ipv4 },
    Bound,
}

pub struct Client {
    xid: u32,
    state: State,
    last_sent: u64,
    output: Option<Vec<u8>>,
}

pub fn message(kind: u8, xid: u32, mac: Mac, extra: &[(u8, &[u8])]) -> Vec<u8> {
    let mut m = alloc::vec![0u8; 240];
    m[0] = 1;
    m[1] = 1;
    m[2] = 6;
    m[4..8].copy_from_slice(&xid.to_be_bytes());
    m[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // replies by broadcast
    m[28..34].copy_from_slice(&mac);
    m[236..240].copy_from_slice(&MAGIC);
    m.extend_from_slice(&[53, 1, kind]);
    for (code, data) in extra {
        m.push(*code);
        m.push(data.len() as u8);
        m.extend_from_slice(data);
    }
    m.extend_from_slice(&[55, 4, 1, 3, 6, 51]); // subnet, router, DNS, lease
    m.push(255);
    m
}

/// Parsed reply: (message type, offered address, options by code).
/// DHCP options as (code, data).
pub type Options = Vec<(u8, Vec<u8>)>;

pub fn parse(m: &[u8], xid: u32, mac: Mac) -> Option<(u8, Ipv4, Options)> {
    if m.len() < 240 || m[0] != 2 || be32(m, 4) != xid || m[28..34] != mac || m[236..240] != MAGIC {
        return None;
    }
    let mut opts = Vec::new();
    let mut kind = 0;
    let mut i = 240;
    while i < m.len() {
        match m[i] {
            0 => i += 1,
            255 => break,
            code => {
                let len = *m.get(i + 1)? as usize;
                let data = m.get(i + 2..i + 2 + len)?;
                if code == 53 && len == 1 {
                    kind = data[0];
                }
                opts.push((code, data.to_vec()));
                i += 2 + len;
            }
        }
    }
    Some((kind, Ipv4(be32(m, 16)), opts))
}

fn opt_ip(opts: &Options, code: u8) -> Ipv4 {
    opts.iter()
        .find(|(c, d)| *c == code && d.len() >= 4)
        .map_or(Ipv4::UNSPECIFIED, |(_, d)| Ipv4(be32(d, 0)))
}

impl Client {
    pub fn new(xid: u32) -> Self {
        Self {
            xid,
            state: State::Init,
            last_sent: 0,
            output: None,
        }
    }

    pub fn bound(&self) -> bool {
        self.state == State::Bound
    }

    pub fn take_output(&mut self) -> Option<Vec<u8>> {
        self.output.take()
    }

    fn send(&mut self, mac: Mac, now: u64) {
        self.last_sent = now;
        self.output = match self.state {
            State::Init | State::Selecting => {
                self.state = State::Selecting;
                Some(message(DISCOVER, self.xid, mac, &[]))
            }
            State::Requesting { offered, server } => Some(message(
                REQUEST,
                self.xid,
                mac,
                &[(50, &offered.octets()), (54, &server.octets())],
            )),
            State::Bound => None,
        };
    }

    pub fn poll(&mut self, mac: Mac, now: u64) {
        match self.state {
            State::Bound => {}
            State::Init => self.send(mac, now),
            _ if now >= self.last_sent + RETRY_MS => {
                // No answer: start over.
                self.state = State::Init;
                self.send(mac, now);
            }
            _ => {}
        }
    }

    /// Handles a server message; returns the configuration once bound.
    pub fn receive(&mut self, m: &[u8], mac: Mac, now: u64) -> Option<Config> {
        let (kind, yiaddr, opts) = parse(m, self.xid, mac)?;
        match (self.state, kind) {
            (State::Selecting, OFFER) => {
                self.state = State::Requesting {
                    offered: yiaddr,
                    server: opt_ip(&opts, 54),
                };
                self.send(mac, now);
                None
            }
            (State::Requesting { offered, .. }, ACK) if yiaddr == offered => {
                self.state = State::Bound;
                let mask = opt_ip(&opts, 1);
                Some(Config {
                    ip: yiaddr,
                    netmask: if mask == Ipv4::UNSPECIFIED {
                        Ipv4::new(255, 255, 255, 0)
                    } else {
                        mask
                    },
                    gateway: opt_ip(&opts, 3),
                    dns: opt_ip(&opts, 6),
                })
            }
            (State::Requesting { .. }, NAK) => {
                self.state = State::Init;
                self.send(mac, now);
                None
            }
            _ => None,
        }
    }
}

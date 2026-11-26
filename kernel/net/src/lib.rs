//! The Velora network stack (no_std + alloc, tested on the host).
//!
//! One Ethernet interface; IPv4 with ARP, ICMP echo, UDP, a DHCP client and
//! TCP. The stack does no I/O itself: the driver feeds received frames to
//! [`Stack::receive`], calls [`Stack::poll`] for timers and sends what
//! [`Stack::take_frame`] returns. Time is passed in as milliseconds.
//!
//! TCP: connection setup and teardown (including half-close), MSS option,
//! the peer's window, retransmission with exponential back-off.
//! NOT IMPLEMENTED: out-of-order reassembly (out-of-order segments are
//! dropped and recovered by retransmission), congestion control, window
//! scaling, SACK, IP fragmentation, IPv6.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod dhcp;
mod tcp;
pub mod wire;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
pub use tcp::TcpState;
use wire::*;

pub use wire::{Ipv4, Mac};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetError {
    /// No address yet, or no route to the destination.
    NoRoute,
    /// The peer refused the connection (RST to our SYN).
    Refused,
    TimedOut,
    /// The peer reset the connection.
    Reset,
    /// The port is in use.
    InUse,
    NotConnected,
    /// The socket was closed.
    Closed,
    Invalid,
    /// A buffer is full; try again later.
    WouldBlock,
}

/// Socket handle.
pub type Handle = u32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub ip: Ipv4,
    pub netmask: Ipv4,
    pub gateway: Ipv4,
    pub dns: Ipv4,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub rx_frames: u64,
    pub tx_frames: u64,
    pub rx_dropped: u64,
}

struct Pending {
    next_hop: Ipv4,
    packet: Vec<u8>,
    since: u64,
}

struct UdpSocket {
    port: u16,
    rx: VecDeque<(Ipv4, u16, Vec<u8>)>,
}

const ARP_TIMEOUT_MS: u64 = 3000;
const ARP_RETRY_MS: u64 = 1000;
const UDP_QUEUE: usize = 64;
const EPHEMERAL: u16 = 49152;

pub struct Stack {
    mac: Mac,
    config: Option<Config>,
    arp: BTreeMap<Ipv4, Mac>,
    arp_asked: BTreeMap<Ipv4, u64>,
    pending: Vec<Pending>,
    unreachable: Vec<Ipv4>,
    tx: VecDeque<Vec<u8>>,
    ip_id: u16,
    next_handle: Handle,
    next_port: u16,
    udp: BTreeMap<Handle, UdpSocket>,
    tcp: tcp::Tcp,
    echo_replies: Vec<(Ipv4, u16, u16)>,
    dhcp: Option<dhcp::Client>,
    pub counters: Counters,
    now: u64,
}

impl Stack {
    pub fn new(mac: Mac, seed: u64) -> Self {
        Self {
            mac,
            config: None,
            arp: BTreeMap::new(),
            arp_asked: BTreeMap::new(),
            pending: Vec::new(),
            unreachable: Vec::new(),
            tx: VecDeque::new(),
            ip_id: seed as u16,
            next_handle: 1,
            next_port: EPHEMERAL + (seed % 1000) as u16,
            udp: BTreeMap::new(),
            tcp: tcp::Tcp::new(seed),
            echo_replies: Vec::new(),
            dhcp: None,
            counters: Counters::default(),
            now: 0,
        }
    }

    pub fn mac(&self) -> Mac {
        self.mac
    }

    pub fn config(&self) -> Option<Config> {
        self.config
    }

    /// Sets a static address (and stops DHCP).
    pub fn set_config(&mut self, c: Config) {
        self.config = Some(c);
        self.dhcp = None;
    }

    /// Starts asking for an address with DHCP.
    pub fn start_dhcp(&mut self, now: u64) {
        self.now = now;
        let xid = self.tcp.random() as u32;
        self.dhcp = Some(dhcp::Client::new(xid));
        self.poll(now);
    }

    pub fn dhcp_done(&self) -> bool {
        self.dhcp.as_ref().is_some_and(|d| d.bound())
    }

    /// The next frame to send, if any.
    pub fn take_frame(&mut self) -> Option<Vec<u8>> {
        let f = self.tx.pop_front();
        if f.is_some() {
            self.counters.tx_frames += 1;
        }
        f
    }

    fn alloc_handle(&mut self) -> Handle {
        let h = self.next_handle;
        self.next_handle += 1;
        h
    }

    fn ephemeral_port(&mut self) -> u16 {
        loop {
            let p = self.next_port;
            self.next_port = if p == u16::MAX { EPHEMERAL } else { p + 1 };
            if !self.udp.values().any(|s| s.port == p) && !self.tcp.port_in_use(p) {
                return p;
            }
        }
    }

    // --- sending ------------------------------------------------------------

    fn src_ip(&self) -> Ipv4 {
        self.config.map_or(Ipv4::UNSPECIFIED, |c| c.ip)
    }

    /// Sends an IPv4 packet, resolving the next hop with ARP.
    fn send_ip(&mut self, dst: Ipv4, proto: u8, payload: &[u8]) -> Result<(), NetError> {
        let h = IpHeader {
            src: self.src_ip(),
            dst,
            proto,
            ttl: 64,
        };
        self.ip_id = self.ip_id.wrapping_add(1);
        let packet = ipv4(&h, self.ip_id, payload);
        if dst == Ipv4::BROADCAST {
            self.tx
                .push_back(ethernet(BROADCAST_MAC, self.mac, ETHERTYPE_IPV4, &packet));
            return Ok(());
        }
        let c = self.config.ok_or(NetError::NoRoute)?;
        let next_hop = if dst.in_subnet(c.ip, c.netmask) {
            dst
        } else if c.gateway != Ipv4::UNSPECIFIED {
            c.gateway
        } else {
            return Err(NetError::NoRoute);
        };
        if let Some(&mac) = self.arp.get(&next_hop) {
            self.tx
                .push_back(ethernet(mac, self.mac, ETHERTYPE_IPV4, &packet));
            return Ok(());
        }
        self.pending.push(Pending {
            next_hop,
            packet,
            since: self.now,
        });
        self.ask_arp(next_hop);
        Ok(())
    }

    fn ask_arp(&mut self, ip: Ipv4) {
        if self
            .arp_asked
            .get(&ip)
            .is_some_and(|&t| self.now < t + ARP_RETRY_MS)
        {
            return;
        }
        self.arp_asked.insert(ip, self.now);
        let a = arp(1, self.mac, self.src_ip(), [0; 6], ip);
        self.tx
            .push_back(ethernet(BROADCAST_MAC, self.mac, ETHERTYPE_ARP, &a));
    }

    // --- receiving ----------------------------------------------------------

    /// Processes one received Ethernet frame.
    pub fn receive(&mut self, frame: &[u8], now: u64) {
        self.now = now;
        self.tcp.now = now;
        self.counters.rx_frames += 1;
        if frame.len() < ETH_HEADER {
            self.counters.rx_dropped += 1;
            return;
        }
        let dst: Mac = frame[0..6].try_into().unwrap();
        if dst != self.mac && dst != BROADCAST_MAC {
            return;
        }
        let src_mac: Mac = frame[6..12].try_into().unwrap();
        let payload = &frame[ETH_HEADER..];
        match be16(frame, 12) {
            ETHERTYPE_ARP => self.receive_arp(payload),
            ETHERTYPE_IPV4 => self.receive_ip(src_mac, payload),
            _ => {}
        }
        self.flush_tcp();
    }

    fn learn(&mut self, ip: Ipv4, mac: Mac) {
        self.arp.insert(ip, mac);
        self.arp_asked.remove(&ip);
        self.unreachable.retain(|&u| u != ip);
        let (ready, rest): (Vec<Pending>, Vec<Pending>) = core::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| p.next_hop == ip);
        self.pending = rest;
        for p in ready {
            self.tx
                .push_back(ethernet(mac, self.mac, ETHERTYPE_IPV4, &p.packet));
        }
    }

    fn receive_arp(&mut self, a: &[u8]) {
        if a.len() < 28 || be16(a, 0) != 1 || be16(a, 2) != ETHERTYPE_IPV4 || a[4] != 6 || a[5] != 4
        {
            return;
        }
        let op = be16(a, 6);
        let sha: Mac = a[8..14].try_into().unwrap();
        let spa = Ipv4(be32(a, 14));
        let tpa = Ipv4(be32(a, 24));
        let me = self.src_ip();
        if spa != Ipv4::UNSPECIFIED && (tpa == me || self.arp.contains_key(&spa)) {
            self.learn(spa, sha);
        }
        if op == 1 && me != Ipv4::UNSPECIFIED && tpa == me {
            let r = arp(2, self.mac, me, sha, spa);
            self.tx
                .push_back(ethernet(sha, self.mac, ETHERTYPE_ARP, &r));
        }
    }

    fn receive_ip(&mut self, src_mac: Mac, p: &[u8]) {
        let Some(ip) = parse_ipv4(p) else {
            self.counters.rx_dropped += 1;
            return;
        };
        let me = self.src_ip();
        let for_us = ip.dst == me || ip.dst == Ipv4::BROADCAST || me == Ipv4::UNSPECIFIED;
        if !for_us {
            return;
        }
        // Learn the sender if it is on our link.
        if let Some(c) = self.config
            && ip.src.in_subnet(c.ip, c.netmask)
            && !self.arp.contains_key(&ip.src)
        {
            self.learn(ip.src, src_mac);
        }
        match ip.proto {
            PROTO_ICMP => self.receive_icmp(ip.src, ip.payload),
            PROTO_UDP => self.receive_udp(ip.src, ip.dst, ip.payload),
            PROTO_TCP => {
                if let Some(seg) = parse_tcp(ip.src, ip.dst, ip.payload) {
                    self.tcp.receive(ip.src, me, &seg, self.now);
                }
            }
            _ => {}
        }
    }

    fn receive_icmp(&mut self, src: Ipv4, m: &[u8]) {
        if m.len() < 8 || checksum(m) != 0 {
            return;
        }
        match m[0] {
            8 => {
                let mut r = m.to_vec();
                r[0] = 0;
                r[2..4].fill(0);
                let c = checksum(&r);
                r[2..4].copy_from_slice(&c.to_be_bytes());
                let _ = self.send_ip(src, PROTO_ICMP, &r);
            }
            0 => self.echo_replies.push((src, be16(m, 4), be16(m, 6))),
            _ => {}
        }
    }

    fn receive_udp(&mut self, src: Ipv4, dst: Ipv4, u: &[u8]) {
        if u.len() < UDP_HEADER {
            return;
        }
        let len = be16(u, 4) as usize;
        if len < UDP_HEADER || len > u.len() {
            return;
        }
        let u = &u[..len];
        if be16(u, 6) != 0 && transport_checksum(src, dst, PROTO_UDP, u) != 0 {
            self.counters.rx_dropped += 1;
            return;
        }
        let (sport, dport, data) = (be16(u, 0), be16(u, 2), &u[UDP_HEADER..]);
        if dport == dhcp::CLIENT_PORT && sport == dhcp::SERVER_PORT {
            if let Some(mut d) = self.dhcp.take() {
                if let Some(cfg) = d.receive(data, self.mac, self.now) {
                    self.config = Some(cfg);
                }
                let out = d.take_output();
                self.dhcp = Some(d);
                if let Some(msg) = out {
                    self.send_dhcp(&msg);
                }
            }
            return;
        }
        if let Some(s) = self.udp.values_mut().find(|s| s.port == dport)
            && s.rx.len() < UDP_QUEUE
        {
            s.rx.push_back((src, sport, data.to_vec()));
        }
    }

    fn send_dhcp(&mut self, msg: &[u8]) {
        let u = udp(
            Ipv4::UNSPECIFIED,
            dhcp::CLIENT_PORT,
            Ipv4::BROADCAST,
            dhcp::SERVER_PORT,
            msg,
        );
        let h = IpHeader {
            src: Ipv4::UNSPECIFIED,
            dst: Ipv4::BROADCAST,
            proto: PROTO_UDP,
            ttl: 64,
        };
        self.ip_id = self.ip_id.wrapping_add(1);
        let p = ipv4(&h, self.ip_id, &u);
        self.tx
            .push_back(ethernet(BROADCAST_MAC, self.mac, ETHERTYPE_IPV4, &p));
    }

    /// Timers: ARP retries, DHCP retransmission, TCP retransmission.
    pub fn poll(&mut self, now: u64) {
        self.now = now;
        self.tcp.now = now;
        // ARP: retry, then give up on the destination.
        let waiting: Vec<Ipv4> = self.pending.iter().map(|p| p.next_hop).collect();
        for ip in waiting {
            self.ask_arp(ip);
        }
        let (old, keep): (Vec<Pending>, Vec<Pending>) = core::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| now >= p.since + ARP_TIMEOUT_MS);
        self.pending = keep;
        for p in old {
            if !self.unreachable.contains(&p.next_hop) {
                self.unreachable.push(p.next_hop);
            }
        }
        if let Some(mut d) = self.dhcp.take() {
            d.poll(self.mac, now);
            let out = d.take_output();
            self.dhcp = Some(d);
            if let Some(msg) = out {
                self.send_dhcp(&msg);
            }
        }
        self.tcp.poll(now);
        self.flush_tcp();
    }

    fn flush_tcp(&mut self) {
        for (dst, seg) in self.tcp.take_output() {
            // Without a route the segment is lost; TCP retransmits it.
            let _ = self.send_ip(dst, PROTO_TCP, &seg);
        }
        // Connections whose next hop never answered ARP.
        if !self.unreachable.is_empty() {
            let gw = self.config.map(|c| (c.ip, c.netmask, c.gateway));
            let unreachable = self.unreachable.clone();
            self.tcp.fail_unreachable(|dst| {
                let hop = match gw {
                    Some((ip, mask, g)) if !dst.in_subnet(ip, mask) => g,
                    _ => dst,
                };
                unreachable.contains(&hop)
            });
        }
    }

    // --- ICMP -----------------------------------------------------------------

    /// Sends an echo request.
    pub fn ping(&mut self, dst: Ipv4, id: u16, seq: u16, payload: &[u8]) -> Result<(), NetError> {
        let mut m = alloc::vec![8u8, 0, 0, 0];
        m.extend_from_slice(&id.to_be_bytes());
        m.extend_from_slice(&seq.to_be_bytes());
        m.extend_from_slice(payload);
        let c = checksum(&m);
        m[2..4].copy_from_slice(&c.to_be_bytes());
        self.send_ip(dst, PROTO_ICMP, &m)
    }

    /// True (once) if the echo reply for (`dst`, `id`, `seq`) arrived.
    pub fn take_echo_reply(&mut self, dst: Ipv4, id: u16, seq: u16) -> bool {
        match self.echo_replies.iter().position(|&r| r == (dst, id, seq)) {
            Some(i) => {
                self.echo_replies.remove(i);
                true
            }
            None => {
                if self.echo_replies.len() > 64 {
                    self.echo_replies.remove(0);
                }
                false
            }
        }
    }

    // --- UDP ------------------------------------------------------------------

    /// A UDP socket on `port` (0 = any free port).
    pub fn udp_bind(&mut self, port: u16) -> Result<Handle, NetError> {
        let port = if port == 0 {
            self.ephemeral_port()
        } else {
            port
        };
        if self.udp.values().any(|s| s.port == port) || port == dhcp::CLIENT_PORT {
            return Err(NetError::InUse);
        }
        let h = self.alloc_handle();
        self.udp.insert(
            h,
            UdpSocket {
                port,
                rx: VecDeque::new(),
            },
        );
        Ok(h)
    }

    pub fn udp_send(
        &mut self,
        h: Handle,
        dst: Ipv4,
        dport: u16,
        data: &[u8],
    ) -> Result<usize, NetError> {
        let port = self.udp.get(&h).ok_or(NetError::Invalid)?.port;
        if data.len() > MTU - IP_HEADER - UDP_HEADER {
            return Err(NetError::Invalid);
        }
        let u = udp(self.src_ip(), port, dst, dport, data);
        self.send_ip(dst, PROTO_UDP, &u)?;
        Ok(data.len())
    }

    pub fn udp_recv(&mut self, h: Handle) -> Result<Option<(Ipv4, u16, Vec<u8>)>, NetError> {
        Ok(self
            .udp
            .get_mut(&h)
            .ok_or(NetError::Invalid)?
            .rx
            .pop_front())
    }

    pub fn udp_close(&mut self, h: Handle) {
        self.udp.remove(&h);
    }

    // --- TCP ------------------------------------------------------------------

    pub fn tcp_connect(&mut self, dst: Ipv4, port: u16) -> Result<Handle, NetError> {
        let c = self.config.ok_or(NetError::NoRoute)?;
        let local = self.ephemeral_port();
        let h = self.alloc_handle();
        self.tcp.connect(h, c.ip, local, dst, port, self.now);
        self.flush_tcp();
        Ok(h)
    }

    pub fn tcp_listen(&mut self, port: u16) -> Result<Handle, NetError> {
        if port == 0 || self.tcp.port_in_use(port) {
            return Err(NetError::InUse);
        }
        let h = self.alloc_handle();
        self.tcp.listen(h, port);
        Ok(h)
    }

    /// An established connection from the listener's queue, if any.
    pub fn tcp_accept(&mut self, listener: Handle) -> Result<Option<Handle>, NetError> {
        let next = &mut self.next_handle;
        self.tcp.accept(listener, || {
            let h = *next;
            *next += 1;
            h
        })
    }

    pub fn tcp_state(&self, h: Handle) -> Option<TcpState> {
        self.tcp.state(h)
    }

    /// The error that ended the connection, if any.
    pub fn tcp_error(&self, h: Handle) -> Option<NetError> {
        self.tcp.error(h)
    }

    /// Queues data; returns how much fit in the send buffer.
    pub fn tcp_send(&mut self, h: Handle, data: &[u8]) -> Result<usize, NetError> {
        let n = self.tcp.send(h, data)?;
        self.flush_tcp();
        Ok(n)
    }

    /// Received data; `Ok(0)` means the peer closed (end of input),
    /// `Err(WouldBlock)` that nothing has arrived yet.
    pub fn tcp_recv(&mut self, h: Handle, buf: &mut [u8]) -> Result<usize, NetError> {
        let r = self.tcp.recv(h, buf);
        self.flush_tcp();
        r
    }

    /// Ends the sending side; the socket can still receive.
    pub fn tcp_shutdown(&mut self, h: Handle) -> Result<(), NetError> {
        let r = self.tcp.shutdown(h);
        self.flush_tcp();
        r
    }

    /// Closes the sending side (sends FIN after the queued data) and frees
    /// the socket once the connection is finished.
    pub fn tcp_close(&mut self, h: Handle) {
        self.tcp.close(h);
        self.flush_tcp();
    }
}

#[cfg(test)]
mod tests;

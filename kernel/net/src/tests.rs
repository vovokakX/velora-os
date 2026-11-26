use super::*;
use crate::wire::*;

const MAC_A: Mac = [2, 0, 0, 0, 0, 0xa];
const MAC_B: Mac = [2, 0, 0, 0, 0, 0xb];
const IP_A: Ipv4 = Ipv4::new(10, 0, 0, 1);
const IP_B: Ipv4 = Ipv4::new(10, 0, 0, 2);

fn cfg(ip: Ipv4) -> Config {
    Config {
        ip,
        netmask: Ipv4::new(255, 255, 255, 0),
        gateway: Ipv4::UNSPECIFIED,
        dns: Ipv4::UNSPECIFIED,
    }
}

/// Two stacks on one link.
struct Link {
    a: Stack,
    b: Stack,
    now: u64,
    /// Drops the n-th frame from A to B when `drop_a(n)` is true.
    drop_a: fn(u64) -> bool,
    sent_a: u64,
}

impl Link {
    fn new() -> Self {
        let mut a = Stack::new(MAC_A, 1);
        let mut b = Stack::new(MAC_B, 2);
        a.set_config(cfg(IP_A));
        b.set_config(cfg(IP_B));
        Link {
            a,
            b,
            now: 1000,
            drop_a: |_| false,
            sent_a: 0,
        }
    }

    /// Moves frames until both sides are quiet, then advances time by `ms`.
    fn run(&mut self, ms: u64) {
        let end = self.now + ms;
        loop {
            let mut moved = false;
            while let Some(f) = self.a.take_frame() {
                self.sent_a += 1;
                if !(self.drop_a)(self.sent_a) {
                    self.b.receive(&f, self.now);
                }
                moved = true;
            }
            while let Some(f) = self.b.take_frame() {
                self.a.receive(&f, self.now);
                moved = true;
            }
            if !moved {
                if self.now >= end {
                    break;
                }
                self.now += 10;
                self.a.poll(self.now);
                self.b.poll(self.now);
            }
        }
    }
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7 + i / 251) as u8).collect()
}

#[test]
fn arp_and_ping() {
    let mut l = Link::new();
    l.a.ping(IP_B, 7, 1, b"velora").unwrap();
    l.run(100);
    assert!(l.a.take_echo_reply(IP_B, 7, 1));
    assert!(!l.a.take_echo_reply(IP_B, 7, 1), "a reply is reported once");
    assert_eq!(l.a.arp.get(&IP_B), Some(&MAC_B));
    assert_eq!(
        l.b.arp.get(&IP_A),
        Some(&MAC_A),
        "B learned A from the request"
    );
}

#[test]
fn udp_exchange() {
    let mut l = Link::new();
    let sa = l.a.udp_bind(0).unwrap();
    let sb = l.b.udp_bind(5000).unwrap();
    assert_eq!(l.b.udp_bind(5000), Err(NetError::InUse));
    l.a.udp_send(sa, IP_B, 5000, b"question").unwrap();
    l.run(50);
    let (src, port, data) = l.b.udp_recv(sb).unwrap().unwrap();
    assert_eq!((src, data.as_slice()), (IP_A, &b"question"[..]));
    l.b.udp_send(sb, src, port, b"answer").unwrap();
    l.run(50);
    assert_eq!(l.a.udp_recv(sa).unwrap().unwrap().2, b"answer");
    assert_eq!(l.a.udp_recv(sa).unwrap(), None);
}

fn connect(l: &mut Link, port: u16) -> (Handle, Handle, Handle) {
    let lst = l.b.tcp_listen(port).unwrap();
    let c = l.a.tcp_connect(IP_B, port).unwrap();
    assert_eq!(l.a.tcp_state(c), Some(TcpState::SynSent));
    l.run(100);
    assert_eq!(l.a.tcp_state(c), Some(TcpState::Established));
    let s =
        l.b.tcp_accept(lst)
            .unwrap()
            .expect("no connection to accept");
    assert_eq!(l.b.tcp_state(s), Some(TcpState::Established));
    (c, lst, s)
}

/// Sends `data` from `c` on A and collects it on `s` on B.
fn transfer(l: &mut Link, c: Handle, s: Handle, data: &[u8]) -> Vec<u8> {
    let mut sent = 0;
    let mut got = Vec::new();
    let mut buf = [0u8; 3000];
    for _ in 0..100_000 {
        if sent < data.len() {
            match l.a.tcp_send(c, &data[sent..]) {
                Ok(n) => sent += n,
                Err(NetError::WouldBlock) => {}
                Err(e) => panic!("send: {e:?}"),
            }
        }
        l.run(10);
        loop {
            match l.b.tcp_recv(s, &mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(NetError::WouldBlock) => break,
                Err(e) => panic!("recv: {e:?}"),
            }
        }
        if got.len() == data.len() {
            break;
        }
    }
    got
}

#[test]
fn tcp_transfer_and_close() {
    let mut l = Link::new();
    let (c, _lst, s) = connect(&mut l, 80);
    let data = pattern(200_000);
    assert_eq!(transfer(&mut l, c, s, &data), data);
    // A closes its side; B sees the end of input but can still answer.
    l.a.tcp_close(c);
    l.run(50);
    let mut buf = [0u8; 64];
    assert_eq!(l.b.tcp_recv(s, &mut buf), Ok(0));
    assert_eq!(l.b.tcp_state(s), Some(TcpState::CloseWait));
    l.b.tcp_send(s, b"bye").unwrap();
    l.b.tcp_close(s);
    l.run(5000);
    assert_eq!(l.b.tcp_state(s), None, "B's socket is freed after LAST-ACK");
    assert_eq!(
        l.a.tcp_state(c),
        None,
        "A's socket is freed after TIME-WAIT"
    );
}

#[test]
fn tcp_half_close_reply_arrives() {
    let mut l = Link::new();
    let (c, _lst, s) = connect(&mut l, 7);
    l.a.tcp_send(c, b"ping").unwrap();
    l.a.tcp_shutdown(c).unwrap(); // FIN after the data
    l.run(50);
    let mut buf = [0u8; 16];
    assert_eq!(l.b.tcp_recv(s, &mut buf), Ok(4));
    assert_eq!(l.b.tcp_recv(s, &mut buf), Ok(0));
    // The answer still reaches A, whose receiving side is open.
    l.b.tcp_send(s, b"pong").unwrap();
    l.b.tcp_close(s);
    l.run(50);
    assert_eq!(l.a.tcp_recv(c, &mut buf).ok(), Some(4));
    assert_eq!(&buf[..4], b"pong");
    assert_eq!(l.a.tcp_recv(c, &mut buf), Ok(0));
    assert_eq!(l.a.tcp_send(c, b"more"), Err(NetError::Closed));
    l.a.tcp_close(c);
    l.run(5000);
    assert_eq!(l.a.tcp_state(c), None);
}

#[test]
fn tcp_survives_packet_loss() {
    let mut l = Link::new();
    // About one frame in five from A is lost, in an irregular pattern.
    l.drop_a = |n| n > 3 && (n.wrapping_mul(2_654_435_761) >> 7) % 5 == 0;
    let (c, _lst, s) = connect(&mut l, 9000);
    let data = pattern(60_000);
    let got = transfer(&mut l, c, s, &data);
    let first_bad = got.iter().zip(&data).position(|(a, b)| a != b);
    assert!(
        got.len() == data.len() && first_bad.is_none(),
        "got {} bytes, first difference at {:?}",
        got.len(),
        first_bad
    );
}

#[test]
fn tcp_refused_and_timed_out() {
    let mut l = Link::new();
    let c = l.a.tcp_connect(IP_B, 81).unwrap();
    l.run(100);
    assert_eq!(l.a.tcp_state(c), Some(TcpState::Closed));
    assert_eq!(l.a.tcp_error(c), Some(NetError::Refused));
    let mut buf = [0u8; 4];
    assert_eq!(l.a.tcp_recv(c, &mut buf), Err(NetError::Refused));
    // The peer stops answering after the handshake.
    let (c, _lst, _s) = connect(&mut l, 82);
    l.drop_a = |_| true;
    l.a.tcp_send(c, b"lost").unwrap();
    l.run(120_000);
    assert_eq!(l.a.tcp_error(c), Some(NetError::TimedOut));
}

#[test]
fn unknown_host_has_no_route() {
    let mut l = Link::new();
    let c = l.a.tcp_connect(Ipv4::new(10, 0, 0, 99), 80).unwrap();
    l.run(5000);
    assert_eq!(l.a.tcp_error(c), Some(NetError::NoRoute));
    let mut s = Stack::new(MAC_A, 3);
    assert_eq!(s.tcp_connect(IP_B, 80), Err(NetError::NoRoute));
}

/// A minimal DHCP server answering B's role.
fn dhcp_server(frame: &[u8]) -> Option<Vec<u8>> {
    let ip = parse_ipv4(&frame[ETH_HEADER..])?;
    let u = ip.payload;
    if be16(u, 2) != 67 {
        return None;
    }
    let m = &u[UDP_HEADER..];
    let xid = be32(m, 4);
    let mac: Mac = m[28..34].try_into().unwrap();
    let kind = m[242];
    let reply_kind = match kind {
        1 => 2, // DISCOVER -> OFFER
        3 => 5, // REQUEST -> ACK
        _ => return None,
    };
    let mut r = vec![0u8; 240];
    r[0] = 2;
    r[1] = 1;
    r[2] = 6;
    r[4..8].copy_from_slice(&xid.to_be_bytes());
    r[16..20].copy_from_slice(&Ipv4::new(10, 0, 2, 15).octets());
    r[28..34].copy_from_slice(&mac);
    r[236..240].copy_from_slice(&[99, 130, 83, 99]);
    r.extend_from_slice(&[53, 1, reply_kind]);
    r.extend_from_slice(&[1, 4, 255, 255, 255, 0]);
    r.extend_from_slice(&[3, 4, 10, 0, 2, 2]);
    r.extend_from_slice(&[6, 4, 10, 0, 2, 3]);
    r.extend_from_slice(&[54, 4, 10, 0, 2, 2]);
    r.push(255);
    let server = Ipv4::new(10, 0, 2, 2);
    let udp_seg = udp(server, 67, Ipv4::BROADCAST, 68, &r);
    let h = IpHeader {
        src: server,
        dst: Ipv4::BROADCAST,
        proto: PROTO_UDP,
        ttl: 64,
    };
    Some(ethernet(
        BROADCAST_MAC,
        MAC_B,
        ETHERTYPE_IPV4,
        &ipv4(&h, 1, &udp_seg),
    ))
}

#[test]
fn dhcp_gets_an_address() {
    let mut a = Stack::new(MAC_A, 5);
    assert_eq!(a.config(), None);
    a.start_dhcp(0);
    let mut now = 0;
    for _ in 0..10 {
        while let Some(f) = a.take_frame() {
            if let Some(reply) = dhcp_server(&f) {
                a.receive(&reply, now);
            }
        }
        if a.dhcp_done() {
            break;
        }
        now += 100;
        a.poll(now);
    }
    let c = a.config().expect("no address");
    assert_eq!(c.ip, Ipv4::new(10, 0, 2, 15));
    assert_eq!(c.gateway, Ipv4::new(10, 0, 2, 2));
    assert_eq!(c.dns, Ipv4::new(10, 0, 2, 3));
    assert_eq!(c.netmask, Ipv4::new(255, 255, 255, 0));
}

#[test]
fn dhcp_retries_without_answers() {
    let mut a = Stack::new(MAC_A, 6);
    a.start_dhcp(0);
    let first = a.take_frame().expect("no DISCOVER");
    assert_eq!(&first[0..6], &BROADCAST_MAC);
    a.poll(1000);
    assert!(a.take_frame().is_none(), "no retry before 2 s");
    a.poll(2500);
    assert!(a.take_frame().is_some(), "DISCOVER is sent again");
}

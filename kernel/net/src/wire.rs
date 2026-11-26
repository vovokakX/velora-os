//! Packet formats: Ethernet, ARP, IPv4, ICMP, UDP, TCP headers and the
//! Internet checksum.

use alloc::vec::Vec;

pub type Mac = [u8; 6];
pub const BROADCAST_MAC: Mac = [0xff; 6];

/// An IPv4 address in network byte order as a u32 (a.b.c.d = a<<24|...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Ipv4(pub u32);

impl Ipv4 {
    pub const UNSPECIFIED: Ipv4 = Ipv4(0);
    pub const BROADCAST: Ipv4 = Ipv4(0xffff_ffff);

    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Ipv4(u32::from_be_bytes([a, b, c, d]))
    }

    pub fn octets(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    pub fn parse(s: &str) -> Option<Ipv4> {
        let mut out = [0u8; 4];
        let mut parts = s.split('.');
        for o in &mut out {
            let p = parts.next()?;
            if p.is_empty() || p.len() > 3 || !p.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            *o = p.parse().ok()?;
        }
        parts
            .next()
            .is_none()
            .then_some(Ipv4(u32::from_be_bytes(out)))
    }

    pub fn in_subnet(self, other: Ipv4, mask: Ipv4) -> bool {
        self.0 & mask.0 == other.0 & mask.0
    }
}

impl core::fmt::Display for Ipv4 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let o = self.octets();
        write!(f, "{}.{}.{}.{}", o[0], o[1], o[2], o[3])
    }
}

pub const ETH_HEADER: usize = 14;
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const IP_HEADER: usize = 20;
pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;
pub const UDP_HEADER: usize = 8;
pub const TCP_HEADER: usize = 20;
/// Largest IPv4 packet sent (Ethernet MTU).
pub const MTU: usize = 1500;

pub fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}
pub fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}

/// Ones' complement sum of 16-bit words (not yet inverted).
pub fn sum16(mut acc: u32, data: &[u8]) -> u32 {
    let (pairs, rest) = data.as_chunks::<2>();
    for p in pairs {
        acc += u16::from_be_bytes(*p) as u32;
    }
    if let [last] = rest {
        acc += (*last as u32) << 8;
    }
    acc
}

pub fn fold(mut acc: u32) -> u16 {
    while acc >> 16 != 0 {
        acc = (acc & 0xffff) + (acc >> 16);
    }
    !(acc as u16)
}

/// The Internet checksum of `data`.
pub fn checksum(data: &[u8]) -> u16 {
    fold(sum16(0, data))
}

/// Checksum of a TCP or UDP segment with the IPv4 pseudo-header.
pub fn transport_checksum(src: Ipv4, dst: Ipv4, proto: u8, seg: &[u8]) -> u16 {
    let mut acc = sum16(0, &src.octets());
    acc = sum16(acc, &dst.octets());
    acc += proto as u32;
    acc += seg.len() as u32;
    fold(sum16(acc, seg))
}

pub fn ethernet(dst: Mac, src: Mac, ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(ETH_HEADER + payload.len().max(46));
    f.extend_from_slice(&dst);
    f.extend_from_slice(&src);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    if f.len() < 60 {
        f.resize(60, 0); // minimum frame size (without FCS)
    }
    f
}

pub fn arp(op: u16, sha: Mac, spa: Ipv4, tha: Mac, tpa: Ipv4) -> Vec<u8> {
    let mut a = Vec::with_capacity(28);
    a.extend_from_slice(&1u16.to_be_bytes());
    a.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
    a.push(6);
    a.push(4);
    a.extend_from_slice(&op.to_be_bytes());
    a.extend_from_slice(&sha);
    a.extend_from_slice(&spa.octets());
    a.extend_from_slice(&tha);
    a.extend_from_slice(&tpa.octets());
    a
}

pub struct IpHeader {
    pub src: Ipv4,
    pub dst: Ipv4,
    pub proto: u8,
    pub ttl: u8,
}

pub fn ipv4(h: &IpHeader, id: u16, payload: &[u8]) -> Vec<u8> {
    let total = IP_HEADER + payload.len();
    let mut p = Vec::with_capacity(total);
    p.push(0x45);
    p.push(0);
    p.extend_from_slice(&(total as u16).to_be_bytes());
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&0x4000u16.to_be_bytes()); // don't fragment
    p.push(h.ttl);
    p.push(h.proto);
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(&h.src.octets());
    p.extend_from_slice(&h.dst.octets());
    let c = checksum(&p[..IP_HEADER]);
    p[10..12].copy_from_slice(&c.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

/// A parsed IPv4 packet: header fields and the payload range.
pub struct Ipv4Packet<'a> {
    pub src: Ipv4,
    pub dst: Ipv4,
    pub proto: u8,
    pub payload: &'a [u8],
}

pub fn parse_ipv4(p: &[u8]) -> Option<Ipv4Packet<'_>> {
    if p.len() < IP_HEADER || p[0] >> 4 != 4 {
        return None;
    }
    let ihl = (p[0] & 0xf) as usize * 4;
    let total = be16(p, 2) as usize;
    if ihl < IP_HEADER || total < ihl || total > p.len() || checksum(&p[..ihl]) != 0 {
        return None;
    }
    let frag = be16(p, 6);
    if frag & 0x3fff != 0 {
        return None; // fragments are not reassembled
    }
    Some(Ipv4Packet {
        src: Ipv4(be32(p, 12)),
        dst: Ipv4(be32(p, 16)),
        proto: p[9],
        payload: &p[ihl..total],
    })
}

pub fn udp(src: Ipv4, sport: u16, dst: Ipv4, dport: u16, data: &[u8]) -> Vec<u8> {
    let len = UDP_HEADER + data.len();
    let mut u = Vec::with_capacity(len);
    u.extend_from_slice(&sport.to_be_bytes());
    u.extend_from_slice(&dport.to_be_bytes());
    u.extend_from_slice(&(len as u16).to_be_bytes());
    u.extend_from_slice(&[0, 0]);
    u.extend_from_slice(data);
    let mut c = transport_checksum(src, dst, PROTO_UDP, &u);
    if c == 0 {
        c = 0xffff;
    }
    u[6..8].copy_from_slice(&c.to_be_bytes());
    u
}

pub mod tcp_flags {
    pub const FIN: u8 = 0x01;
    pub const SYN: u8 = 0x02;
    pub const RST: u8 = 0x04;
    pub const PSH: u8 = 0x08;
    pub const ACK: u8 = 0x10;
}

pub struct TcpSegment<'a> {
    pub sport: u16,
    pub dport: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    /// MSS option on SYN segments.
    pub mss: Option<u16>,
    pub data: &'a [u8],
}

#[allow(clippy::too_many_arguments)]
pub fn tcp(
    src: Ipv4,
    sport: u16,
    dst: Ipv4,
    dport: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    mss: Option<u16>,
    data: &[u8],
) -> Vec<u8> {
    let hlen = TCP_HEADER + if mss.is_some() { 4 } else { 0 };
    let mut t = Vec::with_capacity(hlen + data.len());
    t.extend_from_slice(&sport.to_be_bytes());
    t.extend_from_slice(&dport.to_be_bytes());
    t.extend_from_slice(&seq.to_be_bytes());
    t.extend_from_slice(&ack.to_be_bytes());
    t.push(((hlen / 4) as u8) << 4);
    t.push(flags);
    t.extend_from_slice(&window.to_be_bytes());
    t.extend_from_slice(&[0, 0, 0, 0]);
    if let Some(m) = mss {
        t.extend_from_slice(&[2, 4]);
        t.extend_from_slice(&m.to_be_bytes());
    }
    t.extend_from_slice(data);
    let c = transport_checksum(src, dst, PROTO_TCP, &t);
    t[16..18].copy_from_slice(&c.to_be_bytes());
    t
}

pub fn parse_tcp(src: Ipv4, dst: Ipv4, t: &[u8]) -> Option<TcpSegment<'_>> {
    if t.len() < TCP_HEADER || transport_checksum(src, dst, PROTO_TCP, t) != 0 {
        return None;
    }
    let hlen = (t[12] >> 4) as usize * 4;
    if hlen < TCP_HEADER || hlen > t.len() {
        return None;
    }
    let mut mss = None;
    let mut i = TCP_HEADER;
    while i < hlen {
        match t[i] {
            0 => break,
            1 => i += 1,
            kind => {
                let len = *t.get(i + 1)? as usize;
                if len < 2 || i + len > hlen {
                    return None;
                }
                if kind == 2 && len == 4 {
                    mss = Some(be16(t, i + 2));
                }
                i += len;
            }
        }
    }
    Some(TcpSegment {
        sport: be16(t, 0),
        dport: be16(t, 2),
        seq: be32(t, 4),
        ack: be32(t, 8),
        flags: t[13],
        window: be16(t, 14),
        mss,
        data: &t[hlen..],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_rfc1071_example() {
        // RFC 1071 section 3: 00 01 f2 03 f4 f5 f6 f7 sums to ddf2.
        let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(
            sum16(0, &data) as u16 + (sum16(0, &data) >> 16) as u16,
            0xddf2
        );
        assert_eq!(checksum(&data), !0xddf2);
    }

    #[test]
    fn ipv4_header_known_value() {
        // A sample header with checksum b1e6 (common textbook example).
        let h = [
            0x45, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0x00, 0x00, 0xc0, 0xa8,
            0x00, 0x01, 0xc0, 0xa8, 0x00, 0xc7,
        ];
        assert_eq!(checksum(&h), 0xb861);
        let p = ipv4(
            &IpHeader {
                src: Ipv4::new(192, 168, 0, 1),
                dst: Ipv4::new(192, 168, 0, 199),
                proto: PROTO_UDP,
                ttl: 64,
            },
            0,
            &[0u8; 95],
        );
        assert_eq!(be16(&p, 10), 0xb861);
        let parsed = parse_ipv4(&p).unwrap();
        assert_eq!(parsed.dst, Ipv4::new(192, 168, 0, 199));
        assert_eq!(parsed.payload.len(), 95);
    }

    #[test]
    fn addresses() {
        assert_eq!(Ipv4::parse("10.0.2.15"), Some(Ipv4::new(10, 0, 2, 15)));
        assert_eq!(Ipv4::parse("10.0.2"), None);
        assert_eq!(Ipv4::parse("10.0.2.256"), None);
        assert_eq!(Ipv4::parse("1.2.3.4.5"), None);
        assert_eq!(alloc::format!("{}", Ipv4::new(10, 0, 2, 2)), "10.0.2.2");
        let mask = Ipv4::new(255, 255, 255, 0);
        assert!(Ipv4::new(10, 0, 2, 2).in_subnet(Ipv4::new(10, 0, 2, 15), mask));
        assert!(!Ipv4::new(10, 0, 3, 2).in_subnet(Ipv4::new(10, 0, 2, 15), mask));
    }

    #[test]
    fn tcp_and_udp_round_trip() {
        let (a, b) = (Ipv4::new(10, 0, 0, 1), Ipv4::new(10, 0, 0, 2));
        let seg = tcp(
            a,
            1000,
            b,
            80,
            7,
            9,
            tcp_flags::SYN,
            4096,
            Some(1460),
            b"hi",
        );
        let t = parse_tcp(a, b, &seg).unwrap();
        assert_eq!((t.sport, t.dport, t.seq, t.ack), (1000, 80, 7, 9));
        assert_eq!(
            (t.flags, t.window, t.mss, t.data),
            (tcp_flags::SYN, 4096, Some(1460), &b"hi"[..])
        );
        let mut bad = seg.clone();
        bad[25] ^= 1;
        assert!(
            parse_tcp(a, b, &bad).is_none(),
            "checksum must catch a flipped bit"
        );
        let u = udp(a, 68, b, 67, b"x");
        assert_eq!(transport_checksum(a, b, PROTO_UDP, &u), 0);
    }
}

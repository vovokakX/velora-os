//! DEFLATE decompression (RFC 1951) inside a zlib stream (RFC 1950), as
//! PNG uses it. A small canonical-Huffman decoder that reads one bit at a
//! time: simple and fast enough for images of a few megabytes.

use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input ended in the middle of the stream.
    Truncated,
    /// Not a valid zlib/DEFLATE stream.
    Corrupt,
    /// The zlib checksum does not match the data.
    Checksum,
    /// The output would be larger than the given limit.
    TooLarge,
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    count: u32,
}

impl Bits<'_> {
    fn bits(&mut self, need: u32) -> Result<u32, Error> {
        let mut val = self.buf;
        while self.count < need {
            let b = *self.data.get(self.pos).ok_or(Error::Truncated)?;
            self.pos += 1;
            val |= (b as u32) << self.count;
            self.count += 8;
        }
        self.buf = val >> need;
        self.count -= need;
        Ok(val & ((1 << need) - 1))
    }

    fn align(&mut self) {
        self.buf = 0;
        self.count = 0;
    }
}

const MAX_BITS: usize = 15;

struct Huffman {
    /// Number of codes of each length.
    count: [u16; MAX_BITS + 1],
    /// Symbols ordered by code.
    symbol: Vec<u16>,
}

impl Huffman {
    /// Builds the canonical code for the given code lengths. Incomplete
    /// codes are allowed (a lone distance code is common); over-subscribed
    /// ones are not.
    fn new(lengths: &[u8]) -> Result<Self, Error> {
        let mut count = [0u16; MAX_BITS + 1];
        for &l in lengths {
            count[l as usize] += 1;
        }
        let mut left: i32 = 1;
        for &c in &count[1..] {
            left = left * 2 - c as i32;
            if left < 0 {
                return Err(Error::Corrupt);
            }
        }
        let mut offs = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Self { count, symbol })
    }

    fn decode(&self, b: &mut Bits) -> Result<u16, Error> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAX_BITS {
            code |= b.bits(1)? as i32;
            let count = self.count[len] as i32;
            if code - count < first {
                return Ok(self.symbol[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::Corrupt)
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn codes(
    b: &mut Bits,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
    limit: usize,
) -> Result<(), Error> {
    loop {
        let sym = lit.decode(b)? as usize;
        if sym < 256 {
            if out.len() >= limit {
                return Err(Error::TooLarge);
            }
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else {
            let i = sym - 257;
            if i >= 29 {
                return Err(Error::Corrupt);
            }
            let len = LEN_BASE[i] as usize + b.bits(LEN_EXTRA[i] as u32)? as usize;
            let d = dist.decode(b)? as usize;
            if d >= 30 {
                return Err(Error::Corrupt);
            }
            let back = DIST_BASE[d] as usize + b.bits(DIST_EXTRA[d] as u32)? as usize;
            if back > out.len() {
                return Err(Error::Corrupt);
            }
            if out.len() + len > limit {
                return Err(Error::TooLarge);
            }
            let start = out.len() - back;
            for k in 0..len {
                let c = out[start + k];
                out.push(c);
            }
        }
    }
}

fn fixed() -> (Huffman, Huffman) {
    let mut l = [0u8; 288];
    l[..144].fill(8);
    l[144..256].fill(9);
    l[256..280].fill(7);
    l[280..].fill(8);
    // The fixed tables are always valid.
    (
        Huffman::new(&l).unwrap_or_else(|_| unreachable!()),
        Huffman::new(&[5u8; 30]).unwrap_or_else(|_| unreachable!()),
    )
}

fn dynamic(b: &mut Bits) -> Result<(Huffman, Huffman), Error> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let nlen = b.bits(5)? as usize + 257;
    let ndist = b.bits(5)? as usize + 1;
    let ncode = b.bits(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err(Error::Corrupt);
    }
    let mut cl = [0u8; 19];
    for &o in &ORDER[..ncode] {
        cl[o] = b.bits(3)? as u8;
    }
    let clh = Huffman::new(&cl)?;
    let mut lengths = [0u8; 286 + 30];
    let mut i = 0;
    while i < nlen + ndist {
        let sym = clh.decode(b)?;
        if sym < 16 {
            lengths[i] = sym as u8;
            i += 1;
            continue;
        }
        let (val, rep) = match sym {
            16 => {
                if i == 0 {
                    return Err(Error::Corrupt);
                }
                (lengths[i - 1], 3 + b.bits(2)? as usize)
            }
            17 => (0, 3 + b.bits(3)? as usize),
            _ => (0, 11 + b.bits(7)? as usize),
        };
        if i + rep > nlen + ndist {
            return Err(Error::Corrupt);
        }
        lengths[i..i + rep].fill(val);
        i += rep;
    }
    if lengths[256] == 0 {
        return Err(Error::Corrupt);
    }
    Ok((
        Huffman::new(&lengths[..nlen])?,
        Huffman::new(&lengths[nlen..nlen + ndist])?,
    ))
}

/// Decompresses a raw DEFLATE stream. Returns the data and the number of
/// input bytes used.
pub fn inflate(data: &[u8], limit: usize) -> Result<(Vec<u8>, usize), Error> {
    let mut b = Bits {
        data,
        pos: 0,
        buf: 0,
        count: 0,
    };
    let mut out = Vec::new();
    loop {
        let last = b.bits(1)? == 1;
        match b.bits(2)? {
            0 => {
                b.align();
                let p = b.pos;
                let hdr = data.get(p..p + 4).ok_or(Error::Truncated)?;
                let len = u16::from_le_bytes([hdr[0], hdr[1]]);
                let nlen = u16::from_le_bytes([hdr[2], hdr[3]]);
                if len != !nlen {
                    return Err(Error::Corrupt);
                }
                let body = data
                    .get(p + 4..p + 4 + len as usize)
                    .ok_or(Error::Truncated)?;
                if out.len() + body.len() > limit {
                    return Err(Error::TooLarge);
                }
                out.extend_from_slice(body);
                b.pos = p + 4 + len as usize;
            }
            1 => {
                let (l, d) = fixed();
                codes(&mut b, &mut out, &l, &d, limit)?;
            }
            2 => {
                let (l, d) = dynamic(&mut b)?;
                codes(&mut b, &mut out, &l, &d, limit)?;
            }
            _ => return Err(Error::Corrupt),
        }
        if last {
            return Ok((out, b.pos));
        }
    }
}

pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    b << 16 | a
}

/// Decompresses a zlib stream (header, DEFLATE data, Adler-32), giving up
/// if the result would exceed `limit` bytes.
pub fn zlib(data: &[u8], limit: usize) -> Result<Vec<u8>, Error> {
    let [cmf, flg, ..] = *data else {
        return Err(Error::Truncated);
    };
    // Method 8 (DEFLATE), no preset dictionary, valid header check.
    if cmf & 0x0f != 8
        || cmf >> 4 > 7
        || flg & 0x20 != 0
        || !(cmf as u16 * 256 + flg as u16).is_multiple_of(31)
    {
        return Err(Error::Corrupt);
    }
    let (out, used) = inflate(&data[2..], limit)?;
    let sum = data.get(2 + used..2 + used + 4).ok_or(Error::Truncated)?;
    if u32::from_be_bytes([sum[0], sum[1], sum[2], sum[3]]) != adler32(&out) {
        return Err(Error::Checksum);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with Python's zlib.compress(); see the comments for the input.
    #[test]
    fn stored_block() {
        // zlib.compress(b"hello", 0)
        let z = [
            0x78, 0x01, 0x01, 0x05, 0x00, 0xfa, 0xff, b'h', b'e', b'l', b'l', b'o', 0x06, 0x2c,
            0x02, 0x15,
        ];
        assert_eq!(zlib(&z, 100).unwrap(), b"hello");
    }

    #[test]
    fn fixed_huffman_with_back_references() {
        // zlib.compress(b"abcabcabcabcabcabc")
        let z = [
            0x78, 0x9c, 0x4b, 0x4c, 0x4a, 0x4e, 0x44, 0x45, 0x00, 0x41, 0x7c, 0x06, 0xe5,
        ];
        assert_eq!(zlib(&z, 100).unwrap(), b"abcabcabcabcabcabc");
        assert_eq!(zlib(&z, 10), Err(Error::TooLarge));
        let mut bad = z;
        bad[12] ^= 1;
        assert_eq!(zlib(&bad, 100), Err(Error::Checksum));
        assert_eq!(zlib(&z[..8], 100), Err(Error::Truncated));
    }

    #[test]
    fn dynamic_huffman() {
        // zlib.compress(b"".join(b"line %d: velora os kitty graphics\n" % i
        //     for i in range(40)), 9)
        let z: [u8; 141] = [
            0x78, 0xda, 0x85, 0xd3, 0xcb, 0x09, 0xc2, 0x50, 0x18, 0x44, 0xe1, 0xbd, 0x55, 0xfc,
            0x25, 0x38, 0x33, 0xbe, 0xbb, 0x09, 0x12, 0xf4, 0xe2, 0xc5, 0x48, 0x12, 0x04, 0xbb,
            0x17, 0x1b, 0xf0, 0xec, 0xcf, 0xea, 0x63, 0xa6, 0xb7, 0xe7, 0x58, 0xdb, 0x4b, 0xbd,
            0xc7, 0x3e, 0xcd, 0x43, 0x4d, 0x4b, 0x3d, 0xda, 0xba, 0x7e, 0xea, 0x36, 0x0f, 0xaf,
            0x7b, 0xbb, 0x2e, 0x9b, 0xfe, 0x0b, 0x44, 0x81, 0x29, 0x08, 0x05, 0x3b, 0x0a, 0xf6,
            0x14, 0x1c, 0x28, 0x38, 0x52, 0x70, 0xa2, 0xe0, 0x8c, 0x50, 0x4c, 0x89, 0x96, 0x42,
            0x4c, 0xa1, 0xa6, 0x90, 0x53, 0xe8, 0x29, 0x04, 0x15, 0x8a, 0x0a, 0x49, 0x85, 0xa6,
            0x46, 0x53, 0xf3, 0x3e, 0xd1, 0xd4, 0x68, 0x6a, 0x34, 0x35, 0x9a, 0x1a, 0x4d, 0x8d,
            0xa6, 0x46, 0x53, 0xa3, 0x69, 0xd0, 0x34, 0x68, 0x1a, 0x3e, 0x3d, 0x9a, 0x06, 0x4d,
            0x83, 0xa6, 0x41, 0xd3, 0xa0, 0x69, 0xd0, 0x34, 0xff, 0x4c, 0xbf, 0x1b, 0xc9, 0xd9,
            0xa8,
        ];
        let mut want = Vec::new();
        for i in 0..40 {
            want.extend_from_slice(
                alloc::format!("line {}: velora os kitty graphics\n", i).as_bytes(),
            );
        }
        assert_eq!(zlib(&z, 1 << 20).unwrap(), want);
    }

    #[test]
    fn adler32_known_value() {
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    }
}

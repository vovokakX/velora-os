//! Standard base64 (RFC 4648, with `+` and `/`), as the Kitty graphics
//! protocol uses for its payload.

use alloc::string::String;
use alloc::vec::Vec;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Appends the base64 form of `data` (with `=` padding) to `out`.
pub fn encode_into(data: &[u8], out: &mut String) {
    out.reserve(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
}

pub fn encode(data: &[u8]) -> String {
    let mut s = String::new();
    encode_into(data, &mut s);
    s
}

fn value(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

/// Decodes base64 one character at a time, so a payload can be decoded
/// as it arrives (also across protocol chunks).
#[derive(Clone, Copy, Debug, Default)]
pub struct Decoder {
    acc: u32,
    bits: u32,
}

impl Decoder {
    pub const fn new() -> Self {
        Self { acc: 0, bits: 0 }
    }

    /// Feeds one character. Padding and whitespace are skipped. Returns
    /// false for a character outside the alphabet.
    pub fn push(&mut self, c: u8, out: &mut Vec<u8>) -> bool {
        if c == b'=' || c.is_ascii_whitespace() {
            return true;
        }
        let Some(v) = value(c) else {
            return false;
        };
        self.acc = (self.acc << 6 | v) & 0xff_ffff;
        self.bits += 6;
        if self.bits >= 8 {
            self.bits -= 8;
            out.push((self.acc >> self.bits) as u8);
        }
        true
    }
}

/// Decodes a whole string; `None` if it has a character outside the
/// alphabet.
pub fn decode(s: &[u8]) -> Option<Vec<u8>> {
    let mut d = Decoder::new();
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for &c in s {
        if !d.push(c, &mut out) {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors() {
        let cases: [(&[u8], &str); 7] = [
            (b"", ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ];
        for (raw, text) in cases {
            assert_eq!(encode(raw), text);
            assert_eq!(decode(text.as_bytes()).unwrap(), raw);
        }
    }

    #[test]
    fn all_bytes_round_trip_in_pieces() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let text = encode(&data);
        // Split at arbitrary points, as protocol chunks may be.
        let mut d = Decoder::new();
        let mut out = Vec::new();
        for part in text.as_bytes().chunks(7) {
            for &c in part {
                assert!(d.push(c, &mut out));
            }
        }
        assert_eq!(out, data);
    }

    #[test]
    fn rejects_bad_characters() {
        assert!(decode(b"Zm9v!").is_none());
        assert_eq!(decode(b"Zm9v\nYmFy").unwrap(), b"foobar");
    }
}

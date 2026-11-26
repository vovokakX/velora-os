//! Users and passwords for Velora OS (no_std + alloc, tested on the host).
//!
//! - SHA-256, HMAC-SHA-256 and PBKDF2-HMAC-SHA-256 (RFC 6234, 2104, 8018).
//! - Password hashes in the form
//!   `$pbkdf2-sha256$<iterations>$<salt hex>$<hash hex>`; verification
//!   compares in constant time.
//! - `/etc/passwd`, `/etc/shadow` and `/etc/group` line parsing.
//!
//! Plain-text passwords are never stored: the build writes only hashes
//! (tools/mkpasswd), and `login` compares hashes.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// --- SHA-256 -------------------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: H0,
            buf: [0; 64],
            buf_len: 0,
            total: 0,
        }
    }

    fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, c) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*c);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        if self.buf_len > 0 {
            let n = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + n].copy_from_slice(&data[..n]);
            self.buf_len += n;
            data = &data[n..];
            if self.buf_len == 64 {
                let b = self.buf;
                Self::compress(&mut self.state, &b);
                self.buf_len = 0;
            }
        }
        let (blocks, rest) = data.as_chunks::<64>();
        for b in blocks {
            Self::compress(&mut self.state, b);
        }
        self.buf[..rest.len()].copy_from_slice(rest);
        self.buf_len += rest.len();
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.total * 8;
        self.update(&[0x80]);
        while self.buf_len != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 32];
        for (o, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *o = s.to_be_bytes();
        }
        out
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finish()
}

/// HMAC-SHA-256 with the key already reduced to its two padded states.
#[derive(Clone)]
struct Hmac {
    inner: Sha256,
    outer: Sha256,
}

impl Hmac {
    fn new(key: &[u8]) -> Self {
        let mut k = [0u8; 64];
        if key.len() > 64 {
            k[..32].copy_from_slice(&sha256(key));
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let mut inner = Sha256::new();
        let mut outer = Sha256::new();
        inner.update(&k.map(|b| b ^ 0x36));
        outer.update(&k.map(|b| b ^ 0x5c));
        Self { inner, outer }
    }

    fn mac(&self, data: &[u8]) -> [u8; 32] {
        let mut i = self.inner.clone();
        i.update(data);
        let mut o = self.outer.clone();
        o.update(&i.finish());
        o.finish()
    }
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    Hmac::new(key).mac(data)
}

/// PBKDF2-HMAC-SHA-256 with a 32-byte output (one block).
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let prf = Hmac::new(password);
    let mut first = Vec::with_capacity(salt.len() + 4);
    first.extend_from_slice(salt);
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = prf.mac(&first);
    let mut t = u;
    for _ in 1..iterations.max(1) {
        u = prf.mac(&u);
        for (a, b) in t.iter_mut().zip(u) {
            *a ^= b;
        }
    }
    t
}

// --- password hashes --------------------------------------------------------------

pub const DEFAULT_ITERATIONS: u32 = 10_000;
const SCHEME: &str = "pbkdf2-sha256";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Hashes a password with the given salt (16 random bytes recommended).
pub fn hash_password(password: &str, salt: &[u8], iterations: u32) -> String {
    let dk = pbkdf2_sha256(password.as_bytes(), salt, iterations);
    format!("${}${}${}${}", SCHEME, iterations, hex(salt), hex(&dk))
}

/// Checks a password against a stored hash. Unknown formats, locked
/// accounts ("!" or "*") and empty hashes never match.
pub fn verify_password(password: &str, stored: &str) -> bool {
    let mut parts = stored.split('$');
    let (Some(""), Some(SCHEME), Some(iter), Some(salt), Some(want), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return false;
    };
    let (Ok(iter), Some(salt), Some(want)) = (iter.parse::<u32>(), unhex(salt), unhex(want)) else {
        return false;
    };
    if want.len() != 32 || iter == 0 || iter > 10_000_000 {
        return false;
    }
    let got = pbkdf2_sha256(password.as_bytes(), &salt, iter);
    // Constant time: look at every byte.
    got.iter().zip(&want).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

// --- account files ----------------------------------------------------------------

/// One line of `/etc/passwd`: `name:x:uid:gid:comment:home:shell`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub comment: String,
    pub home: String,
    pub shell: String,
}

/// One line of `/etc/group`: `name:x:gid:member,member`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub gid: u32,
    pub members: Vec<String>,
}

fn lines(text: &str) -> impl Iterator<Item = Vec<&str>> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split(':').collect())
}

pub fn parse_passwd(text: &str) -> Vec<User> {
    lines(text)
        .filter_map(|f| {
            if f.len() != 7 {
                return None;
            }
            Some(User {
                name: f[0].into(),
                uid: f[2].parse().ok()?,
                gid: f[3].parse().ok()?,
                comment: f[4].into(),
                home: f[5].into(),
                shell: f[6].into(),
            })
        })
        .collect()
}

pub fn parse_group(text: &str) -> Vec<Group> {
    lines(text)
        .filter_map(|f| {
            if f.len() != 4 {
                return None;
            }
            Some(Group {
                name: f[0].into(),
                gid: f[2].parse().ok()?,
                members: f[3]
                    .split(',')
                    .filter(|m| !m.is_empty())
                    .map(String::from)
                    .collect(),
            })
        })
        .collect()
}

/// The stored hash for `user` in `/etc/shadow` (`name:hash:...`).
pub fn shadow_hash<'a>(text: &'a str, user: &str) -> Option<&'a str> {
    text.lines().find_map(|l| {
        let mut f = l.trim().split(':');
        (f.next()? == user).then(|| f.next()).flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // Streaming in odd pieces gives the same result.
        let data: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let mut h = Sha256::new();
        for c in data.chunks(37) {
            h.update(c);
        }
        assert_eq!(h.finish(), sha256(&data));
    }

    #[test]
    fn hmac_rfc4231() {
        // Test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn pbkdf2_vectors() {
        // RFC 7914 section 11 (PBKDF2-HMAC-SHA256).
        let dk = pbkdf2_sha256(b"passwd", b"salt", 1);
        assert_eq!(
            hex(&dk),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"
        );
        let dk = pbkdf2_sha256(b"password", b"salt", 4096);
        assert_eq!(
            hex(&dk),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    #[test]
    fn password_round_trip() {
        let h = hash_password("секрет", &[7; 16], 1000);
        assert!(h.starts_with("$pbkdf2-sha256$1000$07070707"));
        assert!(verify_password("секрет", &h));
        assert!(!verify_password("секреТ", &h));
        assert!(!verify_password("", &h));
        assert!(!verify_password("x", "!"));
        assert!(!verify_password("x", ""));
        assert!(!verify_password("x", "$pbkdf2-sha256$0$00$00"));
    }

    #[test]
    fn account_files() {
        let users = parse_passwd(
            "# comment\nroot:x:0:0:root:/root:/bin/vsh\nuser:x:1000:1000:Velora user:/home/user:/bin/vsh\nbroken:x\n",
        );
        assert_eq!(users.len(), 2);
        assert_eq!(users[1].name, "user");
        assert_eq!((users[1].uid, users[1].gid), (1000, 1000));
        assert_eq!(users[1].home, "/home/user");
        let groups = parse_group("root:x:0:\nwheel:x:10:root,user\n");
        assert_eq!(groups[1].members, ["root", "user"]);
        let shadow = "root:$pbkdf2-sha256$1$00$11:0::::::\nuser:!:0::::::\n";
        assert_eq!(shadow_hash(shadow, "root"), Some("$pbkdf2-sha256$1$00$11"));
        assert_eq!(shadow_hash(shadow, "user"), Some("!"));
        assert_eq!(shadow_hash(shadow, "nobody"), None);
    }
}

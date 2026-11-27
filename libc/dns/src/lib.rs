//! DNS (RFC 1035) without I/O: builds a query for the IPv4 addresses
//! (A records) of a name and reads the answer, following CNAME chains
//! inside one answer. vlibc sends the query over UDP.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub const PORT: u16 = 53;
const TYPE_A: u16 = 1;
const TYPE_CNAME: u16 = 5;
const CLASS_IN: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The name is empty, has an empty label or a label over 63 bytes.
    BadName,
    /// The answer is cut short or points outside itself.
    Malformed,
    /// Not the answer to our query (ID or question differs).
    NotOurs,
    /// The server says the name does not exist.
    NoSuchName,
    /// Another server error (RCODE).
    Server(u8),
    /// The name exists but has no IPv4 address.
    NoAddress,
}

/// A standard query with recursion desired for the A records of `name`.
pub fn query(id: u16, name: &str) -> Result<Vec<u8>, Error> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() || name.len() > 253 {
        return Err(Error::BadName);
    }
    let mut q = Vec::with_capacity(18 + name.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00]); // RD
    q.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // 1 question
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(Error::BadName);
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&TYPE_A.to_be_bytes());
    q.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(q)
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, Error> {
    let s = b.get(o..o + 2).ok_or(Error::Malformed)?;
    Ok(u16::from_be_bytes([s[0], s[1]]))
}

/// Reads a (possibly compressed) name at `pos`. Returns the name in lower
/// case and the position after it in the message.
fn read_name(msg: &[u8], mut pos: usize) -> Result<(String, usize), Error> {
    let mut name = String::new();
    let mut end = None;
    let mut jumps = 0;
    loop {
        let len = *msg.get(pos).ok_or(Error::Malformed)? as usize;
        if len & 0xc0 == 0xc0 {
            let ptr = (u16_at(msg, pos)? & 0x3fff) as usize;
            end.get_or_insert(pos + 2);
            jumps += 1;
            if jumps > 32 || ptr >= msg.len() {
                return Err(Error::Malformed);
            }
            pos = ptr;
            continue;
        }
        if len == 0 {
            return Ok((name, end.unwrap_or(pos + 1)));
        }
        let label = msg.get(pos + 1..pos + 1 + len).ok_or(Error::Malformed)?;
        if !name.is_empty() {
            name.push('.');
        }
        for &c in label {
            name.push(c.to_ascii_lowercase() as char);
        }
        if name.len() > 255 {
            return Err(Error::Malformed);
        }
        pos += 1 + len;
    }
}

/// The IPv4 addresses for `name` in the answer to query `id`.
pub fn parse_answer(msg: &[u8], id: u16, name: &str) -> Result<Vec<[u8; 4]>, Error> {
    if msg.len() < 12 {
        return Err(Error::Malformed);
    }
    if u16_at(msg, 0)? != id || msg[2] & 0x80 == 0 {
        return Err(Error::NotOurs);
    }
    match msg[3] & 0x0f {
        0 => {}
        3 => return Err(Error::NoSuchName),
        r => return Err(Error::Server(r)),
    }
    let qd = u16_at(msg, 4)?;
    let an = u16_at(msg, 6)?;
    let mut pos = 12;
    let want = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    for _ in 0..qd {
        let (q, next) = read_name(msg, pos)?;
        if q != want {
            return Err(Error::NotOurs);
        }
        pos = next + 4;
    }
    // Records may alias the name (CNAME) before giving addresses.
    let mut target = want;
    let mut records = Vec::new();
    for _ in 0..an {
        let (owner, next) = read_name(msg, pos)?;
        let rtype = u16_at(msg, next)?;
        let rdlen = u16_at(msg, next + 8)? as usize;
        let data = next + 10;
        if data + rdlen > msg.len() {
            return Err(Error::Malformed);
        }
        records.push((owner, rtype, data, rdlen));
        pos = data + rdlen;
    }
    for _ in 0..8 {
        let cname = records
            .iter()
            .find(|(o, t, _, _)| *t == TYPE_CNAME && *o == target)
            .map(|(_, _, d, _)| read_name(msg, *d).map(|(n, _)| n))
            .transpose()?;
        match cname {
            Some(n) => target = n,
            None => break,
        }
    }
    let addrs: Vec<[u8; 4]> = records
        .iter()
        .filter(|(o, t, _, l)| *t == TYPE_A && *o == target && *l == 4)
        .map(|(_, _, d, _)| [msg[*d], msg[*d + 1], msg[*d + 2], msg[*d + 3]])
        .collect();
    if addrs.is_empty() {
        return Err(Error::NoAddress);
    }
    Ok(addrs)
}

/// Builds an answer (for tests and for a small server): `name` has
/// `addrs`, optionally reached through a CNAME `alias`.
pub fn answer(id: u16, name: &str, alias: Option<&str>, addrs: &[[u8; 4]]) -> Vec<u8> {
    let q = query(id, name).expect("valid name");
    let mut m = q.clone();
    m[2] = 0x81; // response, RD
    m[3] = 0x80; // RA
    let count = addrs.len() as u16 + alias.is_some() as u16;
    m[6..8].copy_from_slice(&count.to_be_bytes());
    let rr = |m: &mut Vec<u8>, owner: &[u8], rtype: u16, data: &[u8]| {
        m.extend_from_slice(owner);
        m.extend_from_slice(&rtype.to_be_bytes());
        m.extend_from_slice(&CLASS_IN.to_be_bytes());
        m.extend_from_slice(&300u32.to_be_bytes());
        m.extend_from_slice(&(data.len() as u16).to_be_bytes());
        m.extend_from_slice(data);
    };
    let question_name = [0xc0, 12]; // pointer to the name in the question
    let mut owner: Vec<u8> = question_name.to_vec();
    if let Some(a) = alias {
        let mut data = Vec::new();
        for label in a.split('.') {
            data.push(label.len() as u8);
            data.extend_from_slice(label.as_bytes());
        }
        data.push(0);
        let at = m.len() + 12; // where the alias name will start
        rr(&mut m, &question_name, TYPE_CNAME, &data);
        owner = alloc::vec![0xc0 | (at >> 8) as u8, at as u8];
    }
    for a in addrs {
        rr(&mut m, &owner, TYPE_A, a);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_layout() {
        let q = query(0x1234, "example.com.").unwrap();
        assert_eq!(&q[..4], &[0x12, 0x34, 0x01, 0x00]);
        assert_eq!(&q[12..], b"\x07example\x03com\x00\x00\x01\x00\x01");
        assert_eq!(query(1, ""), Err(Error::BadName));
        assert_eq!(query(1, "a..b"), Err(Error::BadName));
        assert_eq!(query(1, &"x".repeat(64)), Err(Error::BadName));
    }

    #[test]
    fn answers() {
        let a = answer(7, "velora.test", None, &[[10, 0, 2, 2], [1, 2, 3, 4]]);
        assert_eq!(
            parse_answer(&a, 7, "Velora.TEST"),
            Ok(vec![[10, 0, 2, 2], [1, 2, 3, 4]])
        );
        assert_eq!(parse_answer(&a, 8, "velora.test"), Err(Error::NotOurs));
        assert_eq!(parse_answer(&a, 7, "other.test"), Err(Error::NotOurs));
        let c = answer(
            9,
            "www.velora.test",
            Some("host.velora.test"),
            &[[5, 6, 7, 8]],
        );
        assert_eq!(
            parse_answer(&c, 9, "www.velora.test"),
            Ok(vec![[5, 6, 7, 8]])
        );
        let none = answer(3, "empty.test", None, &[]);
        assert_eq!(parse_answer(&none, 3, "empty.test"), Err(Error::NoAddress));
    }

    #[test]
    fn errors_and_bad_messages() {
        let mut a = answer(5, "x.test", None, &[[1, 1, 1, 1]]);
        a[3] = 0x83;
        assert_eq!(parse_answer(&a, 5, "x.test"), Err(Error::NoSuchName));
        a[3] = 0x82;
        assert_eq!(parse_answer(&a, 5, "x.test"), Err(Error::Server(2)));
        let a = answer(5, "x.test", None, &[[1, 1, 1, 1]]);
        for cut in [3, 13, a.len() - 2] {
            assert!(parse_answer(&a[..cut], 5, "x.test").is_err());
        }
        // A compression pointer loop.
        let mut l = query(6, "a.b").unwrap();
        l[2] = 0x81;
        l[12] = 0xc0;
        l[13] = 12;
        assert_eq!(parse_answer(&l, 6, "a.b"), Err(Error::Malformed));
    }
}

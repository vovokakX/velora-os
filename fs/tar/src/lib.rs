//! ustar (POSIX tar) archives, used for the initramfs.
//!
//! The reader works on a byte slice without allocating, so the kernel can
//! use the archive in place. The writer (behind `alloc`) is used by the
//! `mkinitrd` host tool and by tests.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

const BLOCK: usize = 512;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// A header has a bad checksum or magic.
    BadHeader(usize),
    /// A header or file runs past the end of the archive.
    Truncated,
    /// A name does not fit into the ustar header (max 255 bytes).
    NameTooLong,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    /// Symlinks, devices, etc. (listed but not interpreted).
    Other(u8),
}

#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    /// Path without a leading "./" or "/".
    name: &'a [u8],
    prefix: &'a [u8],
    pub kind: Kind,
    pub mode: u32,
    pub data: &'a [u8],
}

impl<'a> Entry<'a> {
    /// Writes the full path ("prefix/name") into `out` and returns it.
    pub fn path<'b>(&self, out: &'b mut [u8; 256]) -> &'b str {
        let mut n = 0;
        if !self.prefix.is_empty() {
            out[..self.prefix.len()].copy_from_slice(self.prefix);
            n = self.prefix.len();
            out[n] = b'/';
            n += 1;
        }
        out[n..n + self.name.len()].copy_from_slice(self.name);
        n += self.name.len();
        let s = core::str::from_utf8(&out[..n]).unwrap_or("");
        let s = s.trim_start_matches("./").trim_start_matches('/');
        s.trim_end_matches('/')
    }

    /// True if this entry's path equals `path` (leading '/' ignored).
    pub fn is(&self, path: &str) -> bool {
        let mut buf = [0u8; 256];
        self.path(&mut buf) == path.trim_start_matches('/')
    }
}

fn field(b: &[u8]) -> &[u8] {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    &b[..end]
}

fn octal(b: &[u8]) -> Option<u64> {
    let mut v: u64 = 0;
    let mut seen = false;
    for &c in b {
        match c {
            b'0'..=b'7' => {
                v = v.checked_mul(8)?.checked_add((c - b'0') as u64)?;
                seen = true;
            }
            b' ' | 0 if seen => break,
            b' ' | 0 => {}
            _ => return None,
        }
    }
    Some(v)
}

fn checksum(h: &[u8]) -> u64 {
    h.iter()
        .enumerate()
        .map(|(i, &c)| {
            if (148..156).contains(&i) {
                b' ' as u64
            } else {
                c as u64
            }
        })
        .sum()
}

/// Iterates over the entries of an archive.
pub struct Archive<'a> {
    data: &'a [u8],
    off: usize,
    failed: bool,
}

impl<'a> Archive<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            off: 0,
            failed: false,
        }
    }

    /// Finds a file by path ("bin/hello" or "/bin/hello").
    pub fn find(data: &'a [u8], path: &str) -> Option<Entry<'a>> {
        Archive::new(data)
            .filter_map(Result::ok)
            .find(|e| e.is(path))
    }
}

impl<'a> Iterator for Archive<'a> {
    type Item = Result<Entry<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let fail = |s: &mut Self, e| {
            s.failed = true;
            Some(Err(e))
        };
        if self.off + BLOCK > self.data.len() {
            return None; // no end marker; treat end of data as the end
        }
        let h = &self.data[self.off..self.off + BLOCK];
        if h.iter().all(|&b| b == 0) {
            return None;
        }
        let at = self.off;
        if &h[257..262] != b"ustar" {
            return fail(self, Error::BadHeader(at));
        }
        let (Some(sum), Some(size), Some(mode)) = (
            octal(&h[148..156]),
            octal(&h[124..136]),
            octal(&h[100..108]),
        ) else {
            return fail(self, Error::BadHeader(at));
        };
        if sum != checksum(h) {
            return fail(self, Error::BadHeader(at));
        }
        let size = size as usize;
        let start = self.off + BLOCK;
        let Some(end) = start.checked_add(size).filter(|&e| e <= self.data.len()) else {
            return fail(self, Error::Truncated);
        };
        self.off = start + size.div_ceil(BLOCK) * BLOCK;
        let kind = match h[156] {
            b'0' | 0 => Kind::File,
            b'5' => Kind::Directory,
            k => Kind::Other(k),
        };
        Some(Ok(Entry {
            name: field(&h[0..100]),
            prefix: field(&h[345..500]),
            kind,
            mode: mode as u32,
            data: &self.data[start..end],
        }))
    }
}

/// Builds an archive in memory.
#[derive(Default)]
pub struct Builder {
    out: Vec<u8>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    fn header(&mut self, path: &str, kind: u8, mode: u32, size: usize) -> Result<(), Error> {
        let path = path.trim_start_matches('/');
        let (prefix, name) = if path.len() <= 100 {
            ("", path)
        } else {
            // Split at a '/' so that name <= 100 and prefix <= 155.
            let split = path[..path.len().min(156)]
                .rfind('/')
                .ok_or(Error::NameTooLong)?;
            (&path[..split], &path[split + 1..])
        };
        if name.len() > 100 || prefix.len() > 155 || name.is_empty() {
            return Err(Error::NameTooLong);
        }
        let mut h = [0u8; BLOCK];
        h[..name.len()].copy_from_slice(name.as_bytes());
        let put = |h: &mut [u8; BLOCK], at: usize, width: usize, v: u64| {
            let s = alloc::format!("{:0w$o}", v, w = width - 1);
            h[at..at + width - 1].copy_from_slice(s.as_bytes());
        };
        put(&mut h, 100, 8, mode as u64);
        put(&mut h, 108, 8, 0); // uid
        put(&mut h, 116, 8, 0); // gid
        put(&mut h, 124, 12, size as u64);
        put(&mut h, 136, 12, 0); // mtime: fixed for reproducible images
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
        let sum = checksum(&h);
        let s = alloc::format!("{:06o}\0 ", sum);
        h[148..156].copy_from_slice(s.as_bytes());
        self.out.extend_from_slice(&h);
        Ok(())
    }

    pub fn add_file(&mut self, path: &str, mode: u32, data: &[u8]) -> Result<(), Error> {
        self.header(path, b'0', mode, data.len())?;
        self.out.extend_from_slice(data);
        let pad = data.len().div_ceil(BLOCK) * BLOCK - data.len();
        self.out.extend(core::iter::repeat_n(0, pad));
        Ok(())
    }

    pub fn add_dir(&mut self, path: &str, mode: u32) -> Result<(), Error> {
        self.header(path, b'5', mode, 0)
    }

    /// Finishes the archive with the two zero blocks of the end marker.
    pub fn finish(mut self) -> Vec<u8> {
        self.out.extend(core::iter::repeat_n(0, 2 * BLOCK));
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut b = Builder::new();
        b.add_dir("bin", 0o755).unwrap();
        b.add_file("bin/hello", 0o755, b"\x7fELF hello").unwrap();
        b.add_file("/etc/motd", 0o644, b"").unwrap();
        let long = format!("{}/{}", "d".repeat(120), "file.txt");
        b.add_file(&long, 0o600, &[7u8; 1000]).unwrap();
        b.finish()
    }

    #[test]
    fn roundtrip() {
        let t = sample();
        assert_eq!(t.len() % 512, 0);
        let entries: Vec<_> = Archive::new(&t).collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 4);
        let mut buf = [0u8; 256];
        assert_eq!(entries[0].path(&mut buf), "bin");
        assert_eq!(entries[0].kind, Kind::Directory);
        assert_eq!(entries[1].data, b"\x7fELF hello");
        assert_eq!(entries[1].mode, 0o755);
        assert_eq!(entries[2].path(&mut buf), "etc/motd");
        assert!(entries[2].data.is_empty());
        assert_eq!(entries[3].data, &[7u8; 1000][..]);
        assert!(entries[3].path(&mut buf).ends_with("/file.txt"));
    }

    #[test]
    fn find_by_path() {
        let t = sample();
        assert_eq!(
            Archive::find(&t, "/bin/hello").unwrap().data,
            b"\x7fELF hello"
        );
        assert_eq!(Archive::find(&t, "bin/hello").unwrap().kind, Kind::File);
        assert!(Archive::find(&t, "bin/missing").is_none());
    }

    #[test]
    fn corrupt_headers_are_reported() {
        let mut t = sample();
        t[600] ^= 0xff; // inside the second header's name
        let r: Vec<_> = Archive::new(&t).collect();
        assert!(matches!(r.last(), Some(Err(Error::BadHeader(512)))));
        let t = sample();
        let cut = &t[..512 + 512 + 4]; // second file's data cut short
        assert!(Archive::new(cut).any(|e| matches!(e, Err(Error::Truncated))));
    }

    #[test]
    fn names_too_long() {
        let mut b = Builder::new();
        assert_eq!(
            b.add_file(&"x".repeat(101), 0, b""),
            Err(Error::NameTooLong)
        );
    }

    #[test]
    fn octal_parsing() {
        assert_eq!(octal(b"0000644\0"), Some(0o644));
        assert_eq!(octal(b"  12 "), Some(0o12));
        assert_eq!(octal(b"9"), None);
    }
}

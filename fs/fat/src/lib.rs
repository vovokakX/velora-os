//! FAT filesystem support shared by the host tools and (later) the kernel.
//!
//! Current state (stage 1):
//! - [`ImageBuilder`] creates a FAT16 image from an in-memory file tree,
//!   with long file names (VFAT LFN). It is used by `mkiso` for the EFI
//!   boot image on the ISO.
//! - [`read_file`] reads a file back from a FAT16 image (used by tests).
//!
//! Not implemented yet: FAT12/FAT32, writing into an existing image.
//! These come with the filesystem stage (stage 7).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

pub const SECTOR: usize = 512;
const ROOT_ENTRIES: usize = 512;
const RESERVED_SECTORS: usize = 4;
const NUM_FATS: usize = 2;
const DIR_ENTRY: usize = 32;
const FAT16_MIN_CLUSTERS: usize = 4085;
const FAT16_MAX_CLUSTERS: usize = 65524;
const LFN_CHARS: usize = 13;
const MAX_NAME: usize = 255;

const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_LFN: u8 = 0x0f;

/// 2026-01-01 00:00. A fixed timestamp keeps images reproducible.
const FIXED_DATE: u16 = ((2026 - 1980) << 9) | (1 << 5) | 1;
const FIXED_TIME: u16 = 0;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// A path component cannot be stored as a FAT name.
    BadName(String),
    /// A path goes through something that is a file, or a file exists twice.
    PathConflict(String),
    /// The files do not fit in the requested image size.
    TooSmall,
    /// The requested size cannot be formatted as FAT16.
    UnsupportedSize,
    /// The image is not a FAT16 filesystem this reader understands.
    NotFat16,
    NotFound,
    IsADirectory,
    Corrupt(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadName(n) => write!(f, "'{n}' is not a valid FAT file name"),
            Error::PathConflict(p) => write!(f, "path conflict at '{p}'"),
            Error::TooSmall => write!(f, "files do not fit in the image"),
            Error::UnsupportedSize => write!(f, "image size is outside the FAT16 range"),
            Error::NotFat16 => write!(f, "not a FAT16 image"),
            Error::NotFound => write!(f, "file not found"),
            Error::IsADirectory => write!(f, "is a directory"),
            Error::Corrupt(what) => write!(f, "corrupt filesystem: {what}"),
        }
    }
}

// --- names ------------------------------------------------------------------

/// An 8.3 name in on-disk form: 8 bytes of name, 3 of extension, space padded.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ShortName([u8; 11]);

fn short_char(c: u8) -> Option<u8> {
    match c {
        b'a'..=b'z' => Some(c.to_ascii_uppercase()),
        b'A'..=b'Z' | b'0'..=b'9' => Some(c),
        b'_' | b'-' | b'~' | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'(' | b')' | b'{'
        | b'}' | b'^' | b'@' | b'`' => Some(c),
        _ => None,
    }
}

impl ShortName {
    /// Parses a name that is already a valid 8.3 name (case-insensitive).
    pub fn parse(s: &str) -> Result<Self, Error> {
        let bad = || Error::BadName(String::from(s));
        if s.starts_with('.') {
            return Err(bad());
        }
        let (base, ext) = match s.rfind('.') {
            Some(i) => (&s[..i], &s[i + 1..]),
            None => (s, ""),
        };
        if base.is_empty() || base.len() > 8 || ext.len() > 3 {
            return Err(bad());
        }
        let mut out = [b' '; 11];
        for (i, c) in base.bytes().enumerate() {
            out[i] = short_char(c).ok_or_else(bad)?;
        }
        for (i, c) in ext.bytes().enumerate() {
            out[8 + i] = short_char(c).ok_or_else(bad)?;
        }
        Ok(Self(out))
    }

    /// Generates a `BASE~N.EXT` alias for a long name.
    fn alias(long: &str, n: u32) -> Self {
        let (base, ext) = match long.rfind('.') {
            Some(i) if i > 0 => (&long[..i], &long[i + 1..]),
            _ => (long, ""),
        };
        let clean = |s: &str| -> Vec<u8> {
            s.bytes()
                .filter(|&c| c != b'.' && c != b' ')
                .map(|c| short_char(c).unwrap_or(b'_'))
                .collect()
        };
        let mut digits = [0u8; 10];
        let mut len = 0;
        let mut v = n;
        loop {
            digits[len] = b'0' + (v % 10) as u8;
            len += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let tail_len = len + 1; // "~" + digits
        let b = clean(base);
        let keep = b.len().min(8 - tail_len);
        let mut out = [b' '; 11];
        out[..keep].copy_from_slice(&b[..keep]);
        out[keep] = b'~';
        for i in 0..len {
            out[keep + 1 + i] = digits[len - 1 - i];
        }
        let e = clean(ext);
        let elen = e.len().min(3);
        out[8..8 + elen].copy_from_slice(&e[..elen]);
        Self(out)
    }

    pub fn bytes(&self) -> &[u8; 11] {
        &self.0
    }

    fn checksum(&self) -> u8 {
        self.0
            .iter()
            .fold(0u8, |sum, &b| (sum >> 1 | (sum & 1) << 7).wrapping_add(b))
    }
}

/// True if `name` can be stored as a plain short entry (valid 8.3, no
/// lowercase letters).
fn fits_short(name: &str) -> bool {
    ShortName::parse(name).is_ok() && !name.bytes().any(|c| c.is_ascii_lowercase())
}

fn validate_long(name: &str) -> Result<(), Error> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.encode_utf16().count() > MAX_NAME
        || name
            .chars()
            .any(|c| (c as u32) < 0x20 || "\"*/:<>?\\|".contains(c));
    if bad {
        Err(Error::BadName(name.to_string()))
    } else {
        Ok(())
    }
}

/// Case-insensitive key used for directory ordering and conflict checks.
fn fold(name: &str) -> String {
    name.to_uppercase()
}

// --- builder ----------------------------------------------------------------

enum Node {
    File(Vec<u8>),
    Dir(Dir),
}

/// Children keyed by case-folded name; the value keeps the original name.
type Dir = BTreeMap<String, (String, Node)>;

/// Builds a FAT16 image from files added by path.
pub struct ImageBuilder {
    root: Dir,
}

impl Default for ImageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

struct Geometry {
    total_sectors: usize,
    sectors_per_cluster: usize,
    fat_sectors: usize,
    root_sectors: usize,
    clusters: usize,
}

impl Geometry {
    fn fat_start(&self) -> usize {
        RESERVED_SECTORS
    }
    fn root_start(&self) -> usize {
        RESERVED_SECTORS + NUM_FATS * self.fat_sectors
    }
    fn data_start(&self) -> usize {
        self.root_start() + self.root_sectors
    }
    fn cluster_bytes(&self) -> usize {
        self.sectors_per_cluster * SECTOR
    }
    fn cluster_offset(&self, cluster: usize) -> usize {
        (self.data_start() + (cluster - 2) * self.sectors_per_cluster) * SECTOR
    }

    /// Picks the smallest cluster size that gives a valid FAT16 layout.
    fn for_size(total_sectors: usize) -> Result<Self, Error> {
        let root_sectors = ROOT_ENTRIES * DIR_ENTRY / SECTOR;
        for spc in [1usize, 2, 4, 8, 16, 32, 64] {
            let mut fat_sectors = 1;
            for _ in 0..8 {
                let meta = RESERVED_SECTORS + NUM_FATS * fat_sectors + root_sectors;
                if meta >= total_sectors {
                    return Err(Error::UnsupportedSize);
                }
                let clusters = (total_sectors - meta) / spc;
                let needed = ((clusters + 2) * 2).div_ceil(SECTOR);
                if needed == fat_sectors {
                    break;
                }
                fat_sectors = needed;
            }
            let meta = RESERVED_SECTORS + NUM_FATS * fat_sectors + root_sectors;
            let clusters = (total_sectors - meta) / spc;
            if clusters < FAT16_MIN_CLUSTERS {
                return Err(Error::UnsupportedSize);
            }
            if clusters <= FAT16_MAX_CLUSTERS {
                return Ok(Self {
                    total_sectors,
                    sectors_per_cluster: spc,
                    fat_sectors,
                    root_sectors,
                    clusters,
                });
            }
        }
        Err(Error::UnsupportedSize)
    }
}

/// Directory entries for one child: optional LFN entries, then the short one.
fn name_entries(long: &str, short: ShortName, needs_lfn: bool) -> Vec<[u8; 32]> {
    if !needs_lfn {
        return Vec::new();
    }
    let mut units: Vec<u16> = long.encode_utf16().collect();
    let count = units.len().div_ceil(LFN_CHARS);
    if !units.len().is_multiple_of(LFN_CHARS) {
        units.push(0);
    }
    units.resize(count * LFN_CHARS, 0xffff);
    let sum = short.checksum();
    let mut out = Vec::with_capacity(count);
    // Stored last part first; the first stored entry has bit 0x40 set.
    for seq in (1..=count).rev() {
        let part = &units[(seq - 1) * LFN_CHARS..seq * LFN_CHARS];
        let mut e = [0u8; 32];
        e[0] = seq as u8 | if seq == count { 0x40 } else { 0 };
        e[11] = ATTR_LFN;
        e[13] = sum;
        let slots = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
        for (u, off) in part.iter().zip(slots) {
            e[off..off + 2].copy_from_slice(&u.to_le_bytes());
        }
        out.push(e);
    }
    out
}

/// Assigns short names to the children of a directory.
fn plan_names(dir: &Dir) -> Vec<(ShortName, bool)> {
    let mut used = BTreeSet::new();
    // Real short names first, so aliases never collide with them.
    for (name, _) in dir.values() {
        if fits_short(name) {
            used.insert(ShortName::parse(name).unwrap());
        }
    }
    dir.values()
        .map(|(name, _)| {
            if fits_short(name) {
                (ShortName::parse(name).unwrap(), false)
            } else {
                let mut n = 1;
                let alias = loop {
                    let a = ShortName::alias(name, n);
                    if used.insert(a) {
                        break a;
                    }
                    n += 1;
                };
                (alias, true)
            }
        })
        .collect()
}

fn dir_entry_count(dir: &Dir) -> usize {
    dir.values()
        .map(|(name, _)| {
            if fits_short(name) {
                1
            } else {
                1 + name.encode_utf16().count().div_ceil(LFN_CHARS)
            }
        })
        .sum()
}

struct Writer<'a> {
    img: &'a mut [u8],
    geo: &'a Geometry,
    next_cluster: usize,
}

impl Writer<'_> {
    fn alloc_chain(&mut self, bytes: usize) -> Result<usize, Error> {
        let count = bytes.div_ceil(self.geo.cluster_bytes()).max(1);
        let first = self.next_cluster;
        if first + count > self.geo.clusters + 2 {
            return Err(Error::TooSmall);
        }
        for i in 0..count {
            let c = first + i;
            let next = if i + 1 == count {
                0xffff
            } else {
                (c + 1) as u16
            };
            self.set_fat(c, next);
        }
        self.next_cluster += count;
        Ok(first)
    }

    fn set_fat(&mut self, cluster: usize, value: u16) {
        for f in 0..NUM_FATS {
            let off = (self.geo.fat_start() + f * self.geo.fat_sectors) * SECTOR + cluster * 2;
            self.img[off..off + 2].copy_from_slice(&value.to_le_bytes());
        }
    }

    fn write_dir(&mut self, dir: &Dir, own: usize, parent: usize) -> Result<Vec<u8>, Error> {
        let mut raw = Vec::new();
        if own != 0 {
            raw.extend_from_slice(&dir_entry(b".          ", ATTR_DIRECTORY, own, 0));
            raw.extend_from_slice(&dir_entry(b"..         ", ATTR_DIRECTORY, parent, 0));
        }
        let names = plan_names(dir);
        for ((name, node), (short, lfn)) in dir.values().zip(names) {
            for e in name_entries(name, short, lfn) {
                raw.extend_from_slice(&e);
            }
            match node {
                Node::File(data) => {
                    let first = if data.is_empty() {
                        0
                    } else {
                        self.alloc_chain(data.len())?
                    };
                    if first != 0 {
                        let off = self.geo.cluster_offset(first);
                        self.img[off..off + data.len()].copy_from_slice(data);
                    }
                    raw.extend_from_slice(&dir_entry(
                        short.bytes(),
                        ATTR_ARCHIVE,
                        first,
                        data.len() as u32,
                    ));
                }
                Node::Dir(children) => {
                    let size = (dir_entry_count(children) + 2) * DIR_ENTRY;
                    let first = self.alloc_chain(size)?;
                    let body = self.write_dir(children, first, own)?;
                    let off = self.geo.cluster_offset(first);
                    self.img[off..off + body.len()].copy_from_slice(&body);
                    raw.extend_from_slice(&dir_entry(short.bytes(), ATTR_DIRECTORY, first, 0));
                }
            }
        }
        Ok(raw)
    }
}

fn dir_entry(name: &[u8; 11], attr: u8, cluster: usize, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(name);
    e[11] = attr;
    e[14..16].copy_from_slice(&FIXED_TIME.to_le_bytes());
    e[16..18].copy_from_slice(&FIXED_DATE.to_le_bytes());
    e[18..20].copy_from_slice(&FIXED_DATE.to_le_bytes());
    e[22..24].copy_from_slice(&FIXED_TIME.to_le_bytes());
    e[24..26].copy_from_slice(&FIXED_DATE.to_le_bytes());
    e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

impl ImageBuilder {
    pub fn new() -> Self {
        Self {
            root: BTreeMap::new(),
        }
    }

    /// Adds a file. `path` uses '/' separators, e.g. "EFI/BOOT/BOOTX64.EFI".
    /// Intermediate directories are created as needed. Names that are not
    /// valid 8.3 names get a long file name entry.
    pub fn add_file(&mut self, path: &str, data: Vec<u8>) -> Result<(), Error> {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let Some((last, dirs)) = parts.split_last() else {
            return Err(Error::BadName(String::from(path)));
        };
        let mut cur = &mut self.root;
        for d in dirs {
            validate_long(d)?;
            let entry = cur
                .entry(fold(d))
                .or_insert_with(|| (d.to_string(), Node::Dir(BTreeMap::new())));
            cur = match &mut entry.1 {
                Node::Dir(children) => children,
                Node::File(_) => return Err(Error::PathConflict(String::from(path))),
            };
        }
        validate_long(last)?;
        let key = fold(last);
        if cur.contains_key(&key) {
            return Err(Error::PathConflict(String::from(path)));
        }
        cur.insert(key, (last.to_string(), Node::File(data)));
        Ok(())
    }

    /// Total bytes of file and directory data, for sizing the image.
    pub fn content_bytes(&self) -> usize {
        fn walk(d: &Dir) -> usize {
            d.values()
                .map(|(_, n)| match n {
                    Node::File(data) => data.len(),
                    Node::Dir(c) => DIR_ENTRY * (dir_entry_count(c) + 2) + walk(c),
                })
                .sum()
        }
        walk(&self.root)
    }

    /// Produces a FAT16 image of exactly `size` bytes (a multiple of 512).
    pub fn build_fat16(&self, size: usize, label: &str, volume_id: u32) -> Result<Vec<u8>, Error> {
        if !size.is_multiple_of(SECTOR) {
            return Err(Error::UnsupportedSize);
        }
        if dir_entry_count(&self.root) + 1 > ROOT_ENTRIES {
            return Err(Error::TooSmall);
        }
        let geo = Geometry::for_size(size / SECTOR)?;
        let mut img = vec![0u8; size];
        write_boot_sector(&mut img, &geo, label, volume_id);

        let mut w = Writer {
            img: &mut img,
            geo: &geo,
            next_cluster: 2,
        };
        w.set_fat(0, 0xfff8);
        w.set_fat(1, 0xffff);
        let mut root = Vec::new();
        root.extend_from_slice(&dir_entry(&label_bytes(label), ATTR_VOLUME_ID, 0, 0));
        root.extend(w.write_dir(&self.root, 0, 0)?);
        let off = geo.root_start() * SECTOR;
        img[off..off + root.len()].copy_from_slice(&root);
        Ok(img)
    }
}

fn label_bytes(label: &str) -> [u8; 11] {
    let mut out = [b' '; 11];
    for (i, c) in label.bytes().take(11).enumerate() {
        out[i] = c.to_ascii_uppercase();
    }
    out
}

fn write_boot_sector(img: &mut [u8], geo: &Geometry, label: &str, volume_id: u32) {
    let b = &mut img[..SECTOR];
    b[0..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    b[3..11].copy_from_slice(b"VELORA  ");
    b[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    b[13] = geo.sectors_per_cluster as u8;
    b[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
    b[16] = NUM_FATS as u8;
    b[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
    if geo.total_sectors < 0x10000 {
        b[19..21].copy_from_slice(&(geo.total_sectors as u16).to_le_bytes());
    } else {
        b[32..36].copy_from_slice(&(geo.total_sectors as u32).to_le_bytes());
    }
    b[21] = 0xf8;
    b[22..24].copy_from_slice(&(geo.fat_sectors as u16).to_le_bytes());
    b[24..26].copy_from_slice(&32u16.to_le_bytes());
    b[26..28].copy_from_slice(&64u16.to_le_bytes());
    b[36] = 0x80;
    b[38] = 0x29;
    b[39..43].copy_from_slice(&volume_id.to_le_bytes());
    b[43..54].copy_from_slice(&label_bytes(label));
    b[54..62].copy_from_slice(b"FAT16   ");
    // Not bootable as a BIOS volume: halt in a loop.
    b[62] = 0xf4; // hlt
    b[63..65].copy_from_slice(&[0xeb, 0xfd]); // jmp back to hlt
    b[510] = 0x55;
    b[511] = 0xaa;
}

// --- reader -----------------------------------------------------------------

struct Bpb {
    spc: usize,
    fat_start: usize,
    root_start: usize,
    root_entries: usize,
    data_start: usize,
}

fn u16_at(b: &[u8], off: usize) -> usize {
    u16::from_le_bytes([b[off], b[off + 1]]) as usize
}

fn parse_bpb(img: &[u8]) -> Result<Bpb, Error> {
    if img.len() < SECTOR || img[510] != 0x55 || img[511] != 0xaa || &img[54..59] != b"FAT16" {
        return Err(Error::NotFat16);
    }
    if u16_at(img, 11) != SECTOR {
        return Err(Error::NotFat16);
    }
    let spc = img[13] as usize;
    let reserved = u16_at(img, 14);
    let fats = img[16] as usize;
    let root_entries = u16_at(img, 17);
    let fat_sectors = u16_at(img, 22);
    if spc == 0 || fats == 0 || fat_sectors == 0 {
        return Err(Error::Corrupt("zero geometry field"));
    }
    let root_start = reserved + fats * fat_sectors;
    let root_sectors = (root_entries * DIR_ENTRY).div_ceil(SECTOR);
    Ok(Bpb {
        spc,
        fat_start: reserved,
        root_start,
        root_entries,
        data_start: root_start + root_sectors,
    })
}

fn read_chain(img: &[u8], bpb: &Bpb, first: usize, limit: Option<usize>) -> Result<Vec<u8>, Error> {
    let cluster_bytes = bpb.spc * SECTOR;
    let mut out = Vec::new();
    let mut c = first;
    let mut guard = 0;
    while (2..0xfff8).contains(&c) {
        let off = (bpb.data_start + (c - 2) * bpb.spc) * SECTOR;
        let chunk = img
            .get(off..off + cluster_bytes)
            .ok_or(Error::Corrupt("cluster out of range"))?;
        out.extend_from_slice(chunk);
        if limit.is_some_and(|l| out.len() >= l) {
            break;
        }
        c = u16_at(img, bpb.fat_start * SECTOR + c * 2);
        guard += 1;
        if guard > 65536 {
            return Err(Error::Corrupt("cluster chain loop"));
        }
    }
    if let Some(l) = limit {
        if out.len() < l {
            return Err(Error::Corrupt("cluster chain shorter than file"));
        }
        out.truncate(l);
    }
    Ok(out)
}

struct Found {
    cluster: usize,
    size: usize,
    is_dir: bool,
}

/// Looks up `want` in raw directory data, matching long names
/// case-insensitively and falling back to the short name.
fn find_in_dir(dir: &[u8], want: &str) -> Option<Found> {
    let want_fold = fold(want);
    let want_short = ShortName::parse(want).ok();
    let mut lfn: Vec<u16> = Vec::new();
    let mut lfn_sum: Option<u8> = None;
    for e in dir.as_chunks::<DIR_ENTRY>().0 {
        if e[0] == 0 {
            break;
        }
        if e[0] == 0xe5 {
            lfn.clear();
            lfn_sum = None;
            continue;
        }
        if e[11] == ATTR_LFN {
            let seq = (e[0] & 0x1f) as usize;
            if e[0] & 0x40 != 0 {
                lfn = vec![0xffff; seq * LFN_CHARS];
                lfn_sum = Some(e[13]);
            }
            if seq == 0 || seq * LFN_CHARS > lfn.len() {
                lfn_sum = None;
                continue;
            }
            let slots = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
            for (i, off) in slots.iter().enumerate() {
                lfn[(seq - 1) * LFN_CHARS + i] = u16::from_le_bytes([e[*off], e[off + 1]]);
            }
            continue;
        }
        if e[11] & ATTR_VOLUME_ID != 0 {
            lfn_sum = None;
            continue;
        }
        let short = ShortName(e[..11].try_into().unwrap());
        let long_matches = lfn_sum == Some(short.checksum()) && {
            let end = lfn
                .iter()
                .position(|&u| u == 0 || u == 0xffff)
                .unwrap_or(lfn.len());
            let name = String::from_utf16(&lfn[..end]).unwrap_or_default();
            fold(&name) == want_fold
        };
        lfn_sum = None;
        if long_matches || want_short == Some(short) {
            return Some(Found {
                cluster: u16_at(e, 26),
                size: u32::from_le_bytes(e[28..32].try_into().unwrap()) as usize,
                is_dir: e[11] & ATTR_DIRECTORY != 0,
            });
        }
    }
    None
}

/// Reads a whole file from a FAT16 image.
pub fn read_file(img: &[u8], path: &str) -> Result<Vec<u8>, Error> {
    let bpb = parse_bpb(img)?;
    let root_off = bpb.root_start * SECTOR;
    let mut dir: Vec<u8> = img
        .get(root_off..root_off + bpb.root_entries * DIR_ENTRY)
        .ok_or(Error::Corrupt("root directory out of range"))?
        .to_vec();
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    for (i, part) in parts.iter().enumerate() {
        let f = find_in_dir(&dir, part).ok_or(Error::NotFound)?;
        if i + 1 == parts.len() {
            if f.is_dir {
                return Err(Error::IsADirectory);
            }
            return if f.size == 0 {
                Ok(Vec::new())
            } else {
                read_chain(img, &bpb, f.cluster, Some(f.size))
            };
        }
        if !f.is_dir {
            return Err(Error::NotFound);
        }
        dir = read_chain(img, &bpb, f.cluster, None)?;
    }
    Err(Error::IsADirectory)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1024 * 1024;

    #[test]
    fn short_names() {
        assert_eq!(
            ShortName::parse("bootx64.efi").unwrap().bytes(),
            b"BOOTX64 EFI"
        );
        assert_eq!(ShortName::parse("VELORA").unwrap().bytes(), b"VELORA     ");
        assert!(ShortName::parse("toolongname.txt").is_err());
        assert!(ShortName::parse("a.text").is_err());
        assert!(ShortName::parse("sp ace").is_err());
        assert!(ShortName::parse(".hidden").is_err());
        assert!(ShortName::parse("").is_err());
    }

    #[test]
    fn aliases() {
        assert_eq!(ShortName::alias("limine.conf", 1).bytes(), b"LIMINE~1CON");
        assert_eq!(
            ShortName::alias("a very long name.txt", 12).bytes(),
            b"AVERY~12TXT"
        );
        assert_eq!(ShortName::alias("noext", 3).bytes(), b"NOEXT~3    ");
    }

    #[test]
    fn checksum_matches_reference() {
        // Reference value computed with the algorithm from the FAT spec.
        let mut sum = 0u8;
        for &b in b"LIMINE~1CON" {
            sum = (if sum & 1 != 0 { 0x80u8 } else { 0 })
                .wrapping_add(sum >> 1)
                .wrapping_add(b);
        }
        assert_eq!(ShortName(*b"LIMINE~1CON").checksum(), sum);
    }

    #[test]
    fn roundtrip_files_and_dirs() {
        let mut b = ImageBuilder::new();
        let big: Vec<u8> = (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect();
        b.add_file("EFI/BOOT/BOOTX64.EFI", big.clone()).unwrap();
        b.add_file("boot/limine.conf", b"timeout: 0\n".to_vec())
            .unwrap();
        b.add_file("boot/velora", vec![0x7f, b'E', b'L', b'F'])
            .unwrap();
        b.add_file("boot/initramfs.tar", vec![1, 2, 3]).unwrap();
        b.add_file("A rather long directory name/Файл.txt", b"utf16".to_vec())
            .unwrap();
        b.add_file("empty", Vec::new()).unwrap();
        let img = b.build_fat16(8 * MIB, "VELORA", 0x1234_5678).unwrap();
        assert_eq!(img.len(), 8 * MIB);
        assert_eq!(read_file(&img, "EFI/BOOT/BOOTX64.EFI").unwrap(), big);
        assert_eq!(read_file(&img, "efi/boot/bootx64.efi").unwrap(), big);
        assert_eq!(
            read_file(&img, "boot/limine.conf").unwrap(),
            b"timeout: 0\n"
        );
        assert_eq!(
            read_file(&img, "BOOT/LIMINE.CONF").unwrap(),
            b"timeout: 0\n"
        );
        assert_eq!(
            read_file(&img, "boot/LIMINE~1.CON").unwrap(),
            b"timeout: 0\n"
        );
        assert_eq!(read_file(&img, "boot/velora").unwrap(), b"\x7fELF");
        assert_eq!(read_file(&img, "boot/initramfs.tar").unwrap(), [1, 2, 3]);
        assert_eq!(
            read_file(&img, "a rather long directory name/файл.TXT").unwrap(),
            b"utf16"
        );
        assert_eq!(read_file(&img, "empty").unwrap(), b"");
        assert_eq!(read_file(&img, "boot/missing"), Err(Error::NotFound));
        assert_eq!(read_file(&img, "boot"), Err(Error::IsADirectory));
    }

    #[test]
    fn aliases_are_unique() {
        let mut b = ImageBuilder::new();
        b.add_file("limine.conf", b"one".to_vec()).unwrap();
        b.add_file("limine.config", b"two".to_vec()).unwrap();
        let img = b.build_fat16(4 * MIB, "X", 0).unwrap();
        assert_eq!(read_file(&img, "limine.conf").unwrap(), b"one");
        assert_eq!(read_file(&img, "limine.config").unwrap(), b"two");
    }

    #[test]
    fn boot_sector_fields() {
        let img = ImageBuilder::new().build_fat16(4 * MIB, "test", 1).unwrap();
        assert_eq!(&img[510..512], &[0x55, 0xaa]);
        assert_eq!(&img[54..62], b"FAT16   ");
        assert_eq!(&img[43..54], b"TEST       ");
        // Cluster count must be in the FAT16 range or firmware may treat the
        // volume as FAT12/FAT32.
        let geo = Geometry::for_size(4 * MIB / SECTOR).unwrap();
        assert!((FAT16_MIN_CLUSTERS..=FAT16_MAX_CLUSTERS).contains(&geo.clusters));
        let fat1 = RESERVED_SECTORS * SECTOR;
        let fat2 = (RESERVED_SECTORS + geo.fat_sectors) * SECTOR;
        assert_eq!(&img[fat1..fat1 + 4], &[0xf8, 0xff, 0xff, 0xff]);
        assert_eq!(&img[fat2..fat2 + 4], &[0xf8, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn sizes_outside_fat16_are_rejected() {
        assert!(matches!(
            ImageBuilder::new().build_fat16(MIB, "X", 0),
            Err(Error::UnsupportedSize)
        ));
        assert!(matches!(
            ImageBuilder::new().build_fat16(4 * MIB + 1, "X", 0),
            Err(Error::UnsupportedSize)
        ));
    }

    #[test]
    fn too_much_data_is_rejected() {
        let mut b = ImageBuilder::new();
        b.add_file("big", vec![1; 5 * MIB]).unwrap();
        assert_eq!(b.build_fat16(4 * MIB, "X", 0), Err(Error::TooSmall));
    }

    #[test]
    fn bad_and_conflicting_names_are_rejected() {
        let mut b = ImageBuilder::new();
        b.add_file("a/b", vec![1]).unwrap();
        assert!(matches!(
            b.add_file("a/b", vec![2]),
            Err(Error::PathConflict(_))
        ));
        assert!(matches!(
            b.add_file("A/B", vec![2]),
            Err(Error::PathConflict(_))
        ));
        assert!(matches!(
            b.add_file("a/b/c", vec![2]),
            Err(Error::PathConflict(_))
        ));
        assert!(matches!(
            b.add_file("bad:name", vec![]),
            Err(Error::BadName(_))
        ));
        assert!(matches!(b.add_file("x/..", vec![]), Err(Error::BadName(_))));
    }
}

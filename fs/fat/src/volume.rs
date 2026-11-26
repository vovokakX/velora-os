//! A FAT16/FAT32 volume on a block device, read and write, behind the VFS
//! [`FileSystem`] interface; and [`format_fat32`].
//!
//! - Long file names (VFAT) are read and written; names match without
//!   regard to case, like on other systems.
//! - The first FAT is kept in memory (4 bytes per cluster: about 1 MiB for
//!   a 1 GiB volume with 4 KiB clusters). Changed FAT sectors are written
//!   to every FAT copy at the end of each operation.
//! - Inode numbers are the byte position of a file's 8.3 directory entry
//!   (the root is 1). Renaming a file moves its entry, so the number
//!   changes; FAT has no stable file identity.
//! - Owners and permission bits do not exist on FAT: files show as
//!   0644 (0444 when read-only), directories as 0755, owned by root.
//! - FAT has no journal. Changes are written in an order that keeps the
//!   volume readable (data, then FAT, then the directory entry), but a
//!   crash can leave lost clusters. Checking and repair are NOT
//!   IMPLEMENTED; FAT is meant for the EFI system partition and data
//!   exchange, VeloraFS for the system.
//! - FAT12 volumes are refused.

use crate::{
    ATTR_DIRECTORY, ATTR_LFN, ATTR_VOLUME_ID, LFN_CHARS, ShortName, fits_short, fold, name_entries,
};
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use velora_vfs::{
    BlockIo, DirEntry, FileSystem, FsError, FsStat, Ino, Kind, NewAttr, Result, Stat, path,
};

const SECTOR: usize = 512;
const ENTRY: usize = 32;
const ROOT_INO: Ino = 1;
const ATTR_READ_ONLY: u8 = 0x01;
const ATTR_ARCHIVE: u8 = 0x20;
/// End of a cluster chain (normalised; FAT16 stores 0xffff).
const EOC: u32 = 0x0fff_ffff;
const BAD: u32 = 0x0fff_fff7;
const FREE: u32 = 0;
const MAX_FILE: u64 = 0xffff_ffff;
const LFN_SLOTS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FatType {
    Fat16,
    Fat32,
}

// --- time ---------------------------------------------------------------------

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// Seconds since 1970 (UTC) to FAT date and time. FAT has no time zone;
/// Velora stores UTC. Times before 1980 are stored as 1980-01-01.
pub fn to_fat_time(unix: u64) -> (u16, u16) {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    let (y, m, d) = civil_from_days(days);
    if y < 1980 {
        return ((1 << 5) | 1, 0);
    }
    let y = (y - 1980).min(127) as u16;
    let date = (y << 9) | ((m as u16) << 5) | d as u16;
    let time =
        ((secs / 3600) as u16) << 11 | (((secs / 60) % 60) as u16) << 5 | ((secs % 60) / 2) as u16;
    (date, time)
}

pub fn from_fat_time(date: u16, time: u16) -> u64 {
    let y = 1980 + (date >> 9) as i64;
    let m = ((date >> 5) & 15).clamp(1, 12) as u32;
    let d = (date & 31).max(1) as u32;
    let days = days_from_civil(y, m, d);
    let secs = (time >> 11) as u64 * 3600 + ((time >> 5) & 63) as u64 * 60 + (time & 31) as u64 * 2;
    (days.max(0) as u64) * 86_400 + secs
}

// --- entries ------------------------------------------------------------------

fn get16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn get32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn entry_cluster(e: &[u8]) -> u32 {
    (get16(e, 20) as u32) << 16 | get16(e, 26) as u32
}

fn set_entry_cluster(e: &mut [u8], c: u32) {
    e[20..22].copy_from_slice(&((c >> 16) as u16).to_le_bytes());
    e[26..28].copy_from_slice(&(c as u16).to_le_bytes());
}

fn short_display(e: &[u8]) -> String {
    let trim = |b: &[u8], lower: bool| -> String {
        let s: String = b
            .iter()
            .map(|&c| c as char)
            .collect::<String>()
            .trim_end()
            .into();
        if lower { s.to_lowercase() } else { s }
    };
    let mut name = [0u8; 8];
    name.copy_from_slice(&e[..8]);
    if name[0] == 0x05 {
        name[0] = 0xe5;
    }
    let base = trim(&name, e[12] & 0x08 != 0);
    let ext = trim(&e[8..11], e[12] & 0x10 != 0);
    if ext.is_empty() {
        base
    } else {
        alloc::format!("{}.{}", base, ext)
    }
}

/// One directory item: long name (or the short one), its slots.
#[derive(Clone, Debug)]
struct Item {
    name: String,
    short: ShortName,
    attr: u8,
    cluster: u32,
    /// Byte position of the 8.3 entry (the inode number).
    pos: u64,
    /// Positions of all entries of the item (LFN parts and the 8.3 entry).
    slots: Vec<u64>,
}

impl Item {
    fn is_dir(&self) -> bool {
        self.attr & ATTR_DIRECTORY != 0
    }
}

struct Listing {
    items: Vec<Item>,
    /// Every slot position in order, with whether it is free.
    slots: Vec<(u64, bool)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirLoc {
    Root16,
    Chain(u32),
}

pub struct FatVolume<D: BlockIo> {
    dev: D,
    fat_type: FatType,
    spc: u64,
    fat_start: u64,
    fat_sectors: u64,
    nfats: u64,
    root16_start: u64,
    root16_sectors: u64,
    data_start: u64,
    /// Highest valid cluster number + 1.
    cluster_end: u32,
    root_cluster: u32,
    fsinfo: u64,
    fat: Vec<u32>,
    fat_dirty: BTreeSet<u64>,
    free: u32,
    next_free: u32,
    read_only: bool,
}

/// Recognises a FAT16 or FAT32 boot sector.
pub fn probe<D: BlockIo + ?Sized>(dev: &mut D) -> Option<FatType> {
    if dev.sector_size() != SECTOR {
        return None;
    }
    let mut b = [0u8; SECTOR];
    dev.read(0, &mut b).ok()?;
    parse_bpb(&b).ok().map(|g| g.fat_type)
}

struct Geometry {
    fat_type: FatType,
    spc: u64,
    reserved: u64,
    nfats: u64,
    root_entries: u64,
    total: u64,
    fat_sectors: u64,
    root_cluster: u32,
    fsinfo: u64,
    clusters: u64,
}

fn parse_bpb(b: &[u8]) -> Result<Geometry> {
    if b[510] != 0x55 || b[511] != 0xaa || get16(b, 11) as usize != SECTOR {
        return Err(FsError::Invalid);
    }
    let spc = b[13] as u64;
    let reserved = get16(b, 14) as u64;
    let nfats = b[16] as u64;
    let root_entries = get16(b, 17) as u64;
    let total = match get16(b, 19) {
        0 => get32(b, 32) as u64,
        n => n as u64,
    };
    let fat_sectors = match get16(b, 22) {
        0 => get32(b, 36) as u64,
        n => n as u64,
    };
    if spc == 0 || !spc.is_power_of_two() || reserved == 0 || nfats == 0 || fat_sectors == 0 {
        return Err(FsError::Invalid);
    }
    let root_sectors = (root_entries * ENTRY as u64).div_ceil(SECTOR as u64);
    let data_start = reserved + nfats * fat_sectors + root_sectors;
    if data_start >= total {
        return Err(FsError::Invalid);
    }
    let clusters = (total - data_start) / spc;
    let fat_type = if clusters < 4085 {
        return Err(FsError::Unsupported); // FAT12
    } else if clusters < 65525 {
        FatType::Fat16
    } else {
        FatType::Fat32
    };
    let (root_cluster, fsinfo) = if fat_type == FatType::Fat32 {
        if root_entries != 0 {
            return Err(FsError::Invalid);
        }
        (get32(b, 44), get16(b, 48) as u64)
    } else {
        (0, 0)
    };
    let entry_bytes = if fat_type == FatType::Fat32 { 4 } else { 2 };
    if fat_sectors * SECTOR as u64 / entry_bytes < clusters + 2 {
        return Err(FsError::Invalid);
    }
    Ok(Geometry {
        fat_type,
        spc,
        reserved,
        nfats,
        root_entries,
        total,
        fat_sectors,
        root_cluster,
        fsinfo,
        clusters,
    })
}

impl<D: BlockIo> FatVolume<D> {
    pub fn mount(mut dev: D, read_only: bool) -> Result<Self> {
        if dev.sector_size() != SECTOR {
            return Err(FsError::Invalid);
        }
        let mut b = [0u8; SECTOR];
        dev.read(0, &mut b)?;
        let g = parse_bpb(&b)?;
        if g.total > dev.sector_count() {
            return Err(FsError::Corrupt);
        }
        let mut raw = vec![0u8; (g.fat_sectors as usize) * SECTOR];
        dev.read(g.reserved, &mut raw)?;
        let cluster_end = (g.clusters + 2) as u32;
        let fat: Vec<u32> = (0..cluster_end as usize)
            .map(|c| match g.fat_type {
                FatType::Fat32 => {
                    let v = get32(&raw, c * 4) & 0x0fff_ffff;
                    if v >= 0x0fff_fff8 { EOC } else { v }
                }
                FatType::Fat16 => match get16(&raw, c * 2) as u32 {
                    v if v >= 0xfff8 => EOC,
                    0xfff7 => BAD,
                    v => v,
                },
            })
            .collect();
        let free = fat[2..].iter().filter(|&&v| v == FREE).count() as u32;
        let root_sectors = (g.root_entries * ENTRY as u64).div_ceil(SECTOR as u64);
        let v = Self {
            dev,
            fat_type: g.fat_type,
            spc: g.spc,
            fat_start: g.reserved,
            fat_sectors: g.fat_sectors,
            nfats: g.nfats,
            root16_start: g.reserved + g.nfats * g.fat_sectors,
            root16_sectors: root_sectors,
            data_start: g.reserved + g.nfats * g.fat_sectors + root_sectors,
            cluster_end,
            root_cluster: g.root_cluster,
            fsinfo: g.fsinfo,
            fat,
            fat_dirty: BTreeSet::new(),
            free,
            next_free: 2,
            read_only,
        };
        if v.fat_type == FatType::Fat32 && !v.valid_cluster(v.root_cluster) {
            return Err(FsError::Corrupt);
        }
        Ok(v)
    }

    pub fn fat_type(&self) -> FatType {
        self.fat_type
    }

    pub fn into_device(self) -> D {
        self.dev
    }

    fn cluster_bytes(&self) -> u64 {
        self.spc * SECTOR as u64
    }

    fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && c < self.cluster_end
    }

    fn cluster_sector(&self, c: u32) -> u64 {
        self.data_start + (c as u64 - 2) * self.spc
    }

    // --- FAT ------------------------------------------------------------------

    fn set_fat(&mut self, c: u32, v: u32) {
        self.fat[c as usize] = v;
        let per_sector = SECTOR as u64
            / if self.fat_type == FatType::Fat32 {
                4
            } else {
                2
            };
        self.fat_dirty.insert(c as u64 / per_sector);
    }

    fn chain(&self, first: u32) -> Result<Vec<u32>> {
        let mut v = Vec::new();
        let mut c = first;
        while c != EOC {
            if !self.valid_cluster(c) || v.len() >= self.cluster_end as usize {
                return Err(FsError::Corrupt);
            }
            v.push(c);
            c = self.fat[c as usize];
            if c == FREE || c == BAD {
                return Err(FsError::Corrupt);
            }
        }
        Ok(v)
    }

    /// Allocates a cluster, links it after `prev` and zeroes it if asked.
    fn alloc_cluster(&mut self, prev: Option<u32>, zero: bool) -> Result<u32> {
        let n = self.cluster_end;
        let start = self.next_free.clamp(2, n - 1);
        let found = (start..n)
            .chain(2..start)
            .find(|&c| self.fat[c as usize] == FREE);
        let c = found.ok_or(FsError::NoSpace)?;
        self.set_fat(c, EOC);
        if let Some(p) = prev {
            self.set_fat(p, c);
        }
        self.free -= 1;
        self.next_free = c + 1;
        if zero {
            let z = vec![0u8; self.cluster_bytes() as usize];
            self.dev.write(self.cluster_sector(c), &z)?;
        }
        Ok(c)
    }

    fn free_chain(&mut self, first: u32) -> Result<()> {
        if first == 0 {
            return Ok(());
        }
        for c in self.chain(first)? {
            self.set_fat(c, FREE);
            self.free += 1;
        }
        Ok(())
    }

    /// Writes changed FAT sectors to every FAT copy, updates FSInfo and
    /// flushes the device.
    fn commit(&mut self) -> Result<()> {
        let per_sector = SECTOR
            / if self.fat_type == FatType::Fat32 {
                4
            } else {
                2
            };
        for s in core::mem::take(&mut self.fat_dirty) {
            let mut b = [0u8; SECTOR];
            for i in 0..per_sector {
                let c = s as usize * per_sector + i;
                let v = self.fat.get(c).copied().unwrap_or(FREE);
                match self.fat_type {
                    FatType::Fat32 => b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes()),
                    FatType::Fat16 => {
                        let w = match v {
                            EOC => 0xffff,
                            BAD => 0xfff7,
                            v => v as u16,
                        };
                        b[i * 2..i * 2 + 2].copy_from_slice(&w.to_le_bytes())
                    }
                }
            }
            if s == 0 {
                // Entries 0 and 1 carry the media byte and flags, not clusters.
                let mut old = [0u8; SECTOR];
                self.dev.read(self.fat_start, &mut old)?;
                let keep = if self.fat_type == FatType::Fat32 {
                    8
                } else {
                    4
                };
                b[..keep].copy_from_slice(&old[..keep]);
            }
            for f in 0..self.nfats {
                self.dev
                    .write(self.fat_start + f * self.fat_sectors + s, &b)?;
            }
        }
        if self.fsinfo != 0 {
            let mut b = [0u8; SECTOR];
            self.dev.read(self.fsinfo, &mut b)?;
            if get32(&b, 0) == 0x4161_5252 && get32(&b, 484) == 0x6141_7272 {
                b[488..492].copy_from_slice(&self.free.to_le_bytes());
                b[492..496].copy_from_slice(&self.next_free.to_le_bytes());
                self.dev.write(self.fsinfo, &b)?;
            }
        }
        self.dev.flush()
    }

    fn op<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.read_only {
            return Err(FsError::ReadOnly);
        }
        let r = f(self);
        // Even after an error, what was already linked must reach the disk
        // consistently with the directory entries written so far.
        let c = self.commit();
        let v = r?;
        c?;
        Ok(v)
    }

    // --- entries and directories ----------------------------------------------

    fn read_entry(&mut self, pos: u64) -> Result<[u8; ENTRY]> {
        let mut b = [0u8; SECTOR];
        self.dev.read(pos / SECTOR as u64, &mut b)?;
        let o = (pos % SECTOR as u64) as usize;
        Ok(b[o..o + ENTRY].try_into().unwrap())
    }

    fn write_entry(&mut self, pos: u64, e: &[u8; ENTRY]) -> Result<()> {
        let mut b = [0u8; SECTOR];
        let s = pos / SECTOR as u64;
        self.dev.read(s, &mut b)?;
        let o = (pos % SECTOR as u64) as usize;
        b[o..o + ENTRY].copy_from_slice(e);
        self.dev.write(s, &b)
    }

    /// The 8.3 entry of a non-root inode; it must be in use.
    fn entry(&mut self, ino: Ino) -> Result<[u8; ENTRY]> {
        if ino == ROOT_INO || !ino.is_multiple_of(ENTRY as u64) {
            return Err(FsError::NotFound);
        }
        let e = self.read_entry(ino)?;
        if e[0] == 0 || e[0] == 0xe5 || e[11] == ATTR_LFN || e[11] & ATTR_VOLUME_ID != 0 {
            return Err(FsError::NotFound);
        }
        Ok(e)
    }

    fn dir_loc(&mut self, ino: Ino) -> Result<DirLoc> {
        if ino == ROOT_INO {
            return Ok(match self.fat_type {
                FatType::Fat16 => DirLoc::Root16,
                FatType::Fat32 => DirLoc::Chain(self.root_cluster),
            });
        }
        let e = self.entry(ino)?;
        if e[11] & ATTR_DIRECTORY == 0 {
            return Err(FsError::NotDir);
        }
        Ok(DirLoc::Chain(entry_cluster(&e)))
    }

    /// First cluster as stored in ".." entries (0 = root).
    fn dotdot_cluster(&self, loc: DirLoc) -> u32 {
        match loc {
            DirLoc::Root16 => 0,
            DirLoc::Chain(c) if c == self.root_cluster && self.fat_type == FatType::Fat32 => 0,
            DirLoc::Chain(c) => c,
        }
    }

    fn dir_sectors(&self, loc: DirLoc) -> Result<Vec<u64>> {
        Ok(match loc {
            DirLoc::Root16 => {
                (self.root16_start..self.root16_start + self.root16_sectors).collect()
            }
            DirLoc::Chain(first) => self
                .chain(first)?
                .into_iter()
                .flat_map(|c| {
                    let s = self.cluster_sector(c);
                    s..s + self.spc
                })
                .collect(),
        })
    }

    fn list(&mut self, loc: DirLoc) -> Result<Listing> {
        let sectors = self.dir_sectors(loc)?;
        let mut items = Vec::new();
        let mut slots = Vec::new();
        let mut lfn: Vec<u16> = Vec::new();
        let mut lfn_slots: Vec<u64> = Vec::new();
        let mut lfn_sum: Option<u8> = None;
        let mut ended = false;
        let per_cluster = self.spc as usize;
        for run in sectors.chunks(per_cluster.max(1)) {
            let mut buf = vec![0u8; run.len() * SECTOR];
            // Sectors of one cluster are contiguous (and the FAT16 root).
            self.dev.read(run[0], &mut buf)?;
            for (i, e) in buf.as_chunks::<ENTRY>().0.iter().enumerate() {
                let pos = run[0] * SECTOR as u64 + (i * ENTRY) as u64;
                if ended || e[0] == 0 {
                    ended = true;
                    slots.push((pos, true));
                    continue;
                }
                if e[0] == 0xe5 {
                    slots.push((pos, true));
                    lfn_sum = None;
                    continue;
                }
                slots.push((pos, false));
                if e[11] == ATTR_LFN {
                    let seq = (e[0] & 0x1f) as usize;
                    if e[0] & 0x40 != 0 {
                        lfn = vec![0xffff; seq * LFN_CHARS];
                        lfn_sum = Some(e[13]);
                        lfn_slots.clear();
                    }
                    if seq == 0 || seq * LFN_CHARS > lfn.len() || lfn_sum != Some(e[13]) {
                        lfn_sum = None;
                        continue;
                    }
                    for (k, off) in LFN_SLOTS.iter().enumerate() {
                        lfn[(seq - 1) * LFN_CHARS + k] = get16(e, *off);
                    }
                    lfn_slots.push(pos);
                    continue;
                }
                if e[11] & ATTR_VOLUME_ID != 0 || e[0] == b'.' {
                    lfn_sum = None;
                    continue;
                }
                let short = ShortName(e[..11].try_into().unwrap());
                let mut item_slots = Vec::new();
                let long = if lfn_sum == Some(short.checksum()) {
                    let end = lfn
                        .iter()
                        .position(|&u| u == 0 || u == 0xffff)
                        .unwrap_or(lfn.len());
                    item_slots.extend_from_slice(&lfn_slots);
                    String::from_utf16(&lfn[..end]).ok()
                } else {
                    None
                };
                lfn_sum = None;
                item_slots.push(pos);
                items.push(Item {
                    name: long.unwrap_or_else(|| short_display(e)),
                    short,
                    attr: e[11],
                    cluster: entry_cluster(e),
                    pos,
                    slots: item_slots,
                });
            }
        }
        Ok(Listing { items, slots })
    }

    fn find(&mut self, loc: DirLoc, name: &str) -> Result<Option<Item>> {
        let key = fold(name);
        let short = ShortName::parse(name).ok();
        Ok(self
            .list(loc)?
            .items
            .into_iter()
            .find(|i| fold(&i.name) == key || Some(i.short) == short))
    }

    /// Adds directory entries for `name` with the given 8.3 entry fields
    /// (name bytes are filled in here). Returns the 8.3 entry position.
    fn add_entry(&mut self, loc: DirLoc, name: &str, mut short_entry: [u8; ENTRY]) -> Result<u64> {
        validate_name(name)?;
        let listing = self.list(loc)?;
        let key = fold(name);
        if listing.items.iter().any(|i| fold(&i.name) == key) {
            return Err(FsError::Exists);
        }
        let used: BTreeSet<ShortName> = listing.items.iter().map(|i| i.short).collect();
        let (short, needs_lfn) =
            if fits_short(name) && !used.contains(&ShortName::parse(name).unwrap()) {
                (ShortName::parse(name).unwrap(), false)
            } else {
                let mut n = 1;
                loop {
                    let a = ShortName::alias(name, n);
                    if !used.contains(&a) {
                        break (a, true);
                    }
                    n += 1;
                    if n > 999_999 {
                        return Err(FsError::NoSpace);
                    }
                }
            };
        let mut entries = name_entries(name, short, needs_lfn);
        short_entry[..11].copy_from_slice(short.bytes());
        entries.push(short_entry);

        // A run of free slots, growing the directory if needed.
        let mut slots = listing.slots;
        let need = entries.len();
        let run = loop {
            let mut start = None;
            let mut len = 0;
            let mut found = None;
            for (i, &(_, free)) in slots.iter().enumerate() {
                if free {
                    if len == 0 {
                        start = Some(i);
                    }
                    len += 1;
                    if len == need {
                        found = start;
                        break;
                    }
                } else {
                    len = 0;
                }
            }
            if let Some(s) = found {
                break s;
            }
            let DirLoc::Chain(first) = loc else {
                return Err(FsError::NoSpace); // the FAT16 root has a fixed size
            };
            let last = *self.chain(first)?.last().ok_or(FsError::Corrupt)?;
            let c = self.alloc_cluster(Some(last), true)?;
            let s = self.cluster_sector(c) * SECTOR as u64;
            for k in 0..(self.cluster_bytes() / ENTRY as u64) {
                slots.push((s + k * ENTRY as u64, true));
            }
        };
        for (k, e) in entries.iter().enumerate() {
            self.write_entry(slots[run + k].0, e)?;
        }
        Ok(slots[run + need - 1].0)
    }

    fn remove_item(&mut self, item: &Item) -> Result<()> {
        for &p in &item.slots {
            let mut e = self.read_entry(p)?;
            e[0] = 0xe5;
            self.write_entry(p, &e)?;
        }
        Ok(())
    }

    fn dir_is_empty(&mut self, cluster: u32) -> Result<bool> {
        Ok(self.list(DirLoc::Chain(cluster))?.items.is_empty())
    }

    fn new_short_entry(attr: u8, cluster: u32, size: u32, now: u64) -> [u8; ENTRY] {
        let mut e = [0u8; ENTRY];
        e[11] = attr;
        let (d, t) = to_fat_time(now);
        for o in [14, 22] {
            e[o..o + 2].copy_from_slice(&t.to_le_bytes());
        }
        for o in [16, 18, 24] {
            e[o..o + 2].copy_from_slice(&d.to_le_bytes());
        }
        set_entry_cluster(&mut e, cluster);
        e[28..32].copy_from_slice(&size.to_le_bytes());
        e
    }

    fn set_mtime(e: &mut [u8; ENTRY], now: u64) {
        let (d, t) = to_fat_time(now);
        e[22..24].copy_from_slice(&t.to_le_bytes());
        e[24..26].copy_from_slice(&d.to_le_bytes());
        e[18..20].copy_from_slice(&d.to_le_bytes());
    }

    // --- file data --------------------------------------------------------------

    fn read_data(&mut self, first: u32, size: u64, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if offset >= size || first == 0 {
            return Ok(0);
        }
        let end = size.min(offset + buf.len() as u64);
        let cb = self.cluster_bytes();
        let chain = self.chain(first)?;
        let mut pos = offset;
        let mut tmp = vec![0u8; cb as usize];
        while pos < end {
            let ci = (pos / cb) as usize;
            let c = *chain.get(ci).ok_or(FsError::Corrupt)?;
            self.dev.read(self.cluster_sector(c), &mut tmp)?;
            let s = (pos % cb) as usize;
            let n = ((cb - s as u64).min(end - pos)) as usize;
            let o = (pos - offset) as usize;
            buf[o..o + n].copy_from_slice(&tmp[s..s + n]);
            pos += n as u64;
        }
        Ok((end - offset) as usize)
    }

    /// Makes the chain at least `clusters` long; new clusters are zeroed.
    fn grow_chain(&mut self, e: &mut [u8; ENTRY], clusters: usize) -> Result<Vec<u32>> {
        let first = entry_cluster(e);
        let mut chain = if first == 0 {
            Vec::new()
        } else {
            self.chain(first)?
        };
        while chain.len() < clusters {
            let c = self.alloc_cluster(chain.last().copied(), true)?;
            if chain.is_empty() {
                set_entry_cluster(e, c);
            }
            chain.push(c);
        }
        Ok(chain)
    }

    /// Zeroes the rest of the last cluster after the end of the file.
    fn zero_tail(&mut self, first: u32, size: u64) -> Result<()> {
        let cb = self.cluster_bytes();
        let tail = size % cb;
        if first == 0 || tail == 0 {
            return Ok(());
        }
        let chain = self.chain(first)?;
        let c = *chain.get((size / cb) as usize).ok_or(FsError::Corrupt)?;
        let mut tmp = vec![0u8; cb as usize];
        self.dev.read(self.cluster_sector(c), &mut tmp)?;
        tmp[tail as usize..].fill(0);
        self.dev.write(self.cluster_sector(c), &tmp)
    }
}

fn validate_name(name: &str) -> Result<()> {
    path::check_name(name)?;
    if name.encode_utf16().count() > 255 {
        return Err(FsError::NameTooLong);
    }
    if name
        .chars()
        .any(|c| (c as u32) < 0x20 || "\"*/:<>?\\|".contains(c))
        || name.ends_with(['.', ' '])
    {
        return Err(FsError::Invalid);
    }
    Ok(())
}

impl<D: BlockIo> FileSystem for FatVolume<D> {
    fn fs_type(&self) -> &'static str {
        match self.fat_type {
            FatType::Fat16 => "fat16",
            FatType::Fat32 => "fat32",
        }
    }

    fn root(&self) -> Ino {
        ROOT_INO
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn stat(&mut self, ino: Ino) -> Result<Stat> {
        if ino == ROOT_INO {
            return Ok(Stat {
                ino,
                kind: Kind::Dir,
                mode: 0o755,
                uid: 0,
                gid: 0,
                nlink: 2,
                size: 0,
                allocated: 0,
                mtime: 0,
                ctime: 0,
            });
        }
        let e = self.entry(ino)?;
        let is_dir = e[11] & ATTR_DIRECTORY != 0;
        let ro = e[11] & ATTR_READ_ONLY != 0;
        let first = entry_cluster(&e);
        let clusters = if first == 0 {
            0
        } else {
            self.chain(first)?.len() as u64
        };
        let mtime = from_fat_time(get16(&e, 24), get16(&e, 22));
        Ok(Stat {
            ino,
            kind: if is_dir { Kind::Dir } else { Kind::File },
            mode: match (is_dir, ro) {
                (true, _) => 0o755,
                (false, true) => 0o444,
                (false, false) => 0o644,
            },
            uid: 0,
            gid: 0,
            nlink: if is_dir { 2 } else { 1 },
            size: if is_dir {
                clusters * self.cluster_bytes()
            } else {
                get32(&e, 28) as u64
            },
            allocated: clusters * self.cluster_bytes(),
            mtime,
            ctime: mtime,
        })
    }

    fn lookup(&mut self, dir: Ino, name: &str) -> Result<Ino> {
        let loc = self.dir_loc(dir)?;
        self.find(loc, name)?
            .map(|i| i.pos)
            .ok_or(FsError::NotFound)
    }

    fn readdir(&mut self, dir: Ino, pos: u64) -> Result<Option<(DirEntry, u64)>> {
        let loc = self.dir_loc(dir)?;
        let items = self.list(loc)?.items;
        // `pos` is the entry position to continue from (0 = start).
        Ok(items.into_iter().find(|i| i.pos >= pos).map(|i| {
            let kind = if i.is_dir() { Kind::Dir } else { Kind::File };
            (
                DirEntry {
                    ino: i.pos,
                    kind,
                    name: i.name,
                },
                i.pos + 1,
            )
        }))
    }

    fn read(&mut self, ino: Ino, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if ino == ROOT_INO {
            return Err(FsError::IsDir);
        }
        let e = self.entry(ino)?;
        if e[11] & ATTR_DIRECTORY != 0 {
            return Err(FsError::IsDir);
        }
        self.read_data(entry_cluster(&e), get32(&e, 28) as u64, offset, buf)
    }

    fn write(&mut self, ino: Ino, offset: u64, data: &[u8], now: u64) -> Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(FsError::TooBig)?;
        if end > MAX_FILE {
            return Err(FsError::TooBig);
        }
        self.op(|v| {
            let mut e = v.entry(ino)?;
            if e[11] & ATTR_DIRECTORY != 0 {
                return Err(FsError::IsDir);
            }
            let size = get32(&e, 28) as u64;
            if offset > size {
                v.zero_tail(entry_cluster(&e), size)?;
            }
            let cb = v.cluster_bytes();
            let chain = v.grow_chain(&mut e, end.div_ceil(cb) as usize)?;
            let mut tmp = vec![0u8; cb as usize];
            let mut pos = offset;
            while pos < end {
                let c = chain[(pos / cb) as usize];
                let s = (pos % cb) as usize;
                let n = ((cb - s as u64).min(end - pos)) as usize;
                let src = (pos - offset) as usize;
                if n as u64 != cb {
                    v.dev.read(v.cluster_sector(c), &mut tmp)?;
                }
                tmp[s..s + n].copy_from_slice(&data[src..src + n]);
                v.dev.write(v.cluster_sector(c), &tmp)?;
                pos += n as u64;
            }
            // Data and FAT first, then the entry that makes them visible.
            v.commit()?;
            e[28..32].copy_from_slice(&(size.max(end) as u32).to_le_bytes());
            e[11] |= ATTR_ARCHIVE;
            Self::set_mtime(&mut e, now);
            v.write_entry(ino, &e)?;
            Ok(data.len())
        })
    }

    fn truncate(&mut self, ino: Ino, size: u64, now: u64) -> Result<()> {
        if size > MAX_FILE {
            return Err(FsError::TooBig);
        }
        self.op(|v| {
            let mut e = v.entry(ino)?;
            if e[11] & ATTR_DIRECTORY != 0 {
                return Err(FsError::IsDir);
            }
            let old = get32(&e, 28) as u64;
            let cb = v.cluster_bytes();
            let keep = size.div_ceil(cb) as usize;
            if size > old {
                v.zero_tail(entry_cluster(&e), old)?;
                v.grow_chain(&mut e, keep)?;
            } else {
                let first = entry_cluster(&e);
                if first != 0 {
                    let chain = v.chain(first)?;
                    if keep < chain.len() {
                        // Shorten the entry before freeing the clusters.
                        let mut short = e;
                        short[28..32].copy_from_slice(&(size as u32).to_le_bytes());
                        if keep == 0 {
                            set_entry_cluster(&mut short, 0);
                        }
                        v.write_entry(ino, &short)?;
                        e = short;
                        if keep > 0 {
                            v.set_fat(chain[keep - 1], EOC);
                        }
                        v.free_chain(chain[keep])?;
                    }
                }
            }
            e[28..32].copy_from_slice(&(size as u32).to_le_bytes());
            Self::set_mtime(&mut e, now);
            v.commit()?;
            v.write_entry(ino, &e)
        })
    }

    fn create(
        &mut self,
        dir: Ino,
        name: &str,
        kind: Kind,
        _attr: NewAttr,
        now: u64,
    ) -> Result<Ino> {
        if kind != Kind::File && kind != Kind::Dir {
            return Err(FsError::Unsupported);
        }
        self.op(|v| {
            let loc = v.dir_loc(dir)?;
            validate_name(name)?;
            if v.find(loc, name)?.is_some() {
                return Err(FsError::Exists);
            }
            let (attr, cluster) = if kind == Kind::Dir {
                let c = v.alloc_cluster(None, true)?;
                let mut dot = Self::new_short_entry(ATTR_DIRECTORY, c, 0, now);
                dot[..11].copy_from_slice(b".          ");
                let mut dotdot =
                    Self::new_short_entry(ATTR_DIRECTORY, v.dotdot_cluster(loc), 0, now);
                dotdot[..11].copy_from_slice(b"..         ");
                let base = v.cluster_sector(c) * SECTOR as u64;
                v.write_entry(base, &dot)?;
                v.write_entry(base + ENTRY as u64, &dotdot)?;
                (ATTR_DIRECTORY, c)
            } else {
                (ATTR_ARCHIVE, 0)
            };
            v.commit()?;
            v.add_entry(loc, name, Self::new_short_entry(attr, cluster, 0, now))
        })
    }

    fn unlink(&mut self, dir: Ino, name: &str, _now: u64) -> Result<()> {
        self.op(|v| {
            let loc = v.dir_loc(dir)?;
            let item = v.find(loc, name)?.ok_or(FsError::NotFound)?;
            if item.is_dir() && item.cluster != 0 && !v.dir_is_empty(item.cluster)? {
                return Err(FsError::NotEmpty);
            }
            v.remove_item(&item)?;
            v.free_chain(item.cluster)
        })
    }

    fn rename(
        &mut self,
        from: Ino,
        from_name: &str,
        to: Ino,
        to_name: &str,
        now: u64,
    ) -> Result<()> {
        self.op(|v| {
            let floc = v.dir_loc(from)?;
            let tloc = v.dir_loc(to)?;
            let src = v.find(floc, from_name)?.ok_or(FsError::NotFound)?;
            if src.is_dir() && tloc == DirLoc::Chain(src.cluster) {
                return Err(FsError::Invalid);
            }
            if let Some(old) = v.find(tloc, to_name)? {
                if old.pos == src.pos {
                    if old.name == to_name {
                        return Ok(());
                    }
                    // Only the case changes: remove and add again below.
                } else {
                    match (src.is_dir(), old.is_dir()) {
                        (true, false) => return Err(FsError::NotDir),
                        (false, true) => return Err(FsError::IsDir),
                        (true, true) if !v.dir_is_empty(old.cluster)? => {
                            return Err(FsError::NotEmpty);
                        }
                        _ => {}
                    }
                    v.remove_item(&old)?;
                    v.free_chain(old.cluster)?;
                }
            }
            let mut e = v.read_entry(src.pos)?;
            e[12] = 0; // case flags belong to the old short name
            Self::set_mtime(&mut e, now);
            v.remove_item(&src)?;
            v.add_entry(tloc, to_name, e)?;
            if src.is_dir() && floc != tloc {
                let base = v.cluster_sector(src.cluster) * SECTOR as u64 + ENTRY as u64;
                let mut dd = v.read_entry(base)?;
                if &dd[..2] == b".." {
                    set_entry_cluster(&mut dd, v.dotdot_cluster(tloc));
                    v.write_entry(base, &dd)?;
                }
            }
            Ok(())
        })
    }

    fn sync(&mut self) -> Result<()> {
        if self.read_only {
            return Ok(());
        }
        self.commit()
    }

    fn data_sectors(&mut self, ino: Ino) -> Result<Vec<(u64, u64)>> {
        let e = self.entry(ino)?;
        if e[11] & ATTR_DIRECTORY != 0 {
            return Err(FsError::IsDir);
        }
        let first = entry_cluster(&e);
        if first == 0 {
            return Ok(Vec::new());
        }
        let mut runs: Vec<(u64, u64)> = Vec::new();
        for c in self.chain(first)? {
            let s = self.cluster_sector(c);
            match runs.last_mut() {
                Some((start, n)) if *start + *n == s => *n += self.spc,
                _ => runs.push((s, self.spc)),
            }
        }
        Ok(runs)
    }

    fn statfs(&mut self) -> Result<FsStat> {
        Ok(FsStat {
            block_size: self.cluster_bytes(),
            blocks: (self.cluster_end - 2) as u64,
            free_blocks: self.free as u64,
            inodes: 0,
            free_inodes: 0,
        })
    }
}

// --- formatting ---------------------------------------------------------------

/// Sectors per cluster for FAT32 by volume size (the usual table).
fn fat32_spc(sectors: u64) -> u64 {
    let mib = sectors * SECTOR as u64 / (1024 * 1024);
    match mib {
        0..260 => 1,
        260..8192 => 8,
        8192..16384 => 16,
        16384..32768 => 32,
        _ => 64,
    }
}

/// Formats the whole device as FAT32. Needs at least 65525 clusters
/// (about 33 MiB with 512-byte clusters).
pub fn format_fat32<D: BlockIo + ?Sized>(dev: &mut D, label: &str, volume_id: u32) -> Result<()> {
    if dev.sector_size() != SECTOR {
        return Err(FsError::Invalid);
    }
    let total = dev.sector_count().min(u32::MAX as u64);
    let spc = fat32_spc(total);
    let reserved = 32u64;
    let nfats = 2u64;
    // FAT size as in the FAT specification (rounds up slightly).
    let fat_sectors = (total - reserved).div_ceil((256 * spc + nfats) / 2);
    let data_start = reserved + nfats * fat_sectors;
    let clusters = (total - data_start) / spc;
    if clusters < 65525 || total <= data_start {
        return Err(FsError::NoSpace);
    }
    let mut b = [0u8; SECTOR];
    b[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    b[3..11].copy_from_slice(b"VELORA  ");
    b[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    b[13] = spc as u8;
    b[14..16].copy_from_slice(&(reserved as u16).to_le_bytes());
    b[16] = nfats as u8;
    b[21] = 0xf8;
    b[24..26].copy_from_slice(&63u16.to_le_bytes());
    b[26..28].copy_from_slice(&255u16.to_le_bytes());
    b[32..36].copy_from_slice(&(total as u32).to_le_bytes());
    b[36..40].copy_from_slice(&(fat_sectors as u32).to_le_bytes());
    b[44..48].copy_from_slice(&2u32.to_le_bytes()); // root cluster
    b[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo sector
    b[50..52].copy_from_slice(&6u16.to_le_bytes()); // backup boot sector
    b[64] = 0x80;
    b[66] = 0x29;
    b[67..71].copy_from_slice(&volume_id.to_le_bytes());
    let lab = crate::label_bytes(label);
    b[71..82].copy_from_slice(&lab);
    b[82..90].copy_from_slice(b"FAT32   ");
    b[90] = 0xf4; // hlt; not bootable as a BIOS volume
    b[91..93].copy_from_slice(&[0xeb, 0xfd]);
    b[510] = 0x55;
    b[511] = 0xaa;
    let boot = b;
    let mut info = [0u8; SECTOR];
    info[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    info[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    info[488..492].copy_from_slice(&((clusters - 1) as u32).to_le_bytes());
    info[492..496].copy_from_slice(&3u32.to_le_bytes());
    info[508..512].copy_from_slice(&0xaa55_0000u32.to_le_bytes());

    // Zero reserved area, FATs and the root cluster in large writes.
    let zero = vec![0u8; 64 * SECTOR];
    let mut s = 0;
    let end = data_start + spc;
    while s < end {
        let n = (end - s).min(64);
        dev.write(s, &zero[..n as usize * SECTOR])?;
        s += n;
    }
    let mut fat0 = [0u8; SECTOR];
    fat0[0..4].copy_from_slice(&0x0fff_fff8u32.to_le_bytes());
    fat0[4..8].copy_from_slice(&0x0fff_ffffu32.to_le_bytes());
    fat0[8..12].copy_from_slice(&0x0fff_ffffu32.to_le_bytes()); // root
    for f in 0..nfats {
        dev.write(reserved + f * fat_sectors, &fat0)?;
    }
    let mut root = [0u8; SECTOR];
    root[..11].copy_from_slice(&lab);
    root[11] = ATTR_VOLUME_ID;
    dev.write(data_start, &root)?;
    dev.write(1, &info)?;
    dev.write(7, &info)?;
    dev.write(6, &boot)?;
    dev.flush()?;
    dev.write(0, &boot)?;
    dev.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImageBuilder;
    use velora_vfs::{MemDisk, conformance};

    const MIB: u64 = 1024 * 1024;

    fn fat32(size: u64) -> FatVolume<MemDisk> {
        let mut d = MemDisk::new(512, size / 512);
        format_fat32(&mut d, "VELORA", 0x1234).unwrap();
        FatVolume::mount(d, false).unwrap()
    }

    fn remount(v: FatVolume<MemDisk>) -> FatVolume<MemDisk> {
        let img = v.into_device().durable_image();
        FatVolume::mount(MemDisk::from_image(512, img), false).unwrap()
    }

    #[test]
    fn data_sectors_point_at_the_file_contents() {
        let mut v = fat32(40 * MIB);
        let root = v.root();
        let attr = NewAttr {
            mode: 0o644,
            uid: 0,
            gid: 0,
        };
        // Interleave two files so that the first one is fragmented.
        let a = v.create(root, "a.log", Kind::File, attr, 0).unwrap();
        let b = v.create(root, "b.bin", Kind::File, attr, 0).unwrap();
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        for k in 0..4 {
            let part = &data[k * 5000..(k + 1) * 5000];
            v.write(a, (k * 5000) as u64, part, 0).unwrap();
            v.write(b, (k * 5000) as u64, &[0xee; 5000], 0).unwrap();
        }
        v.sync().unwrap();
        let runs = v.data_sectors(a).unwrap();
        assert!(runs.len() > 1, "expected a fragmented file: {:?}", runs);
        let mut disk = v.into_device();
        let mut raw = Vec::new();
        for (start, n) in runs {
            let mut buf = vec![0u8; n as usize * 512];
            disk.read(start, &mut buf).unwrap();
            raw.extend_from_slice(&buf);
        }
        assert!(raw.len() >= data.len());
        assert_eq!(&raw[..data.len()], &data[..]);
        let mut v = FatVolume::mount(disk, false).unwrap();
        let root = v.root();
        let empty = v.create(root, "empty", Kind::File, attr, 0).unwrap();
        assert_eq!(v.data_sectors(empty).unwrap(), vec![]);
        assert_eq!(v.data_sectors(root), Err(FsError::NotFound));
    }

    #[test]
    fn time_conversion() {
        // 2026-11-26 12:34:56 UTC
        let t = 1_795_696_496;
        let (d, tm) = to_fat_time(t);
        assert_eq!(d >> 9, 46);
        assert_eq!((d >> 5) & 15, 11);
        assert_eq!(d & 31, 26);
        assert_eq!(from_fat_time(d, tm), t);
        assert_eq!(to_fat_time(0), ((1 << 5) | 1, 0));
    }

    #[test]
    fn format_and_mount_fat32() {
        let mut d = MemDisk::new(512, 64 * MIB / 512);
        assert!(probe(&mut d).is_none());
        format_fat32(&mut d, "VELORA", 1).unwrap();
        assert_eq!(probe(&mut d), Some(FatType::Fat32));
        let mut v = FatVolume::mount(d, false).unwrap();
        let st = v.statfs().unwrap();
        assert_eq!(st.block_size, 512);
        assert!(st.blocks >= 65525);
        assert_eq!(st.free_blocks, st.blocks - 1);
        assert_eq!(v.readdir(ROOT_INO, 0).unwrap(), None, "label is not listed");
    }

    #[test]
    fn small_volume_cannot_be_fat32() {
        let mut d = MemDisk::new(512, 16 * MIB / 512);
        assert_eq!(format_fat32(&mut d, "X", 1), Err(FsError::NoSpace));
    }

    #[test]
    fn fat32_passes_conformance_and_persists() {
        let mut v = fat32(64 * MIB);
        conformance::run(&mut v, "conformance", 2 * MIB as usize).unwrap();
        let root = v.root();
        let d = v
            .create(
                root,
                "EFI",
                Kind::Dir,
                NewAttr {
                    mode: 0,
                    uid: 0,
                    gid: 0,
                },
                1_800_000_000,
            )
            .unwrap();
        let f = v
            .create(
                d,
                "bootx64.efi",
                Kind::File,
                NewAttr {
                    mode: 0,
                    uid: 0,
                    gid: 0,
                },
                1_800_000_000,
            )
            .unwrap();
        v.write(f, 0, &[0x4d, 0x5a, 1, 2, 3], 1_800_000_000)
            .unwrap();
        let mut v = remount(v);
        let d = v.lookup(ROOT_INO, "efi").unwrap();
        let f = v.lookup(d, "BOOTX64.EFI").unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(v.read(f, 0, &mut buf).unwrap(), 5);
        assert_eq!(&buf[..5], &[0x4d, 0x5a, 1, 2, 3]);
        assert_eq!(v.stat(f).unwrap().mtime, 1_800_000_000);
        let (e, _) = v.readdir(d, 0).unwrap().unwrap();
        assert_eq!(e.name, "bootx64.efi");
    }

    #[test]
    fn reads_and_writes_images_from_the_builder() {
        let mut b = ImageBuilder::new();
        b.add_file("EFI/BOOT/BOOTX64.EFI", vec![9u8; 5000]).unwrap();
        b.add_file("boot/limine.conf", b"timeout: 0\n".to_vec())
            .unwrap();
        let img = b.build_fat16(8 * MIB as usize, "VELORA", 1).unwrap();
        let mut v = FatVolume::mount(MemDisk::from_image(512, img), false).unwrap();
        assert_eq!(v.fat_type(), FatType::Fat16);
        let boot = v.lookup(ROOT_INO, "boot").unwrap();
        let conf = v.lookup(boot, "limine.conf").unwrap();
        let mut buf = [0u8; 64];
        let n = v.read(conf, 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"timeout: 0\n");
        // Writing into the FAT16 root and a subdirectory.
        conformance::run(&mut v, "conformance", 200_000).unwrap();
        let img = v.into_device().durable_image();
        assert_eq!(
            crate::read_file(&img, "EFI/BOOT/BOOTX64.EFI").unwrap(),
            vec![9u8; 5000]
        );
    }

    #[test]
    fn fat16_root_has_a_fixed_size() {
        let img = ImageBuilder::new()
            .build_fat16(8 * MIB as usize, "X", 1)
            .unwrap();
        let mut v = FatVolume::mount(MemDisk::from_image(512, img), false).unwrap();
        let a = NewAttr {
            mode: 0,
            uid: 0,
            gid: 0,
        };
        let mut made = 0;
        loop {
            match v.create(ROOT_INO, &alloc::format!("F{}", made), Kind::File, a, 0) {
                Ok(_) => made += 1,
                Err(FsError::NoSpace) => break,
                Err(e) => panic!("{e}"),
            }
        }
        assert_eq!(made, 511); // 512 slots minus the volume label
    }

    #[test]
    fn invalid_names_are_refused() {
        let mut v = fat32(40 * MIB);
        let a = NewAttr {
            mode: 0,
            uid: 0,
            gid: 0,
        };
        for n in ["a:b", "q?", "trailing.", "x*"] {
            assert_eq!(
                v.create(ROOT_INO, n, Kind::File, a, 0),
                Err(FsError::Invalid),
                "{n}"
            );
        }
        v.create(ROOT_INO, "Case", Kind::File, a, 0).unwrap();
        assert_eq!(
            v.create(ROOT_INO, "CASE", Kind::File, a, 0),
            Err(FsError::Exists)
        );
    }
}

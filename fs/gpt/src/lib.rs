//! GUID partition tables (UEFI specification, chapter 5): reading and
//! writing, with a protective MBR and the backup table at the end of the
//! disk. Disks with 512-byte sectors only.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use velora_vfs::{BlockIo, FsError, Result};

pub const SECTOR: usize = 512;
const SIGNATURE: &[u8; 8] = b"EFI PART";
const REVISION: u32 = 0x0001_0000;
const HEADER_SIZE: u32 = 92;
pub const ENTRY_SIZE: usize = 128;
pub const ENTRY_COUNT: usize = 128;
/// Sectors taken by the entry array (16 KiB).
pub const ENTRY_SECTORS: u64 = (ENTRY_SIZE * ENTRY_COUNT / SECTOR) as u64;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    pub const ZERO: Guid = Guid([0; 16]);
    /// EFI system partition.
    pub const ESP: Guid = Guid::parse_const("C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
    /// Velora OS root file system (VeloraFS). Chosen for this project.
    pub const VELORA_ROOT: Guid = Guid::parse_const("5E1F7A30-9C42-4D8B-A1E7-56454C4F5241");

    const fn hex(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("bad hex digit"),
        }
    }

    /// Parses the text form at compile time (panics on bad input).
    pub const fn parse_const(s: &str) -> Guid {
        let b = s.as_bytes();
        assert!(b.len() == 36);
        // Text byte order; the first three groups are little-endian on disk.
        let order: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut text = [0u8; 16];
        let mut i = 0;
        let mut k = 0;
        while i < 36 {
            if b[i] == b'-' {
                i += 1;
                continue;
            }
            text[k] = Self::hex(b[i]) << 4 | Self::hex(b[i + 1]);
            k += 1;
            i += 2;
        }
        let mut out = [0u8; 16];
        let mut j = 0;
        while j < 16 {
            out[j] = text[order[j]];
            j += 1;
        }
        Guid(out)
    }

    /// Parses "XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX" (any case).
    pub fn parse(s: &str) -> Option<Guid> {
        let b = s.as_bytes();
        if b.len() != 36 || [8, 13, 18, 23].iter().any(|&i| b[i] != b'-') {
            return None;
        }
        if !s
            .chars()
            .enumerate()
            .all(|(i, c)| [8, 13, 18, 23].contains(&i) || c.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Self::parse_const(s))
    }

    /// A version 4 GUID from 16 random bytes.
    pub fn from_random(mut r: [u8; 16]) -> Guid {
        r[7] = (r[7] & 0x0f) | 0x40;
        r[8] = (r[8] & 0x3f) | 0x80;
        Guid(r)
    }
}

impl core::fmt::Display for Guid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
            b[3],
            b[2],
            b[1],
            b[0],
            b[5],
            b[4],
            b[7],
            b[6],
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15]
        )
    }
}

impl core::fmt::Debug for Guid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    pub type_guid: Guid,
    pub unique_guid: Guid,
    pub first_lba: u64,
    /// Inclusive.
    pub last_lba: u64,
    pub attributes: u64,
    pub name: String,
}

impl Partition {
    pub fn sectors(&self) -> u64 {
        self.last_lba + 1 - self.first_lba
    }
}

/// CRC-32 (IEEE 802.3), as GPT uses.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xedb8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn get32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn get64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn read_header<D: BlockIo + ?Sized>(
    dev: &mut D,
    lba: u64,
) -> Result<Option<(Vec<u8>, Vec<Partition>)>> {
    let mut h = vec![0u8; SECTOR];
    dev.read(lba, &mut h)?;
    if &h[0..8] != SIGNATURE {
        return Ok(None);
    }
    let size = get32(&h, 12) as usize;
    if !(HEADER_SIZE as usize..=SECTOR).contains(&size) {
        return Ok(None);
    }
    let mut copy = h[..size].to_vec();
    copy[16..20].fill(0);
    if crc32(&copy) != get32(&h, 16) {
        return Ok(None);
    }
    let entries_lba = get64(&h, 72);
    let count = get32(&h, 80) as usize;
    let esize = get32(&h, 84) as usize;
    if esize < ENTRY_SIZE || !esize.is_multiple_of(8) || count > 1024 {
        return Ok(None);
    }
    let bytes = (count * esize).next_multiple_of(SECTOR);
    let mut e = vec![0u8; bytes];
    dev.read(entries_lba, &mut e)?;
    if crc32(&e[..count * esize]) != get32(&h, 88) {
        return Ok(None);
    }
    let parts = (0..count)
        .map(|i| &e[i * esize..(i + 1) * esize])
        .filter(|x| x[..16] != [0; 16])
        .map(|x| {
            let units: Vec<u16> = x[56..128]
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&u| u != 0)
                .collect();
            Partition {
                type_guid: Guid(x[0..16].try_into().unwrap()),
                unique_guid: Guid(x[16..32].try_into().unwrap()),
                first_lba: get64(x, 32),
                last_lba: get64(x, 40),
                attributes: get64(x, 48),
                name: String::from_utf16_lossy(&units),
            }
        })
        .collect();
    Ok(Some((h, parts)))
}

/// The partitions of a GPT disk, or `None` if the disk has no valid GPT.
/// The backup table is used when the primary one is damaged.
pub fn read<D: BlockIo + ?Sized>(dev: &mut D) -> Result<Option<Vec<Partition>>> {
    if dev.sector_size() != SECTOR {
        return Ok(None);
    }
    if let Some((_, p)) = read_header(dev, 1)? {
        return Ok(Some(p));
    }
    let last = dev.sector_count().saturating_sub(1);
    Ok(read_header(dev, last)?.map(|(_, p)| p))
}

/// First and last LBA usable for partitions on a disk of `sectors`.
pub fn usable_range(sectors: u64) -> (u64, u64) {
    (2 + ENTRY_SECTORS, sectors - 2 - ENTRY_SECTORS)
}

fn header(
    disk_guid: Guid,
    me: u64,
    alt: u64,
    entries_lba: u64,
    first: u64,
    last: u64,
    entries_crc: u32,
) -> Vec<u8> {
    let mut h = vec![0u8; SECTOR];
    h[0..8].copy_from_slice(SIGNATURE);
    h[8..12].copy_from_slice(&REVISION.to_le_bytes());
    h[12..16].copy_from_slice(&HEADER_SIZE.to_le_bytes());
    h[24..32].copy_from_slice(&me.to_le_bytes());
    h[32..40].copy_from_slice(&alt.to_le_bytes());
    h[40..48].copy_from_slice(&first.to_le_bytes());
    h[48..56].copy_from_slice(&last.to_le_bytes());
    h[56..72].copy_from_slice(&disk_guid.0);
    h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
    h[80..84].copy_from_slice(&(ENTRY_COUNT as u32).to_le_bytes());
    h[84..88].copy_from_slice(&(ENTRY_SIZE as u32).to_le_bytes());
    h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&h[..HEADER_SIZE as usize]);
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

/// Writes a new partition table (protective MBR, primary and backup GPT).
/// Everything previously on the disk becomes unreachable.
pub fn write<D: BlockIo + ?Sized>(dev: &mut D, disk_guid: Guid, parts: &[Partition]) -> Result<()> {
    if dev.sector_size() != SECTOR {
        return Err(FsError::Unsupported);
    }
    let total = dev.sector_count();
    if total < 2 * (2 + ENTRY_SECTORS) + 16 || parts.len() > ENTRY_COUNT {
        return Err(FsError::Invalid);
    }
    let (first, last) = usable_range(total);
    let mut sorted: Vec<&Partition> = parts.iter().collect();
    sorted.sort_by_key(|p| p.first_lba);
    for (i, p) in sorted.iter().enumerate() {
        if p.first_lba < first || p.last_lba > last || p.last_lba < p.first_lba {
            return Err(FsError::Invalid);
        }
        if i > 0 && sorted[i - 1].last_lba >= p.first_lba {
            return Err(FsError::Invalid);
        }
        if p.name.encode_utf16().count() > 36 {
            return Err(FsError::NameTooLong);
        }
    }
    let mut entries = vec![0u8; ENTRY_SIZE * ENTRY_COUNT];
    for (i, p) in parts.iter().enumerate() {
        let e = &mut entries[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE];
        e[0..16].copy_from_slice(&p.type_guid.0);
        e[16..32].copy_from_slice(&p.unique_guid.0);
        e[32..40].copy_from_slice(&p.first_lba.to_le_bytes());
        e[40..48].copy_from_slice(&p.last_lba.to_le_bytes());
        e[48..56].copy_from_slice(&p.attributes.to_le_bytes());
        for (k, u) in p.name.encode_utf16().enumerate() {
            e[56 + 2 * k..58 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
    }
    let ecrc = crc32(&entries);
    // Protective MBR: one partition of type 0xEE covering the disk.
    let mut mbr = vec![0u8; SECTOR];
    let p = &mut mbr[446..462];
    p[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    p[4] = 0xee;
    p[5..8].copy_from_slice(&[0xff, 0xff, 0xff]);
    p[8..12].copy_from_slice(&1u32.to_le_bytes());
    p[12..16].copy_from_slice(&((total - 1).min(u32::MAX as u64) as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xaa;
    let last_lba = total - 1;
    let backup_entries = last_lba - ENTRY_SECTORS;
    dev.write(0, &mbr)?;
    dev.write(2, &entries)?;
    dev.write(1, &header(disk_guid, 1, last_lba, 2, first, last, ecrc))?;
    dev.write(backup_entries, &entries)?;
    dev.write(
        last_lba,
        &header(disk_guid, last_lba, 1, backup_entries, first, last, ecrc),
    )?;
    dev.flush()
}

/// The standard Velora layout: a 1 MiB-aligned EFI system partition of
/// `esp_mib` MiB, then the root partition filling the rest.
pub fn velora_layout(
    sectors: u64,
    esp_mib: u64,
    esp_guid: Guid,
    root_guid: Guid,
) -> Result<Vec<Partition>> {
    let (_, last) = usable_range(sectors);
    let start = 2048;
    let esp_end = start + esp_mib * 2048 - 1;
    let root_end = ((last + 1) / 2048) * 2048 - 1;
    if root_end <= esp_end + 2048 * 16 {
        return Err(FsError::NoSpace);
    }
    Ok(vec![
        Partition {
            type_guid: Guid::ESP,
            unique_guid: esp_guid,
            first_lba: start,
            last_lba: esp_end,
            attributes: 0,
            name: String::from("EFI system"),
        },
        Partition {
            type_guid: Guid::VELORA_ROOT,
            unique_guid: root_guid,
            first_lba: esp_end + 1,
            last_lba: root_end,
            attributes: 0,
            name: String::from("Velora root"),
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use velora_vfs::MemDisk;

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn guid_text_round_trip() {
        let g = Guid::ESP;
        assert_eq!(g.0[..4], [0x28, 0x73, 0x2a, 0xc1]);
        assert_eq!(format!("{}", g), "C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
        assert_eq!(Guid::parse("c12a7328-f81f-11d2-ba4b-00a0c93ec93b"), Some(g));
        assert_eq!(Guid::parse("c12a7328f81f-11d2-ba4b-00a0c93ec93b-"), None);
        assert_eq!(Guid::parse("x12a7328-f81f-11d2-ba4b-00a0c93ec93b"), None);
        let r = Guid::from_random([0xff; 16]);
        assert_eq!((r.0[7] >> 4, r.0[8] >> 6), (4, 2));
    }

    #[test]
    fn write_then_read() {
        let mut d = MemDisk::new(512, 256 * 2048);
        assert_eq!(read(&mut d).unwrap(), None);
        let parts = velora_layout(d.sector_count(), 64, Guid([1; 16]), Guid([2; 16])).unwrap();
        write(&mut d, Guid([9; 16]), &parts).unwrap();
        let back = read(&mut d).unwrap().unwrap();
        assert_eq!(back, parts);
        assert_eq!(back[0].first_lba, 2048);
        assert_eq!(back[0].sectors(), 64 * 2048);
        assert_eq!(back[1].first_lba, 2048 + 64 * 2048);
        assert!(back[1].last_lba < usable_range(d.sector_count()).1);
        assert_eq!(back[1].name, "Velora root");
        let img = d.current_image();
        assert_eq!(img[450], 0xee, "protective MBR");
    }

    #[test]
    fn damaged_primary_falls_back_to_backup() {
        let mut d = MemDisk::new(512, 64 * 2048);
        let parts = velora_layout(d.sector_count(), 40, Guid([1; 16]), Guid([2; 16])).unwrap();
        write(&mut d, Guid([9; 16]), &parts).unwrap();
        let mut img = d.current_image();
        img[512 + 30] ^= 0xff; // break the primary header
        let mut d = MemDisk::from_image(512, img);
        assert_eq!(read(&mut d).unwrap(), Some(parts));
    }

    #[test]
    fn bad_layouts_are_refused() {
        let mut d = MemDisk::new(512, 64 * 2048);
        let mut parts = velora_layout(d.sector_count(), 40, Guid([1; 16]), Guid([2; 16])).unwrap();
        parts[1].first_lba = parts[0].last_lba; // overlap
        assert_eq!(write(&mut d, Guid::ZERO, &parts), Err(FsError::Invalid));
        assert_eq!(
            velora_layout(64 * 2048, 60, Guid::ZERO, Guid::ZERO).err(),
            Some(FsError::NoSpace)
        );
    }
}

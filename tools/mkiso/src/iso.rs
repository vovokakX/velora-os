//! Minimal ISO 9660 writer with an El Torito UEFI boot entry.
//!
//! Supports a directory tree with ISO 9660 level 2 names (no Rock Ridge,
//! no Joliet). Limine compares such names case-insensitively, so
//! `boot/limine.conf` is stored as `BOOT/LIMINE.CONF;1`.
//!
//! Sector layout: 0..16 system area, 16 primary volume descriptor,
//! 17 El Torito boot record, 18 terminator, then path tables,
//! directories and file data.

use std::collections::{BTreeMap, VecDeque};

pub const SECTOR: usize = 2048;

const LBA_PVD: usize = 16;
const LBA_BOOT_RECORD: usize = 17;
const LBA_TERMINATOR: usize = 18;
const FIRST_FREE: usize = 19;

const CATALOG_NAME: &str = "BOOT.CAT;1";
const EFI_IMAGE_NAME: &str = "EFIBOOT.IMG;1";
const MAX_NAME: usize = 30;

/// 2026-01-01 00:00:00 UTC, fixed so that builds are reproducible.
const DATE_7: [u8; 7] = [126, 1, 1, 0, 0, 0, 0];
const DATE_17: &[u8; 16] = b"2026010100000000";

enum Node {
    File(Vec<u8>),
    Dir(BTreeMap<String, Node>),
}

pub struct IsoBuilder {
    root: BTreeMap<String, Node>,
}

fn both16(v: u16) -> [u8; 4] {
    let l = v.to_le_bytes();
    let b = v.to_be_bytes();
    [l[0], l[1], b[0], b[1]]
}

fn both32(v: u32) -> [u8; 8] {
    let l = v.to_le_bytes();
    let b = v.to_be_bytes();
    [l[0], l[1], l[2], l[3], b[0], b[1], b[2], b[3]]
}

fn record_len(name_len: usize) -> usize {
    33 + name_len + (name_len + 1) % 2
}

fn dir_record(name: &[u8], lba: u32, size: u32, is_dir: bool) -> Vec<u8> {
    let len = record_len(name.len());
    let mut r = vec![0u8; len];
    r[0] = len as u8;
    r[2..10].copy_from_slice(&both32(lba));
    r[10..18].copy_from_slice(&both32(size));
    r[18..25].copy_from_slice(&DATE_7);
    r[25] = if is_dir { 2 } else { 0 };
    r[28..32].copy_from_slice(&both16(1));
    r[32] = name.len() as u8;
    r[33..33 + name.len()].copy_from_slice(name);
    r
}

fn padded(s: &str, len: usize) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.resize(len, b' ');
    v.truncate(len);
    v
}

fn date17() -> [u8; 17] {
    let mut d = [0u8; 17];
    d[..16].copy_from_slice(DATE_17);
    d
}

/// Converts a path component into an ISO 9660 identifier.
fn iso_name(part: &str, is_dir: bool) -> Result<String, String> {
    let up = part.to_ascii_uppercase();
    let valid = !up.is_empty()
        && up.len() <= MAX_NAME
        && up
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_' || c == b'.')
        && up.bytes().filter(|&c| c == b'.').count() <= if is_dir { 0 } else { 1 };
    if !valid {
        return Err(format!(
            "'{part}' is not a valid ISO 9660 name (A-Z, 0-9, _, one dot)"
        ));
    }
    Ok(if is_dir {
        up
    } else if up.contains('.') {
        format!("{up};1")
    } else {
        format!("{up}.;1")
    })
}

/// Name as a reader sees it: without the version and a trailing dot.
fn plain(id: &str) -> &str {
    let base = id.strip_suffix(";1").unwrap_or(id);
    base.strip_suffix('.').unwrap_or(base)
}

impl IsoBuilder {
    pub fn new() -> Self {
        Self {
            root: BTreeMap::new(),
        }
    }

    pub fn add_file(&mut self, path: &str, data: Vec<u8>) -> Result<(), String> {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let (last, dirs) = parts
            .split_last()
            .ok_or_else(|| format!("empty path '{path}'"))?;
        let mut cur = &mut self.root;
        for d in dirs {
            let dname = iso_name(d, true)?;
            if cur.keys().any(|k| k != &dname && plain(k) == dname) {
                return Err(format!("'{path}' clashes with an existing file"));
            }
            let node = cur
                .entry(dname)
                .or_insert_with(|| Node::Dir(BTreeMap::new()));
            cur = match node {
                Node::Dir(c) => c,
                Node::File(_) => return Err(format!("'{path}' goes through a file")),
            };
        }
        let name = iso_name(last, false)?;
        // Readers strip ";1" (and a trailing dot), so "A.;1" and a directory
        // "A" would look the same.
        let clash = cur.keys().any(|k| plain(k) == plain(&name));
        if clash || name == CATALOG_NAME || name == EFI_IMAGE_NAME {
            return Err(format!("duplicate or reserved path '{path}'"));
        }
        cur.insert(name, Node::File(data));
        Ok(())
    }

    /// Builds the ISO. `efi_image` is a FAT image whose size must be a
    /// non-zero multiple of 512 bytes and at most 65535 * 512 bytes.
    pub fn build(mut self, volume_id: &str, efi_image: Vec<u8>) -> Result<Vec<u8>, String> {
        if efi_image.is_empty() || !efi_image.len().is_multiple_of(512) {
            return Err("EFI image size must be a non-zero multiple of 512".into());
        }
        let efi_sectors_512 = efi_image.len() / 512;
        if efi_sectors_512 > u16::MAX as usize {
            return Err("EFI image is larger than El Torito allows (32 MiB)".into());
        }
        self.root
            .insert(CATALOG_NAME.into(), Node::File(vec![0; SECTOR]));
        self.root
            .insert(EFI_IMAGE_NAME.into(), Node::File(efi_image));
        let layout = Layout::new(&self.root)?;
        let mut iso = layout.write(volume_id);

        let cat_lba = layout.file_lba(CATALOG_NAME);
        let efi_lba = layout.file_lba(EFI_IMAGE_NAME);

        // El Torito boot record volume descriptor.
        let br = &mut iso[LBA_BOOT_RECORD * SECTOR..(LBA_BOOT_RECORD + 1) * SECTOR];
        br[0] = 0;
        br[1..6].copy_from_slice(b"CD001");
        br[6] = 1;
        br[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
        br[0x47..0x4b].copy_from_slice(&(cat_lba as u32).to_le_bytes());

        // Boot catalog: validation entry + one no-emulation EFI entry.
        let cat = &mut iso[cat_lba * SECTOR..cat_lba * SECTOR + 64];
        cat[0] = 1; // header ID
        cat[1] = 0xef; // platform: EFI
        cat[30] = 0x55;
        cat[31] = 0xaa;
        let sum = cat[..32].as_chunks::<2>().0.iter().fold(0u16, |acc, w| {
            acc.wrapping_add(u16::from_le_bytes([w[0], w[1]]))
        });
        cat[28..30].copy_from_slice(&0u16.wrapping_sub(sum).to_le_bytes());
        cat[32] = 0x88; // bootable
        cat[33] = 0; // no emulation
        cat[38..40].copy_from_slice(&(efi_sectors_512 as u16).to_le_bytes());
        cat[40..44].copy_from_slice(&(efi_lba as u32).to_le_bytes());
        Ok(iso)
    }
}

struct FlatDir<'a> {
    name: String,
    parent: usize,
    entries: &'a BTreeMap<String, Node>,
    child_dirs: BTreeMap<String, usize>,
    lba: usize,
    size: usize,
}

struct Layout<'a> {
    dirs: Vec<FlatDir<'a>>,
    files: Vec<(String, usize, &'a [u8])>, // (path key, lba, data)
    path_table_size: usize,
    lba_path_l: usize,
    lba_path_m: usize,
    total_sectors: usize,
}

impl<'a> Layout<'a> {
    fn new(root: &'a BTreeMap<String, Node>) -> Result<Self, String> {
        // Directories in breadth-first order, as the path table requires.
        let mut dirs: Vec<FlatDir> = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back((root, 0usize, String::new()));
        while let Some((entries, parent, name)) = queue.pop_front() {
            let idx = dirs.len();
            if idx != 0 {
                dirs[parent].child_dirs.insert(name.clone(), idx);
            }
            dirs.push(FlatDir {
                name,
                parent,
                entries,
                child_dirs: BTreeMap::new(),
                lba: 0,
                size: 0,
            });
            for (n, node) in entries {
                if let Node::Dir(c) = node {
                    queue.push_back((c, idx, n.clone()));
                }
            }
        }
        if dirs.len() > u16::MAX as usize {
            return Err("too many directories".into());
        }

        let path_table_size: usize = dirs
            .iter()
            .map(|d| 8 + d.name.len().max(1) + d.name.len().max(1) % 2)
            .sum();
        let pt_sectors = path_table_size.div_ceil(SECTOR);
        let lba_path_l = FIRST_FREE;
        let lba_path_m = lba_path_l + pt_sectors;
        let mut next = lba_path_m + pt_sectors;

        // Directory sizes: records may not cross sector boundaries.
        for d in dirs.iter_mut() {
            let mut lens = vec![record_len(1), record_len(1)];
            lens.extend(d.entries.keys().map(|n| record_len(n.len())));
            let mut sectors = 1;
            let mut used = 0;
            for l in lens {
                if used + l > SECTOR {
                    sectors += 1;
                    used = 0;
                }
                used += l;
            }
            d.lba = next;
            d.size = sectors * SECTOR;
            next += sectors;
        }

        // File data.
        let mut files = Vec::new();
        for (i, d) in dirs.iter().enumerate() {
            for (n, node) in d.entries {
                if let Node::File(data) = node {
                    files.push((format!("{i}/{n}"), next, data.as_slice()));
                    next += data.len().div_ceil(SECTOR).max(1);
                }
            }
        }
        Ok(Self {
            dirs,
            files,
            path_table_size,
            lba_path_l,
            lba_path_m,
            total_sectors: next,
        })
    }

    /// LBA of a file in the root directory.
    fn file_lba(&self, name: &str) -> usize {
        let key = format!("0/{name}");
        self.files
            .iter()
            .find(|(k, ..)| *k == key)
            .map(|f| f.1)
            .expect("file in layout")
    }

    fn write(&self, volume_id: &str) -> Vec<u8> {
        let mut iso = vec![0u8; self.total_sectors * SECTOR];
        let file_info: BTreeMap<&str, (usize, usize)> = self
            .files
            .iter()
            .map(|(k, lba, d)| (k.as_str(), (*lba, d.len())))
            .collect();

        // Directories.
        for (i, d) in self.dirs.iter().enumerate() {
            let parent = &self.dirs[d.parent];
            let mut records = vec![
                dir_record(&[0], d.lba as u32, d.size as u32, true),
                dir_record(&[1], parent.lba as u32, parent.size as u32, true),
            ];
            for (n, node) in d.entries {
                records.push(match node {
                    Node::Dir(_) => {
                        let c = &self.dirs[d.child_dirs[n]];
                        dir_record(n.as_bytes(), c.lba as u32, c.size as u32, true)
                    }
                    Node::File(_) => {
                        let (lba, len) = file_info[format!("{i}/{n}").as_str()];
                        dir_record(n.as_bytes(), lba as u32, len as u32, false)
                    }
                });
            }
            let mut off = d.lba * SECTOR;
            let mut used = 0;
            for r in records {
                if used + r.len() > SECTOR {
                    off += SECTOR - used;
                    used = 0;
                }
                iso[off..off + r.len()].copy_from_slice(&r);
                off += r.len();
                used += r.len();
            }
        }

        // Path tables (little and big endian).
        let mut pl = Vec::new();
        let mut pm = Vec::new();
        for d in &self.dirs {
            let name: &[u8] = if d.name.is_empty() {
                &[0]
            } else {
                d.name.as_bytes()
            };
            for (table, big) in [(&mut pl, false), (&mut pm, true)] {
                table.push(name.len() as u8);
                table.push(0);
                let lba = d.lba as u32;
                let parent = (d.parent + 1) as u16;
                table.extend_from_slice(&if big {
                    lba.to_be_bytes()
                } else {
                    lba.to_le_bytes()
                });
                table.extend_from_slice(&if big {
                    parent.to_be_bytes()
                } else {
                    parent.to_le_bytes()
                });
                table.extend_from_slice(name);
                if name.len() % 2 == 1 {
                    table.push(0);
                }
            }
        }
        iso[self.lba_path_l * SECTOR..self.lba_path_l * SECTOR + pl.len()].copy_from_slice(&pl);
        iso[self.lba_path_m * SECTOR..self.lba_path_m * SECTOR + pm.len()].copy_from_slice(&pm);

        // File data.
        for (_, lba, data) in &self.files {
            iso[lba * SECTOR..lba * SECTOR + data.len()].copy_from_slice(data);
        }

        // Primary volume descriptor.
        let root = &self.dirs[0];
        let pvd = &mut iso[LBA_PVD * SECTOR..(LBA_PVD + 1) * SECTOR];
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        pvd[6] = 1;
        pvd[8..40].copy_from_slice(&padded("VELORA", 32));
        pvd[40..72].copy_from_slice(&padded(volume_id, 32));
        pvd[80..88].copy_from_slice(&both32(self.total_sectors as u32));
        pvd[120..124].copy_from_slice(&both16(1));
        pvd[124..128].copy_from_slice(&both16(1));
        pvd[128..132].copy_from_slice(&both16(SECTOR as u16));
        pvd[132..140].copy_from_slice(&both32(self.path_table_size as u32));
        pvd[140..144].copy_from_slice(&(self.lba_path_l as u32).to_le_bytes());
        pvd[148..152].copy_from_slice(&(self.lba_path_m as u32).to_be_bytes());
        pvd[156..190].copy_from_slice(&dir_record(&[0], root.lba as u32, root.size as u32, true));
        pvd[190..318].copy_from_slice(&padded("", 128));
        pvd[318..446].copy_from_slice(&padded("VELORA OS PROJECT", 128));
        pvd[446..574].copy_from_slice(&padded("", 128));
        pvd[574..702].copy_from_slice(&padded("VELORA MKISO", 128));
        pvd[702..813].copy_from_slice(&padded("", 111));
        pvd[813..830].copy_from_slice(&date17());
        pvd[830..847].copy_from_slice(&date17());
        pvd[847..863].copy_from_slice(&[b'0'; 16]);
        pvd[864..881].copy_from_slice(&date17());
        pvd[881] = 1;

        // Terminator.
        let t = &mut iso[LBA_TERMINATOR * SECTOR..(LBA_TERMINATOR + 1) * SECTOR];
        t[0] = 255;
        t[1..6].copy_from_slice(b"CD001");
        t[6] = 1;
        iso
    }
}

/// Test helper: reads a file by path from an image
/// produced by this module (no Rock Ridge/Joliet support needed).
#[cfg(test)]
pub fn read_file(iso: &[u8], path: &str) -> Option<Vec<u8>> {
    let pvd = &iso[LBA_PVD * SECTOR..];
    let rec = &pvd[156..190];
    let mut lba = u32::from_le_bytes(rec[2..6].try_into().ok()?) as usize;
    let mut size = u32::from_le_bytes(rec[10..14].try_into().ok()?) as usize;
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let want = iso_name(part, !last).ok()?;
        let dir = &iso[lba * SECTOR..lba * SECTOR + size];
        let mut off = 0;
        let mut found = None;
        while off < dir.len() {
            let len = dir[off] as usize;
            if len == 0 {
                off = (off / SECTOR + 1) * SECTOR;
                continue;
            }
            let nlen = dir[off + 32] as usize;
            if &dir[off + 33..off + 33 + nlen] == want.as_bytes() {
                let r = &dir[off..off + len];
                found = Some((
                    u32::from_le_bytes(r[2..6].try_into().ok()?) as usize,
                    u32::from_le_bytes(r[10..14].try_into().ok()?) as usize,
                ));
                break;
            }
            off += len;
        }
        (lba, size) = found?;
    }
    Some(iso[lba * SECTOR..lba * SECTOR + size].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Vec<u8>, Vec<u8>) {
        let efi: Vec<u8> = (0..4 * 1024 * 1024).map(|i| (i % 253) as u8).collect();
        let mut b = IsoBuilder::new();
        b.add_file("boot/limine.conf", b"timeout: 0\n".to_vec())
            .unwrap();
        b.add_file("boot/velora", vec![0x7f, b'E', b'L', b'F'])
            .unwrap();
        b.add_file("EFI/BOOT/BOOTX64.EFI", vec![b'M', b'Z'])
            .unwrap();
        (b.build("VELORA_TEST", efi.clone()).unwrap(), efi)
    }

    #[test]
    fn names() {
        assert_eq!(iso_name("limine.conf", false).unwrap(), "LIMINE.CONF;1");
        assert_eq!(iso_name("velora", false).unwrap(), "VELORA.;1");
        assert_eq!(iso_name("boot", true).unwrap(), "BOOT");
        assert!(iso_name("a-b", false).is_err());
        assert!(iso_name("a.b.c", false).is_err());
        assert!(iso_name("dir.x", true).is_err());
    }

    #[test]
    fn volume_descriptors() {
        let (iso, _) = sample();
        let pvd = &iso[16 * SECTOR..17 * SECTOR];
        assert_eq!(pvd[0], 1);
        assert_eq!(&pvd[1..6], b"CD001");
        assert_eq!(&pvd[40..51], b"VELORA_TEST");
        let total = u32::from_le_bytes(pvd[80..84].try_into().unwrap()) as usize;
        assert_eq!(total * SECTOR, iso.len());
        assert_eq!(
            u32::from_be_bytes(pvd[84..88].try_into().unwrap()) as usize,
            total
        );
        let br = &iso[17 * SECTOR..18 * SECTOR];
        assert_eq!(br[0], 0);
        assert_eq!(&br[7..30], b"EL TORITO SPECIFICATION");
        assert_eq!(iso[18 * SECTOR], 255);
    }

    #[test]
    fn boot_catalog_is_valid() {
        let (iso, efi) = sample();
        let br = &iso[17 * SECTOR..18 * SECTOR];
        let cat_lba = u32::from_le_bytes(br[0x47..0x4b].try_into().unwrap()) as usize;
        assert_eq!(read_file(&iso, "BOOT.CAT").unwrap().len(), SECTOR);
        let cat = &iso[cat_lba * SECTOR..cat_lba * SECTOR + 64];
        let sum = cat[..32].as_chunks::<2>().0.iter().fold(0u16, |a, w| {
            a.wrapping_add(u16::from_le_bytes([w[0], w[1]]))
        });
        assert_eq!(sum, 0, "validation entry checksum");
        assert_eq!(&cat[30..32], &[0x55, 0xaa]);
        assert_eq!(cat[32], 0x88);
        let count = u16::from_le_bytes([cat[38], cat[39]]) as usize;
        let lba = u32::from_le_bytes(cat[40..44].try_into().unwrap()) as usize;
        assert_eq!(count * 512, efi.len());
        assert_eq!(&iso[lba * SECTOR..lba * SECTOR + efi.len()], &efi[..]);
    }

    #[test]
    fn files_are_readable_by_path() {
        let (iso, efi) = sample();
        assert_eq!(
            read_file(&iso, "boot/limine.conf").unwrap(),
            b"timeout: 0\n"
        );
        assert_eq!(read_file(&iso, "BOOT/VELORA").unwrap(), b"\x7fELF");
        assert_eq!(read_file(&iso, "efi/boot/bootx64.efi").unwrap(), b"MZ");
        assert_eq!(read_file(&iso, "EFIBOOT.IMG").unwrap(), efi);
        assert!(read_file(&iso, "boot/missing").is_none());
    }

    #[test]
    fn path_table_lists_directories_breadth_first() {
        let (iso, _) = sample();
        let pvd = &iso[16 * SECTOR..17 * SECTOR];
        let size = u32::from_le_bytes(pvd[132..136].try_into().unwrap()) as usize;
        let lba = u32::from_le_bytes(pvd[140..144].try_into().unwrap()) as usize;
        let pt = &iso[lba * SECTOR..lba * SECTOR + size];
        let mut names = Vec::new();
        let mut parents = Vec::new();
        let mut off = 0;
        while off < pt.len() {
            let l = pt[off] as usize;
            parents.push(u16::from_le_bytes([pt[off + 6], pt[off + 7]]));
            names.push(pt[off + 8..off + 8 + l].to_vec());
            off += 8 + l + l % 2;
        }
        assert_eq!(
            names,
            vec![vec![0], b"BOOT".to_vec(), b"EFI".to_vec(), b"BOOT".to_vec()]
        );
        assert_eq!(parents, vec![1, 1, 1, 3]);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(IsoBuilder::new().build("X", vec![]).is_err());
        assert!(IsoBuilder::new().build("X", vec![0; 100]).is_err());
        assert!(
            IsoBuilder::new()
                .build("X", vec![0; 40 * 1024 * 1024])
                .is_err()
        );
        let mut b = IsoBuilder::new();
        b.add_file("a", vec![1]).unwrap();
        assert!(b.add_file("a", vec![2]).is_err());
        assert!(b.add_file("a/b", vec![2]).is_err());
        assert!(b.add_file("boot.cat", vec![2]).is_err());
    }
}

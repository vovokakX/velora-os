//! On-disk structures. All integers are little-endian. See
//! docs/velorafs.md for the format description.

use alloc::vec::Vec;

pub const BLOCK: usize = 4096;
pub const MAGIC: [u8; 8] = *b"VELORAFS";
pub const VERSION: u32 = 1;
pub const INODE_SIZE: usize = 256;
pub const INODES_PER_BLOCK: u64 = (BLOCK / INODE_SIZE) as u64;
pub const BITS_PER_BLOCK: u64 = (BLOCK * 8) as u64;
pub const ROOT_INO: u64 = 1;
/// Offset of the CRC32C in superblock, directory, extent and journal blocks.
pub const CRC_OFF: usize = BLOCK - 4;

pub const INLINE_EXTENTS: usize = 10;
pub const EXT_BLOCK_MAGIC: u32 = u32::from_le_bytes(*b"VEXT");
pub const EXT_BLOCK_HEADER: usize = 16;
pub const EXT_PER_BLOCK: usize = (CRC_OFF - EXT_BLOCK_HEADER) / 16; // 255
pub const MAX_EXTENTS: usize = INLINE_EXTENTS + EXT_PER_BLOCK;

pub const JOURNAL_DESC_MAGIC: u32 = u32::from_le_bytes(*b"VJDS");
pub const JOURNAL_COMMIT_MAGIC: u32 = u32::from_le_bytes(*b"VJCM");
pub const JOURNAL_DESC_HEADER: usize = 32;
/// Home block numbers one descriptor can list.
pub const JOURNAL_DESC_MAX: usize = (CRC_OFF - JOURNAL_DESC_HEADER) / 8;

/// Directory record header: ino u64, rec_len u16, name_len u8, kind u8.
pub const DIRENT_HEADER: usize = 12;
/// Directory records use bytes 0..DIR_AREA of a block; the CRC follows.
pub const DIR_AREA: usize = BLOCK - 8;

pub const KIND_FILE: u8 = 1;
pub const KIND_DIR: u8 = 2;

pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;

pub fn get16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub fn get32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub fn get64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
pub fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
pub fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

// --- CRC32C (Castagnoli) ------------------------------------------------------

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0x82f6_3b78
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC_TABLE: [u32; 256] = crc_table();

/// Continues a CRC32C over `data`; start with `crc = 0`.
pub fn crc32c_update(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

pub fn crc32c(data: &[u8]) -> u32 {
    crc32c_update(0, data)
}

/// Writes the CRC of bytes 0..CRC_OFF into the block's last four bytes.
pub fn seal(block: &mut [u8]) {
    let c = crc32c(&block[..CRC_OFF]);
    put32(block, CRC_OFF, c);
}

pub fn sealed_ok(block: &[u8]) -> bool {
    crc32c(&block[..CRC_OFF]) == get32(block, CRC_OFF)
}

// --- superblock -------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Superblock {
    pub total_blocks: u64,
    pub inode_count: u64,
    pub free_blocks: u64,
    pub free_inodes: u64,
    pub journal_start: u64,
    pub journal_blocks: u64,
    pub ibitmap_start: u64,
    pub ibitmap_blocks: u64,
    pub bbitmap_start: u64,
    pub bbitmap_blocks: u64,
    pub itable_start: u64,
    pub itable_blocks: u64,
    pub data_start: u64,
    /// Sequence number the next journal transaction must carry.
    pub journal_seq: u64,
    pub uuid: [u8; 16],
    pub label: [u8; 32],
    pub created: u64,
    pub mount_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SbError {
    BadMagic,
    BadChecksum,
    BadVersion(u32),
    BadGeometry,
}

impl Superblock {
    pub fn encode(&self, b: &mut [u8]) {
        b.fill(0);
        b[0..8].copy_from_slice(&MAGIC);
        put32(b, 8, VERSION);
        put32(b, 12, BLOCK as u32);
        let fields = [
            self.total_blocks,
            self.inode_count,
            self.free_blocks,
            self.free_inodes,
            self.journal_start,
            self.journal_blocks,
            self.ibitmap_start,
            self.ibitmap_blocks,
            self.bbitmap_start,
            self.bbitmap_blocks,
            self.itable_start,
            self.itable_blocks,
            self.data_start,
            self.journal_seq,
        ];
        for (i, v) in fields.iter().enumerate() {
            put64(b, 16 + i * 8, *v);
        }
        b[128..144].copy_from_slice(&self.uuid);
        b[144..176].copy_from_slice(&self.label);
        put64(b, 176, self.created);
        put64(b, 184, self.mount_count);
        seal(b);
    }

    pub fn decode(b: &[u8]) -> Result<Self, SbError> {
        if b[0..8] != MAGIC {
            return Err(SbError::BadMagic);
        }
        if !sealed_ok(b) {
            return Err(SbError::BadChecksum);
        }
        let v = get32(b, 8);
        if v != VERSION {
            return Err(SbError::BadVersion(v));
        }
        if get32(b, 12) as usize != BLOCK {
            return Err(SbError::BadGeometry);
        }
        let f = |i: usize| get64(b, 16 + i * 8);
        let sb = Superblock {
            total_blocks: f(0),
            inode_count: f(1),
            free_blocks: f(2),
            free_inodes: f(3),
            journal_start: f(4),
            journal_blocks: f(5),
            ibitmap_start: f(6),
            ibitmap_blocks: f(7),
            bbitmap_start: f(8),
            bbitmap_blocks: f(9),
            itable_start: f(10),
            itable_blocks: f(11),
            data_start: f(12),
            journal_seq: f(13),
            uuid: b[128..144].try_into().unwrap(),
            label: b[144..176].try_into().unwrap(),
            created: get64(b, 176),
            mount_count: get64(b, 184),
        };
        sb.check_geometry()?;
        Ok(sb)
    }

    fn check_geometry(&self) -> Result<(), SbError> {
        let ok = self.journal_start == 1
            && self.journal_blocks >= 8
            && self.ibitmap_start == self.journal_start + self.journal_blocks
            && self.ibitmap_blocks == self.inode_count.div_ceil(BITS_PER_BLOCK)
            && self.bbitmap_start == self.ibitmap_start + self.ibitmap_blocks
            && self.bbitmap_blocks == self.total_blocks.div_ceil(BITS_PER_BLOCK)
            && self.itable_start == self.bbitmap_start + self.bbitmap_blocks
            && self.inode_count.is_multiple_of(INODES_PER_BLOCK)
            && self.itable_blocks == self.inode_count / INODES_PER_BLOCK
            && self.data_start == self.itable_start + self.itable_blocks
            && self.data_start < self.total_blocks
            && self.free_blocks <= self.total_blocks
            && self.free_inodes <= self.inode_count;
        if ok {
            Ok(())
        } else {
            Err(SbError::BadGeometry)
        }
    }

    pub fn label_str(&self) -> &str {
        let n = self.label.iter().position(|&c| c == 0).unwrap_or(32);
        core::str::from_utf8(&self.label[..n]).unwrap_or("")
    }
}

// --- inodes -----------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extent {
    /// First file block.
    pub logical: u32,
    pub len: u32,
    /// First device block.
    pub start: u64,
}

impl Extent {
    pub fn end(&self) -> u64 {
        self.logical as u64 + self.len as u64
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inode {
    /// Type and permission bits; 0 in a free slot.
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub size: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub crtime: u64,
    /// Parent directory (directories only).
    pub parent: u64,
    /// Extent block holding extents past the inline ones (0 = none).
    pub ext_block: u64,
    pub flags: u32,
    /// Inline extents (only the first `inline_count` are valid).
    pub inline: [Extent; INLINE_EXTENTS],
    /// Total extents: inline plus those in `ext_block`.
    pub extent_count: u32,
}

impl Inode {
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }

    pub fn is_file(&self) -> bool {
        self.mode & S_IFMT == S_IFREG
    }

    /// Encodes into a 256-byte slot. The CRC covers the inode number too,
    /// so an inode copied to the wrong slot is detected.
    pub fn encode(&self, ino: u64, b: &mut [u8]) {
        b[..INODE_SIZE].fill(0);
        put32(b, 0, self.mode);
        put32(b, 4, self.uid);
        put32(b, 8, self.gid);
        put32(b, 12, self.nlink);
        put64(b, 16, self.size);
        put64(b, 24, self.mtime);
        put64(b, 32, self.ctime);
        put64(b, 40, self.crtime);
        put32(b, 52, self.extent_count);
        put64(b, 56, self.parent);
        put64(b, 64, self.ext_block);
        put32(b, 72, self.flags);
        let inline = (self.extent_count as usize).min(INLINE_EXTENTS);
        for (i, e) in self.inline[..inline].iter().enumerate() {
            let o = 80 + i * 16;
            put32(b, o, e.logical);
            put32(b, o + 4, e.len);
            put64(b, o + 8, e.start);
        }
        let crc = inode_crc(ino, b);
        put32(b, 252, crc);
    }

    pub fn decode(ino: u64, b: &[u8]) -> Option<Self> {
        if inode_crc(ino, b) != get32(b, 252) {
            return None;
        }
        let mut n = Inode {
            mode: get32(b, 0),
            uid: get32(b, 4),
            gid: get32(b, 8),
            nlink: get32(b, 12),
            size: get64(b, 16),
            mtime: get64(b, 24),
            ctime: get64(b, 32),
            crtime: get64(b, 40),
            extent_count: get32(b, 52),
            parent: get64(b, 56),
            ext_block: get64(b, 64),
            flags: get32(b, 72),
            inline: [Extent::default(); INLINE_EXTENTS],
        };
        if n.extent_count as usize > MAX_EXTENTS {
            return None;
        }
        let inline = (n.extent_count as usize).min(INLINE_EXTENTS);
        for (i, slot) in n.inline[..inline].iter_mut().enumerate() {
            let o = 80 + i * 16;
            *slot = Extent {
                logical: get32(b, o),
                len: get32(b, o + 4),
                start: get64(b, o + 8),
            };
        }
        Some(n)
    }
}

fn inode_crc(ino: u64, b: &[u8]) -> u32 {
    crc32c_update(crc32c(&ino.to_le_bytes()), &b[..252])
}

/// Extents stored in an overflow block.
pub fn encode_ext_block(owner: u64, exts: &[Extent], b: &mut [u8]) {
    b.fill(0);
    put32(b, 0, EXT_BLOCK_MAGIC);
    put32(b, 4, exts.len() as u32);
    put64(b, 8, owner);
    for (i, e) in exts.iter().enumerate() {
        let o = EXT_BLOCK_HEADER + i * 16;
        put32(b, o, e.logical);
        put32(b, o + 4, e.len);
        put64(b, o + 8, e.start);
    }
    seal(b);
}

pub fn decode_ext_block(owner: u64, b: &[u8]) -> Option<Vec<Extent>> {
    if get32(b, 0) != EXT_BLOCK_MAGIC || !sealed_ok(b) || get64(b, 8) != owner {
        return None;
    }
    let n = get32(b, 4) as usize;
    if n > EXT_PER_BLOCK {
        return None;
    }
    Some(
        (0..n)
            .map(|i| {
                let o = EXT_BLOCK_HEADER + i * 16;
                Extent {
                    logical: get32(b, o),
                    len: get32(b, o + 4),
                    start: get64(b, o + 8),
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32c_known_values() {
        assert_eq!(crc32c(b""), 0);
        assert_eq!(crc32c(b"123456789"), 0xe306_9283);
        assert_eq!(crc32c_update(crc32c(b"1234"), b"56789"), 0xe306_9283);
    }

    #[test]
    fn inode_round_trip_and_checksum() {
        let mut n = Inode {
            mode: S_IFREG | 0o640,
            uid: 1000,
            gid: 100,
            nlink: 1,
            size: 12345,
            extent_count: 2,
            ..Default::default()
        };
        n.inline[0] = Extent {
            logical: 0,
            len: 3,
            start: 100,
        };
        n.inline[1] = Extent {
            logical: 5,
            len: 1,
            start: 200,
        };
        let mut b = [0u8; INODE_SIZE];
        n.encode(7, &mut b);
        assert_eq!(Inode::decode(7, &b), Some(n.clone()));
        // Wrong slot or a flipped bit is detected.
        assert_eq!(Inode::decode(8, &b), None);
        b[17] ^= 1;
        assert_eq!(Inode::decode(7, &b), None);
    }
}

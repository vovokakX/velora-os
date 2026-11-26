//! Directories: blocks of variable-length records.
//!
//! Record: ino u64 (0 = unused), rec_len u16, name_len u8, kind u8, name.
//! Records are 8-byte aligned and cover bytes 0..DIR_AREA of the block
//! exactly; the block CRC is in the last four bytes.

use crate::layout::*;
use crate::{BLOCK, VeloraFs};
use alloc::string::String;
use alloc::vec::Vec;
use velora_vfs::{BlockIo, DirEntry, FsError, Ino, Kind, Result};

pub(crate) fn init_block(b: &mut [u8]) {
    b.fill(0);
    put16(b, 8, DIR_AREA as u16);
    seal(b);
}

fn rec_size(name_len: usize) -> usize {
    (DIRENT_HEADER + name_len).next_multiple_of(8)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Rec {
    pub off: usize,
    pub ino: u64,
    pub rec_len: usize,
    pub name_len: usize,
    pub kind: u8,
}

/// Parses and validates every record of a directory block.
pub(crate) fn records(b: &[u8]) -> Result<Vec<Rec>> {
    if !sealed_ok(b) {
        return Err(FsError::Corrupt);
    }
    let mut v = Vec::new();
    let mut off = 0;
    while off < DIR_AREA {
        if off + DIRENT_HEADER > DIR_AREA {
            return Err(FsError::Corrupt);
        }
        let r = Rec {
            off,
            ino: get64(b, off),
            rec_len: get16(b, off + 8) as usize,
            name_len: b[off + 10] as usize,
            kind: b[off + 11],
        };
        if r.rec_len < rec_size(0) || !r.rec_len.is_multiple_of(8) || off + r.rec_len > DIR_AREA {
            return Err(FsError::Corrupt);
        }
        if r.ino != 0
            && (r.name_len == 0
                || rec_size(r.name_len) > r.rec_len
                || (r.kind != KIND_FILE && r.kind != KIND_DIR)
                || core::str::from_utf8(&b[off + DIRENT_HEADER..off + DIRENT_HEADER + r.name_len])
                    .is_err())
        {
            return Err(FsError::Corrupt);
        }
        v.push(r);
        off += r.rec_len;
    }
    Ok(v)
}

pub(crate) fn name_of<'a>(b: &'a [u8], r: &Rec) -> &'a [u8] {
    &b[r.off + DIRENT_HEADER..r.off + DIRENT_HEADER + r.name_len]
}

pub(crate) fn kind_byte(k: Kind) -> u8 {
    if k == Kind::Dir { KIND_DIR } else { KIND_FILE }
}

pub(crate) struct Found {
    pub ino: Ino,
}

impl<D: BlockIo> VeloraFs<D> {
    /// Physical blocks of a directory in order. Directories have no holes.
    pub(crate) fn dir_blocks(&mut self, ino: Ino, n: &Inode) -> Result<Vec<u64>> {
        let mut v = Vec::new();
        for e in self.extents(ino, n)? {
            if e.logical as usize != v.len() {
                return Err(FsError::Corrupt);
            }
            v.extend(e.start..e.start + e.len as u64);
        }
        if n.size != (v.len() * BLOCK) as u64 {
            return Err(FsError::Corrupt);
        }
        Ok(v)
    }

    pub(crate) fn dir_find(&mut self, dir: Ino, d: &Inode, name: &str) -> Result<Option<Found>> {
        for p in self.dir_blocks(dir, d)? {
            let b = self.block(p)?;
            for r in records(&b[..])? {
                if r.ino != 0 && name_of(&b[..], &r) == name.as_bytes() {
                    return Ok(Some(Found { ino: r.ino }));
                }
            }
        }
        Ok(None)
    }

    pub(crate) fn dir_is_empty(&mut self, dir: Ino, d: &Inode) -> Result<bool> {
        for p in self.dir_blocks(dir, d)? {
            let b = self.block(p)?;
            if records(&b[..])?.iter().any(|r| r.ino != 0) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn dir_add(&mut self, dir: Ino, name: &str, ino: Ino, kind: Kind) -> Result<()> {
        let need = rec_size(name.len());
        let mut d = self.inode(dir)?;
        let blocks = self.dir_blocks(dir, &d)?;
        let mut slot = None;
        'outer: for &p in &blocks {
            let b = self.block(p)?;
            for r in records(&b[..])? {
                let used = if r.ino == 0 { 0 } else { rec_size(r.name_len) };
                if r.rec_len - used >= need {
                    slot = Some((p, r, used));
                    break 'outer;
                }
            }
        }
        let (p, r, used) = match slot {
            Some(s) => s,
            None => {
                // Grow the directory by one block.
                let goal = blocks.last().map_or(0, |&b| b + 1);
                let p = self.alloc_block(goal)?;
                init_block(self.new_block_mut(p));
                let mut exts = self.extents(dir, &d)?;
                exts.push(Extent {
                    logical: blocks.len() as u32,
                    len: 1,
                    start: p,
                });
                self.set_extents(dir, &mut d, exts)?;
                d.size += BLOCK as u64;
                self.put_inode(dir, &d)?;
                let rec = Rec {
                    off: 0,
                    ino: 0,
                    rec_len: DIR_AREA,
                    name_len: 0,
                    kind: 0,
                };
                (p, rec, 0)
            }
        };
        let b = self.block_mut(p)?;
        let (off, len) = if used == 0 {
            (r.off, r.rec_len)
        } else {
            put16(b, r.off + 8, used as u16);
            (r.off + used, r.rec_len - used)
        };
        b[off..off + len].fill(0);
        put64(b, off, ino);
        put16(b, off + 8, len as u16);
        b[off + 10] = name.len() as u8;
        b[off + 11] = kind_byte(kind);
        b[off + DIRENT_HEADER..off + DIRENT_HEADER + name.len()].copy_from_slice(name.as_bytes());
        seal(b);
        Ok(())
    }

    pub(crate) fn dir_remove(&mut self, dir: Ino, name: &str) -> Result<()> {
        let d = self.inode(dir)?;
        for p in self.dir_blocks(dir, &d)? {
            let b = self.block(p)?;
            let recs = records(&b[..])?;
            let Some(i) = recs
                .iter()
                .position(|r| r.ino != 0 && name_of(&b[..], r) == name.as_bytes())
            else {
                continue;
            };
            let r = recs[i];
            let b = self.block_mut(p)?;
            if i == 0 {
                b[r.off..r.off + r.rec_len].fill(0);
                put16(b, r.off + 8, r.rec_len as u16);
            } else {
                let prev = recs[i - 1];
                put16(b, prev.off + 8, (prev.rec_len + r.rec_len) as u16);
                b[r.off..r.off + r.rec_len].fill(0);
            }
            seal(b);
            return Ok(());
        }
        Err(FsError::NotFound)
    }

    /// The entry at or after `pos` (block index << 12 | byte offset).
    pub(crate) fn dir_next(
        &mut self,
        dir: Ino,
        d: &Inode,
        pos: u64,
    ) -> Result<Option<(DirEntry, u64)>> {
        let blocks = self.dir_blocks(dir, d)?;
        let mut bi = (pos >> 12) as usize;
        let mut from = (pos & 0xfff) as usize;
        while bi < blocks.len() {
            let b = self.block(blocks[bi])?;
            for r in records(&b[..])? {
                if r.off < from || r.ino == 0 {
                    continue;
                }
                let name = String::from(
                    core::str::from_utf8(name_of(&b[..], &r)).map_err(|_| FsError::Corrupt)?,
                );
                let end = r.off + r.rec_len;
                let next = if end >= DIR_AREA {
                    ((bi as u64) + 1) << 12
                } else {
                    ((bi as u64) << 12) | end as u64
                };
                let kind = if r.kind == KIND_DIR {
                    Kind::Dir
                } else {
                    Kind::File
                };
                return Ok(Some((
                    DirEntry {
                        ino: r.ino,
                        kind,
                        name,
                    },
                    next,
                )));
            }
            bi += 1;
            from = 0;
        }
        Ok(None)
    }
}

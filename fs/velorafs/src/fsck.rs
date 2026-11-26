//! Consistency check (fsck). It reads the whole file system and reports
//! problems; it does not change anything. Repair is NOT IMPLEMENTED yet:
//! the journal keeps the file system consistent after crashes, so the
//! check is used to confirm that and to detect corruption.

use crate::VeloraFs;
use crate::dir::records;
use crate::layout::*;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use velora_vfs::{BlockIo, FsError};

const MAX_ERRORS: usize = 100;

#[derive(Debug, Default)]
pub struct Report {
    pub errors: Vec<String>,
    pub files: u64,
    pub dirs: u64,
    /// Data blocks referenced by files and directories.
    pub data_blocks: u64,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }

    fn err(&mut self, s: String) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(s);
        }
    }
}

impl<D: BlockIo> VeloraFs<D> {
    /// Checks every reachable inode, directory and extent, the bitmaps and
    /// the free counts.
    pub fn check(&mut self) -> Report {
        let mut rep = Report::default();
        if !self.dirty.is_empty() {
            rep.err(String::from("uncommitted changes in memory"));
            return rep;
        }
        let total = self.sb.total_blocks;
        let mut used = vec![0u64; total.div_ceil(64) as usize];
        let mark = |used: &mut Vec<u64>, b: u64| -> bool {
            let (w, m) = ((b / 64) as usize, 1u64 << (b % 64));
            let was = used[w] & m != 0;
            used[w] |= m;
            !was
        };
        for b in 0..self.sb.data_start {
            mark(&mut used, b);
        }
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        // (inode, parent directory, kind byte from the directory entry)
        let mut stack = vec![(ROOT_INO, ROOT_INO, KIND_DIR)];
        while let Some((ino, parent, kind)) = stack.pop() {
            if !seen.insert(ino) {
                rep.err(format!("inode {} is referenced more than once", ino));
                continue;
            }
            let n = match self.inode(ino) {
                Ok(n) => n,
                Err(FsError::NotFound) => {
                    rep.err(format!("inode {} is referenced but marked free", ino));
                    continue;
                }
                Err(e) => {
                    rep.err(format!("inode {}: {}", ino, e));
                    continue;
                }
            };
            if n.is_dir() != (kind == KIND_DIR) {
                rep.err(format!(
                    "inode {}: directory entry type does not match the inode",
                    ino
                ));
            }
            let exts = match self.extents(ino, &n) {
                Ok(e) => e,
                Err(e) => {
                    rep.err(format!("inode {}: bad extents ({})", ino, e));
                    continue;
                }
            };
            for e in &exts {
                for b in e.start..e.start + e.len as u64 {
                    rep.data_blocks += 1;
                    if !mark(&mut used, b) {
                        rep.err(format!("block {} is used twice (inode {})", b, ino));
                    }
                }
            }
            if n.ext_block != 0 && !mark(&mut used, n.ext_block) {
                rep.err(format!(
                    "extent block {} is used twice (inode {})",
                    n.ext_block, ino
                ));
            }
            let blocks_for_size = n.size.div_ceil(BLOCK as u64);
            if exts.last().is_some_and(|e| e.end() > blocks_for_size) {
                rep.err(format!("inode {}: extents go past the file size", ino));
            }
            if !n.is_dir() {
                rep.files += 1;
                if n.nlink != 1 {
                    rep.err(format!(
                        "inode {}: link count {} (expected 1)",
                        ino, n.nlink
                    ));
                }
                continue;
            }
            rep.dirs += 1;
            if n.parent != parent {
                rep.err(format!(
                    "directory {}: parent {} (expected {})",
                    ino, n.parent, parent
                ));
            }
            let blocks = match self.dir_blocks(ino, &n) {
                Ok(b) => b,
                Err(e) => {
                    rep.err(format!("directory {}: {}", ino, e));
                    continue;
                }
            };
            let mut names = BTreeSet::new();
            let mut subdirs = 0;
            for p in blocks {
                let b = match self.block(p) {
                    Ok(b) => b.to_vec(),
                    Err(e) => {
                        rep.err(format!("directory {} block {}: {}", ino, p, e));
                        continue;
                    }
                };
                let recs = match records(&b) {
                    Ok(r) => r,
                    Err(_) => {
                        rep.err(format!(
                            "directory {} block {}: bad checksum or records",
                            ino, p
                        ));
                        continue;
                    }
                };
                for r in recs.iter().filter(|r| r.ino != 0) {
                    let name = &b[r.off + DIRENT_HEADER..r.off + DIRENT_HEADER + r.name_len];
                    if !names.insert(name.to_vec()) {
                        rep.err(format!("directory {}: duplicate name", ino));
                    }
                    if r.ino >= self.sb.inode_count {
                        rep.err(format!(
                            "directory {}: entry points to inode {} out of range",
                            ino, r.ino
                        ));
                        continue;
                    }
                    if r.kind == KIND_DIR {
                        subdirs += 1;
                    }
                    stack.push((r.ino, ino, r.kind));
                }
            }
            if n.nlink != 2 + subdirs {
                rep.err(format!(
                    "directory {}: link count {} (expected {})",
                    ino,
                    n.nlink,
                    2 + subdirs
                ));
            }
        }

        // Block bitmap against what the tree uses.
        let mut free_blocks = 0;
        for i in 0..self.sb.bbitmap_blocks {
            let blk = match self.block(self.sb.bbitmap_start + i) {
                Ok(b) => b.to_vec(),
                Err(e) => {
                    rep.err(format!("block bitmap {}: {}", i, e));
                    return rep;
                }
            };
            for k in 0..BITS_PER_BLOCK {
                let b = i * BITS_PER_BLOCK + k;
                if b >= total {
                    break;
                }
                let on_disk = blk[(k / 8) as usize] & (1 << (k % 8)) != 0;
                let in_use = used[(b / 64) as usize] & (1 << (b % 64)) != 0;
                if !on_disk {
                    free_blocks += 1;
                }
                if in_use && !on_disk {
                    rep.err(format!("block {} is in use but marked free", b));
                } else if on_disk && !in_use {
                    rep.err(format!("block {} is marked used but not referenced", b));
                }
            }
        }
        if free_blocks != self.sb.free_blocks {
            rep.err(format!(
                "superblock free blocks {} (counted {})",
                self.sb.free_blocks, free_blocks
            ));
        }

        // Inode bitmap against the reachable inodes.
        let mut free_inodes = 0;
        let mut orphans = BTreeMap::new();
        for i in 0..self.sb.ibitmap_blocks {
            let blk = match self.block(self.sb.ibitmap_start + i) {
                Ok(b) => b.to_vec(),
                Err(e) => {
                    rep.err(format!("inode bitmap {}: {}", i, e));
                    return rep;
                }
            };
            for k in 0..BITS_PER_BLOCK {
                let ino = i * BITS_PER_BLOCK + k;
                if ino >= self.sb.inode_count {
                    break;
                }
                let on_disk = blk[(k / 8) as usize] & (1 << (k % 8)) != 0;
                if !on_disk {
                    free_inodes += 1;
                } else if ino != 0 && !seen.contains(&ino) {
                    orphans.insert(ino, ());
                }
            }
        }
        for ino in orphans.keys() {
            rep.err(format!("inode {} is allocated but not reachable", ino));
        }
        if free_inodes != self.sb.free_inodes {
            rep.err(format!(
                "superblock free inodes {} (counted {})",
                self.sb.free_inodes, free_inodes
            ));
        }
        rep
    }
}

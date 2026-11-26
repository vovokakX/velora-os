//! VeloraFS, the native file system of Velora OS.
//!
//! - 4 KiB blocks; files are lists of extents (10 in the inode, 255 more in
//!   one overflow block); holes read as zeros.
//! - POSIX owner, group and permission bits on every inode.
//! - Metadata journal: every operation is one transaction. Changed metadata
//!   blocks go to the journal first, then to their home location. File data
//!   is written before the transaction commits (ordered mode), so after a
//!   crash metadata never points at unwritten data.
//! - CRC32C on the superblock, inodes, directory blocks, extent blocks and
//!   journal blocks. File data has no checksum.
//! - [`VeloraFs::check`] verifies the whole file system (fsck).
//!
//! The format is described in docs/velorafs.md.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod dir;
mod fsck;
pub mod layout;

pub use fsck::Report;
pub use layout::Superblock;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec;
use alloc::vec::Vec;
use layout::*;
use velora_vfs::{
    BlockIo, DirEntry, FileSystem, FsError, FsStat, Ino, Kind, NewAttr, Result, SetAttr, Stat, path,
};

/// Clean metadata blocks kept in memory before older ones are dropped.
const CACHE_MAX: usize = 2048;
/// Longest run of data blocks moved in one device request.
const RUN_MAX: u64 = 32;

type Block = Box<[u8; BLOCK]>;

fn zero_block() -> Block {
    Box::new([0u8; BLOCK])
}

pub struct FormatOptions<'a> {
    pub label: &'a str,
    /// Creation time, seconds since 1970.
    pub now: u64,
    pub uuid: [u8; 16],
}

/// Creates an empty VeloraFS on the whole device.
pub fn format<D: BlockIo + ?Sized>(dev: &mut D, opts: &FormatOptions) -> Result<Superblock> {
    let ss = dev.sector_size();
    if ss == 0 || !BLOCK.is_multiple_of(ss) {
        return Err(FsError::Invalid);
    }
    let spb = (BLOCK / ss) as u64;
    let total = dev.sector_count() / spb;
    if total < 128 {
        return Err(FsError::NoSpace);
    }
    let journal = (total / 32).clamp(32, 1024);
    let inodes = (total / 4)
        .clamp(64, 1 << 20)
        .next_multiple_of(INODES_PER_BLOCK);
    let ib = inodes.div_ceil(BITS_PER_BLOCK);
    let bb = total.div_ceil(BITS_PER_BLOCK);
    let it = inodes / INODES_PER_BLOCK;
    let data_start = 1 + journal + ib + bb + it;
    if data_start + 16 > total {
        return Err(FsError::NoSpace);
    }
    let mut label = [0u8; 32];
    let n = opts.label.len().min(31);
    label[..n].copy_from_slice(&opts.label.as_bytes()[..n]);
    let sb = Superblock {
        total_blocks: total,
        inode_count: inodes,
        free_blocks: total - data_start - 1,
        free_inodes: inodes - 2,
        journal_start: 1,
        journal_blocks: journal,
        ibitmap_start: 1 + journal,
        ibitmap_blocks: ib,
        bbitmap_start: 1 + journal + ib,
        bbitmap_blocks: bb,
        itable_start: 1 + journal + ib + bb,
        itable_blocks: it,
        data_start,
        journal_seq: 1,
        uuid: opts.uuid,
        label,
        created: opts.now,
        mount_count: 0,
    };
    let wr = |dev: &mut D, blk: u64, b: &[u8]| dev.write(blk * spb, b);
    let mut b = zero_block();
    // Invalidate any old journal.
    wr(dev, sb.journal_start, &b[..])?;
    // Inode bitmap: inode 0 (never used) and the root.
    for i in 0..ib {
        b.fill(0);
        if i == 0 {
            b[0] = 0b11;
        }
        wr(dev, sb.ibitmap_start + i, &b[..])?;
    }
    // Block bitmap: all metadata blocks plus the root directory block.
    let used = data_start + 1;
    for i in 0..bb {
        b.fill(0);
        let first = i * BITS_PER_BLOCK;
        for bit in first..(first + BITS_PER_BLOCK).min(used) {
            let k = (bit - first) as usize;
            b[k / 8] |= 1 << (k % 8);
        }
        // Bits past the end of the device stay set so they are never used.
        for bit in total.max(first)..first + BITS_PER_BLOCK {
            let k = (bit - first) as usize;
            b[k / 8] |= 1 << (k % 8);
        }
        wr(dev, sb.bbitmap_start + i, &b[..])?;
    }
    // Root inode and its first directory block.
    let mut root = Inode {
        mode: S_IFDIR | 0o755,
        nlink: 2,
        size: BLOCK as u64,
        mtime: opts.now,
        ctime: opts.now,
        crtime: opts.now,
        parent: ROOT_INO,
        extent_count: 1,
        ..Default::default()
    };
    root.inline[0] = Extent {
        logical: 0,
        len: 1,
        start: data_start,
    };
    b.fill(0);
    root.encode(ROOT_INO, &mut b[INODE_SIZE..2 * INODE_SIZE]);
    wr(dev, sb.itable_start, &b[..])?;
    b.fill(0);
    dir::init_block(&mut b[..]);
    wr(dev, data_start, &b[..])?;
    dev.flush()?;
    sb.encode(&mut b[..]);
    wr(dev, 0, &b[..])?;
    dev.flush()?;
    Ok(sb)
}

/// Reads and checks the superblock without mounting.
pub fn probe<D: BlockIo + ?Sized>(dev: &mut D) -> Option<Superblock> {
    let ss = dev.sector_size();
    if ss == 0 || !BLOCK.is_multiple_of(ss) {
        return None;
    }
    let mut b = zero_block();
    dev.read(0, &mut b[..]).ok()?;
    Superblock::decode(&b[..]).ok()
}

pub struct VeloraFs<D: BlockIo> {
    dev: D,
    spb: u64,
    sb: Superblock,
    sb_saved: Superblock,
    cache: BTreeMap<u64, Block>,
    dirty: BTreeSet<u64>,
    data_written: bool,
    block_hint: u64,
    inode_hint: u64,
    read_only: bool,
    /// A commit failed: the on-disk state is only known after remounting.
    failed: bool,
    recovered: bool,
    max_txn: usize,
    /// Test hook: after the next journal commit, stop as if the power had
    /// been cut (no checkpoint, every later change fails).
    pub stop_after_commit: bool,
}

impl<D: BlockIo> VeloraFs<D> {
    /// Mounts the file system, replaying the journal if the last
    /// transaction was committed but not written home.
    pub fn mount(mut dev: D, read_only: bool) -> Result<Self> {
        let ss = dev.sector_size();
        if ss == 0 || !BLOCK.is_multiple_of(ss) {
            return Err(FsError::Invalid);
        }
        let spb = (BLOCK / ss) as u64;
        let mut b = zero_block();
        dev.read(0, &mut b[..])?;
        let mut sb = Superblock::decode(&b[..]).map_err(|_| FsError::Corrupt)?;
        if sb.total_blocks * spb > dev.sector_count() {
            return Err(FsError::Corrupt);
        }
        let recovered = replay(&mut dev, spb, &sb)?;
        if recovered {
            dev.read(0, &mut b[..])?;
            sb = Superblock::decode(&b[..]).map_err(|_| FsError::Corrupt)?;
        }
        let max_txn = JOURNAL_DESC_MAX.min(sb.journal_blocks as usize - 2);
        Ok(Self {
            dev,
            spb,
            block_hint: sb.data_start,
            inode_hint: ROOT_INO,
            sb_saved: sb.clone(),
            sb,
            cache: BTreeMap::new(),
            dirty: BTreeSet::new(),
            data_written: false,
            read_only,
            failed: false,
            recovered,
            max_txn,
            stop_after_commit: false,
        })
    }

    /// True if mounting replayed a journal transaction.
    pub fn recovered(&self) -> bool {
        self.recovered
    }

    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    pub fn into_device(self) -> D {
        self.dev
    }

    // --- block access -------------------------------------------------------

    fn dev_read(&mut self, blk: u64, buf: &mut [u8]) -> Result<()> {
        self.dev.read(blk * self.spb, buf)
    }

    fn dev_write(&mut self, blk: u64, buf: &[u8]) -> Result<()> {
        self.dev.write(blk * self.spb, buf)
    }

    fn load(&mut self, blk: u64) -> Result<()> {
        if blk == 0 || blk >= self.sb.total_blocks {
            return Err(FsError::Corrupt);
        }
        if !self.cache.contains_key(&blk) {
            if self.cache.len() >= CACHE_MAX {
                let clean: Vec<u64> = self
                    .cache
                    .keys()
                    .filter(|k| !self.dirty.contains(k))
                    .take(CACHE_MAX / 4)
                    .copied()
                    .collect();
                for k in clean {
                    self.cache.remove(&k);
                }
            }
            let mut b = zero_block();
            self.dev_read(blk, &mut b[..])?;
            self.cache.insert(blk, b);
        }
        Ok(())
    }

    fn block(&mut self, blk: u64) -> Result<&[u8; BLOCK]> {
        self.load(blk)?;
        Ok(&self.cache[&blk])
    }

    fn block_mut(&mut self, blk: u64) -> Result<&mut [u8; BLOCK]> {
        self.load(blk)?;
        self.dirty.insert(blk);
        Ok(self.cache.get_mut(&blk).unwrap())
    }

    /// A freshly allocated metadata block: zeroed, not read from disk.
    fn new_block_mut(&mut self, blk: u64) -> &mut [u8; BLOCK] {
        self.dirty.insert(blk);
        let b = self.cache.entry(blk).or_insert_with(zero_block);
        b.fill(0);
        b
    }

    // --- bitmaps ------------------------------------------------------------

    fn bit(&mut self, start: u64, idx: u64) -> Result<bool> {
        let b = self.block(start + idx / BITS_PER_BLOCK)?;
        let k = (idx % BITS_PER_BLOCK) as usize;
        Ok(b[k / 8] & (1 << (k % 8)) != 0)
    }

    fn set_bit(&mut self, start: u64, idx: u64, v: bool) -> Result<()> {
        let b = self.block_mut(start + idx / BITS_PER_BLOCK)?;
        let k = (idx % BITS_PER_BLOCK) as usize;
        if v {
            b[k / 8] |= 1 << (k % 8);
        } else {
            b[k / 8] &= !(1 << (k % 8));
        }
        Ok(())
    }

    /// First clear bit in `from..to`.
    fn find_clear(&mut self, start: u64, from: u64, to: u64) -> Result<Option<u64>> {
        let mut idx = from;
        while idx < to {
            let blk = idx / BITS_PER_BLOCK;
            let b = self.block(start + blk)?;
            let mut k = idx % BITS_PER_BLOCK;
            while k < BITS_PER_BLOCK {
                let word_i = (k / 64) as usize;
                let mut w = u64::from_le_bytes(b[word_i * 8..word_i * 8 + 8].try_into().unwrap());
                w |= (1u64 << (k % 64)) - 1; // ignore bits below k
                if w != u64::MAX {
                    let found =
                        blk * BITS_PER_BLOCK + word_i as u64 * 64 + (!w).trailing_zeros() as u64;
                    return Ok((found < to).then_some(found));
                }
                k = (word_i as u64 + 1) * 64;
            }
            idx = (blk + 1) * BITS_PER_BLOCK;
        }
        Ok(None)
    }

    fn alloc_block(&mut self, goal: u64) -> Result<u64> {
        let (ds, total) = (self.sb.data_start, self.sb.total_blocks);
        let from = if goal >= ds && goal < total {
            goal
        } else {
            self.block_hint.clamp(ds, total - 1)
        };
        let bb = self.sb.bbitmap_start;
        let found = match self.find_clear(bb, from, total)? {
            Some(b) => Some(b),
            None => self.find_clear(bb, ds, from)?,
        };
        let b = found.ok_or(FsError::NoSpace)?;
        self.set_bit(bb, b, true)?;
        self.sb.free_blocks = self.sb.free_blocks.checked_sub(1).ok_or(FsError::Corrupt)?;
        self.block_hint = b + 1;
        Ok(b)
    }

    fn free_block(&mut self, b: u64) -> Result<()> {
        if b < self.sb.data_start || b >= self.sb.total_blocks {
            return Err(FsError::Corrupt);
        }
        let bb = self.sb.bbitmap_start;
        if !self.bit(bb, b)? {
            return Err(FsError::Corrupt); // double free
        }
        self.set_bit(bb, b, false)?;
        self.sb.free_blocks += 1;
        Ok(())
    }

    // --- inodes -------------------------------------------------------------

    fn inode_loc(&self, ino: Ino) -> Result<(u64, usize)> {
        if ino == 0 || ino >= self.sb.inode_count {
            return Err(FsError::NotFound);
        }
        Ok((
            self.sb.itable_start + ino / INODES_PER_BLOCK,
            (ino % INODES_PER_BLOCK) as usize * INODE_SIZE,
        ))
    }

    fn inode(&mut self, ino: Ino) -> Result<Inode> {
        let (blk, off) = self.inode_loc(ino)?;
        if !self.bit(self.sb.ibitmap_start, ino)? {
            return Err(FsError::NotFound);
        }
        let b = self.block(blk)?;
        let n = Inode::decode(ino, &b[off..off + INODE_SIZE]).ok_or(FsError::Corrupt)?;
        if !n.is_dir() && !n.is_file() {
            return Err(FsError::Corrupt);
        }
        Ok(n)
    }

    fn put_inode(&mut self, ino: Ino, n: &Inode) -> Result<()> {
        let (blk, off) = self.inode_loc(ino)?;
        let b = self.block_mut(blk)?;
        n.encode(ino, &mut b[off..off + INODE_SIZE]);
        Ok(())
    }

    fn alloc_inode(&mut self) -> Result<Ino> {
        let (ib, count) = (self.sb.ibitmap_start, self.sb.inode_count);
        let hint = self.inode_hint.clamp(1, count - 1);
        let found = match self.find_clear(ib, hint, count)? {
            Some(i) => Some(i),
            None => self.find_clear(ib, 1, hint)?,
        };
        let ino = found.ok_or(FsError::NoSpace)?;
        self.set_bit(ib, ino, true)?;
        self.sb.free_inodes = self.sb.free_inodes.checked_sub(1).ok_or(FsError::Corrupt)?;
        self.inode_hint = ino + 1;
        Ok(ino)
    }

    /// Frees an inode and every block it owns.
    fn release(&mut self, ino: Ino, n: &Inode) -> Result<()> {
        for e in self.extents(ino, n)? {
            for b in e.start..e.start + e.len as u64 {
                self.free_block(b)?;
            }
        }
        if n.ext_block != 0 {
            self.free_block(n.ext_block)?;
        }
        let (blk, off) = self.inode_loc(ino)?;
        self.block_mut(blk)?[off..off + INODE_SIZE].fill(0);
        self.set_bit(self.sb.ibitmap_start, ino, false)?;
        self.sb.free_inodes += 1;
        Ok(())
    }

    // --- extents ------------------------------------------------------------

    fn extents(&mut self, ino: Ino, n: &Inode) -> Result<Vec<Extent>> {
        let count = n.extent_count as usize;
        let mut v: Vec<Extent> = n.inline[..count.min(INLINE_EXTENTS)].to_vec();
        if count > INLINE_EXTENTS {
            let more =
                decode_ext_block(ino, &self.block(n.ext_block)?[..]).ok_or(FsError::Corrupt)?;
            if more.len() != count - INLINE_EXTENTS {
                return Err(FsError::Corrupt);
            }
            v.extend(more);
        } else if n.ext_block != 0 {
            return Err(FsError::Corrupt);
        }
        let (ds, total) = (self.sb.data_start, self.sb.total_blocks);
        let mut prev_end = 0;
        for e in &v {
            if e.len == 0
                || e.start < ds
                || e.start + e.len as u64 > total
                || (e.logical as u64) < prev_end
            {
                return Err(FsError::Corrupt);
            }
            prev_end = e.end();
        }
        Ok(v)
    }

    /// Stores `exts` (sorted by logical block) into the inode, merging
    /// neighbours and moving the overflow into an extent block.
    fn set_extents(&mut self, ino: Ino, n: &mut Inode, exts: Vec<Extent>) -> Result<()> {
        let mut merged: Vec<Extent> = Vec::with_capacity(exts.len());
        for e in exts.into_iter().filter(|e| e.len > 0) {
            if let Some(last) = merged.last_mut()
                && last.end() == e.logical as u64
                && last.start + last.len as u64 == e.start
            {
                last.len += e.len;
                continue;
            }
            merged.push(e);
        }
        if merged.len() > MAX_EXTENTS {
            return Err(FsError::TooBig);
        }
        n.inline = [Extent::default(); INLINE_EXTENTS];
        let k = merged.len().min(INLINE_EXTENTS);
        n.inline[..k].copy_from_slice(&merged[..k]);
        n.extent_count = merged.len() as u32;
        if merged.len() > INLINE_EXTENTS {
            if n.ext_block == 0 {
                let goal = merged.last().map(|e| e.start + e.len as u64).unwrap_or(0);
                n.ext_block = self.alloc_block(goal)?;
                self.new_block_mut(n.ext_block);
            }
            let blk = n.ext_block;
            encode_ext_block(ino, &merged[INLINE_EXTENTS..], self.block_mut(blk)?);
        } else if n.ext_block != 0 {
            self.free_block(n.ext_block)?;
            n.ext_block = 0;
        }
        Ok(())
    }

    fn data_blocks(n: &Inode, exts: &[Extent]) -> u64 {
        exts.iter().map(|e| e.len as u64).sum::<u64>() + (n.ext_block != 0) as u64
    }

    // --- file data ----------------------------------------------------------

    fn read_data(
        &mut self,
        exts: &[Extent],
        size: u64,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize> {
        if offset >= size {
            return Ok(0);
        }
        let end = size.min(offset + buf.len() as u64);
        let mut pos = offset;
        let mut tmp = Vec::new();
        while pos < end {
            let lb = pos / BLOCK as u64;
            let in_block = (pos % BLOCK as u64) as usize;
            let e = exts.iter().find(|e| e.logical as u64 <= lb && lb < e.end());
            let (run, phys) = match e {
                Some(e) => {
                    let blocks_left = (end - lb * BLOCK as u64).div_ceil(BLOCK as u64);
                    (
                        (e.end() - lb).min(blocks_left).min(RUN_MAX),
                        Some(e.start + (lb - e.logical as u64)),
                    )
                }
                None => {
                    let next = exts
                        .iter()
                        .map(|e| e.logical as u64)
                        .filter(|&l| l > lb)
                        .min();
                    (next.map_or(u64::MAX, |n| n - lb), None)
                }
            };
            let span_end = end.min((lb + run.min(u64::MAX / BLOCK as u64 - lb)) * BLOCK as u64);
            let n = (span_end - pos) as usize;
            let out = &mut buf[(pos - offset) as usize..(pos - offset) as usize + n];
            match phys {
                Some(p) => {
                    tmp.resize(run as usize * BLOCK, 0);
                    self.dev_read(p, &mut tmp)?;
                    out.copy_from_slice(&tmp[in_block..in_block + n]);
                }
                None => out.fill(0),
            }
            pos = span_end;
        }
        Ok((end - offset) as usize)
    }

    fn write_data(&mut self, ino: Ino, n: &mut Inode, offset: u64, data: &[u8]) -> Result<()> {
        let end = offset + data.len() as u64;
        let first = offset / BLOCK as u64;
        let last = (end - 1) / BLOCK as u64;
        if last >= u32::MAX as u64 {
            return Err(FsError::TooBig);
        }
        let mut exts = self.extents(ino, n)?;
        // Allocate the holes in first..=last, keeping runs contiguous.
        let mut fresh = BTreeSet::new();
        let mut lb = first;
        while lb <= last {
            if let Some(e) = exts.iter().find(|e| e.logical as u64 <= lb && lb < e.end()) {
                lb = e.end();
                continue;
            }
            let mut goal = exts
                .iter()
                .filter(|e| e.end() <= lb)
                .max_by_key(|e| e.logical)
                .map_or(0, |e| e.start + e.len as u64 + (lb - e.end()));
            let hole_end = exts
                .iter()
                .map(|e| e.logical as u64)
                .filter(|&l| l > lb)
                .min()
                .unwrap_or(u64::MAX)
                .min(last + 1);
            while lb < hole_end {
                let p = self.alloc_block(goal)?;
                exts.push(Extent {
                    logical: lb as u32,
                    len: 1,
                    start: p,
                });
                fresh.insert(lb);
                goal = p + 1;
                lb += 1;
            }
            exts.sort_by_key(|e| e.logical);
            self.set_extents(ino, n, core::mem::take(&mut exts))?;
            exts = self.extents(ino, n)?;
        }
        // Write the data in runs of physically contiguous blocks.
        let mut run: Vec<u8> = Vec::new();
        let mut run_start = 0u64;
        let mut lb = first;
        while lb <= last {
            let e = exts
                .iter()
                .find(|e| e.logical as u64 <= lb && lb < e.end())
                .ok_or(FsError::Corrupt)?;
            let p = e.start + (lb - e.logical as u64);
            let bstart = lb * BLOCK as u64;
            let s = offset.max(bstart) - bstart;
            let t = end.min(bstart + BLOCK as u64) - bstart;
            let mut block = [0u8; BLOCK];
            if (s != 0 || t != BLOCK as u64) && !fresh.contains(&lb) {
                self.dev_read(p, &mut block)?;
            }
            let src = (bstart + s - offset) as usize;
            block[s as usize..t as usize].copy_from_slice(&data[src..src + (t - s) as usize]);
            if !run.is_empty()
                && (run_start + (run.len() / BLOCK) as u64 != p
                    || run.len() / BLOCK >= RUN_MAX as usize)
            {
                self.dev_write(run_start, &run)?;
                run.clear();
            }
            if run.is_empty() {
                run_start = p;
            }
            run.extend_from_slice(&block);
            lb += 1;
        }
        if !run.is_empty() {
            self.dev_write(run_start, &run)?;
        }
        self.data_written = true;
        Ok(())
    }

    /// Zeroes the bytes after the end of the file in its last block. Done
    /// before the file grows, so old bytes left there by an earlier
    /// truncate never become visible. (Zeroing at truncate time instead
    /// would change file data before the truncate commits.)
    fn zero_tail(&mut self, ino: Ino, n: &Inode) -> Result<()> {
        let tail = (n.size % BLOCK as u64) as usize;
        if tail == 0 {
            return Ok(());
        }
        let lb = n.size / BLOCK as u64;
        let exts = self.extents(ino, n)?;
        if let Some(e) = exts.iter().find(|e| e.logical as u64 <= lb && lb < e.end()) {
            let p = e.start + (lb - e.logical as u64);
            let mut block = [0u8; BLOCK];
            self.dev_read(p, &mut block)?;
            if block[tail..].iter().any(|&b| b != 0) {
                block[tail..].fill(0);
                self.dev_write(p, &block)?;
                self.data_written = true;
            }
        }
        Ok(())
    }

    fn truncate_inner(&mut self, ino: Ino, n: &mut Inode, size: u64) -> Result<()> {
        if size > n.size {
            self.zero_tail(ino, n)?;
        }
        if size < n.size {
            let keep = size.div_ceil(BLOCK as u64);
            let mut exts = self.extents(ino, n)?;
            for e in exts.iter_mut() {
                let cut_from = (e.logical as u64).max(keep);
                if cut_from < e.end() {
                    let first_freed = e.start + (cut_from - e.logical as u64);
                    for b in first_freed..e.start + e.len as u64 {
                        self.free_block(b)?;
                    }
                    e.len = (cut_from - e.logical as u64) as u32;
                }
            }
            self.set_extents(ino, n, exts)?;
        }
        n.size = size;
        Ok(())
    }

    // --- transactions -------------------------------------------------------

    /// Runs one change as a transaction: committed on success, discarded
    /// on error.
    fn txn<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.read_only {
            return Err(FsError::ReadOnly);
        }
        if self.failed {
            return Err(FsError::Io);
        }
        self.sb_saved = self.sb.clone();
        let r = f(self).and_then(|v| self.commit().map(|_| v));
        if r.is_err() && !self.failed {
            self.rollback();
        }
        r
    }

    fn rollback(&mut self) {
        for b in core::mem::take(&mut self.dirty) {
            self.cache.remove(&b);
        }
        self.sb = self.sb_saved.clone();
        self.data_written = false;
    }

    fn commit(&mut self) -> Result<()> {
        if self.dirty.is_empty() {
            if core::mem::take(&mut self.data_written) {
                self.dev.flush()?;
            }
            return Ok(());
        }
        // Every transaction also rewrites the superblock.
        if self.dirty.len() + 1 > self.max_txn {
            return Err(FsError::TooBig);
        }
        match self.commit_inner() {
            Ok(()) => Ok(()),
            Err(e) => {
                self.failed = true;
                Err(e)
            }
        }
    }

    fn commit_inner(&mut self) -> Result<()> {
        let seq = self.sb.journal_seq;
        self.sb.journal_seq = seq + 1;
        let mut sbb = zero_block();
        self.sb.encode(&mut sbb[..]);
        let homes: Vec<u64> = self.dirty.iter().copied().collect();
        let count = homes.len() + 1;

        // Ordered mode: file data reaches the disk before the metadata
        // that points at it is committed.
        if self.data_written {
            self.dev.flush()?;
        }

        // Descriptor, the block copies and the commit record, in one write.
        let mut j = vec![0u8; (count + 2) * BLOCK];
        {
            let (desc, rest) = j.split_at_mut(BLOCK);
            put32(desc, 0, JOURNAL_DESC_MAGIC);
            put32(desc, 4, count as u32);
            put64(desc, 8, seq);
            for (i, h) in homes.iter().chain(core::iter::once(&0)).enumerate() {
                put64(desc, JOURNAL_DESC_HEADER + i * 8, *h);
            }
            seal(desc);
            let (payload, commit) = rest.split_at_mut(count * BLOCK);
            for (i, h) in homes.iter().enumerate() {
                payload[i * BLOCK..(i + 1) * BLOCK].copy_from_slice(&self.cache[h][..]);
            }
            payload[(count - 1) * BLOCK..].copy_from_slice(&sbb[..]);
            put32(commit, 0, JOURNAL_COMMIT_MAGIC);
            put32(commit, 4, count as u32);
            put64(commit, 8, seq);
            put32(commit, 16, crc32c(payload));
            seal(commit);
        }
        let js = self.sb.journal_start;
        self.dev_write(js, &j)?;
        self.dev.flush()?;
        if self.stop_after_commit {
            self.failed = true;
            return Ok(());
        }

        // Checkpoint: metadata to its home blocks, the superblock last.
        let mut i = 0;
        while i < homes.len() {
            let mut k = i + 1;
            while k < homes.len() && homes[k] == homes[k - 1] + 1 && k - i < RUN_MAX as usize {
                k += 1;
            }
            let mut buf = Vec::with_capacity((k - i) * BLOCK);
            for h in &homes[i..k] {
                buf.extend_from_slice(&self.cache[h][..]);
            }
            self.dev_write(homes[i], &buf)?;
            i = k;
        }
        self.dev.flush()?;
        self.dev_write(0, &sbb[..])?;
        self.dev.flush()?;
        self.dirty.clear();
        self.data_written = false;
        Ok(())
    }

    fn stat_of(&mut self, ino: Ino, n: &Inode) -> Result<Stat> {
        let exts = self.extents(ino, n)?;
        Ok(Stat {
            ino,
            kind: if n.is_dir() { Kind::Dir } else { Kind::File },
            mode: n.mode & 0o7777,
            uid: n.uid,
            gid: n.gid,
            nlink: n.nlink,
            size: n.size,
            allocated: Self::data_blocks(n, &exts) * BLOCK as u64,
            mtime: n.mtime,
            ctime: n.ctime,
        })
    }

    fn dir_inode(&mut self, ino: Ino) -> Result<Inode> {
        let n = self.inode(ino)?;
        if !n.is_dir() {
            return Err(FsError::NotDir);
        }
        Ok(n)
    }

    fn touch_dir(&mut self, ino: Ino, nlink_delta: i32, now: u64) -> Result<()> {
        let mut d = self.inode(ino)?;
        d.nlink = d
            .nlink
            .checked_add_signed(nlink_delta)
            .ok_or(FsError::Corrupt)?;
        d.mtime = now;
        d.ctime = now;
        self.put_inode(ino, &d)
    }
}

/// Replays a committed journal transaction. Returns true if one was found.
fn replay<D: BlockIo>(dev: &mut D, spb: u64, sb: &Superblock) -> Result<bool> {
    let js = sb.journal_start;
    let mut desc = zero_block();
    dev.read(js * spb, &mut desc[..])?;
    if get32(&desc[..], 0) != JOURNAL_DESC_MAGIC
        || !sealed_ok(&desc[..])
        || get64(&desc[..], 8) != sb.journal_seq
    {
        return Ok(false);
    }
    let count = get32(&desc[..], 4) as usize;
    if count == 0 || count > JOURNAL_DESC_MAX || count as u64 + 2 > sb.journal_blocks {
        return Ok(false);
    }
    let mut rest = vec![0u8; (count + 1) * BLOCK];
    dev.read((js + 1) * spb, &mut rest)?;
    let (payload, commit) = rest.split_at(count * BLOCK);
    if get32(commit, 0) != JOURNAL_COMMIT_MAGIC
        || !sealed_ok(commit)
        || get64(commit, 8) != sb.journal_seq
        || get32(commit, 4) as usize != count
        || get32(commit, 16) != crc32c(payload)
    {
        return Ok(false);
    }
    let homes: Vec<u64> = (0..count)
        .map(|i| get64(&desc[..], JOURNAL_DESC_HEADER + i * 8))
        .collect();
    if homes[count - 1] != 0
        || homes[..count - 1]
            .iter()
            .any(|&h| h == 0 || h >= sb.total_blocks)
    {
        return Err(FsError::Corrupt);
    }
    for (i, h) in homes[..count - 1].iter().enumerate() {
        dev.write(h * spb, &payload[i * BLOCK..(i + 1) * BLOCK])?;
    }
    dev.flush()?;
    dev.write(0, &payload[(count - 1) * BLOCK..])?;
    dev.flush()?;
    Ok(true)
}

impl<D: BlockIo> FileSystem for VeloraFs<D> {
    fn fs_type(&self) -> &'static str {
        "velorafs"
    }

    fn root(&self) -> Ino {
        ROOT_INO
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn stat(&mut self, ino: Ino) -> Result<Stat> {
        let n = self.inode(ino)?;
        self.stat_of(ino, &n)
    }

    fn lookup(&mut self, dir: Ino, name: &str) -> Result<Ino> {
        let d = self.dir_inode(dir)?;
        self.dir_find(dir, &d, name)?
            .map(|f| f.ino)
            .ok_or(FsError::NotFound)
    }

    fn readdir(&mut self, dir: Ino, pos: u64) -> Result<Option<(DirEntry, u64)>> {
        let d = self.dir_inode(dir)?;
        self.dir_next(dir, &d, pos)
    }

    fn read(&mut self, ino: Ino, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let n = self.inode(ino)?;
        if n.is_dir() {
            return Err(FsError::IsDir);
        }
        let exts = self.extents(ino, &n)?;
        self.read_data(&exts, n.size, offset, buf)
    }

    fn write(&mut self, ino: Ino, offset: u64, data: &[u8], now: u64) -> Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        offset
            .checked_add(data.len() as u64)
            .ok_or(FsError::TooBig)?;
        self.txn(|fs| {
            let mut n = fs.inode(ino)?;
            if n.is_dir() {
                return Err(FsError::IsDir);
            }
            if offset > n.size {
                fs.zero_tail(ino, &n)?;
            }
            fs.write_data(ino, &mut n, offset, data)?;
            n.size = n.size.max(offset + data.len() as u64);
            n.mtime = now;
            n.ctime = now;
            fs.put_inode(ino, &n)?;
            Ok(data.len())
        })
    }

    fn truncate(&mut self, ino: Ino, size: u64, now: u64) -> Result<()> {
        if size / BLOCK as u64 >= u32::MAX as u64 {
            return Err(FsError::TooBig);
        }
        self.txn(|fs| {
            let mut n = fs.inode(ino)?;
            if n.is_dir() {
                return Err(FsError::IsDir);
            }
            fs.truncate_inner(ino, &mut n, size)?;
            n.mtime = now;
            n.ctime = now;
            fs.put_inode(ino, &n)
        })
    }

    fn create(&mut self, dir: Ino, name: &str, kind: Kind, attr: NewAttr, now: u64) -> Result<Ino> {
        path::check_name(name)?;
        let ty = match kind {
            Kind::File => S_IFREG,
            Kind::Dir => S_IFDIR,
            _ => return Err(FsError::Unsupported),
        };
        self.txn(|fs| {
            let d = fs.dir_inode(dir)?;
            if fs.dir_find(dir, &d, name)?.is_some() {
                return Err(FsError::Exists);
            }
            let ino = fs.alloc_inode()?;
            let n = Inode {
                mode: ty | (attr.mode & 0o7777),
                uid: attr.uid,
                gid: attr.gid,
                nlink: if kind == Kind::Dir { 2 } else { 1 },
                mtime: now,
                ctime: now,
                crtime: now,
                parent: if kind == Kind::Dir { dir } else { 0 },
                ..Default::default()
            };
            fs.put_inode(ino, &n)?;
            fs.dir_add(dir, name, ino, kind)?;
            fs.touch_dir(dir, if kind == Kind::Dir { 1 } else { 0 }, now)?;
            Ok(ino)
        })
    }

    fn unlink(&mut self, dir: Ino, name: &str, now: u64) -> Result<()> {
        self.txn(|fs| {
            let d = fs.dir_inode(dir)?;
            let f = fs.dir_find(dir, &d, name)?.ok_or(FsError::NotFound)?;
            let n = fs.inode(f.ino)?;
            if n.is_dir() && !fs.dir_is_empty(f.ino, &n)? {
                return Err(FsError::NotEmpty);
            }
            fs.dir_remove(dir, name)?;
            fs.release(f.ino, &n)?;
            fs.touch_dir(dir, if n.is_dir() { -1 } else { 0 }, now)
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
        path::check_name(to_name)?;
        self.txn(|fs| {
            let fd = fs.dir_inode(from)?;
            let src = fs
                .dir_find(from, &fd, from_name)?
                .ok_or(FsError::NotFound)?;
            let td = fs.dir_inode(to)?;
            let n = fs.inode(src.ino)?;
            let is_dir = n.is_dir();
            if is_dir {
                // The target directory must not be the source or below it.
                let mut p = to;
                loop {
                    if p == src.ino {
                        return Err(FsError::Invalid);
                    }
                    if p == ROOT_INO {
                        break;
                    }
                    p = fs.dir_inode(p)?.parent;
                }
            }
            let mut to_delta = if is_dir { 1 } else { 0 };
            if let Some(old) = fs.dir_find(to, &td, to_name)? {
                if old.ino == src.ino {
                    return Ok(());
                }
                let o = fs.inode(old.ino)?;
                match (is_dir, o.is_dir()) {
                    (true, false) => return Err(FsError::NotDir),
                    (false, true) => return Err(FsError::IsDir),
                    (true, true) if !fs.dir_is_empty(old.ino, &o)? => {
                        return Err(FsError::NotEmpty);
                    }
                    _ => {}
                }
                fs.dir_remove(to, to_name)?;
                fs.release(old.ino, &o)?;
                if o.is_dir() {
                    to_delta -= 1;
                }
            }
            fs.dir_remove(from, from_name)?;
            fs.dir_add(
                to,
                to_name,
                src.ino,
                if is_dir { Kind::Dir } else { Kind::File },
            )?;
            fs.touch_dir(from, if is_dir { -1 } else { 0 }, now)?;
            fs.touch_dir(to, to_delta, now)?;
            let mut n = fs.inode(src.ino)?;
            if is_dir {
                n.parent = to;
            }
            n.ctime = now;
            fs.put_inode(src.ino, &n)
        })
    }

    fn setattr(&mut self, ino: Ino, attr: SetAttr, now: u64) -> Result<()> {
        self.txn(|fs| {
            let mut n = fs.inode(ino)?;
            if let Some(m) = attr.mode {
                n.mode = (n.mode & S_IFMT) | (m & 0o7777);
            }
            if let Some(u) = attr.uid {
                n.uid = u;
            }
            if let Some(g) = attr.gid {
                n.gid = g;
            }
            n.ctime = now;
            fs.put_inode(ino, &n)
        })
    }

    fn sync(&mut self) -> Result<()> {
        if self.failed {
            return Err(FsError::Io);
        }
        if self.read_only {
            return Ok(());
        }
        self.dev.flush()
    }

    fn fsck(&mut self) -> Result<Vec<alloc::string::String>> {
        if self.failed {
            return Err(FsError::Io);
        }
        Ok(self.check().errors)
    }

    fn statfs(&mut self) -> Result<FsStat> {
        Ok(FsStat {
            block_size: BLOCK as u64,
            blocks: self.sb.total_blocks,
            free_blocks: self.sb.free_blocks,
            inodes: self.sb.inode_count - 1,
            free_inodes: self.sb.free_inodes,
        })
    }
}

#[cfg(test)]
mod tests;

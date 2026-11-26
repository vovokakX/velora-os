//! A disk in memory, for tests. It can model a write cache (writes are
//! durable only after `flush`) and a power cut after a number of writes.

use crate::{BlockIo, FsError, Result};
use alloc::vec;
use alloc::vec::Vec;

pub struct MemDisk {
    sector: usize,
    /// What is durably stored.
    data: Vec<u8>,
    /// Writes since the last flush (when the write cache is on).
    pending: Vec<(u64, Vec<u8>)>,
    write_cache: bool,
    /// Writes left before the power is cut; `None` = never.
    budget: Option<u64>,
    pub writes: u64,
    pub flushes: u64,
}

impl MemDisk {
    pub fn new(sector: usize, sectors: u64) -> Self {
        Self::from_image(sector, vec![0; sector * sectors as usize])
    }

    pub fn from_image(sector: usize, data: Vec<u8>) -> Self {
        assert!(data.len().is_multiple_of(sector));
        Self {
            sector,
            data,
            pending: Vec::new(),
            write_cache: false,
            budget: None,
            writes: 0,
            flushes: 0,
        }
    }

    /// With the cache on, writes reach `durable_image` only on `flush`.
    pub fn set_write_cache(&mut self, on: bool) {
        self.write_cache = on;
    }

    /// Cuts the power after `n` more writes: later writes and flushes are
    /// silently ignored, as if the machine had stopped.
    pub fn cut_power_after(&mut self, n: u64) {
        self.budget = Some(n);
    }

    pub fn powered(&self) -> bool {
        self.budget != Some(0)
    }

    /// The contents that survive a crash right now.
    pub fn durable_image(&self) -> Vec<u8> {
        self.data.clone()
    }

    /// The contents as a reader would see them (cache included).
    pub fn current_image(&self) -> Vec<u8> {
        let mut d = self.data.clone();
        for (lba, buf) in &self.pending {
            let off = *lba as usize * self.sector;
            d[off..off + buf.len()].copy_from_slice(buf);
        }
        d
    }

    fn range(&self, lba: u64, len: usize) -> Result<core::ops::Range<usize>> {
        if len == 0 || !len.is_multiple_of(self.sector) {
            return Err(FsError::Invalid);
        }
        let start = (lba as usize)
            .checked_mul(self.sector)
            .ok_or(FsError::Invalid)?;
        let end = start.checked_add(len).ok_or(FsError::Invalid)?;
        if end > self.data.len() {
            return Err(FsError::Invalid);
        }
        Ok(start..end)
    }
}

impl BlockIo for MemDisk {
    fn sector_size(&self) -> usize {
        self.sector
    }

    fn sector_count(&self) -> u64 {
        (self.data.len() / self.sector) as u64
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()> {
        let r = self.range(lba, buf.len())?;
        buf.copy_from_slice(&self.data[r]);
        // Later cached writes win.
        for (plba, pbuf) in &self.pending {
            let (ps, pe) = (*plba, *plba + (pbuf.len() / self.sector) as u64);
            let (rs, re) = (lba, lba + (buf.len() / self.sector) as u64);
            let (s, e) = (ps.max(rs), pe.min(re));
            if s < e {
                let n = (e - s) as usize * self.sector;
                let src = (s - ps) as usize * self.sector;
                let dst = (s - rs) as usize * self.sector;
                buf[dst..dst + n].copy_from_slice(&pbuf[src..src + n]);
            }
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()> {
        let r = self.range(lba, buf.len())?;
        match self.budget {
            Some(0) => return Ok(()),
            Some(ref mut n) => *n -= 1,
            None => {}
        }
        self.writes += 1;
        if self.write_cache {
            self.pending.push((lba, buf.to_vec()));
        } else {
            self.data[r].copy_from_slice(buf);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if !self.powered() {
            return Ok(());
        }
        self.flushes += 1;
        for (lba, buf) in core::mem::take(&mut self.pending) {
            let off = lba as usize * self.sector;
            self.data[off..off + buf.len()].copy_from_slice(&buf);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_and_power_cut() {
        let mut d = MemDisk::new(512, 8);
        d.set_write_cache(true);
        d.write(1, &[1u8; 512]).unwrap();
        let mut b = [0u8; 1024];
        d.read(0, &mut b).unwrap();
        assert_eq!((b[0], b[512]), (0, 1));
        assert_eq!(d.durable_image()[512], 0);
        d.flush().unwrap();
        assert_eq!(d.durable_image()[512], 1);
        d.cut_power_after(1);
        d.write(2, &[2u8; 512]).unwrap();
        d.write(3, &[3u8; 512]).unwrap();
        d.flush().unwrap();
        assert_eq!(d.durable_image()[1024], 0);
        assert_eq!(d.current_image()[1024], 2);
        assert_eq!(d.current_image()[1536], 0);
        assert!(d.write(8, &[0u8; 512]).is_err());
    }
}

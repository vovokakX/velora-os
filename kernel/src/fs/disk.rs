//! Block devices as `BlockIo` for the disk file systems.

use crate::drivers::block::BlockDevice;
use alloc::sync::Arc;
use velora_vfs::{BlockIo, FsError, Result};

pub struct DiskIo {
    dev: Arc<dyn BlockDevice>,
}

impl DiskIo {
    pub fn new(dev: Arc<dyn BlockDevice>) -> Self {
        Self { dev }
    }
}

impl BlockIo for DiskIo {
    fn sector_size(&self) -> usize {
        self.dev.sector_size()
    }

    fn sector_count(&self) -> u64 {
        self.dev.sector_count()
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()> {
        self.dev.read(lba, buf).map_err(|_| FsError::Io)
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()> {
        self.dev.write(lba, buf).map_err(|_| FsError::Io)
    }

    fn flush(&mut self) -> Result<()> {
        self.dev.flush().map_err(|_| FsError::Io)
    }
}

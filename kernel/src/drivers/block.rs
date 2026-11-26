//! Block devices: the interface every disk driver implements and the list
//! of disks found at boot. File systems (stage 7) use this interface.

use crate::sync::SpinLock;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// The request goes past the end of the device.
    OutOfRange,
    /// The buffer length is not a multiple of the sector size.
    BadLength,
    /// The device reported an error.
    Device,
    /// The device did not answer in time.
    Timeout,
    NoMemory,
}

pub trait BlockDevice: Send + Sync {
    /// Short name such as "vd0", "sd0", "nvme0n1".
    fn name(&self) -> &str;
    /// Driver and model, for listings.
    fn description(&self) -> String;
    fn sector_size(&self) -> usize;
    fn sector_count(&self) -> u64;
    /// Reads whole sectors starting at `lba` into `buf`.
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    /// Writes whole sectors starting at `lba` from `buf`.
    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;
    /// Makes previous writes durable.
    fn flush(&self) -> Result<(), BlockError>;

    fn size_bytes(&self) -> u64 {
        self.sector_count() * self.sector_size() as u64
    }
}

/// Checks a request against the device size; returns the sector count.
pub fn check_request(dev: &dyn BlockDevice, lba: u64, len: usize) -> Result<u64, BlockError> {
    let ss = dev.sector_size();
    if len == 0 || !len.is_multiple_of(ss) {
        return Err(BlockError::BadLength);
    }
    let count = (len / ss) as u64;
    match lba.checked_add(count) {
        Some(end) if end <= dev.sector_count() => Ok(count),
        _ => Err(BlockError::OutOfRange),
    }
}

static DEVICES: SpinLock<Vec<Arc<dyn BlockDevice>>> = SpinLock::new(Vec::new());

pub fn register(dev: Arc<dyn BlockDevice>) {
    DEVICES.lock().push(dev);
}

pub fn devices() -> Vec<Arc<dyn BlockDevice>> {
    DEVICES.lock().clone()
}

pub fn find(name: &str) -> Option<Arc<dyn BlockDevice>> {
    DEVICES.lock().iter().find(|d| d.name() == name).cloned()
}

/// Polls `done` until it returns true or about `timeout_ms` pass.
pub fn poll(timeout_ms: u64, mut done: impl FnMut() -> bool) -> Result<(), BlockError> {
    let deadline = crate::arch::cpu::rdtsc() + crate::time::tsc_hz() * timeout_ms / 1000;
    loop {
        if done() {
            return Ok(());
        }
        if crate::arch::cpu::rdtsc() > deadline {
            return Err(BlockError::Timeout);
        }
        core::hint::spin_loop();
    }
}

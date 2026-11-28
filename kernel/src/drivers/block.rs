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
    /// Model name for the installer ("" if the device has none).
    fn model(&self) -> String {
        String::new()
    }
    /// `velora_abi::disk_kind` value.
    fn kind(&self) -> u32 {
        velora_abi::disk_kind::OTHER
    }
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

    /// Unique GUID of a GPT partition (None for whole disks).
    fn partition_guid(&self) -> Option<[u8; 16]> {
        None
    }

    /// The whole disk this device is on (its own name for a disk).
    fn disk_name(&self) -> String {
        String::from(self.name())
    }

    /// Releases the driver's request lock so the panic log can be
    /// written even if the panic happened in the middle of a request.
    ///
    /// # Safety
    /// Only for the panic path, where the holder will never run again.
    unsafe fn force_unlock(&self) {}
}

/// A GPT partition: a window of its disk.
pub struct Partition {
    name: String,
    disk: Arc<dyn BlockDevice>,
    start: u64,
    count: u64,
    guid: [u8; 16],
    kind: String,
}

impl BlockDevice for Partition {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        alloc::format!("{} partition of {}", self.kind, self.disk.name())
    }

    fn model(&self) -> String {
        self.disk.model()
    }

    fn kind(&self) -> u32 {
        velora_abi::disk_kind::PARTITION
    }

    fn sector_size(&self) -> usize {
        self.disk.sector_size()
    }

    fn sector_count(&self) -> u64 {
        self.count
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        check_request(self, lba, buf.len())?;
        self.disk.read(self.start + lba, buf)
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        check_request(self, lba, buf.len())?;
        self.disk.write(self.start + lba, buf)
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.disk.flush()
    }

    fn partition_guid(&self) -> Option<[u8; 16]> {
        Some(self.guid)
    }

    fn disk_name(&self) -> String {
        String::from(self.disk.name())
    }

    unsafe fn force_unlock(&self) {
        // SAFETY: forwarded from the caller.
        unsafe { self.disk.force_unlock() }
    }
}

/// Reads the GPT of every disk and registers its partitions as devices
/// named `<disk>p<N>`. Returns the number of partitions found.
pub fn scan_partitions() -> usize {
    let mut found = 0;
    for disk in devices() {
        if disk.partition_guid().is_some() {
            continue;
        }
        let mut io = crate::fs::DiskIo::new(disk.clone());
        let Ok(Some(parts)) = velora_gpt::read(&mut io) else {
            continue;
        };
        for (i, p) in parts.iter().enumerate() {
            if p.last_lba >= disk.sector_count() || p.first_lba > p.last_lba {
                crate::kwarn!("{}: partition {} lies outside the disk", disk.name(), i + 1);
                continue;
            }
            let kind = match p.type_guid {
                velora_gpt::Guid::ESP => "EFI system",
                velora_gpt::Guid::VELORA_ROOT => "Velora root",
                _ => "data",
            };
            register(Arc::new(Partition {
                name: alloc::format!("{}p{}", disk.name(), i + 1),
                disk: disk.clone(),
                start: p.first_lba,
                count: p.sectors(),
                guid: p.unique_guid.0,
                kind: String::from(kind),
            }));
            found += 1;
        }
    }
    found
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

/// The disk the kernel was loaded from (a USB drive with the Live system,
/// or the disk of an installed system); None after a boot from a CD.
pub fn boot_disk() -> Option<String> {
    let guid = crate::boot::info().boot_partition?;
    DEVICES
        .lock()
        .iter()
        .find(|d| d.partition_guid() == Some(guid))
        .map(|d| d.disk_name())
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

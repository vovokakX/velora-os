//! virtio-blk over modern virtio-pci (virtio 1.x). Legacy (pre-1.0)
//! devices are not supported.
//!
//! One request is in flight at a time and completion is polled. The data
//! goes through a 64 KiB bounce buffer.

use super::block::{self, BlockDevice, BlockError};
use super::dma::DmaBuffer;
use super::pci;
use crate::mm::mmio;
use crate::sync::SpinLock;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{Ordering, fence};
use velora_pci::{Address, ConfigAccess, Device};

const VENDOR: u16 = 0x1af4;
/// Modern virtio-blk; 0x1001 is the transitional id.
const DEVICE_IDS: [u16; 2] = [0x1042, 0x1001];

const CAP_VENDOR: u8 = 0x09;
const CFG_COMMON: u8 = 1;
const CFG_NOTIFY: u8 = 2;
const CFG_DEVICE: u8 = 4;

// Common configuration registers.
const DEVICE_FEATURE_SELECT: u64 = 0x00;
const DEVICE_FEATURE: u64 = 0x04;
const DRIVER_FEATURE_SELECT: u64 = 0x08;
const DRIVER_FEATURE: u64 = 0x0c;
const DEVICE_STATUS: u64 = 0x14;
const QUEUE_SELECT: u64 = 0x16;
const QUEUE_SIZE: u64 = 0x18;
const QUEUE_ENABLE: u64 = 0x1c;
const QUEUE_NOTIFY_OFF: u64 = 0x1e;
const QUEUE_DESC: u64 = 0x20;
const QUEUE_DRIVER: u64 = 0x28;
const QUEUE_DEVICE: u64 = 0x30;

const STATUS_ACK: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;

const F_VERSION_1: u64 = 1 << 32;
const F_BLK_RO: u64 = 1 << 5;
const F_BLK_FLUSH: u64 = 1 << 9;

const REQ_IN: u32 = 0;
const REQ_OUT: u32 = 1;
const REQ_FLUSH: u32 = 4;

const DESC_NEXT: u16 = 1;
const DESC_WRITE: u16 = 2;

const QUEUE_LEN: u16 = 16;
const BOUNCE: usize = 64 * 1024;
const SECTOR: usize = 512;

// Layout of the queue buffer.
const DESC_OFF: usize = 0;
const AVAIL_OFF: usize = 1024;
const USED_OFF: usize = 2048;
// Layout of the request buffer.
const HDR_OFF: usize = 0;
const STATUS_OFF: usize = 16;
const DATA_OFF: usize = 4096;

struct Queue {
    ring: DmaBuffer,
    req: DmaBuffer,
    avail_idx: u16,
    used_idx: u16,
}

pub struct VirtioBlk {
    name: String,
    common: u64,
    notify: u64,
    sectors: u64,
    read_only: bool,
    flush_supported: bool,
    queue: SpinLock<Queue>,
}

fn find_caps(cfg: &pci::Config, d: &Device) -> Option<(u64, u64, u32, u64)> {
    let mut common = None;
    let mut notify = None;
    let mut device = None;
    for (id, off) in velora_pci::capabilities(cfg, d.addr) {
        if id != CAP_VENDOR {
            continue;
        }
        let off = off as u16;
        let cfg_type = cfg.read8(d.addr, off + 3);
        let bar = cfg.read8(d.addr, off + 4) as usize;
        let offset = cfg.read32(d.addr, off + 8) as u64;
        let length = cfg.read32(d.addr, off + 12) as u64;
        let Some((base, size)) = d.bars.get(bar).and_then(|b| b.memory()) else {
            continue;
        };
        if offset + length > size {
            continue;
        }
        let va = mmio::map(base + offset, length)?;
        match cfg_type {
            CFG_COMMON => common = Some(va),
            CFG_NOTIFY => notify = Some((va, cfg.read32(d.addr, off + 16))),
            CFG_DEVICE => device = Some(va),
            _ => {}
        }
    }
    let (notify, mult) = notify?;
    Some((common?, notify, mult, device?))
}

impl VirtioBlk {
    fn probe(addr: Address, d: &Device, index: usize) -> Result<Self, &'static str> {
        let cfg = pci::config();
        velora_pci::enable_device(cfg, addr);
        // Experiment for issue #19: keep INTx enabled for virtio.
        let cmd = cfg.read16(addr, velora_pci::COMMAND);
        cfg.write16(addr, velora_pci::COMMAND, cmd & !velora_pci::CMD_INTX_DISABLE);
        let (common, notify_base, mult, devcfg) =
            find_caps(cfg, d).ok_or("no modern virtio capabilities")?;
        // SAFETY (all register accesses below): the regions were mapped
        // from the device's BARs as described by its capabilities.
        unsafe {
            mmio::write8(common + DEVICE_STATUS, 0);
            block::poll(100, || mmio::read8(common + DEVICE_STATUS) == 0)
                .map_err(|_| "reset timed out")?;
            mmio::write8(common + DEVICE_STATUS, STATUS_ACK);
            mmio::write8(common + DEVICE_STATUS, STATUS_ACK | STATUS_DRIVER);
            mmio::write32(common + DEVICE_FEATURE_SELECT, 0);
            let lo = mmio::read32(common + DEVICE_FEATURE) as u64;
            mmio::write32(common + DEVICE_FEATURE_SELECT, 1);
            let hi = mmio::read32(common + DEVICE_FEATURE) as u64;
            let offered = hi << 32 | lo;
            if offered & F_VERSION_1 == 0 {
                return Err("device does not offer VIRTIO_F_VERSION_1");
            }
            let wanted = offered & (F_VERSION_1 | F_BLK_FLUSH | F_BLK_RO);
            mmio::write32(common + DRIVER_FEATURE_SELECT, 0);
            mmio::write32(common + DRIVER_FEATURE, wanted as u32);
            mmio::write32(common + DRIVER_FEATURE_SELECT, 1);
            mmio::write32(common + DRIVER_FEATURE, (wanted >> 32) as u32);
            mmio::write8(
                common + DEVICE_STATUS,
                STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK,
            );
            if mmio::read8(common + DEVICE_STATUS) & STATUS_FEATURES_OK == 0 {
                return Err("device refused the feature set");
            }

            mmio::write16(common + QUEUE_SELECT, 0);
            let max = mmio::read16(common + QUEUE_SIZE);
            if max == 0 {
                return Err("request queue missing");
            }
            let len = max.min(QUEUE_LEN);
            mmio::write16(common + QUEUE_SIZE, len);
            let ring = DmaBuffer::new(4096).ok_or("out of memory")?;
            let req = DmaBuffer::new(DATA_OFF + BOUNCE).ok_or("out of memory")?;
            mmio::write64(common + QUEUE_DESC, ring.phys() + DESC_OFF as u64);
            mmio::write64(common + QUEUE_DRIVER, ring.phys() + AVAIL_OFF as u64);
            mmio::write64(common + QUEUE_DEVICE, ring.phys() + USED_OFF as u64);
            let notify_off = mmio::read16(common + QUEUE_NOTIFY_OFF) as u64;
            mmio::write16(common + QUEUE_ENABLE, 1);
            mmio::write8(
                common + DEVICE_STATUS,
                STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
            );
            let sectors = mmio::read32(devcfg) as u64 | (mmio::read32(devcfg + 4) as u64) << 32;
            Ok(Self {
                name: format!("vd{}", index),
                common,
                notify: notify_base + notify_off * mult as u64,
                sectors,
                read_only: wanted & F_BLK_RO != 0,
                flush_supported: wanted & F_BLK_FLUSH != 0,
                queue: SpinLock::new(Queue {
                    ring,
                    req,
                    avail_idx: 0,
                    used_idx: 0,
                }),
            })
        }
    }

    /// Runs one request: header, optional data, status.
    fn request(
        &self,
        kind: u32,
        lba: u64,
        data_len: usize,
        device_writes_data: bool,
    ) -> Result<(), BlockError> {
        let mut q = self.queue.lock();
        let q = &mut *q;
        let req_phys = q.req.phys();
        q.req.write::<u32>(HDR_OFF, kind);
        q.req.write::<u32>(HDR_OFF + 4, 0);
        q.req.write::<u64>(HDR_OFF + 8, lba);
        q.req.write::<u8>(STATUS_OFF, 0xff);

        let desc = |ring: &mut DmaBuffer, i: usize, addr: u64, len: u32, flags: u16, next: u16| {
            let o = DESC_OFF + i * 16;
            ring.write::<u64>(o, addr);
            ring.write::<u32>(o + 8, len);
            ring.write::<u16>(o + 12, flags);
            ring.write::<u16>(o + 14, next);
        };
        desc(&mut q.ring, 0, req_phys + HDR_OFF as u64, 16, DESC_NEXT, 1);
        if data_len > 0 {
            let flags = DESC_NEXT | if device_writes_data { DESC_WRITE } else { 0 };
            desc(
                &mut q.ring,
                1,
                req_phys + DATA_OFF as u64,
                data_len as u32,
                flags,
                2,
            );
            desc(
                &mut q.ring,
                2,
                req_phys + STATUS_OFF as u64,
                1,
                DESC_WRITE,
                0,
            );
        } else {
            desc(&mut q.ring, 0, req_phys + HDR_OFF as u64, 16, DESC_NEXT, 2);
            desc(
                &mut q.ring,
                2,
                req_phys + STATUS_OFF as u64,
                1,
                DESC_WRITE,
                0,
            );
        }
        let len = QUEUE_LEN as usize;
        let slot = q.avail_idx as usize % len;
        q.ring.write::<u16>(AVAIL_OFF + 4 + slot * 2, 0);
        fence(Ordering::SeqCst);
        q.avail_idx = q.avail_idx.wrapping_add(1);
        q.ring.write::<u16>(AVAIL_OFF + 2, q.avail_idx);
        fence(Ordering::SeqCst);
        // SAFETY: the notify register of this queue.
        unsafe { mmio::write16(self.notify, 0) };

        let want = q.used_idx.wrapping_add(1);
        let ring = &q.ring;
        // Diagnostic for a KVM-only hang: notify again every ~100 ms.
        let mut kicks = 0u32;
        let mut last = crate::arch::cpu::rdtsc();
        let tick = crate::time::tsc_hz() / 10;
        let notify = self.notify;
        let r = block::poll(5000, || {
            if ring.read::<u16>(USED_OFF + 2) == want {
                return true;
            }
            let now = crate::arch::cpu::rdtsc();
            if now - last > tick {
                last = now;
                kicks += 1;
                // SAFETY: the notify register of this queue.
                unsafe { mmio::write16(notify, 0) };
            }
            false
        });
        if kicks > 0 {
            crate::kwarn!(
                "{}: completion needed {} extra notifications",
                self.name,
                kicks
            );
        }
        if let Err(e) = r {
            // SAFETY: reads of this device's common configuration.
            let (status, enabled) = unsafe {
                mmio::write16(self.common + QUEUE_SELECT, 0);
                (
                    mmio::read8(self.common + DEVICE_STATUS),
                    mmio::read16(self.common + QUEUE_ENABLE),
                )
            };
            crate::kwarn!(
                "{}: request timed out (device status {:#x}, queue enabled {}, avail {}, used {}, used flags {:#x}, notify {:#x})",
                self.name,
                status,
                enabled,
                q.ring.read::<u16>(AVAIL_OFF + 2),
                q.ring.read::<u16>(USED_OFF + 2),
                q.ring.read::<u16>(USED_OFF),
                self.notify
            );
            return Err(e);
        }
        q.used_idx = want;
        fence(Ordering::SeqCst);
        match q.req.read::<u8>(STATUS_OFF) {
            0 => Ok(()),
            _ => Err(BlockError::Device),
        }
    }

    fn data(&self) -> *mut u8 {
        (self.queue.lock().req.virt() + DATA_OFF as u64) as *mut u8
    }
}

impl BlockDevice for VirtioBlk {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        format!(
            "virtio-blk{}",
            if self.read_only { ", read-only" } else { "" }
        )
    }

    fn sector_size(&self) -> usize {
        SECTOR
    }

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        block::check_request(self, lba, buf.len())?;
        for (i, chunk) in buf.chunks_mut(BOUNCE).enumerate() {
            let at = lba + (i * BOUNCE / SECTOR) as u64;
            self.request(REQ_IN, at, chunk.len(), true)?;
            // SAFETY: the bounce buffer holds `chunk.len()` bytes from the device.
            unsafe { core::ptr::copy_nonoverlapping(self.data(), chunk.as_mut_ptr(), chunk.len()) };
        }
        Ok(())
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        if self.read_only {
            return Err(BlockError::Device);
        }
        block::check_request(self, lba, buf.len())?;
        for (i, chunk) in buf.chunks(BOUNCE).enumerate() {
            let at = lba + (i * BOUNCE / SECTOR) as u64;
            // SAFETY: the bounce buffer is BOUNCE bytes long.
            unsafe { core::ptr::copy_nonoverlapping(chunk.as_ptr(), self.data(), chunk.len()) };
            self.request(REQ_OUT, at, chunk.len(), false)?;
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), BlockError> {
        if self.flush_supported {
            self.request(REQ_FLUSH, 0, 0, false)
        } else {
            Ok(())
        }
    }
}

impl Drop for VirtioBlk {
    fn drop(&mut self) {
        // SAFETY: resetting the device stops it from using our buffers.
        unsafe { mmio::write8(self.common + DEVICE_STATUS, 0) };
    }
}

/// Finds and starts every virtio-blk device.
pub fn probe_all() -> usize {
    let mut n = 0;
    for d in pci::devices()
        .iter()
        .filter(|d| d.vendor == VENDOR && DEVICE_IDS.contains(&d.device))
    {
        match VirtioBlk::probe(d.addr, d, n) {
            Ok(dev) => {
                block::register(Arc::new(dev));
                n += 1;
            }
            Err(e) => crate::kwarn!("virtio-blk {}: {}", d.addr, e),
        }
    }
    n
}

//! virtio-net over modern virtio-pci (virtio 1.x): one receive and one
//! transmit queue, 64 buffers of 2 KiB each, no offloads. Received frames
//! are taken by polling the used ring; the MSI-X interrupt only wakes the
//! network thread.

use super::block;
use super::dma::DmaBuffer;
use super::msix::MsiX;
use super::pci;
use crate::arch::x86_64::{idt::InterruptFrame, irq};
use crate::mm::mmio;
use alloc::vec::Vec;
use core::sync::atomic::{Ordering, fence};
use velora_pci::{ConfigAccess, Device};

const VENDOR: u16 = 0x1af4;
const DEVICE_IDS: [u16; 2] = [0x1041, 0x1000];

const CAP_VENDOR: u8 = 0x09;
const CFG_COMMON: u8 = 1;
const CFG_NOTIFY: u8 = 2;
const CFG_DEVICE: u8 = 4;

const DEVICE_FEATURE_SELECT: u64 = 0x00;
const DEVICE_FEATURE: u64 = 0x04;
const DRIVER_FEATURE_SELECT: u64 = 0x08;
const DRIVER_FEATURE: u64 = 0x0c;
const MSIX_CONFIG: u64 = 0x10;
const DEVICE_STATUS: u64 = 0x14;
const QUEUE_SELECT: u64 = 0x16;
const QUEUE_SIZE: u64 = 0x18;
const QUEUE_MSIX_VECTOR: u64 = 0x1a;
const QUEUE_ENABLE: u64 = 0x1c;
const QUEUE_NOTIFY_OFF: u64 = 0x1e;
const QUEUE_DESC: u64 = 0x20;
const QUEUE_DRIVER: u64 = 0x28;
const QUEUE_DEVICE: u64 = 0x30;
const NO_VECTOR: u16 = 0xffff;

const STATUS_ACK: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;

const F_MAC: u64 = 1 << 5;
const F_VERSION_1: u64 = 1 << 32;

const DESC_WRITE: u16 = 2;

/// `struct virtio_net_hdr_v1` (with num_buffers).
const NET_HDR: usize = 12;
const QLEN: u16 = 64;
const BUF: usize = 2048;
const DESC_OFF: usize = 0;
const AVAIL_OFF: usize = 1024;
const USED_OFF: usize = 2048;

/// Vector of the queue interrupts (after the disk vectors).
const VECTOR_OFFSET: u8 = 12;

struct Queue {
    ring: DmaBuffer,
    bufs: DmaBuffer,
    len: u16,
    avail_idx: u16,
    used_idx: u16,
    notify: u64,
    /// Transmit only: descriptors not in use.
    free: Vec<u16>,
}

impl Queue {
    fn new(len: u16) -> Option<Self> {
        Some(Self {
            ring: DmaBuffer::new(4096)?,
            bufs: DmaBuffer::new(len as usize * BUF)?,
            len,
            avail_idx: 0,
            used_idx: 0,
            notify: 0,
            free: (0..len).collect(),
        })
    }

    fn set_desc(&mut self, i: u16, len: u32, flags: u16) {
        let o = DESC_OFF + i as usize * 16;
        let addr = self.bufs.phys() + i as u64 * BUF as u64;
        self.ring.write::<u64>(o, addr);
        self.ring.write::<u32>(o + 8, len);
        self.ring.write::<u16>(o + 12, flags);
        self.ring.write::<u16>(o + 14, 0);
    }

    fn push_avail(&mut self, i: u16) {
        let slot = self.avail_idx % self.len;
        self.ring.write::<u16>(AVAIL_OFF + 4 + slot as usize * 2, i);
        fence(Ordering::SeqCst);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        self.ring.write::<u16>(AVAIL_OFF + 2, self.avail_idx);
        fence(Ordering::SeqCst);
    }

    /// Next used element: (descriptor, length).
    fn pop_used(&mut self) -> Option<(u16, u32)> {
        fence(Ordering::SeqCst);
        if self.ring.read::<u16>(USED_OFF + 2) == self.used_idx {
            return None;
        }
        let slot = self.used_idx % self.len;
        let o = USED_OFF + 4 + slot as usize * 8;
        let id = self.ring.read::<u32>(o) as u16;
        let len = self.ring.read::<u32>(o + 4);
        self.used_idx = self.used_idx.wrapping_add(1);
        Some((id, len))
    }
}

pub struct VirtioNet {
    common: u64,
    mac: [u8; 6],
    rx: Queue,
    tx: Queue,
    _msix: Option<MsiX>,
}

fn on_interrupt(_: &mut InterruptFrame) {
    crate::net::wake();
}

impl VirtioNet {
    fn probe(d: &Device) -> Result<Self, &'static str> {
        let cfg = pci::config();
        velora_pci::enable_device(cfg, d.addr);
        let msix = match MsiX::enable(d) {
            Ok(m) => {
                let vector = irq::VECTOR_DEVICE_BASE + VECTOR_OFFSET;
                irq::set_handler(vector, on_interrupt);
                m.route(0, vector).ok().map(|_| m)
            }
            Err(_) => None,
        };
        let mut common = None;
        let mut notify = None;
        let mut device = None;
        for (id, off) in velora_pci::capabilities(cfg, d.addr) {
            if id != CAP_VENDOR {
                continue;
            }
            let off = off as u16;
            let kind = cfg.read8(d.addr, off + 3);
            let bar = cfg.read8(d.addr, off + 4) as usize;
            let offset = cfg.read32(d.addr, off + 8) as u64;
            let length = cfg.read32(d.addr, off + 12) as u64;
            let Some((base, size)) = d.bars.get(bar).and_then(|b| b.memory()) else {
                continue;
            };
            if offset + length > size {
                continue;
            }
            let Some(va) = mmio::map(base + offset, length) else {
                continue;
            };
            match kind {
                CFG_COMMON => common = Some(va),
                CFG_NOTIFY => notify = Some((va, cfg.read32(d.addr, off + 16))),
                CFG_DEVICE => device = Some(va),
                _ => {}
            }
        }
        let (Some(common), Some((notify_base, mult)), Some(devcfg)) = (common, notify, device)
        else {
            return Err("no modern virtio capabilities");
        };
        // SAFETY (all register accesses below): regions mapped from the
        // device's BARs as its capabilities describe.
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
            let wanted = offered & (F_VERSION_1 | F_MAC);
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
            mmio::write16(common + MSIX_CONFIG, NO_VECTOR);
            let mut queues = Vec::new();
            for q in 0..2u16 {
                mmio::write16(common + QUEUE_SELECT, q);
                let max = mmio::read16(common + QUEUE_SIZE);
                if max == 0 {
                    return Err("queue missing");
                }
                let len = max.min(QLEN);
                mmio::write16(common + QUEUE_SIZE, len);
                let mut queue = Queue::new(len).ok_or("out of memory")?;
                mmio::write64(common + QUEUE_DESC, queue.ring.phys() + DESC_OFF as u64);
                mmio::write64(common + QUEUE_DRIVER, queue.ring.phys() + AVAIL_OFF as u64);
                mmio::write64(common + QUEUE_DEVICE, queue.ring.phys() + USED_OFF as u64);
                let noff = mmio::read16(common + QUEUE_NOTIFY_OFF) as u64;
                queue.notify = notify_base + noff * mult as u64;
                if msix.is_some() {
                    mmio::write16(common + QUEUE_MSIX_VECTOR, 0);
                }
                mmio::write16(common + QUEUE_ENABLE, 1);
                queues.push(queue);
            }
            let tx = queues.pop().unwrap();
            let mut rx = queues.pop().unwrap();
            let mut mac = [0u8; 6];
            if wanted & F_MAC != 0 {
                for (i, b) in mac.iter_mut().enumerate() {
                    *b = mmio::read8(devcfg + i as u64);
                }
            } else {
                mac = [0x02, 0x56, 0x45, 0x4c, 0x00, 0x01]; // locally administered
            }
            // Give the device every receive buffer.
            for i in 0..rx.len {
                rx.set_desc(i, BUF as u32, DESC_WRITE);
                rx.push_avail(i);
            }
            rx.free.clear();
            mmio::write8(
                common + DEVICE_STATUS,
                STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
            );
            mmio::write16(rx.notify, 0);
            Ok(Self {
                common,
                mac,
                rx,
                tx,
                _msix: msix,
            })
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    /// Queues one Ethernet frame. Returns false if the frame is too large
    /// or every transmit buffer is busy.
    pub fn send(&mut self, frame: &[u8]) -> bool {
        // Reclaim finished transmissions.
        while let Some((id, _)) = self.tx.pop_used() {
            self.tx.free.push(id);
        }
        if frame.len() + NET_HDR > BUF {
            return false;
        }
        let Some(i) = self.tx.free.pop() else {
            return false;
        };
        let off = i as usize * BUF;
        let b = self.tx.bufs.as_mut_slice();
        b[off..off + NET_HDR].fill(0);
        b[off + NET_HDR..off + NET_HDR + frame.len()].copy_from_slice(frame);
        self.tx.set_desc(i, (NET_HDR + frame.len()) as u32, 0);
        self.tx.push_avail(i);
        // SAFETY: the transmit queue's notify register.
        unsafe { mmio::write16(self.tx.notify, 1) };
        true
    }

    /// The next received frame, if one is waiting.
    pub fn receive(&mut self) -> Option<Vec<u8>> {
        let (id, len) = self.rx.pop_used()?;
        let off = id as usize * BUF;
        let len = (len as usize).min(BUF);
        let frame = if len > NET_HDR {
            self.rx.bufs.as_slice()[off + NET_HDR..off + len].to_vec()
        } else {
            Vec::new()
        };
        self.rx.set_desc(id, BUF as u32, DESC_WRITE);
        self.rx.push_avail(id);
        // SAFETY: the receive queue's notify register.
        unsafe { mmio::write16(self.rx.notify, 0) };
        Some(frame)
    }
}

impl Drop for VirtioNet {
    fn drop(&mut self) {
        // SAFETY: resetting the device stops it from using our buffers.
        unsafe { mmio::write8(self.common + DEVICE_STATUS, 0) };
    }
}

/// Starts the first virtio-net device, if there is one.
pub fn probe() -> Option<VirtioNet> {
    let devs = pci::devices();
    let d = devs
        .iter()
        .find(|d| d.vendor == VENDOR && DEVICE_IDS.contains(&d.device))?;
    match VirtioNet::probe(d) {
        Ok(n) => Some(n),
        Err(e) => {
            crate::kwarn!("virtio-net {}: {}", d.addr, e);
            None
        }
    }
}

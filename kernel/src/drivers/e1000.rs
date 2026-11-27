//! Intel 8254x/8257x/I21x gigabit Ethernet ("e1000" and "e1000e"), with
//! legacy descriptors and polling (the `net` thread looks every tick).
//!
//! Tested in QEMU with the 82540EM (`-device e1000`) and the 82574L
//! (`-device e1000e`). The I217/I218/I219 controllers built into many
//! Intel chipsets use the same descriptors and are accepted, but were not
//! tested on real hardware (some may need PHY set-up this driver does not
//! do).

use super::NetDevice;
use super::dma::DmaBuffer;
use super::pci;
use crate::mm::mmio;
use crate::time;
use alloc::string::String;
use alloc::vec::Vec;

const VENDOR_INTEL: u16 = 0x8086;
/// (device id, name)
const DEVICES: &[(u16, &str)] = &[
    (0x100e, "82540EM"),
    (0x100f, "82545EM"),
    (0x10d3, "82574L"),
    (0x153a, "I217-LM"),
    (0x153b, "I217-V"),
    (0x155a, "I218-LM"),
    (0x1559, "I218-V"),
    (0x15a0, "I218-LM"),
    (0x15a1, "I218-V"),
    (0x15a2, "I218-LM"),
    (0x15a3, "I218-V"),
    (0x156f, "I219-LM"),
    (0x1570, "I219-V"),
    (0x15b7, "I219-LM"),
    (0x15b8, "I219-V"),
    (0x15b9, "I219-LM"),
    (0x15bb, "I219-LM"),
    (0x15bc, "I219-V"),
    (0x15bd, "I219-LM"),
    (0x15be, "I219-V"),
    (0x15d6, "I219-V"),
    (0x15d7, "I219-LM"),
    (0x15d8, "I219-V"),
    (0x15e3, "I219-LM"),
    (0x0d4c, "I219-LM"),
    (0x0d4d, "I219-V"),
    (0x0d4e, "I219-LM"),
    (0x0d4f, "I219-V"),
    (0x15fb, "I219-LM"),
    (0x15fc, "I219-V"),
    (0x1a1e, "I219-LM"),
    (0x1a1f, "I219-V"),
];

const CTRL: u64 = 0x0000;
const STATUS: u64 = 0x0008;
const EERD: u64 = 0x0014;
const ICR: u64 = 0x00c0;
const IMC: u64 = 0x00d8;
const RCTL: u64 = 0x0100;
const TCTL: u64 = 0x0400;
const TIPG: u64 = 0x0410;
const RDBAL: u64 = 0x2800;
const RDBAH: u64 = 0x2804;
const RDLEN: u64 = 0x2808;
const RDH: u64 = 0x2810;
const RDT: u64 = 0x2818;
const TDBAL: u64 = 0x3800;
const TDBAH: u64 = 0x3804;
const TDLEN: u64 = 0x3808;
const TDH: u64 = 0x3810;
const TDT: u64 = 0x3818;
const MTA: u64 = 0x5200;
const RAL0: u64 = 0x5400;
const RAH0: u64 = 0x5404;

const CTRL_SLU: u32 = 1 << 6;
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_RST: u32 = 1 << 26;
const STATUS_LU: u32 = 1 << 1;
const RCTL_EN: u32 = 1 << 1;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_SECRC: u32 = 1 << 26;
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;
const RAH_VALID: u32 = 1 << 31;
const DESC_DD: u8 = 1;
const DESC_EOP: u8 = 1 << 1;
const CMD_EOP: u8 = 1;
const CMD_IFCS: u8 = 1 << 1;
const CMD_RS: u8 = 1 << 3;

const RING: usize = 32;
const BUF: usize = 2048;

pub struct E1000 {
    regs: u64,
    name: String,
    mac: [u8; 6],
    rx_ring: DmaBuffer,
    rx_bufs: DmaBuffer,
    rx_next: usize,
    tx_ring: DmaBuffer,
    tx_bufs: DmaBuffer,
    tx_next: usize,
}

// SAFETY: the registers and buffers belong to this driver only.
unsafe impl Send for E1000 {}

impl E1000 {
    fn r(&self, reg: u64) -> u32 {
        // SAFETY: a register of the mapped BAR0.
        unsafe { mmio::read32(self.regs + reg) }
    }

    fn w(&self, reg: u64, v: u32) {
        // SAFETY: as above.
        unsafe { mmio::write32(self.regs + reg, v) }
    }

    /// Reads a 16-bit EEPROM word (two register layouts exist).
    fn eeprom(&self, word: u32) -> Option<u16> {
        for (start, done, shift) in [(1u32, 1u32 << 4, 8u32), (1, 1 << 1, 2)] {
            self.w(EERD, start | word << shift);
            for _ in 0..1000 {
                let v = self.r(EERD);
                if v & done != 0 {
                    return Some((v >> 16) as u16);
                }
                time::delay_us(5);
            }
        }
        None
    }

    fn read_mac(&self) -> Option<[u8; 6]> {
        let lo = self.r(RAL0);
        let hi = self.r(RAH0);
        if hi & RAH_VALID != 0 && (lo != 0 || hi & 0xffff != 0) {
            let l = lo.to_le_bytes();
            let h = hi.to_le_bytes();
            return Some([l[0], l[1], l[2], l[3], h[0], h[1]]);
        }
        let mut m = [0u8; 6];
        for i in 0..3 {
            let w = self.eeprom(i)?;
            m[i as usize * 2] = w as u8;
            m[i as usize * 2 + 1] = (w >> 8) as u8;
        }
        Some(m)
    }

    fn start(d: &velora_pci::Device, model: &str) -> Result<E1000, &'static str> {
        let cfg = pci::config();
        velora_pci::enable_device(cfg, d.addr);
        let (bar, size) = d.bars[0].memory().ok_or("BAR0 missing")?;
        let regs = mmio::map(bar, size.clamp(0x20000, 0x40000)).ok_or("cannot map registers")?;
        let mut n = E1000 {
            regs,
            name: String::from(model),
            mac: [0; 6],
            rx_ring: DmaBuffer::new(RING * 16).ok_or("out of memory")?,
            rx_bufs: DmaBuffer::new(RING * BUF).ok_or("out of memory")?,
            rx_next: 0,
            tx_ring: DmaBuffer::new(RING * 16).ok_or("out of memory")?,
            tx_bufs: DmaBuffer::new(RING * BUF).ok_or("out of memory")?,
            tx_next: 0,
        };
        // Reset, no interrupts (polled), link up.
        n.w(IMC, u32::MAX);
        n.w(CTRL, n.r(CTRL) | CTRL_RST);
        time::delay_us(10_000);
        for _ in 0..100 {
            if n.r(CTRL) & CTRL_RST == 0 {
                break;
            }
            time::delay_us(1000);
        }
        n.w(IMC, u32::MAX);
        let _ = n.r(ICR);
        n.w(CTRL, (n.r(CTRL) | CTRL_SLU | CTRL_ASDE) & !CTRL_RST);
        n.mac = n.read_mac().ok_or("no MAC address")?;
        let m = n.mac;
        n.w(RAL0, u32::from_le_bytes([m[0], m[1], m[2], m[3]]));
        n.w(RAH0, u32::from_le_bytes([m[4], m[5], 0, 0]) | RAH_VALID);
        for i in 0..128 {
            n.w(MTA + i * 4, 0);
        }

        // Receive ring: every descriptor belongs to the card.
        for i in 0..RING {
            let p = n.rx_bufs.phys() + (i * BUF) as u64;
            n.rx_ring.write(i * 16, p);
            n.rx_ring.write(i * 16 + 8, 0u64);
        }
        let rx = n.rx_ring.phys();
        n.w(RDBAL, rx as u32);
        n.w(RDBAH, (rx >> 32) as u32);
        n.w(RDLEN, (RING * 16) as u32);
        n.w(RDH, 0);
        n.w(RDT, (RING - 1) as u32);
        n.w(RCTL, RCTL_EN | RCTL_BAM | RCTL_SECRC);

        // Transmit ring: all descriptors start free (done).
        for i in 0..RING {
            let p = n.tx_bufs.phys() + (i * BUF) as u64;
            n.tx_ring.write(i * 16, p);
            n.tx_ring.write(i * 16 + 8, 0u32);
            n.tx_ring.write(i * 16 + 12, DESC_DD as u32);
        }
        let tx = n.tx_ring.phys();
        n.w(TDBAL, tx as u32);
        n.w(TDBAH, (tx >> 32) as u32);
        n.w(TDLEN, (RING * 16) as u32);
        n.w(TDH, 0);
        n.w(TDT, 0);
        n.w(TIPG, 0x0060_200a);
        n.w(TCTL, TCTL_EN | TCTL_PSP | 0x10 << 4 | 0x40 << 12);
        Ok(n)
    }

    pub fn link_up(&self) -> bool {
        self.r(STATUS) & STATUS_LU != 0
    }
}

impl NetDevice for E1000 {
    fn name(&self) -> &str {
        &self.name
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        let i = self.tx_next;
        let status: u8 = self.tx_ring.read(i * 16 + 12);
        if status & DESC_DD == 0 || frame.len() > BUF {
            return false; // ring full (still being sent) or too large
        }
        self.tx_bufs.as_mut_slice()[i * BUF..i * BUF + frame.len()].copy_from_slice(frame);
        self.tx_ring.write(i * 16 + 8, frame.len() as u16);
        self.tx_ring.write(i * 16 + 11, CMD_EOP | CMD_IFCS | CMD_RS);
        self.tx_ring.write(i * 16 + 12, 0u8);
        self.tx_next = (i + 1) % RING;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.w(TDT, self.tx_next as u32);
        true
    }

    fn receive(&mut self) -> Option<Vec<u8>> {
        loop {
            let i = self.rx_next;
            let status: u8 = self.rx_ring.read(i * 16 + 12);
            if status & DESC_DD == 0 {
                return None;
            }
            let len = (self.rx_ring.read::<u16>(i * 16 + 8) as usize).min(BUF);
            let errors: u8 = self.rx_ring.read(i * 16 + 13);
            let frame = (status & DESC_EOP != 0 && errors == 0)
                .then(|| self.rx_bufs.as_slice()[i * BUF..i * BUF + len].to_vec());
            self.rx_ring.write(i * 16 + 8, 0u64);
            self.rx_next = (i + 1) % RING;
            // Give the descriptor back to the card.
            self.w(RDT, i as u32);
            if let Some(f) = frame {
                return Some(f);
            }
        }
    }
}

/// Starts the first supported Intel Ethernet controller.
pub fn probe() -> Option<E1000> {
    for d in pci::devices().iter().filter(|d| d.vendor == VENDOR_INTEL) {
        let Some(&(_, model)) = DEVICES.iter().find(|(id, _)| *id == d.device) else {
            continue;
        };
        match E1000::start(d, model) {
            Ok(n) => return Some(n),
            Err(e) => crate::kwarn!("e1000 {} at {}: {}", model, d.addr, e),
        }
    }
    None
}

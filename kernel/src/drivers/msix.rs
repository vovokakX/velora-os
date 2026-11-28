//! MSI-X and MSI: message-signalled interrupts for PCI devices.
//!
//! Each MSI-X table entry (or the one MSI message) holds the address/data
//! pair the device writes to raise an interrupt. Velora sends each vector
//! to the local APIC of the CPU that sets it up (the boot CPU).

use super::pci;
use crate::arch::x86_64::apic;
use crate::mm::mmio;
use velora_pci::{ConfigAccess, Device};

const CTRL_ENABLE: u16 = 1 << 15;
const CTRL_FUNCTION_MASK: u16 = 1 << 14;

pub struct MsiX {
    table: u64,
    entries: u16,
}

impl MsiX {
    /// Finds the MSI-X capability, maps its table, masks every entry and
    /// enables MSI-X (which also turns off INTx for the function).
    pub fn enable(d: &Device) -> Result<Self, &'static str> {
        let cfg = pci::config();
        let cap = velora_pci::capabilities(cfg, d.addr)
            .into_iter()
            .find(|&(id, _)| id == velora_pci::CAP_MSIX)
            .ok_or("no MSI-X capability")?
            .1 as u16;
        let ctrl = cfg.read16(d.addr, cap + 2);
        let entries = (ctrl & 0x7ff) + 1;
        let table_reg = cfg.read32(d.addr, cap + 4);
        let bir = (table_reg & 7) as usize;
        let offset = (table_reg & !7) as u64;
        let (base, size) = d
            .bars
            .get(bir)
            .and_then(|b| b.memory())
            .ok_or("MSI-X table BAR missing")?;
        let bytes = entries as u64 * 16;
        if offset + bytes > size {
            return Err("MSI-X table outside its BAR");
        }
        let table = mmio::map(base + offset, bytes).ok_or("cannot map the MSI-X table")?;
        // Mask the whole function while the table is programmed.
        cfg.write16(d.addr, cap + 2, ctrl | CTRL_ENABLE | CTRL_FUNCTION_MASK);
        let m = Self { table, entries };
        for i in 0..entries {
            m.mask(i);
        }
        cfg.write16(d.addr, cap + 2, (ctrl | CTRL_ENABLE) & !CTRL_FUNCTION_MASK);
        Ok(m)
    }

    pub fn entries(&self) -> u16 {
        self.entries
    }

    fn mask(&self, entry: u16) {
        // SAFETY: entry < entries, inside the mapped table.
        unsafe { mmio::write32(self.table + entry as u64 * 16 + 12, 1) }
    }

    /// Routes table entry `entry` to `vector` on this CPU and unmasks it.
    pub fn route(&self, entry: u16, vector: u8) -> Result<(), &'static str> {
        if entry >= self.entries {
            return Err("MSI-X entry out of range");
        }
        let e = self.table + entry as u64 * 16;
        let (address, data) = velora_pci::msi_message(apic::id(), vector);
        // SAFETY: inside the mapped table. Address: fixed delivery to the
        // local APIC of this CPU; data: the vector, edge triggered.
        unsafe {
            mmio::write32(e + 12, 1);
            mmio::write32(e, address as u32);
            mmio::write32(e + 4, (address >> 32) as u32);
            mmio::write32(e + 8, data);
            mmio::write32(e + 12, 0);
        }
        Ok(())
    }
}

/// Enables MSI (one message) with `vector` on this CPU; false when the
/// function has no MSI capability.
pub fn enable_msi(d: &Device, vector: u8) -> bool {
    let (address, data) = velora_pci::msi_message(apic::id(), vector);
    velora_pci::enable_msi(pci::config(), d.addr, address, data)
}

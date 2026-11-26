//! PCI configuration access for the kernel (ECAM from the ACPI MCFG table,
//! or the legacy 0xCF8/0xCFC ports) and the list of devices found at boot.

use crate::arch::x86_64::port::outl;
use crate::mm::mmio;
use crate::sync::SpinLock;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use velora_acpi::EcamRegion;
use velora_pci::{Address, ConfigAccess, Device};

/// Configuration access through ECAM when available, otherwise I/O ports.
pub struct Config {
    ecam: Option<EcamRegion>,
    /// Mapped 4 KiB config pages, one per function.
    pages: SpinLock<BTreeMap<Address, u64>>,
}

impl Config {
    fn page(&self, region: &EcamRegion, addr: Address) -> Option<u64> {
        let mut pages = self.pages.lock();
        if let Some(&va) = pages.get(&addr) {
            return Some(va);
        }
        let phys = region.address(addr.bus, addr.device, addr.function)?;
        let va = mmio::map(phys, 4096)?;
        pages.insert(addr, va);
        Some(va)
    }

    fn port_address(addr: Address, offset: u16) -> u32 {
        0x8000_0000
            | (addr.bus as u32) << 16
            | (addr.device as u32) << 11
            | (addr.function as u32) << 8
            | (offset as u32 & 0xfc)
    }
}

impl ConfigAccess for Config {
    fn read32(&self, addr: Address, offset: u16) -> u32 {
        match &self.ecam {
            Some(r) => match self.page(r, addr) {
                // SAFETY: the page maps this function's config space.
                Some(va) => unsafe { mmio::read32(va + offset as u64) },
                None => 0xffff_ffff,
            },
            None => {
                if offset >= 256 {
                    return 0xffff_ffff;
                }
                // SAFETY: standard configuration mechanism #1.
                unsafe {
                    outl(0xcf8, Self::port_address(addr, offset));
                    port_inl(0xcfc)
                }
            }
        }
    }

    fn write32(&self, addr: Address, offset: u16, value: u32) {
        match &self.ecam {
            Some(r) => {
                if let Some(va) = self.page(r, addr) {
                    // SAFETY: the page maps this function's config space.
                    unsafe { mmio::write32(va + offset as u64, value) }
                }
            }
            None => {
                if offset < 256 {
                    // SAFETY: standard configuration mechanism #1.
                    unsafe {
                        outl(0xcf8, Self::port_address(addr, offset));
                        outl(0xcfc, value);
                    }
                }
            }
        }
    }
}

/// # Safety
/// Reading an I/O port can have side effects.
unsafe fn port_inl(port: u16) -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("in eax, dx", out("eax") v, in("dx") port, options(nomem, nostack, preserves_flags))
    }
    v
}

static CONFIG: SpinLock<Option<&'static Config>> = SpinLock::new(None);
static DEVICES: SpinLock<Vec<Device>> = SpinLock::new(Vec::new());

pub fn config() -> &'static Config {
    CONFIG.lock().expect("PCI not initialised")
}

/// Enumerates the bus. Returns (number of functions, uses ECAM).
pub fn init(mcfg: &[EcamRegion]) -> (usize, bool) {
    // Only segment 0 is supported (one PCI segment covers normal PCs).
    let ecam = mcfg.iter().copied().find(|r| r.segment == 0);
    let cfg: &'static Config = alloc::boxed::Box::leak(alloc::boxed::Box::new(Config {
        ecam,
        pages: SpinLock::new(BTreeMap::new()),
    }));
    *CONFIG.lock() = Some(cfg);
    let devices = velora_pci::enumerate(cfg);
    let n = devices.len();
    *DEVICES.lock() = devices;
    (n, ecam.is_some())
}

pub fn devices() -> Vec<Device> {
    DEVICES.lock().clone()
}

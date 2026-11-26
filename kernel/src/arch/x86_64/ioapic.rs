//! I/O APICs: route device interrupts (global system interrupts) to
//! vectors on a CPU.

use crate::mm::mmio;
use crate::sync::SpinLock;
use alloc::vec::Vec;

const REG_VERSION: u32 = 0x01;
const REG_REDIRECTION: u32 = 0x10;

const MASKED: u64 = 1 << 16;
const ACTIVE_LOW: u64 = 1 << 13;
const LEVEL: u64 = 1 << 15;

#[derive(Clone, Copy, Debug)]
struct Chip {
    base: u64,
    gsi_base: u32,
    entries: u32,
}

impl Chip {
    fn read(&self, reg: u32) -> u32 {
        // SAFETY: IOREGSEL at +0, IOWIN at +0x10 of the mapped register page.
        unsafe {
            mmio::write32(self.base, reg);
            mmio::read32(self.base + 0x10)
        }
    }

    fn write(&self, reg: u32, value: u32) {
        // SAFETY: see `read`.
        unsafe {
            mmio::write32(self.base, reg);
            mmio::write32(self.base + 0x10, value);
        }
    }

    fn read_entry(&self, index: u32) -> u64 {
        let lo = self.read(REG_REDIRECTION + index * 2) as u64;
        let hi = self.read(REG_REDIRECTION + index * 2 + 1) as u64;
        hi << 32 | lo
    }

    fn write_entry(&self, index: u32, value: u64) {
        // Mask first so the entry is never live in a half-written state.
        self.write(REG_REDIRECTION + index * 2, MASKED as u32);
        self.write(REG_REDIRECTION + index * 2 + 1, (value >> 32) as u32);
        self.write(REG_REDIRECTION + index * 2, value as u32);
    }
}

static CHIPS: SpinLock<Vec<Chip>> = SpinLock::new(Vec::new());

/// Maps every I/O APIC from the MADT and masks all of their inputs.
pub fn init(madt: &velora_acpi::Madt) -> Result<usize, &'static str> {
    let mut chips = CHIPS.lock();
    for io in &madt.ioapics {
        let base = mmio::map(io.address as u64, 0x20).ok_or("cannot map an I/O APIC")?;
        let mut chip = Chip {
            base,
            gsi_base: io.gsi_base,
            entries: 0,
        };
        chip.entries = ((chip.read(REG_VERSION) >> 16) & 0xff) + 1;
        for i in 0..chip.entries {
            chip.write_entry(i, MASKED);
        }
        chips.push(chip);
    }
    if chips.is_empty() {
        return Err("no I/O APIC");
    }
    Ok(chips.len())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub vector: u8,
    pub active_low: bool,
    pub level: bool,
    pub dest_apic: u8,
}

fn with_chip<R>(gsi: u32, f: impl FnOnce(&Chip, u32) -> R) -> Option<R> {
    let chips = CHIPS.lock();
    let chip = chips
        .iter()
        .find(|c| gsi >= c.gsi_base && gsi < c.gsi_base + c.entries)?;
    Some(f(chip, gsi - chip.gsi_base))
}

/// Routes `gsi` to `route.vector` (fixed delivery, physical destination)
/// and unmasks it.
pub fn route(gsi: u32, route: Route) -> Result<(), &'static str> {
    let mut e = route.vector as u64;
    if route.active_low {
        e |= ACTIVE_LOW;
    }
    if route.level {
        e |= LEVEL;
    }
    e |= (route.dest_apic as u64) << 56;
    with_chip(gsi, |c, i| c.write_entry(i, e)).ok_or("no I/O APIC serves this GSI")
}

/// Masks `gsi` again.
pub fn mask(gsi: u32) {
    with_chip(gsi, |c, i| c.write_entry(i, c.read_entry(i) | MASKED));
}

/// Current routing of `gsi`: (vector, masked).
pub fn entry(gsi: u32) -> Option<(u8, bool)> {
    with_chip(gsi, |c, i| {
        let e = c.read_entry(i);
        (e as u8, e & MASKED != 0)
    })
}

/// Routes an ISA IRQ, applying the MADT override and bus defaults
/// (ISA: active high, edge triggered).
pub fn route_isa(
    madt: &velora_acpi::Madt,
    irq: u8,
    vector: u8,
    dest_apic: u8,
) -> Result<u32, &'static str> {
    use velora_acpi::{Polarity, Trigger};
    let (gsi, polarity, trigger) = madt.isa_irq(irq);
    route(
        gsi,
        Route {
            vector,
            active_low: polarity == Polarity::ActiveLow,
            level: trigger == Trigger::Level,
            dest_apic,
        },
    )?;
    Ok(gsi)
}

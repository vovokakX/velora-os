//! High Precision Event Timer: its main counter is a reference clock for
//! calibrating the TSC and the local APIC timer. No HPET interrupts are
//! used.

use crate::mm::mmio;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const REG_CAPS: u64 = 0x00;
const REG_CONFIG: u64 = 0x10;
const REG_COUNTER: u64 = 0xf0;
const CONFIG_ENABLE: u64 = 1;
const CAPS_64BIT: u64 = 1 << 13;
/// The specification allows periods up to 100 ns (in femtoseconds).
const MAX_PERIOD_FS: u64 = 100_000_000;

static BASE: AtomicU64 = AtomicU64::new(0);
static HZ: AtomicU64 = AtomicU64::new(0);
static WIDE: AtomicBool = AtomicBool::new(false);

/// Maps the timer block at `phys` and starts the main counter.
/// Returns the counter frequency in Hz.
pub fn init(phys: u64) -> Result<u64, &'static str> {
    let va = mmio::map(phys, 0x400).ok_or("cannot map the HPET registers")?;
    // SAFETY: the HPET register block was just mapped.
    let caps = unsafe { mmio::read64(va + REG_CAPS) };
    let period = caps >> 32;
    if period == 0 || period > MAX_PERIOD_FS {
        return Err("HPET reports an invalid counter period");
    }
    // SAFETY: as above; setting ENABLE starts the main counter. The
    // legacy replacement bit is left as the firmware set it.
    unsafe {
        let cfg = mmio::read64(va + REG_CONFIG);
        mmio::write64(va + REG_CONFIG, cfg | CONFIG_ENABLE);
    }
    let hz = 1_000_000_000_000_000 / period;
    BASE.store(va, Ordering::SeqCst);
    HZ.store(hz, Ordering::SeqCst);
    WIDE.store(caps & CAPS_64BIT != 0, Ordering::SeqCst);
    Ok(hz)
}

pub fn available() -> bool {
    BASE.load(Ordering::Relaxed) != 0
}

pub fn hz() -> u64 {
    HZ.load(Ordering::Relaxed)
}

/// The main counter (0 without an HPET).
pub fn counter() -> u64 {
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 {
        return 0;
    }
    // SAFETY: mapped in `init`.
    let v = unsafe { mmio::read64(base + REG_COUNTER) };
    if WIDE.load(Ordering::Relaxed) {
        v
    } else {
        v & 0xffff_ffff
    }
}

/// Ticks between two counter values, allowing for one wrap-around.
pub fn elapsed(from: u64, to: u64) -> u64 {
    if WIDE.load(Ordering::Relaxed) {
        to.wrapping_sub(from)
    } else {
        to.wrapping_sub(from) & 0xffff_ffff
    }
}

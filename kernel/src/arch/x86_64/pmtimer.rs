//! ACPI power management timer: a 24- or 32-bit counter at 3.579545 MHz
//! in an I/O port, present on every ACPI PC. Used as a reference clock
//! when there is no HPET.

use super::port::inl;
use core::sync::atomic::{AtomicU32, Ordering};

pub use velora_acpi::PM_TIMER_HZ as HZ;

static PORT: AtomicU32 = AtomicU32::new(0);
static MASK: AtomicU32 = AtomicU32::new(0);

/// Uses the timer at `port` (from the FADT); `wide` = 32-bit counter.
pub fn init(port: u32, wide: bool) -> Result<(), &'static str> {
    if port == 0 || port > 0xffff {
        return Err("no ACPI PM timer");
    }
    PORT.store(port, Ordering::SeqCst);
    MASK.store(if wide { u32::MAX } else { 0x00ff_ffff }, Ordering::SeqCst);
    Ok(())
}

pub fn available() -> bool {
    PORT.load(Ordering::Relaxed) != 0
}

pub fn counter() -> u32 {
    let port = PORT.load(Ordering::Relaxed);
    if port == 0 {
        return 0;
    }
    // SAFETY: the port comes from the FADT; reading it has no side effects.
    unsafe { inl(port as u16) & MASK.load(Ordering::Relaxed) }
}

/// Ticks between two counter values, allowing for one wrap-around.
pub fn elapsed(from: u32, to: u32) -> u32 {
    to.wrapping_sub(from) & MASK.load(Ordering::Relaxed)
}

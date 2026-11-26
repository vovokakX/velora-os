//! 8254 PIT, used only as a reference clock for calibrating the TSC and
//! the local APIC timer. Channel 2 is used so no interrupt is involved.

use super::port::{inb, outb};

pub const FREQUENCY: u64 = 1_193_182;

/// Starts a one-shot countdown of `ms` milliseconds (max 54) on channel 2.
/// Returns a handle that reports when it has expired.
pub fn start_oneshot(ms: u64) -> Oneshot {
    let count = (FREQUENCY * ms / 1000).min(0xffff) as u16;
    // SAFETY: programs PIT channel 2 and the speaker gate (speaker stays off).
    unsafe {
        let gate = inb(0x61) & !0x02; // speaker data off
        outb(0x61, gate & !0x01); // gate low
        outb(0x43, 0b1011_0000); // channel 2, lo/hi byte, mode 0
        outb(0x42, count as u8);
        outb(0x42, (count >> 8) as u8);
        outb(0x61, gate | 0x01); // gate high: counting starts
    }
    Oneshot
}

pub struct Oneshot;

impl Oneshot {
    /// The channel 2 output goes high when the count reaches zero.
    pub fn expired(&self) -> bool {
        // SAFETY: reading port 0x61 has no side effects.
        unsafe { inb(0x61) & 0x20 != 0 }
    }
}

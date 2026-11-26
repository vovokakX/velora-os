//! CMOS real-time clock (read-only). The time is read once at boot; the
//! TSC keeps it running afterwards (`time::realtime_ns`).

use crate::arch::x86_64::port::{inb, outb};
use velora_rtc::{DateTime, RtcRegisters};

fn cmos(reg: u8) -> u8 {
    // SAFETY: CMOS index/data ports. Bit 7 of the index (NMI disable) is
    // left clear.
    unsafe {
        outb(0x70, reg & 0x7f);
        inb(0x71)
    }
}

fn update_in_progress() -> bool {
    cmos(0x0a) & 0x80 != 0
}

fn raw(century_register: u8) -> RtcRegisters {
    RtcRegisters {
        second: cmos(0x00),
        minute: cmos(0x02),
        hour: cmos(0x04),
        day: cmos(0x07),
        month: cmos(0x08),
        year: cmos(0x09),
        century: if century_register != 0 {
            cmos(century_register)
        } else {
            0
        },
        status_b: cmos(0x0b),
    }
}

/// Reads the clock, retrying until two reads agree (the RTC may be in
/// the middle of an update). `century_register` comes from the FADT.
pub fn read(century_register: u8) -> Option<DateTime> {
    for _ in 0..1000 {
        while update_in_progress() {
            core::hint::spin_loop();
        }
        let a = raw(century_register);
        let b = raw(century_register);
        if a == b {
            let t = a.decode();
            return t.is_valid().then_some(t);
        }
    }
    None
}

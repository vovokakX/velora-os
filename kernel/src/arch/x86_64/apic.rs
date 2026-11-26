//! Local APIC: interrupt delivery to this CPU, EOI and the APIC timer.
//! Works in xAPIC (MMIO) and x2APIC (MSR) mode, whichever the firmware
//! left enabled.

use super::cpu;
use super::pit;
use crate::mm::mmio;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MSR_APIC_BASE: u32 = 0x1b;
const APIC_BASE_ENABLE: u64 = 1 << 11;
const APIC_BASE_X2APIC: u64 = 1 << 10;

const REG_ID: u32 = 0x20;
const REG_VERSION: u32 = 0x30;
const REG_TPR: u32 = 0x80;
const REG_EOI: u32 = 0xb0;
const REG_SVR: u32 = 0xf0;
const REG_ESR: u32 = 0x280;
const REG_LVT_TIMER: u32 = 0x320;
const REG_LVT_LINT0: u32 = 0x350;
const REG_LVT_LINT1: u32 = 0x360;
const REG_LVT_ERROR: u32 = 0x370;
const REG_TIMER_INITIAL: u32 = 0x380;
const REG_TIMER_CURRENT: u32 = 0x390;
const REG_TIMER_DIVIDE: u32 = 0x3e0;

const LVT_MASKED: u32 = 1 << 16;
const LVT_PERIODIC: u32 = 1 << 17;
const LVT_NMI: u32 = 0b100 << 8;
const LVT_ACTIVE_LOW: u32 = 1 << 13;
const LVT_LEVEL: u32 = 1 << 15;
const SVR_ENABLE: u32 = 1 << 8;
const TIMER_DIVIDE_16: u32 = 0b0011;

pub const VECTOR_TIMER: u8 = 0x20;
pub const VECTOR_ERROR: u8 = 0xfe;
pub const VECTOR_SPURIOUS: u8 = 0xff;

static BASE: AtomicU64 = AtomicU64::new(0);
static X2APIC: AtomicBool = AtomicBool::new(false);
/// APIC timer ticks (after the divider) per millisecond.
static TIMER_PER_MS: AtomicU64 = AtomicU64::new(0);

fn read(reg: u32) -> u32 {
    if X2APIC.load(Ordering::Relaxed) {
        // SAFETY: x2APIC registers are MSRs 0x800 + reg/16.
        unsafe { cpu::rdmsr(0x800 + (reg >> 4)) as u32 }
    } else {
        // SAFETY: BASE is the mapped xAPIC register page.
        unsafe { mmio::read32(BASE.load(Ordering::Relaxed) + reg as u64) }
    }
}

fn write(reg: u32, value: u32) {
    if X2APIC.load(Ordering::Relaxed) {
        // SAFETY: see `read`.
        unsafe { cpu::wrmsr(0x800 + (reg >> 4), value as u64) }
    } else {
        // SAFETY: see `read`.
        unsafe { mmio::write32(BASE.load(Ordering::Relaxed) + reg as u64, value) }
    }
}

/// NMI wiring of a LINT pin, from the MADT.
pub struct LintNmi {
    pub lint: u8,
    pub active_low: bool,
    pub level: bool,
}

/// Enables the local APIC of this CPU.
pub fn init(madt_address: u64, nmis: &[LintNmi]) -> Result<(), &'static str> {
    // SAFETY: IA32_APIC_BASE exists on every x86_64 CPU with an APIC.
    let msr = unsafe { cpu::rdmsr(MSR_APIC_BASE) };
    if msr & APIC_BASE_X2APIC != 0 {
        X2APIC.store(true, Ordering::SeqCst);
    } else {
        let phys = if madt_address != 0 {
            madt_address
        } else {
            msr & 0x000f_ffff_ffff_f000
        };
        let va = mmio::map(phys, 0x1000).ok_or("cannot map the local APIC")?;
        BASE.store(va, Ordering::SeqCst);
    }
    if msr & APIC_BASE_ENABLE == 0 {
        // SAFETY: setting the global enable bit of our own APIC.
        unsafe { cpu::wrmsr(MSR_APIC_BASE, msr | APIC_BASE_ENABLE) };
    }
    write(REG_TPR, 0);
    write(REG_LVT_LINT0, LVT_MASKED);
    write(REG_LVT_LINT1, LVT_MASKED);
    for n in nmis {
        let mut v = LVT_NMI;
        if n.active_low {
            v |= LVT_ACTIVE_LOW;
        }
        if n.level {
            v |= LVT_LEVEL;
        }
        match n.lint {
            0 => write(REG_LVT_LINT0, v),
            1 => write(REG_LVT_LINT1, v),
            _ => {}
        }
    }
    write(REG_LVT_ERROR, VECTOR_ERROR as u32);
    write(REG_ESR, 0);
    write(REG_ESR, 0);
    write(REG_LVT_TIMER, LVT_MASKED);
    write(REG_SVR, SVR_ENABLE | VECTOR_SPURIOUS as u32);
    eoi();
    Ok(())
}

#[inline]
pub fn eoi() {
    write(REG_EOI, 0);
}

pub fn id() -> u32 {
    if X2APIC.load(Ordering::Relaxed) {
        read(REG_ID)
    } else {
        read(REG_ID) >> 24
    }
}

pub fn version() -> u32 {
    read(REG_VERSION) & 0xff
}

pub fn is_x2apic() -> bool {
    X2APIC.load(Ordering::Relaxed)
}

pub fn software_enabled() -> bool {
    read(REG_SVR) & SVR_ENABLE != 0
}

/// Result of measuring the TSC and the APIC timer against the PIT.
pub struct Calibration {
    pub tsc_hz: u64,
    pub timer_per_ms: u64,
}

/// Measures the TSC and APIC timer frequencies over 50 ms of PIT time.
/// Interrupts must be disabled.
pub fn calibrate() -> Result<Calibration, &'static str> {
    const MS: u64 = 50;
    write(REG_TIMER_DIVIDE, TIMER_DIVIDE_16);
    write(REG_LVT_TIMER, LVT_MASKED);
    let oneshot = pit::start_oneshot(MS);
    write(REG_TIMER_INITIAL, u32::MAX);
    let tsc0 = cpu::rdtsc();
    let mut spins = 0u64;
    while !oneshot.expired() {
        spins += 1;
        if spins > 200_000_000 {
            write(REG_TIMER_INITIAL, 0);
            return Err("PIT did not count (no legacy timer?)");
        }
        core::hint::spin_loop();
    }
    let tsc1 = cpu::rdtsc();
    let elapsed = u32::MAX - read(REG_TIMER_CURRENT);
    write(REG_TIMER_INITIAL, 0);
    let c = Calibration {
        tsc_hz: (tsc1 - tsc0) * 1000 / MS,
        timer_per_ms: elapsed as u64 / MS,
    };
    if c.timer_per_ms == 0 || c.tsc_hz < 1_000_000 {
        return Err("calibration gave implausible values");
    }
    TIMER_PER_MS.store(c.timer_per_ms, Ordering::SeqCst);
    Ok(c)
}

/// Starts the periodic timer interrupt at `hz` on `VECTOR_TIMER`.
pub fn start_periodic(hz: u64) {
    let per_ms = TIMER_PER_MS.load(Ordering::SeqCst);
    let count = (per_ms * 1000 / hz).clamp(1, u32::MAX as u64) as u32;
    write(REG_TIMER_DIVIDE, TIMER_DIVIDE_16);
    write(REG_LVT_TIMER, VECTOR_TIMER as u32 | LVT_PERIODIC);
    write(REG_TIMER_INITIAL, count);
}

pub fn timer_per_ms() -> u64 {
    TIMER_PER_MS.load(Ordering::Relaxed)
}

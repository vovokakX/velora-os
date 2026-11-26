//! Kernel time: a calibrated TSC for precise time since boot and a
//! periodic APIC timer tick for sleeping (and, in stage 4, scheduling).
//!
//! Wall-clock time (RTC) comes with the drivers in stage 6.

use crate::arch::cpu;
use crate::arch::x86_64::{apic, idt::InterruptFrame, irq};
use core::sync::atomic::{AtomicU64, Ordering};

/// Timer interrupt rate. 100 Hz keeps idle wakeups low.
pub const TICK_HZ: u64 = 100;

static BOOT_TSC: AtomicU64 = AtomicU64::new(0);
static TSC_HZ: AtomicU64 = AtomicU64::new(0);
static TICKS: AtomicU64 = AtomicU64::new(0);
/// UNIX time (ns) that corresponds to uptime 0; 0 = unknown.
static WALL_AT_BOOT_NS: AtomicU64 = AtomicU64::new(0);

/// Records the TSC at kernel entry, before anything else runs.
pub fn mark_boot() {
    BOOT_TSC.store(cpu::rdtsc(), Ordering::Relaxed);
}

pub fn boot_tsc() -> u64 {
    BOOT_TSC.load(Ordering::Relaxed)
}

fn on_tick(_frame: &mut InterruptFrame) {
    let now = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    crate::sched::on_tick(now);
}

/// Calibrates against the PIT and starts the timer interrupt.
/// Returns (TSC Hz, APIC timer ticks per ms).
pub fn init() -> Result<(u64, u64), &'static str> {
    let c = apic::calibrate()?;
    TSC_HZ.store(c.tsc_hz, Ordering::SeqCst);
    irq::set_handler(apic::VECTOR_TIMER, on_tick);
    apic::start_periodic(TICK_HZ);
    Ok((c.tsc_hz, c.timer_per_ms))
}

pub fn tsc_hz() -> u64 {
    TSC_HZ.load(Ordering::Relaxed)
}

/// Timer interrupts since `init`.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Converts a TSC difference to nanoseconds (0 before calibration).
pub fn tsc_to_ns(cycles: u64) -> u64 {
    let hz = tsc_hz();
    if hz == 0 {
        0
    } else {
        (cycles as u128 * 1_000_000_000 / hz as u128) as u64
    }
}

/// Nanoseconds since kernel entry.
pub fn uptime_ns() -> u64 {
    tsc_to_ns(cpu::rdtsc().wrapping_sub(boot_tsc()))
}

pub fn uptime_ms() -> u64 {
    uptime_ns() / 1_000_000
}

/// Sets the wall clock from a UNIX time read now (from the RTC).
pub fn set_wall_clock(unix_seconds: i64) {
    if unix_seconds <= 0 {
        return;
    }
    let base = (unix_seconds as u64 * 1_000_000_000).saturating_sub(uptime_ns());
    WALL_AT_BOOT_NS.store(base, Ordering::SeqCst);
}

/// Nanoseconds since 1970-01-01 UTC, if the wall clock is known.
pub fn realtime_ns() -> Option<u64> {
    match WALL_AT_BOOT_NS.load(Ordering::SeqCst) {
        0 => None,
        base => Some(base + uptime_ns()),
    }
}

/// Busy-waits for `us` microseconds using the TSC.
pub fn delay_us(us: u64) {
    let target = cpu::rdtsc() + tsc_hz() * us / 1_000_000;
    while cpu::rdtsc() < target {
        core::hint::spin_loop();
    }
}

/// Sleeps at least `ms` milliseconds. With the scheduler running the
/// thread blocks; before that the CPU halts between timer interrupts.
pub fn sleep_ms(ms: u64) {
    if crate::sched::is_running() {
        return crate::sched::sleep_ms(ms);
    }
    let was = cpu::interrupts_enabled();
    let target = cpu::rdtsc() + tsc_hz() * ms / 1000;
    while cpu::rdtsc() < target {
        cpu::enable_interrupts_and_halt();
    }
    if !was {
        cpu::disable_interrupts();
    }
}

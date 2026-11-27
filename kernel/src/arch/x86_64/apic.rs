//! Local APIC: interrupt delivery to this CPU, EOI, the APIC timer and
//! interprocessor interrupts. Works in xAPIC (MMIO) and x2APIC (MSR)
//! mode, whichever the firmware left enabled on the boot CPU; the other
//! CPUs are put in the same mode.

use super::cpu;
use super::{hpet, pit, pmtimer};
use crate::mm::mmio;
use crate::sync::SpinLock;
use alloc::vec::Vec;
use core::arch::asm;
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
const REG_ICR_LOW: u32 = 0x300;
const REG_ICR_HIGH: u32 = 0x310;
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
const ICR_NMI: u32 = 0b100 << 8;
const ICR_ASSERT: u32 = 1 << 14;
const ICR_PENDING: u32 = 1 << 12;
const ICR_ALL_BUT_SELF: u32 = 0b11 << 18;
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
#[derive(Clone, Copy)]
pub struct LintNmi {
    pub lint: u8,
    pub active_low: bool,
    pub level: bool,
}

/// LINT NMI wiring, kept for the other CPUs.
static NMIS: SpinLock<Vec<LintNmi>> = SpinLock::new(Vec::new());

/// Enables the local APIC of this (the boot) CPU.
pub fn init(madt_address: u64, nmis: &[LintNmi]) -> Result<(), &'static str> {
    *NMIS.lock() = nmis.to_vec();
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
    program(nmis);
    Ok(())
}

/// Enables the local APIC of an application processor in the boot
/// CPU's mode and starts its periodic timer at `hz` (the boot CPU's
/// calibration is reused: all APIC timers run from the same clock).
pub fn init_ap(hz: u64) {
    // SAFETY: IA32_APIC_BASE of this CPU.
    unsafe {
        let msr = cpu::rdmsr(MSR_APIC_BASE);
        let mut want = msr | APIC_BASE_ENABLE;
        if X2APIC.load(Ordering::Relaxed) {
            // xAPIC must be enabled before x2APIC can be.
            if msr & APIC_BASE_ENABLE == 0 {
                cpu::wrmsr(MSR_APIC_BASE, want);
            }
            want |= APIC_BASE_X2APIC;
        }
        if want != msr {
            cpu::wrmsr(MSR_APIC_BASE, want);
        }
    }
    let nmis = NMIS.lock().clone();
    program(&nmis);
    start_periodic(hz);
}

fn program(nmis: &[LintNmi]) {
    write(REG_TPR, 0);
    choose_tpr_path();
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
}

fn send_icr(dest: u32, low: u32) {
    if X2APIC.load(Ordering::Relaxed) {
        // SAFETY: the x2APIC ICR is one 64-bit MSR (0x830).
        unsafe { cpu::wrmsr(0x830, ((dest as u64) << 32) | low as u64) };
    } else {
        write(REG_ICR_HIGH, dest << 24);
        write(REG_ICR_LOW, low);
        while read(REG_ICR_LOW) & ICR_PENDING != 0 {
            core::hint::spin_loop();
        }
    }
}

/// Sends interrupt `vector` to the CPU with local APIC id `apic_id`.
pub fn send_ipi(apic_id: u32, vector: u8) {
    send_icr(apic_id, vector as u32 | ICR_ASSERT);
}

/// Sends an NMI to the CPU with local APIC id `apic_id`.
pub fn send_nmi(apic_id: u32) {
    send_icr(apic_id, ICR_NMI | ICR_ASSERT);
}

/// Sends an NMI to every other CPU (used to stop them on a panic).
pub fn send_nmi_to_others() {
    send_icr(0, ICR_NMI | ICR_ASSERT | ICR_ALL_BUT_SELF);
}

/// The task priority goes through CR8 instead of the APIC register (see
/// `choose_tpr_path`).
static TPR_VIA_CR8: AtomicBool = AtomicBool::new(false);
static TPR_CHOSEN: AtomicBool = AtomicBool::new(false);

fn read_cr8() -> u8 {
    let v: u64;
    // SAFETY: reading CR8 has no side effects.
    unsafe { asm!("mov {}, cr8", out(reg) v, options(nomem, nostack)) }
    v as u8
}

fn write_cr8(class: u8) {
    // SAFETY: CR8 only sets which interrupt classes are delivered.
    unsafe { asm!("mov cr8, {}", in(reg) (class & 0xf) as u64, options(nomem, nostack)) }
}

/// Decides once how to set the task priority. On hardware and under KVM
/// the APIC register and CR8 are the same thing, and the register is
/// used (writing CR8 under KVM on AMD hosts stopped timer interrupts,
/// #48). WHPX with QEMU's own APIC keeps the guest's CR8 apart from the
/// register and overwrites the register from it (#45): there CR8 is used.
fn choose_tpr_path() {
    if TPR_CHOSEN.swap(true, Ordering::SeqCst) {
        return;
    }
    write(REG_TPR, 0xe0);
    let linked = read_cr8() == 0xe;
    write(REG_TPR, 0);
    if !linked {
        write_cr8(0);
    }
    TPR_VIA_CR8.store(!linked, Ordering::SeqCst);
}

/// Sets the task priority: interrupts of priority class `class` (vector
/// / 16) and below are held back.
pub fn set_task_priority(class: u8) {
    if TPR_VIA_CR8.load(Ordering::Relaxed) {
        write_cr8(class);
    } else {
        write(REG_TPR, ((class & 0xf) as u32) << 4);
    }
}

/// The current task priority class.
pub fn task_priority() -> u8 {
    if TPR_VIA_CR8.load(Ordering::Relaxed) {
        read_cr8()
    } else {
        (read(REG_TPR) >> 4) as u8 & 0xf
    }
}

/// True if the task priority is set through CR8 (for the boot log).
pub fn tpr_via_cr8() -> bool {
    TPR_VIA_CR8.load(Ordering::Relaxed)
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

/// Reference clocks for calibration, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reference {
    Hpet,
    PmTimer,
    Pit,
}

impl Reference {
    pub const ALL: [Reference; 3] = [Reference::Hpet, Reference::PmTimer, Reference::Pit];

    pub fn name(self) -> &'static str {
        match self {
            Reference::Hpet => "HPET",
            Reference::PmTimer => "ACPI PM timer",
            Reference::Pit => "PIT",
        }
    }

    pub fn available(self) -> bool {
        match self {
            Reference::Hpet => hpet::available(),
            Reference::PmTimer => pmtimer::available(),
            Reference::Pit => true,
        }
    }

    /// Busy-waits `ms` milliseconds (at most 54) of this clock. Fails if
    /// the clock does not count.
    pub fn wait_ms(self, ms: u64) -> Result<(), &'static str> {
        const SPIN_LIMIT: u64 = 400_000_000;
        let mut spins = 0u64;
        let mut spin = || {
            spins += 1;
            core::hint::spin_loop();
            spins < SPIN_LIMIT
        };
        match self {
            Reference::Hpet => {
                if !hpet::available() {
                    return Err("no HPET");
                }
                let target = ms * hpet::hz() / 1000;
                let c0 = hpet::counter();
                while hpet::elapsed(c0, hpet::counter()) < target {
                    if !spin() {
                        return Err("the HPET does not count");
                    }
                }
            }
            Reference::PmTimer => {
                if !pmtimer::available() {
                    return Err("no ACPI PM timer");
                }
                let target = (ms * pmtimer::HZ / 1000) as u32;
                let c0 = pmtimer::counter();
                while pmtimer::elapsed(c0, pmtimer::counter()) < target {
                    if !spin() {
                        return Err("the ACPI PM timer does not count");
                    }
                }
            }
            Reference::Pit => {
                let oneshot = pit::start_oneshot(ms);
                while !oneshot.expired() {
                    if !spin() {
                        return Err("the PIT does not count (no legacy timer?)");
                    }
                }
            }
        }
        Ok(())
    }
}

/// Result of measuring the TSC and the APIC timer against a reference.
pub struct Calibration {
    pub tsc_hz: u64,
    pub timer_per_ms: u64,
}

const CALIBRATION_MS: u64 = 50;

/// Measures the TSC and APIC timer frequencies over 50 ms of `r`.
/// Interrupts must be disabled; the APIC timer is stopped afterwards.
pub fn calibrate(r: Reference) -> Result<Calibration, &'static str> {
    write(REG_TIMER_DIVIDE, TIMER_DIVIDE_16);
    write(REG_LVT_TIMER, LVT_MASKED);
    write(REG_TIMER_INITIAL, u32::MAX);
    let tsc0 = cpu::rdtsc();
    let waited = r.wait_ms(CALIBRATION_MS);
    let tsc1 = cpu::rdtsc();
    let elapsed = u32::MAX - read(REG_TIMER_CURRENT);
    write(REG_TIMER_INITIAL, 0);
    waited?;
    let c = Calibration {
        tsc_hz: (tsc1 - tsc0) * 1000 / CALIBRATION_MS,
        timer_per_ms: elapsed as u64 / CALIBRATION_MS,
    };
    if c.timer_per_ms == 0 || c.tsc_hz < 1_000_000 {
        return Err("calibration gave implausible values");
    }
    TIMER_PER_MS.store(c.timer_per_ms, Ordering::SeqCst);
    Ok(c)
}

/// Measures only the TSC against `r` (for tests; the APIC timer keeps
/// running). Interrupts must be disabled.
pub fn measure_tsc_hz(r: Reference) -> Result<u64, &'static str> {
    let tsc0 = cpu::rdtsc();
    r.wait_ms(CALIBRATION_MS)?;
    Ok((cpu::rdtsc() - tsc0) * 1000 / CALIBRATION_MS)
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

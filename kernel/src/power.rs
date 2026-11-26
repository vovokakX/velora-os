//! Restart, power off and the power button (ACPI).
//!
//! Both power changes first write cached file system data to the disks.
//! Power off uses ACPI sleep state S5: `_S5` comes from the DSDT or an
//! SSDT (see `velora_acpi::find_s5`), and the sleep type is written to the
//! PM1 control registers, or to the sleep control register on
//! hardware-reduced ACPI. Restart tries the ACPI reset register (I/O,
//! memory or PCI configuration space), then the PCI reset port 0xCF9,
//! then the keyboard controller, and finally a triple fault.
//!
//! At boot [`init`] switches the firmware to ACPI mode if needed (SMI_CMD),
//! turns off all general-purpose events, routes the SCI and enables the
//! fixed power button event. Pressing the button wakes the `acpi` thread,
//! which powers off in an orderly way.
//!
//! NOT IMPLEMENTED: an AML interpreter. Without it `_PTS` is not run
//! before sleeping, and a power button that is a control method (reported
//! through a GPE and `Notify`, common on laptops with an embedded
//! controller) or a hardware-reduced generic event device is not seen.

use crate::arch::x86_64::{apic, cpu, ioapic, irq, port};
use crate::sched::{self, Priority, WaitQueue};
use crate::{acpi, fs, kprintln, kwarn};
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use velora_acpi::{Fadt, Polarity, Trigger};

pub const VECTOR_SCI: u8 = 0x22;

// PM1 status, enable and control bits.
const PWRBTN_STS: u16 = 1 << 8;
const WAK_STS: u16 = 1 << 15;
const PWRBTN_EN: u16 = 1 << 8;
const SCI_EN: u16 = 1;
const SLP_TYP_MASK: u16 = 7 << 10;
const SLP_EN: u16 = 1 << 13;
// Hardware-reduced sleep control register.
const HR_SLP_EN: u8 = 1 << 5;

static BUTTON: AtomicBool = AtomicBool::new(false);
static BUTTON_WAIT: WaitQueue = WaitQueue::new();
static SCI_COUNT: AtomicU64 = AtomicU64::new(0);
/// GSI of the SCI + 1 (0 = not routed).
static SCI_GSI: AtomicU32 = AtomicU32::new(0);
static BUTTON_ON: AtomicBool = AtomicBool::new(false);

/// What [`init`] set up, for the boot log and the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// The SCI is routed (interrupt line of ACPI events).
    pub sci_gsi: Option<u32>,
    /// The fixed power button event is enabled.
    pub power_button: bool,
    /// SCI interrupts seen.
    pub sci_count: u64,
}

/// The firmware has handed ACPI events to the OS (SCI_EN is set).
pub fn acpi_mode() -> bool {
    sci_enabled(&acpi::get().fadt)
}

pub fn status() -> Status {
    Status {
        sci_gsi: SCI_GSI.load(Ordering::Relaxed).checked_sub(1),
        power_button: BUTTON_ON.load(Ordering::Relaxed),
        sci_count: SCI_COUNT.load(Ordering::Relaxed),
    }
}

/// # Safety
/// Writes a 16-bit value to an I/O port.
unsafe fn outw(p: u16, v: u16) {
    unsafe { asm!("out dx, ax", in("dx") p, in("ax") v, options(nomem, nostack, preserves_flags)) }
}

/// # Safety
/// Reads a 16-bit I/O port.
unsafe fn inw(p: u16) -> u16 {
    let v: u16;
    unsafe { asm!("in ax, dx", out("ax") v, in("dx") p, options(nomem, nostack, preserves_flags)) }
    v
}

/// The I/O ports of PM1a and PM1b at `offset` in their blocks (0 = none).
fn pm1_ports(f: &Fadt, offset: u32) -> [u16; 2] {
    let at = |base: u32| if base == 0 { 0 } else { (base + offset) as u16 };
    [at(f.pm1a_evt), at(f.pm1b_evt)]
}

fn pm1_status(f: &Fadt) -> u16 {
    let mut v = 0;
    for p in pm1_ports(f, 0).into_iter().filter(|&p| p != 0) {
        // SAFETY: PM1 status registers from the FADT.
        v |= unsafe { inw(p) };
    }
    v
}

/// Clears status bits (they are write-one-to-clear).
fn pm1_clear(f: &Fadt, bits: u16) {
    for p in pm1_ports(f, 0).into_iter().filter(|&p| p != 0) {
        // SAFETY: as above.
        unsafe { outw(p, bits) };
    }
}

fn pm1_enable(f: &Fadt, bits: u16) {
    let half = (f.pm1_evt_len / 2) as u32;
    for p in pm1_ports(f, half).into_iter().filter(|&p| p != 0) {
        // SAFETY: PM1 enable registers (second half of the event block).
        unsafe { outw(p, bits) };
    }
}

fn sci_enabled(f: &Fadt) -> bool {
    // SAFETY: PM1a control register from the FADT.
    f.pm1a_cnt != 0 && unsafe { inw(f.pm1a_cnt as u16) } & SCI_EN != 0
}

/// Asks the firmware to hand ACPI events to the OS, if it has not yet.
fn enable_acpi_mode(f: &Fadt) -> Result<(), &'static str> {
    if sci_enabled(f) {
        return Ok(());
    }
    if f.smi_cmd == 0 || f.acpi_enable == 0 {
        return Err("SCI_EN is off and the FADT gives no way to enable ACPI mode");
    }
    // SAFETY: SMI_CMD and ACPI_ENABLE come from the FADT.
    unsafe { port::outb(f.smi_cmd as u16, f.acpi_enable) };
    for _ in 0..300 {
        if sci_enabled(f) {
            return Ok(());
        }
        crate::time::delay_us(10_000);
    }
    Err("the firmware did not enable ACPI mode within 3 s")
}

/// Turns off every general-purpose event and clears its status: without
/// an AML interpreter nothing could handle them, and an enabled GPE with
/// its status set would raise the SCI forever.
fn disable_gpes(f: &Fadt) {
    for (base, len) in [(f.gpe0_blk, f.gpe0_len), (f.gpe1_blk, f.gpe1_len)] {
        if base == 0 || len < 2 {
            continue;
        }
        let half = (len / 2) as u32;
        for i in 0..half {
            // SAFETY: GPE status and enable bytes inside the FADT block.
            unsafe {
                port::outb((base + half + i) as u16, 0);
                port::outb((base + i) as u16, 0xff);
            }
        }
    }
}

fn on_sci(_frame: &mut crate::arch::x86_64::idt::InterruptFrame) {
    SCI_COUNT.fetch_add(1, Ordering::Relaxed);
    let f = acpi::get().fadt;
    let sts = pm1_status(&f);
    if sts != 0 {
        pm1_clear(&f, sts);
    }
    disable_gpes(&f);
    if sts & PWRBTN_STS != 0 {
        BUTTON.store(true, Ordering::SeqCst);
        BUTTON_WAIT.wake_all();
    }
}

extern "C" fn button_thread(_: u64) -> u64 {
    loop {
        cpu::disable_interrupts();
        if !BUTTON.load(Ordering::SeqCst) {
            BUTTON_WAIT.wait(None);
            continue;
        }
        cpu::enable_interrupts();
        kprintln!();
        kprintln!("Power button pressed: powering off.");
        power_off();
    }
}

/// ACPI mode, SCI and the power button. Needs the scheduler (for the
/// `acpi` thread); interrupts may still be off. Returns a summary.
pub fn init(madt: &velora_acpi::Madt) -> Result<&'static str, &'static str> {
    let f = acpi::get().fadt;
    if f.flags & Fadt::HW_REDUCED_ACPI != 0 {
        return Ok("hardware-reduced ACPI (no fixed power button)");
    }
    if f.pm1a_cnt == 0 || f.pm1a_evt == 0 || f.pm1_evt_len < 4 {
        return Err("the FADT has no PM1 registers");
    }
    enable_acpi_mode(&f)?;
    disable_gpes(&f);
    pm1_enable(&f, 0);
    pm1_clear(&f, pm1_status(&f));

    // The SCI is a shared, level-triggered, active-low ISA interrupt
    // unless the MADT says otherwise.
    let (gsi, polarity, trigger) = madt.isa_irq(f.sci_int as u8);
    irq::set_handler(VECTOR_SCI, on_sci);
    ioapic::route(
        gsi,
        ioapic::Route {
            vector: VECTOR_SCI,
            active_low: polarity != Polarity::ActiveHigh,
            level: trigger != Trigger::Edge,
            dest_apic: apic::id() as u8,
        },
    )?;
    SCI_GSI.store(gsi + 1, Ordering::SeqCst);

    if f.flags & Fadt::PWR_BUTTON_IS_METHOD != 0 {
        return Ok("ACPI mode, SCI routed; the power button is a control method (not supported)");
    }
    sched::spawn_kernel("acpi", button_thread, 0, Priority::High)
        .ok_or("cannot start the acpi thread")?;
    pm1_enable(&f, PWRBTN_EN);
    BUTTON_ON.store(true, Ordering::SeqCst);
    Ok("ACPI mode, SCI routed, power button enabled")
}

fn sync_disks() {
    if let Err(e) = fs::with(|v| v.sync_all()) {
        kwarn!("sync before power change: {}", e);
    }
}

fn spin_ms(ms: u64) {
    let end = cpu::rdtsc() + crate::time::tsc_hz() * ms / 1000;
    while cpu::rdtsc() < end {
        core::hint::spin_loop();
    }
}

/// Writes one byte to a generic address (I/O, memory or PCI
/// configuration space of bus 0). Returns false for other spaces.
fn write_gas8(space: u8, address: u64, value: u8) -> bool {
    match space {
        velora_acpi::SPACE_IO => {
            // SAFETY: a register named by the FADT.
            unsafe { port::outb(address as u16, value) };
            true
        }
        velora_acpi::SPACE_MEMORY => match crate::mm::mmio::map(address, 1) {
            Some(va) => {
                // SAFETY: just mapped; a register named by the FADT.
                unsafe { crate::mm::mmio::write8(va, value) };
                true
            }
            None => false,
        },
        velora_acpi::SPACE_PCI => {
            use velora_pci::ConfigAccess;
            let addr = velora_pci::Address {
                bus: 0,
                device: ((address >> 32) & 0x1f) as u8,
                function: ((address >> 16) & 0x7) as u8,
            };
            let offset = (address & 0xffff) as u16;
            let cfg = crate::drivers::pci::config();
            let shift = (offset & 3) * 8;
            let old = cfg.read32(addr, offset & !3);
            let new = (old & !(0xff << shift)) | ((value as u32) << shift);
            cfg.write32(addr, offset & !3, new);
            true
        }
        _ => false,
    }
}

pub fn power_off() -> ! {
    sync_disks();
    kprintln!("Velora OS: powering off");
    cpu::disable_interrupts();
    let f = acpi::get().fadt;
    match acpi::s5() {
        Some((a, _)) if f.flags & Fadt::HW_REDUCED_ACPI != 0 => {
            let (space, address) = f.sleep_control;
            write_gas8(space, address, ((a & 7) << 2) | HR_SLP_EN);
            spin_ms(500);
        }
        Some((a, b)) => {
            pm1_clear(&f, WAK_STS);
            // SAFETY: PM1 control ports come from the FADT; writing
            // SLP_TYP with SLP_EN enters the requested sleep state. Other
            // bits (SCI_EN) are kept.
            unsafe {
                for (port, typ) in [(f.pm1a_cnt, a), (f.pm1b_cnt, b)] {
                    if port != 0 {
                        let v = inw(port as u16) & !(SLP_TYP_MASK | SLP_EN);
                        outw(port as u16, v | ((typ as u16) << 10) | SLP_EN);
                    }
                }
            }
            spin_ms(500);
        }
        None => kwarn!("no \\_S5 sleep state in the ACPI tables"),
    }
    kprintln!("Power off did not work. It is now safe to turn off the computer.");
    cpu::halt_forever()
}

pub fn restart() -> ! {
    sync_disks();
    kprintln!("Velora OS: restarting");
    cpu::disable_interrupts();
    let f = acpi::get().fadt;
    if f.flags & Fadt::RESET_REG_SUPPORTED != 0
        && write_gas8(f.reset_space, f.reset_address, f.reset_value)
    {
        spin_ms(100);
    }
    // SAFETY: the reset mechanisms below only reset the machine.
    unsafe {
        port::outb(0xcf9, 0x02);
        port::outb(0xcf9, 0x06);
        spin_ms(100);
        for _ in 0..10_000 {
            if port::inb(0x64) & 2 == 0 {
                break;
            }
        }
        port::outb(0x64, 0xfe);
        spin_ms(100);
        // Triple fault: an empty IDT and an exception.
        let idt: [u64; 2] = [0, 0];
        asm!("lidt [{}]", "int3", in(reg) &idt, options(noreturn));
    }
}

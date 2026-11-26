//! Restart and power off.
//!
//! Both first write cached file system data to the disks. Power off uses
//! ACPI sleep state S5 (the `_S5` values come from the DSDT, see
//! `velora_acpi::find_s5`). Restart tries the ACPI reset register, then
//! the PCI reset port 0xCF9, then the keyboard controller, and finally a
//! triple fault.

use crate::arch::x86_64::{cpu, port};
use crate::{acpi, fs, kprintln, kwarn};
use core::arch::asm;

fn sync_disks() {
    if let Err(e) = fs::with(|v| v.sync_all()) {
        kwarn!("sync before power change: {}", e);
    }
}

/// # Safety
/// Writes a 16-bit value to an I/O port.
unsafe fn outw(p: u16, v: u16) {
    unsafe { asm!("out dx, ax", in("dx") p, in("ax") v, options(nomem, nostack, preserves_flags)) }
}

fn spin_ms(ms: u64) {
    let end = cpu::rdtsc() + crate::time::tsc_hz() * ms / 1000;
    while cpu::rdtsc() < end {
        core::hint::spin_loop();
    }
}

pub fn power_off() -> ! {
    sync_disks();
    kprintln!("Velora OS: powering off");
    cpu::disable_interrupts();
    let fadt = acpi::get().fadt;
    if let Some((a, b)) = acpi::dsdt().and_then(velora_acpi::find_s5) {
        const SLP_EN: u16 = 1 << 13;
        // SAFETY: PM1 control ports come from the FADT; writing SLP_TYP
        // with SLP_EN enters the requested sleep state.
        unsafe {
            if fadt.pm1a_cnt != 0 {
                outw(fadt.pm1a_cnt as u16, ((a as u16) << 10) | SLP_EN);
            }
            if fadt.pm1b_cnt != 0 {
                outw(fadt.pm1b_cnt as u16, ((b as u16) << 10) | SLP_EN);
            }
        }
        spin_ms(500);
    }
    kprintln!("Power off did not work (no ACPI S5). It is now safe to turn off the computer.");
    cpu::halt_forever()
}

pub fn restart() -> ! {
    sync_disks();
    kprintln!("Velora OS: restarting");
    cpu::disable_interrupts();
    let fadt = acpi::get().fadt;
    // SAFETY: the reset mechanisms below only reset the machine.
    unsafe {
        if fadt.flags & velora_acpi::Fadt::RESET_REG_SUPPORTED != 0 && fadt.reset_space == 1 {
            port::outb(fadt.reset_address as u16, fadt.reset_value);
            spin_ms(100);
        }
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

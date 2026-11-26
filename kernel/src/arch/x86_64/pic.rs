//! Legacy 8259 PICs. The kernel uses the APIC, so the PICs are remapped
//! away from the exception vectors and fully masked. Spurious IRQs they
//! may still raise arrive at vectors 0x70..=0x7f and are ignored.

use super::port::{inb, outb};

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xa0;
const PIC2_DATA: u16 = 0xa1;

pub const VECTOR_BASE: u8 = 0x70;

pub fn disable() {
    // SAFETY: standard 8259 initialisation sequence.
    unsafe {
        outb(PIC1_CMD, 0x11); // ICW1: init, expect ICW4
        outb(PIC2_CMD, 0x11);
        outb(PIC1_DATA, VECTOR_BASE); // ICW2: vector offsets
        outb(PIC2_DATA, VECTOR_BASE + 8);
        outb(PIC1_DATA, 4); // ICW3: slave on IRQ2
        outb(PIC2_DATA, 2);
        outb(PIC1_DATA, 0x01); // ICW4: 8086 mode
        outb(PIC2_DATA, 0x01);
        outb(PIC1_DATA, 0xff); // mask everything
        outb(PIC2_DATA, 0xff);
    }
}

/// Interrupt mask registers of both PICs (0xffff = all masked).
pub fn masks() -> u16 {
    // SAFETY: reading the mask registers has no side effects.
    unsafe { (inb(PIC2_DATA) as u16) << 8 | inb(PIC1_DATA) as u16 }
}

//! 8042 PS/2 controller and keyboard.
//!
//! The controller translates the keyboard's scancode set 2 into set 1,
//! which `velora_input` decodes. Only the first port (keyboard) is used;
//! a PS/2 mouse can be added on the second port later.

use crate::arch::x86_64::idt::InterruptFrame;
use crate::arch::x86_64::port::{inb, outb};
use crate::arch::x86_64::{apic, ioapic, irq};
use crate::sync::SpinLock;
use crate::{input, time};
use velora_input::{Keyboard, Layout};

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;
const COMMAND: u16 = 0x64;

const STATUS_OUTPUT_FULL: u8 = 1 << 0;
const STATUS_INPUT_FULL: u8 = 1 << 1;

const CFG_PORT1_IRQ: u8 = 1 << 0;
const CFG_PORT2_IRQ: u8 = 1 << 1;
const CFG_PORT1_CLOCK_OFF: u8 = 1 << 4;
const CFG_TRANSLATE: u8 = 1 << 6;

const KBD_ACK: u8 = 0xfa;
const KBD_SET_LEDS: u8 = 0xed;

struct State {
    keyboard: Keyboard,
    /// LED byte waiting for the ACK of a "set LEDs" command.
    pending_leds: Option<u8>,
}

static STATE: SpinLock<State> = SpinLock::new(State {
    keyboard: Keyboard::new(Layout::Us, Layout::Russian),
    pending_leds: None,
});

fn status() -> u8 {
    // SAFETY: reading the status register has no side effects.
    unsafe { inb(STATUS) }
}

fn wait_input_empty() -> bool {
    for _ in 0..100_000 {
        if status() & STATUS_INPUT_FULL == 0 {
            return true;
        }
        time::delay_us(10);
    }
    false
}

fn wait_output_full(timeout_ms: u64) -> bool {
    for _ in 0..timeout_ms * 100 {
        if status() & STATUS_OUTPUT_FULL != 0 {
            return true;
        }
        time::delay_us(10);
    }
    false
}

fn command(c: u8) -> Result<(), &'static str> {
    if !wait_input_empty() {
        return Err("PS/2 controller does not accept commands");
    }
    // SAFETY: writing a controller command.
    unsafe { outb(COMMAND, c) };
    Ok(())
}

fn write_data(d: u8) -> Result<(), &'static str> {
    if !wait_input_empty() {
        return Err("PS/2 controller input buffer stuck");
    }
    // SAFETY: writing to the data port (to the device or command argument).
    unsafe { outb(DATA, d) };
    Ok(())
}

fn read_data(timeout_ms: u64) -> Option<u8> {
    if wait_output_full(timeout_ms) {
        // SAFETY: reading the data port.
        Some(unsafe { inb(DATA) })
    } else {
        None
    }
}

fn flush() {
    for _ in 0..64 {
        if status() & STATUS_OUTPUT_FULL == 0 {
            break;
        }
        // SAFETY: discarding a pending byte.
        unsafe { inb(DATA) };
    }
}

fn read_config() -> Result<u8, &'static str> {
    command(0x20)?;
    read_data(50).ok_or("PS/2 controller did not return its configuration")
}

fn write_config(cfg: u8) -> Result<(), &'static str> {
    command(0x60)?;
    write_data(cfg)
}

/// Sends a byte to the keyboard and waits for its ACK.
fn keyboard_command(byte: u8) -> Result<(), &'static str> {
    for _ in 0..3 {
        write_data(byte)?;
        match read_data(200) {
            Some(KBD_ACK) => return Ok(()),
            Some(0xfe) => continue, // resend
            _ => return Err("keyboard did not acknowledge a command"),
        }
    }
    Err("keyboard keeps asking for a resend")
}

fn on_irq(_frame: &mut InterruptFrame) {
    let mut st = STATE.lock();
    while status() & STATUS_OUTPUT_FULL != 0 {
        // SAFETY: reading the byte that caused the interrupt.
        let byte = unsafe { inb(DATA) };
        if byte == KBD_ACK {
            if let Some(leds) = st.pending_leds.take() {
                // SAFETY: the keyboard is waiting for the LED argument.
                unsafe { outb(DATA, leds) };
            }
            continue;
        }
        let (event, fx) = st.keyboard.feed(byte);
        if let Some(ev) = event {
            input::push(ev);
        }
        if fx.update_leds && status() & STATUS_INPUT_FULL == 0 {
            st.pending_leds = Some(st.keyboard.led_bits());
            // SAFETY: "set LEDs" command; the argument follows on ACK.
            unsafe { outb(DATA, KBD_SET_LEDS) };
        }
    }
}

/// Initialises the controller and keyboard and routes IRQ 1.
/// Interrupts must be disabled; `time` must be calibrated.
pub fn init(madt: &velora_acpi::Madt) -> Result<(), &'static str> {
    if status() == 0xff {
        return Err("no PS/2 controller");
    }
    command(0xad)?; // disable port 1
    command(0xa7)?; // disable port 2
    flush();

    let mut cfg = read_config()?;
    cfg &= !(CFG_PORT1_IRQ | CFG_PORT2_IRQ | CFG_PORT1_CLOCK_OFF);
    cfg |= CFG_TRANSLATE;
    write_config(cfg)?;

    command(0xaa)?; // controller self-test
    if read_data(200) != Some(0x55) {
        return Err("PS/2 controller self-test failed");
    }
    write_config(cfg)?; // some controllers reset the config on self-test

    command(0xab)?; // test port 1
    if read_data(200) != Some(0x00) {
        return Err("PS/2 port 1 test failed");
    }
    command(0xae)?; // enable port 1

    // Reset the keyboard: ACK, then 0xAA (self-test passed).
    keyboard_command(0xff)?;
    if read_data(1000) != Some(0xaa) {
        return Err("keyboard self-test failed");
    }
    keyboard_command(KBD_SET_LEDS)?;
    keyboard_command(0)?;
    flush();

    irq::set_handler(irq::VECTOR_KEYBOARD, on_irq);
    ioapic::route_isa(madt, 1, irq::VECTOR_KEYBOARD, apic::id() as u8)?;
    write_config(cfg | CFG_PORT1_IRQ)?;
    Ok(())
}

pub fn layout() -> Layout {
    STATE.lock().keyboard.layout()
}

//! 16550-compatible UART driver (COM1), used for the kernel log and tests.

use super::port::{inb, outb};
use crate::sync::SpinLock;
use core::fmt;

const COM1: u16 = 0x3f8;

const DATA: u16 = 0;
const INT_ENABLE: u16 = 1;
const FIFO_CTRL: u16 = 2;
const LINE_CTRL: u16 = 3;
const MODEM_CTRL: u16 = 4;
const LINE_STATUS: u16 = 5;

const LSR_TX_EMPTY: u8 = 1 << 5;

pub struct Serial {
    base: u16,
    present: bool,
}

impl Serial {
    const fn new(base: u16) -> Self {
        Self {
            base,
            present: false,
        }
    }

    /// Programs 115200 8N1 and checks the chip with a loopback test.
    fn init(&mut self) {
        // SAFETY: standard COM1 programming sequence.
        unsafe {
            outb(self.base + INT_ENABLE, 0x00);
            outb(self.base + LINE_CTRL, 0x80); // DLAB on
            outb(self.base + DATA, 0x01); // divisor 1 -> 115200 baud
            outb(self.base + INT_ENABLE, 0x00);
            outb(self.base + LINE_CTRL, 0x03); // 8N1, DLAB off
            outb(self.base + FIFO_CTRL, 0xc7); // FIFO on, cleared, 14-byte threshold
            outb(self.base + MODEM_CTRL, 0x1e); // loopback mode
            outb(self.base + DATA, 0xae);
            self.present = inb(self.base + DATA) == 0xae;
            outb(self.base + MODEM_CTRL, 0x0f); // normal mode, OUT1/OUT2, RTS/DTR
        }
    }

    pub fn write_byte(&mut self, b: u8) {
        if !self.present {
            return;
        }
        // SAFETY: polling the line status register and writing THR.
        unsafe {
            let mut spins = 0u32;
            while inb(self.base + LINE_STATUS) & LSR_TX_EMPTY == 0 {
                spins += 1;
                if spins > 1_000_000 {
                    return; // give up rather than hang the kernel on a broken UART
                }
                core::hint::spin_loop();
            }
            outb(self.base + DATA, b);
        }
    }

    pub fn is_present(&self) -> bool {
        self.present
    }
}

impl fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(b);
        }
        Ok(())
    }
}

pub static SERIAL: SpinLock<Serial> = SpinLock::new(Serial::new(COM1));

pub fn init() {
    SERIAL.lock().init();
}

pub fn is_present() -> bool {
    SERIAL.lock().is_present()
}

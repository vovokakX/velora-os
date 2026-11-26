//! Kernel console: every message goes to the serial port and, once it is
//! set up, to the framebuffer text console.

pub mod font;
pub mod framebuffer;
pub mod text;

use crate::arch::x86_64::serial::SERIAL;
use crate::boot::limine;
use crate::sync::SpinLock;
use core::fmt::{self, Write};
use framebuffer::Framebuffer;
use text::TextConsole;

static SCREEN: SpinLock<Option<TextConsole>> = SpinLock::new(None);

/// Sets up the framebuffer console. Returns (width, height) on success.
pub fn init() -> Option<(usize, usize)> {
    let resp = limine::FRAMEBUFFER.response()?;
    let raw = resp.framebuffers().first()?;
    let fb = Framebuffer::from_limine(raw)?;
    let size = (fb.width, fb.height);
    *SCREEN.lock() = Some(TextConsole::new(fb));
    Some(size)
}

/// Moves the screen console to a RAM back buffer (needs the heap).
pub fn enable_back_buffer() -> bool {
    match SCREEN.lock().as_mut() {
        Some(c) => {
            c.framebuffer_mut().enable_back_buffer();
            true
        }
        None => false,
    }
}

/// Text grid size of the screen console, if there is one.
pub fn screen_size() -> Option<(usize, usize)> {
    SCREEN.lock().as_ref().map(|c| (c.cols, c.rows))
}

struct Both;

impl Write for Both {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let _ = SERIAL.lock().write_str(s);
        if let Some(c) = SCREEN.lock().as_mut() {
            c.write_str(s);
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = Both.write_fmt(args);
}

/// Output path for panics and fatal exceptions. The normal locks may be
/// held by the code that just crashed, so they are forcibly released.
/// Only called with interrupts disabled, on the way to halting.
pub fn emergency(f: impl FnOnce(&mut dyn Write)) {
    // SAFETY: the system is going down; whoever held these locks will not
    // run again.
    unsafe {
        SERIAL.force_unlock();
        SCREEN.force_unlock();
    }
    f(&mut Both);
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => { $crate::console::_print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! kprintln {
    () => { $crate::kprint!("\n") };
    ($($arg:tt)*) => { $crate::console::_print(format_args!("{}\n", format_args!($($arg)*))) };
}

#[macro_export]
macro_rules! kinfo {
    ($($arg:tt)*) => { $crate::kprintln!("\x1b[36m[ info ]\x1b[0m {}", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! kok {
    ($($arg:tt)*) => { $crate::kprintln!("\x1b[32m[  ok  ]\x1b[0m {}", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! kwarn {
    ($($arg:tt)*) => { $crate::kprintln!("\x1b[33m[ warn ]\x1b[0m {}", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! kerror {
    ($($arg:tt)*) => { $crate::kprintln!("\x1b[31m[ fail ]\x1b[0m {}", format_args!($($arg)*)) };
}

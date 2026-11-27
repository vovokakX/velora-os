//! The console terminal: line mode or raw keys, foreground process, size.

use crate::sys::syscall2;
use velora_abi::{nr, tty};

pub use velora_abi::tty::{NO_ECHO, RAW};

/// Sets the mode bits (`RAW`, `NO_ECHO`); returns the previous ones.
pub fn set_mode(mode: u64) -> u64 {
    // SAFETY: no pointers.
    unsafe { syscall2(nr::TTY, tty::SET_MODE, mode) as u64 }
}

/// The process Ctrl+C ends in line mode (0 = none).
pub fn set_foreground(pid: u64) {
    // SAFETY: no pointers.
    unsafe { syscall2(nr::TTY, tty::SET_FOREGROUND, pid) };
}

/// (columns, rows) of the console.
pub fn size() -> (u32, u32) {
    // SAFETY: no pointers.
    let r = unsafe { syscall2(nr::TTY, tty::SIZE, 0) } as u64;
    ((r >> 32) as u32, r as u32)
}

/// Character cell size in pixels, or `None` if the console has no
/// screen (serial port only).
pub fn cell_size() -> Option<(u32, u32)> {
    // SAFETY: no pointers.
    let r = unsafe { syscall2(nr::TTY, tty::CELL_SIZE, 0) };
    if r < 0 {
        return None;
    }
    let r = r as u64;
    Some(((r >> 32) as u32, r as u32))
}

/// True if `fd` is the console (not a file or pipe).
pub fn is_console(fd: u64) -> bool {
    let mut st = velora_abi::Stat::default();
    // SAFETY: `st` is writable and has the kernel's layout.
    let r = unsafe { syscall2(nr::FSTAT, fd, &mut st as *mut velora_abi::Stat as u64) };
    r == 0 && st.mode & velora_abi::mode::TYPE_MASK == velora_abi::mode::CHAR_DEVICE
}

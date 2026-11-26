//! Console input and output.

use crate::sys::syscall3;
use crate::{Errno, check};
use core::fmt;
use velora_abi::{STDERR, STDIN, STDOUT, nr};

/// Writes bytes to a file descriptor; returns how many were written.
pub fn write(fd: u64, buf: &[u8]) -> Result<usize, Errno> {
    // SAFETY: the buffer is valid for `len` bytes.
    check(unsafe { syscall3(nr::WRITE, fd, buf.as_ptr() as u64, buf.len() as u64) })
        .map(|n| n as usize)
}

/// Writes the whole buffer.
pub fn write_all(fd: u64, mut buf: &[u8]) -> Result<(), Errno> {
    while !buf.is_empty() {
        let n = write(fd, buf)?;
        buf = &buf[n..];
    }
    Ok(())
}

/// Reads from a file descriptor. For the keyboard (fd 0) this waits for
/// at least one character and returns whole UTF-8 characters.
pub fn read(fd: u64, buf: &mut [u8]) -> Result<usize, Errno> {
    // SAFETY: the buffer is valid and writable for `len` bytes.
    check(unsafe { syscall3(nr::READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64) })
        .map(|n| n as usize)
}

/// Reads typed characters from the keyboard.
pub fn read_stdin(buf: &mut [u8]) -> Result<usize, Errno> {
    read(STDIN, buf)
}

pub struct Stdout;
pub struct Stderr;

impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_all(STDOUT, s.as_bytes()).map_err(|_| fmt::Error)
    }
}

impl fmt::Write for Stderr {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_all(STDERR, s.as_bytes()).map_err(|_| fmt::Error)
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = fmt::Write::write_fmt(&mut Stdout, args);
}

#[doc(hidden)]
pub fn _eprint(args: fmt::Arguments) {
    let _ = fmt::Write::write_fmt(&mut Stderr, args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::io::_print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::io::_print(format_args!("{}\n", format_args!($($arg)*))) };
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => { $crate::io::_eprint(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! eprintln {
    () => { $crate::eprint!("\n") };
    ($($arg:tt)*) => { $crate::io::_eprint(format_args!("{}\n", format_args!($($arg)*))) };
}

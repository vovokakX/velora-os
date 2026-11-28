//! The line discipline of a pseudo-terminal: what happens to the bytes a
//! terminal program (the master side, for example the Lumi terminal)
//! writes before the program on the other side (the slave, for example
//! vsh) reads them, and to what that program writes back.
//!
//! It behaves like the console terminal (`kernel/src/tty.rs`):
//!
//! - Line mode (default): typed characters are echoed, Backspace (0x7f or
//!   0x08) removes the last character, Enter ('\n' or '\r') delivers the
//!   line with '\n'. Ctrl+D (0x04) on an empty line is the end of input;
//!   Ctrl+C (0x03) drops the line and asks to end the foreground process.
//!   Escape sequences (arrow keys) are ignored.
//! - Raw mode ([`tty::RAW`]): every byte is delivered as it is, without
//!   echo.
//!
//! The kernel wraps it with locks and wait queues; this crate is plain
//! logic and is tested on the host.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use velora_abi::tty;

/// Program output the master has not read yet, at most this much; a
/// program writing more waits.
pub const OUTPUT_LIMIT: usize = 256 * 1024;
/// Typed input the program has not read yet, at most this much (more is
/// dropped, as a full keyboard buffer drops keys).
pub const INPUT_LIMIT: usize = 64 * 1024;

/// What a master write asks the kernel to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Effects {
    /// Ctrl+C in line mode: end this process (the foreground one).
    pub interrupt: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Esc {
    None,
    /// After ESC.
    Start,
    /// Inside `ESC [ ...` until the final byte.
    Csi,
}

pub struct LineDiscipline {
    pub mode: u64,
    /// The process Ctrl+C ends in line mode (0: none).
    pub foreground: u64,
    pub cols: u32,
    pub rows: u32,
    /// Bytes the program can read.
    input: VecDeque<u8>,
    /// The line being typed (line mode).
    line: Vec<u8>,
    /// Ctrl+D on an empty line: the next read returns the end of input.
    eof: bool,
    esc: Esc,
    /// Bytes for the master: program output and echo.
    output: VecDeque<u8>,
}

impl Default for LineDiscipline {
    fn default() -> Self {
        Self::new(80, 25)
    }
}

impl LineDiscipline {
    pub fn new(cols: u32, rows: u32) -> Self {
        Self {
            mode: 0,
            foreground: 0,
            cols,
            rows,
            input: VecDeque::new(),
            line: Vec::new(),
            eof: false,
            esc: Esc::None,
            output: VecDeque::new(),
        }
    }

    /// Sets the mode bits; returns the previous ones. Leaving line mode
    /// delivers a half-typed line as it is.
    pub fn set_mode(&mut self, mode: u64) -> u64 {
        let old = self.mode;
        self.mode = mode & (tty::RAW | tty::NO_ECHO);
        if self.mode & tty::RAW != 0 && !self.line.is_empty() {
            let line = core::mem::take(&mut self.line);
            self.push_input(&line);
        }
        old
    }

    fn push_input(&mut self, b: &[u8]) {
        let room = INPUT_LIMIT.saturating_sub(self.input.len());
        self.input.extend(&b[..b.len().min(room)]);
    }

    fn echo(&mut self, b: &[u8]) {
        if self.mode & tty::NO_ECHO == 0 {
            self.output.extend(b);
        }
    }

    /// Bytes typed on the terminal (written by the master).
    pub fn from_master(&mut self, bytes: &[u8]) -> Effects {
        let mut fx = Effects::default();
        if self.mode & tty::RAW != 0 {
            self.push_input(bytes);
            return fx;
        }
        for &b in bytes {
            match self.esc {
                Esc::Start => {
                    self.esc = if b == b'[' || b == b'O' {
                        Esc::Csi
                    } else {
                        Esc::None
                    };
                    continue;
                }
                Esc::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.esc = Esc::None;
                    }
                    continue;
                }
                Esc::None => {}
            }
            match b {
                0x1b => self.esc = Esc::Start,
                b'\n' | b'\r' => {
                    let mut line = core::mem::take(&mut self.line);
                    line.push(b'\n');
                    self.push_input(&line);
                    // The newline is echoed even without echo, as on the
                    // console (the password prompt moves on).
                    self.output.push_back(b'\n');
                }
                0x7f | 0x08 => {
                    // Remove one character: its continuation bytes too.
                    let mut removed = false;
                    while let Some(c) = self.line.pop() {
                        removed = true;
                        if c & 0xc0 != 0x80 {
                            break;
                        }
                    }
                    if removed {
                        self.echo(b"\x08 \x08");
                    }
                }
                0x04 => {
                    if self.line.is_empty() {
                        self.eof = true;
                    }
                }
                0x03 => {
                    self.line.clear();
                    self.output.extend(b"^C\n");
                    if self.foreground != 0 {
                        fx.interrupt = Some(self.foreground);
                    }
                }
                b'\t' => {
                    self.line.push(b);
                    self.echo(&[b]);
                }
                c if c < 0x20 => {}
                c => {
                    self.line.push(c);
                    self.echo(&[c]);
                }
            }
        }
        fx
    }

    /// Output of the program (written by the slave).
    pub fn from_slave(&mut self, bytes: &[u8]) {
        self.output.extend(bytes);
    }

    /// Room for more program output before a writer has to wait.
    pub fn output_room(&self) -> usize {
        OUTPUT_LIMIT.saturating_sub(self.output.len())
    }

    /// A read by the program: `None` when it has to wait, an empty
    /// vector at the end of input (Ctrl+D on an empty line).
    pub fn slave_read(&mut self, max: usize) -> Option<Vec<u8>> {
        if max == 0 {
            return Some(Vec::new());
        }
        if !self.input.is_empty() {
            let n = max.min(self.input.len());
            return Some(self.input.drain(..n).collect());
        }
        if self.eof {
            self.eof = false;
            return Some(Vec::new());
        }
        None
    }

    /// A read by the master: everything waiting, up to `max` bytes.
    pub fn master_read(&mut self, max: usize) -> Vec<u8> {
        let n = max.min(self.output.len());
        self.output.drain(..n).collect()
    }

    pub fn has_output(&self) -> bool {
        !self.output.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_mode_echo_edit_and_interrupt() {
        let mut l = LineDiscipline::new(80, 24);
        l.from_master("ab\x7fcя\x7fд\x1b[Dx".as_bytes());
        assert_eq!(l.slave_read(64), None, "no line yet");
        // Echo: a, b, erase, c, я, erase, д, x (the arrow is ignored).
        assert_eq!(
            l.master_read(256),
            "ab\x08 \x08cя\x08 \x08дx".as_bytes()
        );
        l.from_master(b"\n");
        assert_eq!(l.slave_read(64).unwrap(), "acдx\n".as_bytes());
        assert_eq!(l.master_read(256), b"\n");
        // Ctrl+C drops the line and names the foreground process.
        l.foreground = 7;
        l.from_master(b"zz");
        assert_eq!(l.from_master(b"\x03").interrupt, Some(7));
        l.from_master(b"\r");
        assert_eq!(l.slave_read(64).unwrap(), b"\n");
        // Ctrl+D on an empty line is the end of input, once.
        l.from_master(b"\x04");
        assert_eq!(l.slave_read(64).unwrap(), b"");
        assert_eq!(l.slave_read(64), None);
        // Without echo the characters are not shown, the newline is.
        l.master_read(256);
        l.set_mode(tty::NO_ECHO);
        l.from_master(b"pw\n");
        assert_eq!(l.master_read(256), b"\n");
        assert_eq!(l.slave_read(2).unwrap(), b"pw");
        assert_eq!(l.slave_read(8).unwrap(), b"\n");
    }

    #[test]
    fn raw_mode_passes_bytes() {
        let mut l = LineDiscipline::default();
        l.from_master(b"ab");
        l.set_mode(tty::RAW);
        // The half-typed line is delivered when raw mode starts.
        assert_eq!(l.slave_read(64).unwrap(), b"ab");
        l.master_read(64);
        l.foreground = 3;
        assert_eq!(l.from_master(b"\x1b[A\x03\x7f").interrupt, None);
        assert_eq!(l.slave_read(64).unwrap(), b"\x1b[A\x03\x7f");
        assert!(!l.has_output(), "raw mode does not echo");
        l.from_slave(b"out");
        assert_eq!(l.master_read(2), b"ou");
        assert_eq!(l.master_read(9), b"t");
    }
}

//! The console terminal behind descriptors on `/dev/console`.
//!
//! Line mode (default): typed characters are echoed, Backspace edits the
//! line, Enter delivers it with '\n'. Ctrl+D on an empty line is the end
//! of input (read returns 0); Ctrl+C drops the line. When a foreground
//! process is set, Ctrl+C ends it at once, even if it does not read the
//! keyboard (the key is caught as it arrives, see [`intercept`]).
//!
//! Raw mode (`tty::RAW`): every key is delivered at once without echo;
//! arrows and other special keys arrive as ANSI sequences, Backspace as
//! 0x7f. vsh uses this to edit its own line.

use crate::arch::x86_64::cpu;
use crate::sched::{Priority, WaitQueue};
use crate::sync::SpinLock;
use crate::{input, kprint, sched};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use velora_abi::tty;
use velora_input::{KeyCode, KeyEvent};

struct Tty {
    mode: u64,
    foreground: u64,
    /// Bytes produced but not yet read (a line longer than the reader's
    /// buffer, or the rest of an escape sequence).
    pending: VecDeque<u8>,
}

static TTY: SpinLock<Tty> = SpinLock::new(Tty {
    mode: 0,
    foreground: 0,
    pending: VecDeque::new(),
});

/// Exit status of a process ended with Ctrl+C (128 + SIGINT, as shells
/// report it).
pub const INTERRUPTED: i64 = 130;

static INTERRUPT: AtomicBool = AtomicBool::new(false);
static INTERRUPT_WAIT: WaitQueue = WaitQueue::new();

/// Called for every key event before it is queued (interrupt context).
/// Ctrl+C in line mode with a foreground process is taken out of the
/// input and handled by the `tty-intr` thread.
pub fn intercept(ev: &KeyEvent) -> bool {
    if !ev.pressed || ev.ch != Some('\x03') || !sched::is_running() {
        return false;
    }
    let t = TTY.lock();
    if t.mode & tty::RAW != 0 || t.foreground == 0 {
        return false;
    }
    drop(t);
    INTERRUPT.store(true, Ordering::SeqCst);
    INTERRUPT_WAIT.wake_all();
    true
}

extern "C" fn interrupt_thread(_: u64) -> u64 {
    loop {
        cpu::disable_interrupts();
        if !INTERRUPT.swap(false, Ordering::SeqCst) {
            INTERRUPT_WAIT.wait(None);
            continue;
        }
        cpu::enable_interrupts();
        let fg = TTY.lock().foreground;
        if fg != 0 {
            kprint!("^C\n");
            sched::kill(fg, INTERRUPTED);
        }
    }
}

/// Starts the thread that handles Ctrl+C.
pub fn init() {
    sched::spawn_kernel("tty-intr", interrupt_thread, 0, Priority::High)
        .expect("cannot start the tty thread");
}

pub fn set_mode(mode: u64) -> u64 {
    let mut t = TTY.lock();
    let old = t.mode;
    t.mode = mode & (tty::RAW | tty::NO_ECHO);
    old
}

pub fn set_foreground(pid: u64) {
    TTY.lock().foreground = pid;
}

/// Puts the terminal back in line mode with echo (after a session ends).
pub fn reset() {
    let mut t = TTY.lock();
    t.mode = 0;
    t.foreground = 0;
    t.pending.clear();
}

fn take(max: usize) -> Option<Vec<u8>> {
    let mut t = TTY.lock();
    if t.pending.is_empty() {
        return None;
    }
    let n = max.min(t.pending.len());
    Some(t.pending.drain(..n).collect())
}

/// Bytes a key press produces in raw mode.
pub fn raw_bytes(ev: &KeyEvent) -> Vec<u8> {
    let seq: &[u8] = match ev.key {
        KeyCode::Up => b"\x1b[A",
        KeyCode::Down => b"\x1b[B",
        KeyCode::Right => b"\x1b[C",
        KeyCode::Left => b"\x1b[D",
        KeyCode::Home => b"\x1b[H",
        KeyCode::End => b"\x1b[F",
        KeyCode::Delete => b"\x1b[3~",
        KeyCode::PageUp => b"\x1b[5~",
        KeyCode::PageDown => b"\x1b[6~",
        KeyCode::Backspace => b"\x7f",
        _ => {
            let mut buf = [0u8; 4];
            return match ev.ch {
                Some(c) => Vec::from(c.encode_utf8(&mut buf).as_bytes()),
                None => Vec::new(),
            };
        }
    };
    Vec::from(seq)
}

/// Reads up to `max` bytes, blocking until there is at least one (or
/// the end of input in line mode, which returns an empty vector).
pub fn read(max: usize) -> Vec<u8> {
    if max == 0 {
        return Vec::new();
    }
    if let Some(v) = take(max) {
        return v;
    }
    let mode = TTY.lock().mode;
    if mode & tty::RAW != 0 {
        loop {
            let Some(ev) = input::wait(None) else {
                continue;
            };
            if !ev.pressed {
                continue;
            }
            let bytes = raw_bytes(&ev);
            if bytes.is_empty() {
                continue;
            }
            TTY.lock().pending.extend(bytes);
            // Collect keys that are already queued.
            while let Some(ev) = input::pop() {
                if ev.pressed {
                    TTY.lock().pending.extend(raw_bytes(&ev));
                }
            }
            return take(max).unwrap_or_default();
        }
    }
    let echo = mode & tty::NO_ECHO == 0;
    let mut line = String::new();
    loop {
        let Some(ev) = input::wait(None) else {
            continue;
        };
        if !ev.pressed {
            continue;
        }
        match ev.ch {
            Some('\n') => {
                line.push('\n');
                kprint!("\n");
                break;
            }
            Some('\x08') => {
                if line.pop().is_some() && echo {
                    kprint!("\x08 \x08");
                }
            }
            Some('\x04') if line.is_empty() => return Vec::new(),
            Some('\x03') => {
                kprint!("^C\n");
                line.clear();
                let fg = TTY.lock().foreground;
                if fg != 0 {
                    // Ends the reader too if it is the foreground process.
                    let was = cpu::irq_save();
                    sched::kill(fg, INTERRUPTED);
                    cpu::irq_restore(was);
                }
            }
            Some(c) if c == '\t' || !c.is_control() => {
                line.push(c);
                if echo {
                    kprint!("{}", c);
                }
            }
            _ => {}
        }
    }
    let mut bytes = line.into_bytes();
    if bytes.len() > max {
        TTY.lock().pending.extend(bytes.drain(max..));
    }
    bytes
}

/// Console size in characters.
pub fn size() -> (u64, u64) {
    crate::console::screen_size().map_or((80, 25), |(c, r)| (c as u64, r as u64))
}

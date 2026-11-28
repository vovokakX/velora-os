//! Pseudo-terminals: a terminal drawn by a program (the master side, the
//! Lumi terminal) for programs that expect the console (the slave side:
//! vsh, Plume). The line discipline (line mode, echo, raw mode) is in
//! `velora-tty`; this module adds locking and waiting.
//!
//! - The slave reads typed input (waiting for it) and writes output; it
//!   looks like the console (`fstat` gives a character device), and the
//!   `tty` calls of a process whose descriptor 0 is a slave act on its
//!   pseudo-terminal.
//! - The master writes typed bytes and reads output without waiting
//!   (`EAGAIN` when there is none, 0 once every slave descriptor is
//!   closed). New output wakes a program waiting in `input_wait`.

use crate::arch::x86_64::cpu;
use crate::sched::WaitQueue;
use crate::sync::SpinLock;
use crate::{input, sched};
use alloc::vec::Vec;
use velora_tty::LineDiscipline;

pub struct Pty {
    ld: SpinLock<State>,
    /// The slave waits for input here.
    can_read: WaitQueue,
    /// The slave waits for room for output here.
    can_write: WaitQueue,
}

struct State {
    ld: LineDiscipline,
    master_open: bool,
    slave_open: bool,
}

impl Pty {
    pub fn new(cols: u32, rows: u32) -> Self {
        Self {
            ld: SpinLock::new(State {
                ld: LineDiscipline::new(cols.max(1), rows.max(1)),
                master_open: true,
                slave_open: true,
            }),
            can_read: WaitQueue::new(),
            can_write: WaitQueue::new(),
        }
    }

    /// The last descriptor of one side was closed.
    pub fn close(&self, master: bool) {
        let mut s = self.ld.lock();
        if master {
            s.master_open = false;
        } else {
            s.slave_open = false;
        }
        drop(s);
        self.can_read.wake_all();
        self.can_write.wake_all();
        if !master {
            // The master sees the end of output.
            input::nudge();
        }
    }

    /// Typed bytes from the master. Ctrl+C in line mode ends the
    /// foreground process.
    pub fn master_write(&self, data: &[u8]) -> usize {
        let fx = self.ld.lock().ld.from_master(data);
        self.can_read.wake_all();
        if let Some(pid) = fx.interrupt {
            let was = cpu::irq_save();
            sched::kill(pid, crate::tty::INTERRUPTED);
            cpu::irq_restore(was);
        }
        data.len()
    }

    /// Output for the master: `None` if there is none yet, empty at the
    /// end (the slave side is closed).
    pub fn master_read(&self, max: usize) -> Option<Vec<u8>> {
        let mut s = self.ld.lock();
        let out = s.ld.master_read(max);
        let closed = !s.slave_open;
        drop(s);
        if !out.is_empty() {
            self.can_write.wake_all();
            return Some(out);
        }
        if closed { Some(Vec::new()) } else { None }
    }

    /// Reads typed input, waiting for it; empty at the end of input.
    pub fn slave_read(&self, max: usize) -> Vec<u8> {
        let was = cpu::irq_save();
        let out = loop {
            cpu::disable_interrupts();
            let mut s = self.ld.lock();
            if let Some(v) = s.ld.slave_read(max) {
                break v;
            }
            if !s.master_open {
                break Vec::new();
            }
            drop(s);
            self.can_read.wait(None);
        };
        cpu::irq_restore(was);
        out
    }

    /// Writes program output, waiting while the master is behind. Output
    /// with nobody to read it is dropped.
    pub fn slave_write(&self, data: &[u8]) -> usize {
        let was = cpu::irq_save();
        let mut done = 0;
        while done < data.len() {
            cpu::disable_interrupts();
            let mut s = self.ld.lock();
            if !s.master_open {
                done = data.len();
                break;
            }
            let room = s.ld.output_room();
            if room == 0 {
                drop(s);
                input::nudge();
                self.can_write.wait(None);
                continue;
            }
            let n = room.min(data.len() - done);
            s.ld.from_slave(&data[done..done + n]);
            done += n;
        }
        cpu::irq_restore(was);
        input::nudge();
        done
    }

    pub fn set_mode(&self, mode: u64) -> u64 {
        self.ld.lock().ld.set_mode(mode)
    }

    pub fn set_foreground(&self, pid: u64) {
        self.ld.lock().ld.foreground = pid;
    }

    pub fn size(&self) -> (u64, u64) {
        let s = self.ld.lock();
        (s.ld.cols as u64, s.ld.rows as u64)
    }

    pub fn resize(&self, cols: u32, rows: u32) {
        let mut s = self.ld.lock();
        s.ld.cols = cols.max(1);
        s.ld.rows = rows.max(1);
    }
}

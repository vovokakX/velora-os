//! Pipes: a 64 KiB buffer between a write end and a read end.
//!
//! Reading an empty pipe blocks until data arrives or every write end is
//! closed (then it returns 0, the end of input). Writing to a full pipe
//! blocks until there is room; writing when every read end is closed
//! fails with EPIPE.

use crate::arch::x86_64::cpu;
use crate::sched::WaitQueue;
use crate::sync::SpinLock;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

pub const CAPACITY: usize = 64 * 1024;

struct Inner {
    buf: VecDeque<u8>,
    readers: usize,
    writers: usize,
}

pub struct Pipe {
    inner: SpinLock<Inner>,
    can_read: WaitQueue,
    can_write: WaitQueue,
}

/// Writing failed because nobody reads the pipe any more.
#[derive(Debug, PartialEq, Eq)]
pub struct Broken;

impl Pipe {
    /// A pipe with one read end and one write end open.
    pub fn new() -> Self {
        Self {
            inner: SpinLock::new(Inner {
                buf: VecDeque::new(),
                readers: 1,
                writers: 1,
            }),
            can_read: WaitQueue::new(),
            can_write: WaitQueue::new(),
        }
    }

    /// One end was closed (its last descriptor went away: descriptors
    /// copied by dup and spawn share one open file).
    pub fn close_end(&self, write: bool) {
        let mut i = self.inner.lock();
        if write {
            i.writers -= 1;
        } else {
            i.readers -= 1;
        }
        drop(i);
        self.can_read.wake_all();
        self.can_write.wake_all();
    }

    /// Reads up to `max` bytes; an empty result is the end of input.
    pub fn read(&self, max: usize) -> Vec<u8> {
        let was = cpu::irq_save();
        let out = loop {
            cpu::disable_interrupts();
            let mut i = self.inner.lock();
            if !i.buf.is_empty() {
                let n = max.min(i.buf.len());
                break i.buf.drain(..n).collect();
            }
            if i.writers == 0 || max == 0 {
                break Vec::new();
            }
            drop(i);
            self.can_read.wait(None);
        };
        self.can_write.wake_all();
        cpu::irq_restore(was);
        out
    }

    /// Writes all of `data`, waiting for room as needed. Returns the number
    /// of bytes written; fewer than asked only if the readers went away.
    pub fn write(&self, data: &[u8]) -> Result<usize, Broken> {
        let was = cpu::irq_save();
        let mut done = 0;
        let result = loop {
            cpu::disable_interrupts();
            let mut i = self.inner.lock();
            if i.readers == 0 {
                break if done == 0 { Err(Broken) } else { Ok(done) };
            }
            let room = CAPACITY - i.buf.len();
            if room > 0 {
                let n = room.min(data.len() - done);
                i.buf.extend(&data[done..done + n]);
                done += n;
                drop(i);
                self.can_read.wake_all();
                if done == data.len() {
                    break Ok(done);
                }
                continue;
            }
            drop(i);
            self.can_write.wait(None);
        };
        cpu::irq_restore(was);
        result
    }
}

impl Default for Pipe {
    fn default() -> Self {
        Self::new()
    }
}

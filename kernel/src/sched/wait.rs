//! Wait queues: threads block until an event (for example keyboard input)
//! wakes them.

use super::{Tid, block_current, current_tid, wake};
use crate::sync::SpinLock;
use alloc::vec::Vec;

pub struct WaitQueue {
    waiters: SpinLock<Vec<Tid>>,
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            waiters: SpinLock::new(Vec::new()),
        }
    }

    /// Blocks the current thread until `wake_all` or the deadline tick.
    /// The caller must have interrupts disabled and must re-check its
    /// condition after this returns.
    pub fn wait(&self, deadline: Option<u64>) {
        let me = current_tid();
        self.waiters.lock().push(me);
        block_current(deadline);
        self.waiters.lock().retain(|&t| t != me);
    }

    /// Wakes every waiting thread.
    pub fn wake_all(&self) {
        let waiters = core::mem::take(&mut *self.waiters.lock());
        for t in waiters {
            wake(t);
        }
    }
}

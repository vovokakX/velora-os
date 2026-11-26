//! Kernel input event queue. Drivers push key events from interrupt
//! handlers; consumers (the console today, `/dev/input` and Lumi later)
//! pop them.

use crate::arch::cpu;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};
use velora_input::KeyEvent;

const CAPACITY: usize = 128;

struct Queue {
    buf: [Option<KeyEvent>; CAPACITY],
    head: usize,
    len: usize,
}

static QUEUE: SpinLock<Queue> = SpinLock::new(Queue {
    buf: [None; CAPACITY],
    head: 0,
    len: 0,
});
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Adds an event. When the queue is full the event is dropped and counted.
pub fn push(ev: KeyEvent) {
    let mut q = QUEUE.lock();
    if q.len == CAPACITY {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let tail = (q.head + q.len) % CAPACITY;
    q.buf[tail] = Some(ev);
    q.len += 1;
}

pub fn pop() -> Option<KeyEvent> {
    let mut q = QUEUE.lock();
    if q.len == 0 {
        return None;
    }
    let head = q.head;
    let ev = q.buf[head].take();
    q.head = (q.head + 1) % CAPACITY;
    q.len -= 1;
    ev
}

pub fn dropped() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

/// Waits for the next event, halting the CPU in between. Returns `None`
/// if `timeout_ms` passes first (`None` timeout waits forever).
pub fn wait(timeout_ms: Option<u64>) -> Option<KeyEvent> {
    let deadline = timeout_ms.map(|ms| cpu::rdtsc() + crate::time::tsc_hz() * ms / 1000);
    loop {
        // Check with interrupts off, then sti+hlt atomically, so an event
        // arriving between the check and the halt still wakes us.
        cpu::disable_interrupts();
        if let Some(ev) = pop() {
            cpu::enable_interrupts();
            return Some(ev);
        }
        if deadline.is_some_and(|d| cpu::rdtsc() >= d) {
            cpu::enable_interrupts();
            return None;
        }
        cpu::enable_interrupts_and_halt();
    }
}

//! Kernel input event queue. Drivers push key events from interrupt
//! handlers; consumers (the console today, `/dev/input` and Lumi later)
//! pop them.

use crate::arch::cpu;
use crate::sched::{self, WaitQueue};
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
static READERS: WaitQueue = WaitQueue::new();

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
    drop(q);
    if sched::is_running() {
        READERS.wake_all();
    }
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

/// Waits for the next event. Returns `None` if `timeout_ms` passes first
/// (`None` waits forever). With the scheduler running the thread blocks;
/// before that the CPU halts between interrupts.
pub fn wait(timeout_ms: Option<u64>) -> Option<KeyEvent> {
    let deadline_tsc = timeout_ms.map(|ms| cpu::rdtsc() + crate::time::tsc_hz() * ms / 1000);
    let deadline_tick =
        timeout_ms.map(|ms| crate::time::ticks() + (ms * crate::time::TICK_HZ).div_ceil(1000) + 1);
    let was = cpu::irq_save();
    let result = loop {
        // Check with interrupts off, then block (or sti+hlt atomically),
        // so an event arriving in between still wakes us.
        cpu::disable_interrupts();
        if let Some(ev) = pop() {
            break Some(ev);
        }
        if deadline_tsc.is_some_and(|d| cpu::rdtsc() >= d) {
            break None;
        }
        if sched::is_running() {
            READERS.wait(deadline_tick);
        } else {
            cpu::enable_interrupts_and_halt();
        }
    };
    cpu::irq_restore(was);
    result
}

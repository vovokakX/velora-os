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
/// Programs waiting for any input (`input_wait`).
static ANY: WaitQueue = WaitQueue::new();

/// Adds an event. When the queue is full the event is dropped and counted.
pub fn push(ev: KeyEvent) {
    if crate::tty::intercept(&ev) {
        return;
    }
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
        ANY.wake_all();
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

/// A key event as a `/dev/keyboard` packet (see `velora_abi::key`).
pub fn key_packet(ev: &KeyEvent) -> [u8; velora_abi::key::PACKET] {
    use velora_abi::key;
    use velora_input::KeyCode as K;
    let code = match ev.key {
        K::Escape => key::ESCAPE,
        K::Tab => key::TAB,
        K::Enter | K::KeypadEnter => key::ENTER,
        K::Backspace => key::BACKSPACE,
        K::Delete => key::DELETE,
        K::Left => key::LEFT,
        K::Right => key::RIGHT,
        K::Up => key::UP,
        K::Down => key::DOWN,
        K::Home => key::HOME,
        K::End => key::END,
        K::PageUp => key::PAGE_UP,
        K::PageDown => key::PAGE_DOWN,
        K::Space => key::SPACE,
        K::F1 => key::F1,
        K::F2 => key::F1 + 1,
        K::F3 => key::F1 + 2,
        K::F4 => key::F1 + 3,
        K::F5 => key::F1 + 4,
        K::F6 => key::F1 + 5,
        K::F7 => key::F1 + 6,
        K::F8 => key::F1 + 7,
        K::F9 => key::F1 + 8,
        K::F10 => key::F1 + 9,
        K::F11 => key::F1 + 10,
        K::F12 => key::F12,
        K::LeftShift | K::RightShift => key::SHIFT_KEY,
        K::LeftCtrl | K::RightCtrl => key::CTRL_KEY,
        K::LeftAlt | K::RightAlt => key::ALT_KEY,
        K::LeftMeta | K::RightMeta => key::SUPER_KEY,
        K::Insert => key::INSERT,
        _ if ev.ch.is_some_and(|c| !c.is_control()) => key::CHAR,
        _ if ctrl_letter(ev).is_some() => key::CHAR,
        _ => key::OTHER,
    };
    let mut flags = 0;
    if ev.pressed {
        flags |= key::PRESSED;
    }
    if ev.mods.shift {
        flags |= key::SHIFT;
    }
    if ev.mods.ctrl {
        flags |= key::CTRL;
    }
    if ev.mods.alt {
        flags |= key::ALT;
    }
    if ev.mods.meta {
        flags |= key::SUPER;
    }
    let ch = ev
        .ch
        .filter(|c| !c.is_control())
        .or_else(|| ctrl_letter(ev))
        .map_or(0, |c| c as u32);
    let c = code.to_le_bytes();
    let ch = ch.to_le_bytes();
    [c[0], c[1], flags, 0, ch[0], ch[1], ch[2], ch[3]]
}

/// The letter of Ctrl + a letter key (which types a control character on
/// the console), for graphical programs: Ctrl+C is 'c'.
fn ctrl_letter(ev: &KeyEvent) -> Option<char> {
    match ev.ch {
        Some(c @ ''..='') if ev.mods.ctrl => {
            char::from_u32(c as u32 - 1 + 'a' as u32)
        }
        _ => None,
    }
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

// --- mouse ------------------------------------------------------------------

/// One mouse movement or button change. Read from `/dev/mouse` as 4-byte
/// packets (buttons, dx, dy, wheel; relative events only) and from
/// `/dev/pointer` as 8-byte packets ([`packet`]).
pub use velora_input::mouse::MouseEvent;

/// The `/dev/pointer` packet of `ev` (`velora_abi::pointer`).
pub fn packet(ev: &MouseEvent) -> [u8; velora_abi::pointer::PACKET] {
    let (flags, x, y) = match ev.abs {
        Some((x, y)) => (velora_abi::pointer::ABSOLUTE, x as i16, y as i16),
        None => (0, ev.dx as i16, ev.dy as i16),
    };
    let (x, y) = (x.to_le_bytes(), y.to_le_bytes());
    [flags, ev.buttons, ev.wheel as u8, 0, x[0], x[1], y[0], y[1]]
}

const MOUSE_CAPACITY: usize = 64;

struct MouseQueue {
    buf: [MouseEvent; MOUSE_CAPACITY],
    head: usize,
    len: usize,
}

static MOUSE: SpinLock<MouseQueue> = SpinLock::new(MouseQueue {
    buf: [MouseEvent {
        buttons: 0,
        dx: 0,
        dy: 0,
        wheel: 0,
        abs: None,
    }; MOUSE_CAPACITY],
    head: 0,
    len: 0,
});
static MOUSE_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Adds a mouse event. A movement joins the last queued event when no
/// information is lost (see [`MouseEvent::merge`]), so a program that
/// reads late still gets the whole movement; the oldest event is dropped
/// only when the queue is full of events that cannot be joined.
pub fn push_mouse(ev: MouseEvent) {
    MOUSE_TOTAL.fetch_add(1, Ordering::Relaxed);
    let mut q = MOUSE.lock();
    if q.len > 0 {
        let last = (q.head + q.len - 1) % MOUSE_CAPACITY;
        if q.buf[last].merge(&ev) {
            drop(q);
            if sched::is_running() {
                ANY.wake_all();
            }
            return;
        }
    }
    if q.len == MOUSE_CAPACITY {
        q.head = (q.head + 1) % MOUSE_CAPACITY;
        q.len -= 1;
    }
    let tail = (q.head + q.len) % MOUSE_CAPACITY;
    q.buf[tail] = ev;
    q.len += 1;
    drop(q);
    if sched::is_running() {
        ANY.wake_all();
    }
}

/// Waits until a key or mouse event is queued or `timeout_ms` passes;
/// true if one is waiting. For graphical programs, which read the devices
/// without waiting and must also keep drawing.
pub fn wait_any(timeout_ms: u64) -> bool {
    let deadline_tsc = cpu::rdtsc() + crate::time::tsc_hz() * timeout_ms / 1000;
    // The next tick at the earliest (as `sleep_ms`); the TSC decides when
    // the time is really up.
    let deadline_tick =
        crate::time::ticks() + (timeout_ms * crate::time::TICK_HZ).div_ceil(1000).max(1);
    let was = cpu::irq_save();
    let result = loop {
        cpu::disable_interrupts();
        if QUEUE.lock().len > 0 || MOUSE.lock().len > 0 {
            break true;
        }
        if cpu::rdtsc() >= deadline_tsc || !sched::is_running() {
            break false;
        }
        ANY.wait(Some(deadline_tick));
    };
    cpu::irq_restore(was);
    result
}

pub fn pop_mouse() -> Option<MouseEvent> {
    let mut q = MOUSE.lock();
    if q.len == 0 {
        return None;
    }
    let ev = q.buf[q.head];
    q.head = (q.head + 1) % MOUSE_CAPACITY;
    q.len -= 1;
    Some(ev)
}

/// Mouse events received since boot.
pub fn mouse_events() -> u64 {
    MOUSE_TOTAL.load(Ordering::Relaxed)
}

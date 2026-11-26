//! Hardware interrupt dispatch: a handler table indexed by vector, plus
//! spurious interrupt handling and EOI.
//!
//! Vector plan: 0x00-0x1f exceptions, 0x20 APIC timer, 0x21 keyboard,
//! 0x30-0x6f device IRQs (later), 0x70-0x7f masked legacy PICs,
//! 0xfe APIC error, 0xff APIC spurious.

use super::apic;
use super::idt::InterruptFrame;
use super::pic;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub type Handler = fn(&mut InterruptFrame);

pub const VECTOR_KEYBOARD: u8 = 0x21;

static HANDLERS: [AtomicUsize; 256] = [const { AtomicUsize::new(0) }; 256];
static SPURIOUS: AtomicU64 = AtomicU64::new(0);
static UNHANDLED: AtomicU64 = AtomicU64::new(0);
static APIC_ERRORS: AtomicU64 = AtomicU64::new(0);

/// Installs `handler` for `vector` (32..=253). Replaces any previous one.
pub fn set_handler(vector: u8, handler: Handler) {
    assert!(
        (32..apic::VECTOR_ERROR).contains(&vector),
        "vector {} is reserved",
        vector
    );
    HANDLERS[vector as usize].store(handler as usize, Ordering::SeqCst);
}

pub fn clear_handler(vector: u8) {
    HANDLERS[vector as usize].store(0, Ordering::SeqCst);
}

/// Called from `interrupt_dispatch` for vectors 32..=255.
pub fn dispatch(frame: &mut InterruptFrame) {
    let vector = frame.vector as u8;
    if vector == apic::VECTOR_SPURIOUS
        || (pic::VECTOR_BASE..pic::VECTOR_BASE + 16).contains(&vector)
    {
        // Spurious interrupts must not be acknowledged.
        SPURIOUS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if vector == apic::VECTOR_ERROR {
        APIC_ERRORS.fetch_add(1, Ordering::Relaxed);
        apic::eoi();
        return;
    }
    let h = HANDLERS[vector as usize].load(Ordering::Acquire);
    if h == 0 {
        if UNHANDLED.fetch_add(1, Ordering::Relaxed) < 8 {
            crate::kwarn!("unhandled interrupt vector {:#x}", vector);
        }
    } else {
        // SAFETY: only `Handler` values are stored in the table.
        let f: Handler = unsafe { core::mem::transmute::<usize, Handler>(h) };
        f(frame);
    }
    apic::eoi();
}

pub struct IrqStats {
    pub spurious: u64,
    pub unhandled: u64,
    pub apic_errors: u64,
}

pub fn stats() -> IrqStats {
    IrqStats {
        spurious: SPURIOUS.load(Ordering::Relaxed),
        unhandled: UNHANDLED.load(Ordering::Relaxed),
        apic_errors: APIC_ERRORS.load(Ordering::Relaxed),
    }
}

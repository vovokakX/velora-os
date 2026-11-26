//! Kernel stacks with an unmapped guard page below each one, so an
//! overflow faults instead of silently overwriting other memory.
//!
//! The stack window is split into fixed slots (guard page + up to
//! `MAX_PAGES` pages). Freed slots are reused.

use super::paging::{self, Kind};
use super::{PAGE_SIZE, frame};
use crate::sync::SpinLock;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

pub const STACKS_START: u64 = 0xffff_e000_0000_0000;
pub const STACKS_END: u64 = 0xffff_e100_0000_0000;
/// Largest stack a slot can hold (64 KiB).
pub const MAX_PAGES: u64 = 16;
const SLOT: u64 = (MAX_PAGES + 1) * PAGE_SIZE;

static NEXT: AtomicU64 = AtomicU64::new(STACKS_START);
static FREE_SLOTS: SpinLock<Vec<u64>> = SpinLock::new(Vec::new());
static MAPPED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
pub struct KernelStack {
    /// Lowest mapped address.
    pub bottom: u64,
    /// One past the highest address; the initial stack pointer.
    pub top: u64,
    /// Address of the unmapped guard page.
    pub guard: u64,
}

impl KernelStack {
    pub fn pages(&self) -> u64 {
        (self.top - self.bottom) / PAGE_SIZE
    }
}

/// Allocates a stack of `pages` pages (at most `MAX_PAGES`).
pub fn alloc(pages: u64) -> Option<KernelStack> {
    assert!((1..=MAX_PAGES).contains(&pages), "bad kernel stack size");
    {
        let mut cache = CACHE.lock();
        if let Some(i) = cache.iter().position(|st| st.pages() == pages) {
            return Some(cache.swap_remove(i));
        }
    }
    let slot = match FREE_SLOTS.lock().pop() {
        Some(s) => s,
        None => {
            let s = NEXT.fetch_add(SLOT, Ordering::SeqCst);
            if s + SLOT > STACKS_END {
                return None;
            }
            s
        }
    };
    // The stack sits at the top of the slot; the guard page right below it.
    let top = slot + SLOT;
    let bottom = top - pages * PAGE_SIZE;
    for i in 0..pages {
        let mapped = frame::alloc_zeroed().and_then(|phys| {
            paging::map(bottom + i * PAGE_SIZE, phys, Kind::Data)
                .map_err(|_| frame::free(phys))
                .ok()
        });
        if mapped.is_none() {
            release(slot, bottom, bottom + i * PAGE_SIZE);
            return None;
        }
    }
    MAPPED.fetch_add(pages, Ordering::Relaxed);
    Some(KernelStack {
        bottom,
        top,
        guard: bottom - PAGE_SIZE,
    })
}

fn release(slot: u64, from: u64, to: u64) {
    let mut v = from;
    while v < to {
        if let Some(phys) = paging::unmap(v) {
            frame::free(phys);
        }
        v += PAGE_SIZE;
    }
    FREE_SLOTS.lock().push(slot);
}

/// Frees a stack. Nothing may run on it any more.
///
/// The stack stays mapped and goes to a cache for the next `alloc` of the
/// same size. Another CPU may still have its pages in its TLB (kernel
/// pages are global); as long as the mapping never changes, such entries
/// stay correct and no TLB shootdown is needed (D43).
pub fn free(stack: KernelStack) {
    CACHE.lock().push(stack);
}

/// Stacks freed and kept mapped for reuse.
static CACHE: SpinLock<Vec<KernelStack>> = SpinLock::new(Vec::new());

/// Stacks waiting in the reuse cache.
pub fn cached() -> usize {
    CACHE.lock().len()
}

pub fn mapped_pages() -> u64 {
    MAPPED.load(Ordering::Relaxed)
}

/// Switches to `stack` and calls `entry`, which must never return.
///
/// # Safety
/// Nothing on the old stack may be used afterwards.
pub unsafe fn switch_to(stack: &KernelStack, entry: extern "C" fn() -> !) -> ! {
    // RBP = 0 ends backtraces at the new stack's first frame.
    unsafe {
        core::arch::asm!(
            "mov rsp, {top}",
            "xor ebp, ebp",
            "call {entry}",
            "ud2",
            top = in(reg) stack.top,
            entry = in(reg) entry,
            options(noreturn)
        )
    }
}

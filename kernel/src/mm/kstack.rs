//! Kernel stacks with an unmapped guard page below each one, so an
//! overflow faults instead of silently overwriting other memory.

use super::paging::{self, Kind};
use super::{PAGE_SIZE, frame};
use core::sync::atomic::{AtomicU64, Ordering};

pub const STACKS_START: u64 = 0xffff_e000_0000_0000;
pub const STACKS_END: u64 = 0xffff_e100_0000_0000;

static NEXT: AtomicU64 = AtomicU64::new(STACKS_START);
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

/// Allocates a stack of `pages` pages. Virtual space is never reused for
/// now; freeing stacks comes with threads in stage 4.
pub fn alloc(pages: u64) -> Option<KernelStack> {
    let span = (pages + 1) * PAGE_SIZE;
    let guard = NEXT.fetch_add(span, Ordering::SeqCst);
    if guard + span > STACKS_END {
        return None;
    }
    let bottom = guard + PAGE_SIZE;
    for i in 0..pages {
        let phys = frame::alloc_zeroed()?;
        paging::map(bottom + i * PAGE_SIZE, phys, Kind::Data).ok()?;
    }
    MAPPED.fetch_add(pages, Ordering::Relaxed);
    Some(KernelStack {
        bottom,
        top: bottom + pages * PAGE_SIZE,
        guard,
    })
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

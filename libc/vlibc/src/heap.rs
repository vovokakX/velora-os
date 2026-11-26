//! The program heap: the same allocator as the kernel heap
//! (`velora_mm::heap`), growing with fixed `mmap` calls from `HEAP_BASE`.

use crate::mem::{map, mmap, prot};
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::NonNull;
use velora_mm::PAGE_SIZE;
use velora_mm::heap::{Heap, HeapStats, PageSource};

/// Start of the heap window in every program.
pub const HEAP_BASE: u64 = 0x0000_1000_0000_0000;
const HEAP_END: u64 = 0x0000_1800_0000_0000;

struct MmapPages {
    next: u64,
}

impl PageSource for MmapPages {
    fn grow(&mut self, pages: usize) -> Option<NonNull<u8>> {
        let len = pages * PAGE_SIZE;
        if self.next + len as u64 > HEAP_END {
            return None;
        }
        let p = mmap(self.next, len, prot::READ | prot::WRITE, map::FIXED).ok()?;
        self.next += len as u64;
        NonNull::new(p)
    }
}

struct Allocator(UnsafeCell<Heap<MmapPages>>);

// SAFETY: programs are single-threaded until threads exist in user space.
unsafe impl Sync for Allocator {}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator(UnsafeCell::new(Heap::new(MmapPages { next: HEAP_BASE })));

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: single-threaded access.
        unsafe { (*self.0.get()).alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: single-threaded access; `ptr` came from `alloc`.
        unsafe { (*self.0.get()).dealloc(ptr, layout) }
    }
}

pub fn stats() -> HeapStats {
    // SAFETY: single-threaded access.
    unsafe { (*ALLOCATOR.0.get()).stats() }
}

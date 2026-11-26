//! Kernel heap: `velora_mm::heap::Heap` on top of a virtual region that is
//! backed by frames on demand. Enables `alloc` (Box, Vec, String, ...).

use super::paging::{self, Kind};
use super::{PAGE_SIZE, frame};
use crate::sync::SpinLock;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use velora_mm::heap::{Heap, HeapStats, PageSource};

pub const HEAP_START: u64 = 0xffff_c000_0000_0000;
/// Upper bound of the heap window (see docs/memory-map.md).
pub const HEAP_MAX: u64 = 0xffff_c100_0000_0000;

pub struct KernelPages {
    next: u64,
}

impl PageSource for KernelPages {
    fn grow(&mut self, pages: usize) -> Option<NonNull<u8>> {
        let start = self.next;
        let end = start + pages as u64 * PAGE_SIZE;
        if end > HEAP_MAX {
            return None;
        }
        let mut v = start;
        while v < end {
            let Some(phys) = frame::alloc() else {
                self.rollback(start, v);
                return None;
            };
            if paging::map(v, phys, Kind::Data).is_err() {
                frame::free(phys);
                self.rollback(start, v);
                return None;
            }
            v += PAGE_SIZE;
        }
        self.next = end;
        NonNull::new(start as *mut u8)
    }
}

impl KernelPages {
    fn rollback(&mut self, from: u64, to: u64) {
        let mut v = from;
        while v < to {
            if let Some(phys) = paging::unmap(v) {
                frame::free(phys);
            }
            v += PAGE_SIZE;
        }
    }
}

struct KernelAllocator {
    heap: SpinLock<Heap<KernelPages>>,
    ready: AtomicBool,
}

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator {
    heap: SpinLock::new(Heap::new(KernelPages { next: HEAP_START })),
    ready: AtomicBool::new(false),
};

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !self.ready.load(Ordering::Acquire) {
            return core::ptr::null_mut();
        }
        self.heap.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` with the same layout.
        unsafe { self.heap.lock().dealloc(ptr, layout) }
    }
}

pub fn init() {
    ALLOCATOR.ready.store(true, Ordering::Release);
}

pub fn stats() -> HeapStats {
    ALLOCATOR.heap.lock().stats()
}

/// (number of free page runs, pages in them).
pub fn free_runs() -> (usize, usize) {
    ALLOCATOR.heap.lock().free_runs()
}

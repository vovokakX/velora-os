//! Kernel heap allocator.
//!
//! - Requests up to 2048 bytes use size classes (16, 32, ..., 2048). Each
//!   class has a free list; an empty class is refilled by splitting one
//!   page. Blocks are naturally aligned to their size, so any alignment up
//!   to the class size is satisfied.
//! - Larger requests take whole pages from a list of free page runs
//!   (first fit, sorted by address, neighbours merged on free).
//! - When nothing fits, the heap asks its [`PageSource`] for more pages at
//!   the end of the heap region.
//!
//! Pages that were split for a size class stay with that class. This keeps
//! the code simple; it costs at most a few pages per class.

use crate::PAGE_SIZE;
use core::alloc::Layout;
use core::ptr::{self, NonNull};

const MIN_CLASS_SHIFT: u32 = 4; // 16 bytes
const MAX_CLASS_SHIFT: u32 = 11; // 2048 bytes
const CLASSES: usize = (MAX_CLASS_SHIFT - MIN_CLASS_SHIFT + 1) as usize;

/// Provides contiguous, page-aligned memory at the end of the heap region.
pub trait PageSource {
    /// Maps `pages` more pages and returns the start of the new range, or
    /// `None` if memory is exhausted. Consecutive calls return adjacent
    /// ranges.
    fn grow(&mut self, pages: usize) -> Option<NonNull<u8>>;
}

struct FreeBlock {
    next: *mut FreeBlock,
}

struct FreeRun {
    pages: usize,
    next: *mut FreeRun,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeapStats {
    /// Pages obtained from the page source.
    pub pages_mapped: usize,
    /// Bytes handed out (rounded up to class size or whole pages).
    pub bytes_in_use: usize,
    pub allocations: usize,
}

pub struct Heap<S: PageSource> {
    source: S,
    classes: [*mut FreeBlock; CLASSES],
    runs: *mut FreeRun,
    stats: HeapStats,
}

// SAFETY: the heap owns the memory its pointers refer to; callers wrap it in
// a lock.
unsafe impl<S: PageSource + Send> Send for Heap<S> {}

fn class_of(size: usize) -> Option<usize> {
    if size > 1 << MAX_CLASS_SHIFT {
        return None;
    }
    let shift = size
        .max(1 << MIN_CLASS_SHIFT)
        .next_power_of_two()
        .trailing_zeros();
    Some((shift - MIN_CLASS_SHIFT) as usize)
}

fn class_size(class: usize) -> usize {
    1 << (class as u32 + MIN_CLASS_SHIFT)
}

impl<S: PageSource> Heap<S> {
    pub const fn new(source: S) -> Self {
        Self {
            source,
            classes: [ptr::null_mut(); CLASSES],
            runs: ptr::null_mut(),
            stats: HeapStats {
                pages_mapped: 0,
                bytes_in_use: 0,
                allocations: 0,
            },
        }
    }

    pub fn stats(&self) -> HeapStats {
        self.stats
    }

    pub fn source(&self) -> &S {
        &self.source
    }

    /// Size and kind actually used for `layout`: `Ok(class)` or `Err(pages)`.
    fn plan(layout: Layout) -> Option<Result<usize, usize>> {
        let size = layout.size().max(layout.align());
        if let Some(c) = class_of(size) {
            return Some(Ok(c));
        }
        if layout.align() > PAGE_SIZE {
            return None; // not supported; no kernel user needs it
        }
        Some(Err(size.div_ceil(PAGE_SIZE)))
    }

    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        match Self::plan(layout) {
            Some(Ok(c)) => {
                if self.classes[c].is_null() && !self.refill(c) {
                    return ptr::null_mut();
                }
                let b = self.classes[c];
                // SAFETY: `b` is a free block owned by this heap.
                self.classes[c] = unsafe { (*b).next };
                self.stats.bytes_in_use += class_size(c);
                self.stats.allocations += 1;
                b.cast()
            }
            Some(Err(pages)) => match self.alloc_pages(pages) {
                Some(p) => {
                    self.stats.bytes_in_use += pages * PAGE_SIZE;
                    self.stats.allocations += 1;
                    p
                }
                None => ptr::null_mut(),
            },
            None => ptr::null_mut(),
        }
    }

    /// # Safety
    /// `p` must come from `alloc` on this heap with the same `layout`.
    pub unsafe fn dealloc(&mut self, p: *mut u8, layout: Layout) {
        match Self::plan(layout) {
            Some(Ok(c)) => {
                let b = p.cast::<FreeBlock>();
                // SAFETY: the block is at least 16 bytes and now free.
                unsafe { (*b).next = self.classes[c] };
                self.classes[c] = b;
                self.stats.bytes_in_use -= class_size(c);
            }
            Some(Err(pages)) => {
                // SAFETY: caller guarantees the run came from this heap.
                unsafe { self.free_pages(p, pages) };
                self.stats.bytes_in_use -= pages * PAGE_SIZE;
            }
            None => return,
        }
        self.stats.allocations -= 1;
    }

    fn refill(&mut self, c: usize) -> bool {
        let Some(page) = self.alloc_pages(1) else {
            return false;
        };
        let size = class_size(c);
        let count = PAGE_SIZE / size;
        // Link the blocks in address order.
        for i in (0..count).rev() {
            // SAFETY: inside the page just obtained.
            unsafe {
                let b = page.add(i * size).cast::<FreeBlock>();
                (*b).next = self.classes[c];
                self.classes[c] = b;
            }
        }
        true
    }

    fn alloc_pages(&mut self, pages: usize) -> Option<*mut u8> {
        // First fit in the free runs.
        let mut prev: *mut *mut FreeRun = &mut self.runs;
        // SAFETY: the list only contains runs owned by this heap.
        unsafe {
            while !(*prev).is_null() {
                let run = *prev;
                if (*run).pages >= pages {
                    let rest = (*run).pages - pages;
                    if rest == 0 {
                        *prev = (*run).next;
                    } else {
                        // Keep the tail in the list; hand out the head.
                        let tail = run.cast::<u8>().add(pages * PAGE_SIZE).cast::<FreeRun>();
                        (*tail).pages = rest;
                        (*tail).next = (*run).next;
                        *prev = tail;
                    }
                    return Some(run.cast());
                }
                prev = &mut (*run).next;
            }
        }
        let p = self.source.grow(pages)?;
        self.stats.pages_mapped += pages;
        Some(p.as_ptr())
    }

    unsafe fn free_pages(&mut self, p: *mut u8, pages: usize) {
        let new = p.cast::<FreeRun>();
        // SAFETY: walks and edits the run list owned by this heap.
        unsafe {
            (*new).pages = pages;
            let mut prev: *mut FreeRun = ptr::null_mut();
            let mut cur = self.runs;
            while !cur.is_null() && (cur as usize) < (new as usize) {
                prev = cur;
                cur = (*cur).next;
            }
            (*new).next = cur;
            if prev.is_null() {
                self.runs = new;
            } else {
                (*prev).next = new;
            }
            // Merge with the following run.
            if !cur.is_null() && new as usize + (*new).pages * PAGE_SIZE == cur as usize {
                (*new).pages += (*cur).pages;
                (*new).next = (*cur).next;
            }
            // Merge with the preceding run.
            if !prev.is_null() && prev as usize + (*prev).pages * PAGE_SIZE == new as usize {
                (*prev).pages += (*new).pages;
                (*prev).next = (*new).next;
            }
        }
    }

    /// Number of free page runs and the pages in them (for diagnostics).
    pub fn free_runs(&self) -> (usize, usize) {
        let mut n = 0;
        let mut pages = 0;
        let mut cur = self.runs;
        while !cur.is_null() {
            n += 1;
            // SAFETY: list of runs owned by this heap.
            unsafe {
                pages += (*cur).pages;
                cur = (*cur).next;
            }
        }
        (n, pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{alloc_zeroed, dealloc as sys_dealloc};

    struct Arena {
        base: *mut u8,
        used: usize,
        limit: usize,
    }

    impl Arena {
        fn new(pages: usize) -> Self {
            let l = Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap();
            Self {
                base: unsafe { alloc_zeroed(l) },
                used: 0,
                limit: pages,
            }
        }
    }

    impl Drop for Arena {
        fn drop(&mut self) {
            let l = Layout::from_size_align(self.limit * PAGE_SIZE, PAGE_SIZE).unwrap();
            unsafe { sys_dealloc(self.base, l) }
        }
    }

    impl PageSource for Arena {
        fn grow(&mut self, pages: usize) -> Option<NonNull<u8>> {
            if self.used + pages > self.limit {
                return None;
            }
            let p = unsafe { self.base.add(self.used * PAGE_SIZE) };
            self.used += pages;
            NonNull::new(p)
        }
    }

    fn lay(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn classes() {
        assert_eq!(class_of(1), Some(0));
        assert_eq!(class_of(16), Some(0));
        assert_eq!(class_of(17), Some(1));
        assert_eq!(class_of(2048), Some(7));
        assert_eq!(class_of(2049), None);
    }

    #[test]
    fn small_allocations_are_aligned_and_distinct() {
        let mut h = Heap::new(Arena::new(512));
        let mut ptrs = Vec::new();
        for i in 0..500 {
            let size = 1 + (i * 37) % 2048;
            let align = 1 << (i % 6);
            let p = h.alloc(lay(size, align));
            assert!(!p.is_null());
            assert_eq!(p as usize % align, 0);
            unsafe { ptr::write_bytes(p, i as u8, size) };
            ptrs.push((p, size, align, i as u8));
        }
        for &(p, size, _, v) in &ptrs {
            let s = unsafe { core::slice::from_raw_parts(p, size) };
            assert!(s.iter().all(|&b| b == v), "block was overwritten");
        }
        for (p, size, align, _) in ptrs {
            unsafe { h.dealloc(p, lay(size, align)) };
        }
        assert_eq!(h.stats().bytes_in_use, 0);
        assert_eq!(h.stats().allocations, 0);
    }

    #[test]
    fn freed_blocks_are_reused() {
        let mut h = Heap::new(Arena::new(8));
        let a = h.alloc(lay(64, 8));
        unsafe { h.dealloc(a, lay(64, 8)) };
        let b = h.alloc(lay(60, 4));
        assert_eq!(a, b);
    }

    #[test]
    fn large_runs_merge_after_free() {
        let mut h = Heap::new(Arena::new(32));
        let a = h.alloc(lay(3 * PAGE_SIZE, 8));
        let b = h.alloc(lay(5 * PAGE_SIZE, 8));
        let c = h.alloc(lay(2 * PAGE_SIZE, PAGE_SIZE));
        assert_eq!(h.stats().pages_mapped, 10);
        unsafe {
            h.dealloc(a, lay(3 * PAGE_SIZE, 8));
            h.dealloc(c, lay(2 * PAGE_SIZE, PAGE_SIZE));
            h.dealloc(b, lay(5 * PAGE_SIZE, 8));
        }
        assert_eq!(h.free_runs(), (1, 10));
        // The merged run is reused without growing the heap.
        let d = h.alloc(lay(10 * PAGE_SIZE, 8));
        assert_eq!(d, a);
        assert_eq!(h.stats().pages_mapped, 10);
    }

    #[test]
    fn run_is_split_and_tail_kept() {
        let mut h = Heap::new(Arena::new(16));
        let a = h.alloc(lay(8 * PAGE_SIZE, 8));
        unsafe { h.dealloc(a, lay(8 * PAGE_SIZE, 8)) };
        let b = h.alloc(lay(3 * PAGE_SIZE, 8));
        assert_eq!(b, a);
        assert_eq!(h.free_runs(), (1, 5));
    }

    #[test]
    fn exhaustion_returns_null() {
        let mut h = Heap::new(Arena::new(4));
        assert!(h.alloc(lay(5 * PAGE_SIZE, 8)).is_null());
        assert!(!h.alloc(lay(4 * PAGE_SIZE, 8)).is_null());
        assert!(h.alloc(lay(16, 8)).is_null());
        assert!(h.alloc(lay(8192, 8192)).is_null()); // alignment > page: unsupported
    }

    #[test]
    fn randomized_stress_keeps_contents() {
        let mut h = Heap::new(Arena::new(2048));
        let mut live: Vec<(*mut u8, Layout, u8)> = Vec::new();
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for step in 0..20_000 {
            if live.len() > 300 || (rnd() % 3 == 0 && !live.is_empty()) {
                let i = rnd() as usize % live.len();
                let (p, l, v) = live.swap_remove(i);
                let s = unsafe { core::slice::from_raw_parts(p, l.size()) };
                assert!(s.iter().all(|&b| b == v), "corruption at step {step}");
                unsafe { h.dealloc(p, l) };
            } else {
                let size = match rnd() % 10 {
                    0 => 4096 + rnd() as usize % 20000,
                    _ => 1 + rnd() as usize % 600,
                };
                let l = lay(size, 1 << (rnd() % 5));
                let p = h.alloc(l);
                assert!(!p.is_null());
                let v = (step % 251) as u8;
                unsafe { ptr::write_bytes(p, v, size) };
                live.push((p, l, v));
            }
        }
        for (p, l, _) in live {
            unsafe { h.dealloc(p, l) };
        }
        assert_eq!(h.stats().bytes_in_use, 0);
    }
}

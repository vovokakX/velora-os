//! Physically contiguous, zeroed memory for device DMA, reached through
//! the HHDM. Freed when dropped.

use crate::mm::{PAGE_SIZE, frame, phys_to_virt};

pub struct DmaBuffer {
    phys: u64,
    pages: usize,
}

// SAFETY: the buffer is plain memory owned by one driver.
unsafe impl Send for DmaBuffer {}
unsafe impl Sync for DmaBuffer {}

impl DmaBuffer {
    /// Allocates `bytes` (rounded up to pages), aligned to a page.
    pub fn new(bytes: usize) -> Option<Self> {
        let pages = bytes.div_ceil(PAGE_SIZE as usize).max(1);
        let phys = frame::alloc_contiguous(pages, 1)?;
        let b = Self { phys, pages };
        // SAFETY: freshly allocated frames, mapped in the HHDM.
        unsafe { core::ptr::write_bytes(b.virt() as *mut u8, 0, b.len()) };
        Some(b)
    }

    pub fn phys(&self) -> u64 {
        self.phys
    }

    pub fn virt(&self) -> u64 {
        phys_to_virt(self.phys)
    }

    pub fn len(&self) -> usize {
        self.pages * PAGE_SIZE as usize
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: the buffer is mapped and `len` bytes long.
        unsafe { core::slice::from_raw_parts(self.virt() as *const u8, self.len()) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: as above, and `&mut self` makes the access exclusive.
        unsafe { core::slice::from_raw_parts_mut(self.virt() as *mut u8, self.len()) }
    }

    /// Volatile read of a value at byte offset `off` (device-written data).
    pub fn read<T: Copy>(&self, off: usize) -> T {
        assert!(off + size_of::<T>() <= self.len());
        // SAFETY: bounds checked; alignment is the caller's layout choice.
        unsafe { ((self.virt() + off as u64) as *const T).read_volatile() }
    }

    pub fn write<T: Copy>(&mut self, off: usize, v: T) {
        assert!(off + size_of::<T>() <= self.len());
        // SAFETY: bounds checked.
        unsafe { ((self.virt() + off as u64) as *mut T).write_volatile(v) }
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        for i in 0..self.pages as u64 {
            frame::free(self.phys + i * PAGE_SIZE);
        }
    }
}

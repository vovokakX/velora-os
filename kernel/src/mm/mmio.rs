//! Mapping device registers (MMIO) into the kernel's MMIO window,
//! uncached. Mappings are permanent; drivers keep them for their lifetime.

use super::PAGE_SIZE;
use super::paging::{self, Kind};
use core::sync::atomic::{AtomicU64, Ordering};

pub const MMIO_START: u64 = 0xffff_d000_0000_0000;
/// The top 16 MiB of the window are left for tests.
pub const MMIO_END: u64 = 0xffff_d0ff_ff00_0000;

static NEXT: AtomicU64 = AtomicU64::new(MMIO_START);

/// Maps `size` bytes of device memory at physical `phys` and returns the
/// virtual address that corresponds to `phys`.
pub fn map(phys: u64, size: u64) -> Option<u64> {
    let offset = phys & (PAGE_SIZE - 1);
    let base = phys - offset;
    let pages = (offset + size.max(1)).div_ceil(PAGE_SIZE);
    let va = NEXT.fetch_add(pages * PAGE_SIZE, Ordering::SeqCst);
    if va + pages * PAGE_SIZE > MMIO_END {
        return None;
    }
    for i in 0..pages {
        paging::map(va + i * PAGE_SIZE, base + i * PAGE_SIZE, Kind::Mmio).ok()?;
    }
    Some(va + offset)
}

/// Volatile 32-bit register access.
///
/// # Safety
/// `addr` must be a mapped device register.
#[inline]
pub unsafe fn read32(addr: u64) -> u32 {
    unsafe { (addr as *const u32).read_volatile() }
}

/// # Safety
/// `addr` must be a mapped device register.
#[inline]
pub unsafe fn write32(addr: u64, value: u32) {
    unsafe { (addr as *mut u32).write_volatile(value) }
}

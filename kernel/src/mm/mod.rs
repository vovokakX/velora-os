//! Memory management: physical frames, page tables, kernel heap, stacks.
//!
//! Initialisation order (see `kmain`):
//! 1. [`frame::init`] builds the frame bitmap from the boot memory map.
//!    Bootloader-reclaimable memory stays reserved for now.
//! 2. [`paging::init`] builds the kernel's own page tables (HHDM plus the
//!    kernel image with W^X permissions) and switches CR3 to them.
//! 3. [`heap::init`] enables `alloc`.
//! 4. After the kernel moves to its own stack, bootloader memory is released
//!    (`boot::release_bootloader_memory`).

pub mod frame;
pub mod heap;
pub mod kstack;
pub mod mmio;
pub mod paging;

use crate::boot;

pub const PAGE_SIZE: u64 = 4096;

/// Converts a physical address to its HHDM virtual address.
#[inline]
pub fn phys_to_virt(phys: u64) -> u64 {
    phys + boot::info().hhdm_offset
}

/// Converts an HHDM virtual address back to physical.
#[inline]
pub fn virt_to_phys_hhdm(virt: u64) -> u64 {
    virt - boot::info().hhdm_offset
}

pub fn init() {
    frame::init();
    paging::init();
    heap::init();
}

/// Where the memory in use goes, in bytes.
#[derive(Clone, Copy, Debug)]
pub struct Usage {
    /// RAM the allocator manages (usable + released bootloader memory).
    pub managed: u64,
    /// Managed RAM that is allocated, plus the kernel image.
    pub in_use: u64,
    pub kernel_image: u64,
    /// Files loaded by the bootloader (the initramfs).
    pub modules: u64,
    pub frame_bitmap: u64,
    pub page_tables: u64,
    pub heap: u64,
    pub stacks: u64,
    /// Bootloader memory not yet released.
    pub bootloader: u64,
}

pub fn usage() -> Usage {
    let f = frame::stats();
    let kernel_image = paging::kernel_image_size();
    let modules: u64 = boot::info()
        .modules()
        .iter()
        .map(|m| m.size.next_multiple_of(PAGE_SIZE))
        .sum();
    Usage {
        managed: f.managed_frames * PAGE_SIZE,
        in_use: (f.managed_frames - f.free_frames) * PAGE_SIZE + kernel_image + modules,
        kernel_image,
        modules,
        frame_bitmap: f.bitmap_frames * PAGE_SIZE,
        page_tables: paging::table_frames() * PAGE_SIZE,
        heap: heap::stats().pages_mapped as u64 * PAGE_SIZE,
        stacks: kstack::mapped_pages() * PAGE_SIZE,
        bootloader: f.bootloader_reserved_frames * PAGE_SIZE,
    }
}

//! Physical frame allocator: a bitmap over all RAM below the highest
//! usable address, stored in usable memory and accessed through the HHDM.

use super::PAGE_SIZE;
use crate::boot::{self, limine::MemoryKind};
use crate::sync::SpinLock;
use velora_mm::bitmap::{FrameBitmap, FreeError};

struct State {
    bitmap: FrameBitmap<'static>,
    managed: usize,
    bitmap_frames: usize,
    bootloader_reserved: usize,
}

static STATE: SpinLock<Option<State>> = SpinLock::new(None);

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub managed_frames: u64,
    pub free_frames: u64,
    pub bitmap_frames: u64,
    pub bootloader_reserved_frames: u64,
}

fn frame_range(base: u64, length: u64) -> (usize, usize) {
    // Only whole frames inside the region.
    let start = base.div_ceil(PAGE_SIZE);
    let end = (base + length) / PAGE_SIZE;
    (start as usize, end.saturating_sub(start) as usize)
}

pub fn init() {
    let info = boot::info();
    let tracked =
        |k: MemoryKind| matches!(k, MemoryKind::Usable | MemoryKind::BootloaderReclaimable);
    let top = info
        .regions()
        .iter()
        .filter(|r| tracked(r.kind))
        .map(|r| r.end())
        .max()
        .expect("no usable memory");
    let frames = top.div_ceil(PAGE_SIZE) as usize;
    let words = FrameBitmap::words_for(frames);
    let bitmap_frames = (words * 8).div_ceil(PAGE_SIZE as usize);

    // Put the bitmap at the start of the first usable region large enough.
    let home = info
        .regions()
        .iter()
        .filter(|r| r.kind == MemoryKind::Usable)
        .map(|r| frame_range(r.base, r.length))
        .find(|&(start, count)| start > 0 && count >= bitmap_frames)
        .expect("no room for the frame bitmap");
    let bitmap_phys = home.0 as u64 * PAGE_SIZE;
    // SAFETY: the region is usable RAM, mapped by the bootloader's HHDM,
    // and reserved below so nothing else will use it.
    let storage = unsafe {
        core::slice::from_raw_parts_mut(super::phys_to_virt(bitmap_phys) as *mut u64, words)
    };
    let mut bitmap = FrameBitmap::new(storage, frames);
    let mut managed = 0;
    let mut bootloader_reserved = 0;
    for r in info.regions() {
        let (start, count) = frame_range(r.base, r.length);
        match r.kind {
            MemoryKind::Usable => {
                bitmap.mark_free(start, count);
                managed += count;
            }
            MemoryKind::BootloaderReclaimable => bootloader_reserved += count,
            _ => {}
        }
    }
    // Frame 0 is never handed out, so a physical address of 0 can mean "none".
    bitmap.mark_used(0, 1);
    bitmap.mark_used(home.0, bitmap_frames);
    *STATE.lock() = Some(State {
        bitmap,
        managed,
        bitmap_frames,
        bootloader_reserved,
    });
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock();
    f(guard.as_mut().expect("frame allocator not initialised"))
}

/// Allocates one frame and returns its physical address.
pub fn alloc() -> Option<u64> {
    with(|s| s.bitmap.alloc()).map(|f| f as u64 * PAGE_SIZE)
}

/// Allocates one zero-filled frame.
pub fn alloc_zeroed() -> Option<u64> {
    let phys = alloc()?;
    // SAFETY: the frame was just allocated and is mapped in the HHDM.
    unsafe { core::ptr::write_bytes(super::phys_to_virt(phys) as *mut u8, 0, PAGE_SIZE as usize) };
    Some(phys)
}

/// Allocates `count` physically contiguous frames aligned to `align` frames.
pub fn alloc_contiguous(count: usize, align: usize) -> Option<u64> {
    with(|s| s.bitmap.alloc_contiguous(count, align)).map(|f| f as u64 * PAGE_SIZE)
}

/// Frees a frame. Freeing a frame that is not allocated is a kernel bug.
pub fn free(phys: u64) {
    if let Err(e) = try_free(phys) {
        panic!("frame free failed: {:?} (phys {:#x})", e, phys);
    }
}

pub fn try_free(phys: u64) -> Result<(), FreeError> {
    with(|s| s.bitmap.free((phys / PAGE_SIZE) as usize))
}

/// Hands a bootloader-reclaimable region to the allocator. Returns bytes.
pub fn release_region(base: u64, length: u64) -> u64 {
    let (start, count) = frame_range(base, length);
    with(|s| {
        s.bitmap.mark_free(start, count);
        s.managed += count;
        s.bootloader_reserved = s.bootloader_reserved.saturating_sub(count);
    });
    count as u64 * PAGE_SIZE
}

pub fn stats() -> FrameStats {
    with(|s| FrameStats {
        managed_frames: s.managed as u64,
        free_frames: s.bitmap.free_frames() as u64,
        bitmap_frames: s.bitmap_frames as u64,
        bootloader_reserved_frames: s.bootloader_reserved as u64,
    })
}

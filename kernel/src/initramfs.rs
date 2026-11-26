//! The initramfs: a ustar archive the bootloader loads as a module. The VFS
//! mounts it read-only at `/` (see `fs::tarfs`).

use crate::boot;
use crate::mm::phys_to_virt;
use core::sync::atomic::{AtomicU64, Ordering};
use velora_tar::{Archive, Kind};

static BASE: AtomicU64 = AtomicU64::new(0);
static SIZE: AtomicU64 = AtomicU64::new(0);

/// Finds the initramfs module. Returns (number of files, size in bytes).
pub fn init() -> Result<(usize, u64), &'static str> {
    let m = boot::info()
        .modules()
        .iter()
        .find(|m| m.path().ends_with("initramfs.tar"))
        .ok_or("no initramfs module")?;
    BASE.store(phys_to_virt(m.phys), Ordering::SeqCst);
    SIZE.store(m.size, Ordering::SeqCst);
    let mut files = 0;
    for e in Archive::new(data()) {
        match e {
            Ok(e) if e.kind == Kind::File => files += 1,
            Ok(_) => {}
            Err(_) => return Err("initramfs archive is corrupt"),
        }
    }
    Ok((files, m.size))
}

pub fn data() -> &'static [u8] {
    let base = BASE.load(Ordering::SeqCst);
    if base == 0 {
        return &[];
    }
    // SAFETY: the module stays mapped in the HHDM for the kernel's lifetime
    // (its memory type is never reclaimed) and is only read.
    unsafe { core::slice::from_raw_parts(base as *const u8, SIZE.load(Ordering::SeqCst) as usize) }
}

/// Contents of a regular file, e.g. `read("/bin/hello")`.
pub fn read(path: &str) -> Option<&'static [u8]> {
    Archive::find(data(), path)
        .filter(|e| e.kind == Kind::File)
        .map(|e| e.data)
}

/// Calls `f(path, size, mode)` for every regular file.
pub fn for_each_file(mut f: impl FnMut(&str, usize, u32)) {
    for e in Archive::new(data()).flatten() {
        if e.kind == Kind::File {
            let mut buf = [0u8; 256];
            f(e.path(&mut buf), e.data.len(), e.mode);
        }
    }
}

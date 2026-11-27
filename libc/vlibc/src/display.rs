//! The screen for graphical programs, the keyboard device and disk
//! information.

use crate::sys::{syscall2, syscall3};
use crate::{Errno, check};
use velora_abi::{DiskInfo, DisplayInfo, display, nr};

/// The frame buffer's size and pixel layout.
pub fn info() -> Result<DisplayInfo, Errno> {
    let mut i = DisplayInfo::default();
    // SAFETY: `i` is writable and has the kernel's layout.
    check(unsafe {
        syscall2(
            nr::DISPLAY,
            display::INFO,
            &mut i as *mut DisplayInfo as u64,
        )
    })?;
    Ok(i)
}

/// Maps the frame buffer and takes the screen from the text console.
/// The memory is `pitch * height` bytes.
pub fn map() -> Result<*mut u8, Errno> {
    // SAFETY: no pointers.
    let a = check(unsafe { syscall2(nr::DISPLAY, display::MAP, 0) })?;
    Ok(a as *mut u8)
}

/// Gives the screen back to the text console (also happens at exit).
pub fn release() -> Result<(), Errno> {
    // SAFETY: no pointers.
    check(unsafe { syscall2(nr::DISPLAY, display::RELEASE, 0) })?;
    Ok(())
}

/// Size, kind and model of `/dev/NAME`.
pub fn disk_info(path: &str) -> Result<DiskInfo, Errno> {
    let mut d = DiskInfo {
        size_bytes: 0,
        sector_size: 0,
        kind: 0,
        model: [0; 48],
    };
    // SAFETY: the path and `d` are valid for the call.
    check(unsafe {
        syscall3(
            nr::DISKINFO,
            path.as_ptr() as u64,
            path.len() as u64,
            &mut d as *mut DiskInfo as u64,
        )
    })?;
    Ok(d)
}

//! C API for programs written in C (feature `cabi`). Declarations are in
//! `include/vlibc.h`. Only a small subset exists so far; it grows with the
//! system (files arrive in stage 7).

use crate::{io, process};
use core::alloc::Layout;
use core::ffi::{CStr, c_char, c_int, c_long, c_void};

/// Every allocation keeps its size in a 16-byte header so `free` and
/// `realloc` know the layout.
const HEADER: usize = 16;

#[unsafe(no_mangle)]
pub extern "C" fn exit(code: c_int) -> ! {
    process::exit(code)
}

#[unsafe(no_mangle)]
pub extern "C" fn _exit(code: c_int) -> ! {
    process::exit(code)
}

#[unsafe(no_mangle)]
pub extern "C" fn getpid() -> c_long {
    process::getpid() as c_long
}

/// Returns the number of bytes written or -1.
///
/// # Safety
/// `buf` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn write(fd: c_int, buf: *const c_void, len: usize) -> c_long {
    let s = unsafe { core::slice::from_raw_parts(buf.cast::<u8>(), len) };
    io::write(fd as u64, s).map(|n| n as c_long).unwrap_or(-1)
}

/// # Safety
/// `buf` must be valid and writable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn read(fd: c_int, buf: *mut c_void, len: usize) -> c_long {
    let s = unsafe { core::slice::from_raw_parts_mut(buf.cast::<u8>(), len) };
    io::read(fd as u64, s).map(|n| n as c_long).unwrap_or(-1)
}

/// # Safety
/// `s` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn puts(s: *const c_char) -> c_int {
    let s = unsafe { CStr::from_ptr(s) };
    let ok = io::write_all(1, s.to_bytes()).and_then(|_| io::write_all(1, b"\n"));
    if ok.is_ok() { 0 } else { -1 }
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: usize) -> *mut c_void {
    let Ok(layout) = Layout::from_size_align(size + HEADER, 16) else {
        return core::ptr::null_mut();
    };
    // SAFETY: layout has a non-zero size.
    let p = unsafe { alloc::alloc::alloc(layout) };
    if p.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the block is at least HEADER bytes.
    unsafe {
        p.cast::<usize>().write(size);
        p.add(HEADER).cast()
    }
}

/// # Safety
/// `p` must be null or come from `malloc`/`calloc`/`realloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    unsafe {
        let base = p.cast::<u8>().sub(HEADER);
        let size = base.cast::<usize>().read();
        alloc::alloc::dealloc(base, Layout::from_size_align_unchecked(size + HEADER, 16));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn calloc(n: usize, size: usize) -> *mut c_void {
    let Some(total) = n.checked_mul(size) else {
        return core::ptr::null_mut();
    };
    let p = malloc(total);
    if !p.is_null() {
        // SAFETY: `p` has `total` bytes.
        unsafe { core::ptr::write_bytes(p.cast::<u8>(), 0, total) };
    }
    p
}

/// # Safety
/// `p` must be null or come from `malloc`/`calloc`/`realloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(p: *mut c_void, size: usize) -> *mut c_void {
    if p.is_null() {
        return malloc(size);
    }
    let old = unsafe { p.cast::<u8>().sub(HEADER).cast::<usize>().read() };
    let new = malloc(size);
    if !new.is_null() {
        unsafe {
            core::ptr::copy_nonoverlapping(p.cast::<u8>(), new.cast::<u8>(), old.min(size));
            free(p);
        }
    }
    new
}

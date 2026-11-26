//! Memory mapping.

use crate::sys::{syscall2, syscall4};
use crate::{Errno, check};
use velora_abi::nr;

pub use velora_abi::{map, prot};

/// Maps `len` bytes of zeroed memory. With `map::FIXED`, exactly at `addr`.
pub fn mmap(addr: u64, len: usize, protection: u64, flags: u64) -> Result<*mut u8, Errno> {
    // SAFETY: mmap only creates new mappings; it never touches existing ones.
    check(unsafe { syscall4(nr::MMAP, addr, len as u64, protection, flags) }).map(|a| a as *mut u8)
}

/// Removes mappings in `[addr, addr + len)`.
///
/// # Safety
/// Nothing may use the memory afterwards.
pub unsafe fn munmap(addr: *mut u8, len: usize) -> Result<(), Errno> {
    check(unsafe { syscall2(nr::MUNMAP, addr as u64, len as u64) }).map(|_| ())
}

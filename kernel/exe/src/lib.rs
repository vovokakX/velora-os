//! Executable formats, independent of the kernel so they can be tested on
//! the host:
//! - [`elf`]: full parsing and validation of static ELF64 executables.
//! - [`pe`]: recognition and header parsing of PE (.exe) files. Loading
//!   them is planned together with the Win32 personality.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod elf;
pub mod pe;

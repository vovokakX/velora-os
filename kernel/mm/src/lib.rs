//! Hardware-independent memory management algorithms.
//!
//! The kernel supplies the memory (through the HHDM and its page tables);
//! this crate only does the bookkeeping, so it is tested on the host with
//! `cargo test`.

#![cfg_attr(not(test), no_std)]

pub mod bitmap;
pub mod heap;

pub const PAGE_SIZE: usize = 4096;

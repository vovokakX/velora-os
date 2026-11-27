//! USB logic that needs no hardware, tested on the host:
//!
//! - [`desc`]: standard requests and descriptor parsing;
//! - [`hid`]: boot protocol keyboard and mouse reports. Keyboard reports
//!   become PS/2 scancode set 1 bytes, so USB keyboards go through the
//!   same decoder and layouts as PS/2 ones;
//! - [`msc`]: mass storage bulk-only transport and the SCSI commands a
//!   flash drive needs.
//!
//! The xHCI host controller driver itself is in the kernel
//! (`kernel/src/drivers/xhci.rs`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod desc;
pub mod hid;
pub mod msc;

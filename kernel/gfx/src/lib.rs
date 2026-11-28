//! Images for the text terminal (no_std + alloc, tested on the host).
//!
//! - [`kitty`]: the Kitty terminal graphics protocol. The kernel console
//!   uses the receiving side ([`kitty::Parser`]), programs use the sending
//!   side ([`kitty::transmit`], [`kitty::QUERY`]).
//! - [`base64`]: the payload encoding of the protocol.
//! - [`image`]: premultiplied RGBA images, area-average scaling, blending.
//! - [`png`] and [`inflate`]: PNG decoding for programs that ship images.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod base64;
pub mod image;
pub mod inflate;
pub mod kitty;
pub mod png;

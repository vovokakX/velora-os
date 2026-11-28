//! Intel High Definition Audio without the hardware (no_std + alloc,
//! tested on the host):
//!
//! - [`codec`]: codec verbs, the widget graph of an audio function group,
//!   paths from output pins to converters (DACs), setting a path up and
//!   its volume. The kernel driver sends the verbs through CORB/RIRB.
//! - [`ring`]: the cyclic playback buffer the controller reads by DMA:
//!   what is written, what was played, what must be cleared to silence.
//! - [`mixer`]: the text of `/dev/mixer` (state and commands).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod codec;
pub mod mixer;
pub mod ring;

/// The one format the driver plays: 48 kHz, 16-bit, stereo.
pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
/// Bytes per frame (one sample for each channel).
pub const FRAME: usize = 4;

/// The stream format word for [`RATE`], 16 bits, [`CHANNELS`]: base 48
/// kHz, no multiplier or divider, BITS = 1 (16), CHN = channels - 1.
pub const FORMAT_48K_16_STEREO: u16 = (1 << 4) | (CHANNELS as u16 - 1);

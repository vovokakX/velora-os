//! Synchronisation primitives.

mod spin;

pub use spin::{RacyCell, SpinLock};

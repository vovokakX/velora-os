//! Device drivers that live inside the kernel crate because they need
//! kernel services directly. Hardware-independent parts are in the
//! `drivers/` crates (for example `velora-input`).

pub mod ps2;

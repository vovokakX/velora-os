//! Architecture-specific code. Only x86_64 exists today; the rest of the
//! kernel uses the re-exports below so another port stays possible.

pub mod x86_64;

pub use x86_64::{cpu, fatal_exit, init, init_ap};

//! Lumi, the graphical shell of Velora OS: the login screen, windows,
//! the panel, the app menu and the apps (terminal, files, settings,
//! about). Everything here is plain logic and drawing, tested and drawn
//! on the host; the `lumi` program connects it to the screen, the input
//! devices and the system (`host::Host`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod apps;
pub mod config;
pub mod cursor;
pub mod desktop;
pub mod fake;
pub mod gfx;
pub mod host;
mod launcher;
mod login;
mod panel;
pub mod tz;
pub mod vt;
pub mod wallpaper;

pub use desktop::{Desktop, Exit, Mode, Pointer};

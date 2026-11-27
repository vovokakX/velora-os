//! velora-ui: the graphical toolkit of Velora OS (the graphical installer
//! now, the Lumi shell later). Drawing with anti-aliasing and alpha,
//! TrueType text, line icons, images, the mouse pointer, and widgets
//! controlled with the mouse and the keyboard. It does not touch the
//! hardware: a program gives it input events and shows the canvas (the `screen`
//! module with the `velora` feature).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod canvas;
pub mod decor;
pub mod gallery;
pub mod geom;
pub mod icons;
pub mod image;
#[cfg(feature = "velora")]
pub mod screen;
pub mod text;
pub mod theme;
pub mod ui;

pub use canvas::{Canvas, Color};
pub use geom::Rect;
pub use ui::{Ctx, Event, Key, Ui};

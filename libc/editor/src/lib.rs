//! Plume, the Velora text editor, without any I/O of its own.
//!
//! - [`KeyDecoder`] turns raw terminal bytes into [`Key`]s.
//! - [`Buffer`] holds the text as lines and loads and saves files
//!   (UTF-8, LF or CRLF line ends).
//! - [`Editor`] applies keys: editing, cut and paste, search, prompts,
//!   saving through the [`Io`] trait and the quit confirmation.
//! - [`Editor::render`] produces a [`Frame`]; [`Screen`] turns frames
//!   into ANSI output and redraws only the rows that changed.
//!
//! `/bin/plume` (userland/plume) connects these to the console and files.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod buffer;
mod editor;
mod key;
mod screen;

pub use buffer::{Buffer, LoadError, Pos, char_width};
pub use editor::{Editor, Io, Status};
pub use key::{Key, KeyDecoder};
pub use screen::{Cell, Frame, Screen};

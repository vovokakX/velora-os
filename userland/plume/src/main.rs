//! /bin/plume [FILE]: a text editor for the console.
//!
//! The editing logic, key decoding and screen drawing are in
//! `velora-editor` (tested on the host); this program connects them to
//! the console in raw mode and to files.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::{String, ToString};
use coreutils::{describe, fail};
use velora_editor::{Editor, Io, KeyDecoder, Screen, Status};
use vlibc::abi::errno;
use vlibc::fs::{self, File, flags};
use vlibc::{eprintln, io, println, term};

vlibc::main!(main);

struct Files;

impl Io for Files {
    fn save(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        let err = |e| describe(e).to_string();
        let mut f = File::open_with(path, flags::WRITE | flags::CREATE | flags::TRUNCATE, 0o644)
            .map_err(err)?;
        f.write_all(data).map_err(err)?;
        f.close().map_err(err)?;
        // Put the file on the disk now, not at the next sync.
        fs::sync().map_err(err)?;
        Ok(())
    }

    fn exists(&self, path: &str) -> bool {
        fs::exists(path)
    }
}

fn usage() -> i32 {
    println!("usage: plume [FILE]");
    println!("Opens FILE (or creates it when saved) in the text editor.");
    println!("Inside the editor ^G shows the keys; ^X exits.");
    0
}

fn open(path: &str) -> Result<Editor, i32> {
    match fs::stat(path) {
        Ok(st) if fs::is_dir(&st) => {
            eprintln!("plume: {}: is a directory", path);
            Err(1)
        }
        Ok(_) => match fs::read(path) {
            Ok(bytes) => Ok(Editor::open(path, &bytes)),
            Err(e) => Err(fail(path, e)),
        },
        Err(e) if e.0 == errno::ENOENT => Ok(Editor::new_file(path)),
        Err(e) => Err(fail(path, e)),
    }
}

fn main() -> i32 {
    let args = vlibc::env::args();
    if args.len() > 2 || matches!(args.get(1), Some("-h" | "--help")) {
        return usage();
    }
    if !term::is_console(0) || !term::is_console(1) {
        eprintln!("plume: the editor works on the console only");
        return 1;
    }
    let mut editor = match args.get(1) {
        Some(path) => match open(path) {
            Ok(e) => e,
            Err(status) => return status,
        },
        None => Editor::new(None),
    };
    let (cols, rows) = term::size();
    editor.resize(cols as usize, rows as usize);

    let old_mode = term::set_mode(term::RAW);
    let mut screen = Screen::new();
    let mut keys = KeyDecoder::new();
    let mut files = Files;
    let mut buf = [0u8; 1024];
    'run: loop {
        let out = screen.draw(&editor.render());
        let _ = io::write_all(1, out.as_bytes());
        let n = match io::read(0, &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for key in keys.decode(&buf[..n]) {
            if editor.handle(key, &mut files) == Status::Quit {
                break 'run;
            }
        }
    }
    let _ = io::write_all(1, screen.leave().as_bytes());
    term::set_mode(old_mode);
    0
}

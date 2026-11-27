//! velofetch: a system summary next to the Velora logo.
//!
//! On the screen console the logo is the image from `assets/`, sent with
//! the Kitty graphics protocol and scaled to the height of the summary in
//! pixels of the current font. Without graphics (output to a file or a
//! pipe, a console without a screen, a terminal that does not answer the
//! graphics query, `--ascii`) the logo is ASCII art.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use coreutils::*;
use velora_gfx::image::{self, Image};
use velora_gfx::{kitty, png};
use vlibc::process::{self, field_str};
use vlibc::{env, fs, io, println, term};

vlibc::main!(main);

const LOGO: [&str; 9] = [
    "  __      __  ",
    "  \\ \\    / /  ",
    "   \\ \\  / /   ",
    "    \\ \\/ /    ",
    "     \\  /     ",
    "      \\/      ",
    "              ",
    "  V E L O R A ",
    "              ",
];

const LOGO_PNG: &[u8] = include_bytes!("../../../../assets/velora-logo.png");

/// Columns the summary needs next to the image; narrower screens get
/// the ASCII logo.
const TEXT_COLS: usize = 40;

fn main() -> i32 {
    let ascii = env::args().iter().skip(1).any(|a| a == "--ascii");
    let s = match process::sysinfo() {
        Ok(s) => s,
        Err(e) => return fail("sysinfo", e),
    };
    let user = user_name(process::getuid().0);
    let shell = env::var("SHELL").unwrap_or("/bin/vsh");
    let disks: Vec<String> = fs::read_dir("/mnt")
        .map(|d| d.flatten().map(|e| e.name).collect())
        .unwrap_or_default();
    let (cols, rows) = term::size();
    let title = format!("{}@velora", user);
    let lines = [
        title.clone(),
        "-".repeat(title.chars().count()),
        format!("OS:      Velora OS {} x86_64", field_str(&s.version)),
        format!("Kernel:  velora {}", field_str(&s.version)),
        format!("Uptime:  {}", duration(s.uptime_ns / 1_000_000_000)),
        format!("Shell:   {}", shell),
        format!(
            "CPU:     {} ({} core{})",
            field_str(&s.cpu),
            s.cpus,
            if s.cpus == 1 { "" } else { "s" }
        ),
        format!("Memory:  {} / {}", human(s.mem_used), human(s.mem_total)),
        format!("Console: {}x{}, processes: {}", cols, rows, s.processes),
        format!(
            "Disks:   {}",
            if disks.is_empty() {
                String::from("none mounted")
            } else {
                disks.join(", ")
            }
        ),
    ];
    let picture = if ascii {
        None
    } else {
        graphics_cell().and_then(|(cw, ch)| logo_image(lines.len(), cw, ch, cols as usize))
    };
    let color = colors();
    match picture {
        Some((sequence, width)) => {
            // The image keeps the cursor in place (C=1); each line then
            // skips the columns it covers.
            let _ = io::write_all(1, sequence.as_bytes());
            for (i, l) in lines.iter().enumerate() {
                if color && i == 0 {
                    println!("\x1b[{}C\x1b[1;35m{}\x1b[0m", width + 2, l);
                } else {
                    println!("\x1b[{}C{}", width + 2, l);
                }
            }
        }
        None => {
            for (i, l) in lines.iter().enumerate() {
                let logo = LOGO.get(i).copied().unwrap_or("              ");
                if color {
                    println!("\x1b[1;35m{}\x1b[0m  {}", logo, l);
                } else {
                    println!("{}  {}", logo, l);
                }
            }
        }
    }
    0
}

/// The cell size in pixels if the terminal can show images: both input
/// and output are the console, the console has a screen, and it answers
/// the Kitty graphics query.
fn graphics_cell() -> Option<(usize, usize)> {
    if !term::is_console(0) || !term::is_console(1) {
        return None;
    }
    let (cw, ch) = term::cell_size()?;
    if cw == 0 || ch == 0 || !shows_images() {
        return None;
    }
    Some((cw as usize, ch as usize))
}

/// Sends the graphics query and reads the answers up to the device
/// attributes answer, which every terminal sends.
fn shows_images() -> bool {
    let old = term::set_mode(term::RAW | term::NO_ECHO);
    let mut answers = Vec::new();
    let result = if io::write_all(1, kitty::QUERY.as_bytes()).is_err() {
        false
    } else {
        let mut buf = [0u8; 128];
        loop {
            match kitty::support(&answers) {
                kitty::Support::Yes => break true,
                kitty::Support::No => break false,
                kitty::Support::Waiting => {}
            }
            if answers.len() > 1024 {
                break false;
            }
            match io::read(0, &mut buf) {
                Ok(n) if n > 0 => answers.extend_from_slice(&buf[..n]),
                _ => break false,
            }
        }
    };
    term::set_mode(old);
    result
}

/// The escape sequence that shows the logo over `rows` text rows, and
/// the number of columns it covers. The image is made in the exact pixel
/// size of those cells, so the terminal shows it without scaling.
fn logo_image(rows: usize, cw: usize, ch: usize, screen_cols: usize) -> Option<(String, usize)> {
    let h = rows * ch;
    let cols = h.div_ceil(cw);
    if cols + 2 + TEXT_COLS > screen_cols {
        return None;
    }
    let w = cols * cw;
    let logo = png::decode(LOGO_PNG, 4 << 20).ok()?;
    let (lw, lh) = image::fit(logo.width, logo.height, w, h);
    let mut canvas = Image::new(w, h);
    canvas.draw(&logo.resize(lw, lh), (w - lw) / 2, (h - lh) / 2);
    let keys = format!("a=T,f=32,s={},v={},c={},r={},C=1,q=2", w, h, cols, rows);
    Some((kitty::transmit(&keys, &canvas.to_rgba()), cols))
}

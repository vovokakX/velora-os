//! velofetch: a system summary next to the Velora logo.
//!
//! On the screen console the logo is the Velora mark drawn from its
//! geometry (`velora_gfx::logo`) at the height of the summary in pixels of
//! the current font, on a transparent background, and sent with the Kitty
//! graphics protocol. Without graphics (output to a file or a
//! pipe, a console without a screen, a terminal that does not answer the
//! graphics query, `--ascii`) the logo is ASCII art.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use coreutils::*;
use velora_gfx::image::Image;
use velora_gfx::{kitty, logo};
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

/// Columns the summary needs next to the image; narrower screens get
/// the ASCII logo.
const TEXT_COLS: usize = 40;

const ACCENT: &str = "\x1b[1;35m";
const RESET: &str = "\x1b[0m";

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
    let color = colors();
    let fields = [
        ("OS", format!("Velora OS {} x86_64", field_str(&s.version))),
        ("Kernel", format!("velora {}", field_str(&s.version))),
        ("Uptime", duration(s.uptime_ns / 1_000_000_000)),
        ("Shell", String::from(shell)),
        (
            "CPU",
            format!(
                "{} ({} core{})",
                field_str(&s.cpu),
                s.cpus,
                if s.cpus == 1 { "" } else { "s" }
            ),
        ),
        (
            "Memory",
            format!("{} / {}", human(s.mem_used), human(s.mem_total)),
        ),
        (
            "Console",
            format!("{}x{}, processes: {}", cols, rows, s.processes),
        ),
        (
            "Disks",
            if disks.is_empty() {
                String::from("none mounted")
            } else {
                disks.join(", ")
            },
        ),
    ];
    let title_len = user.chars().count() + "@velora".len();
    let mut lines = Vec::new();
    if color {
        lines.push(format!(
            "{}{}{}@{}velora{}",
            ACCENT, user, RESET, ACCENT, RESET
        ));
    } else {
        lines.push(format!("{}@velora", user));
    }
    lines.push("-".repeat(title_len));
    for (label, value) in &fields {
        let pad = " ".repeat(9 - label.len() - 1);
        if color {
            lines.push(format!("{}{}:{}{}{}", ACCENT, label, RESET, pad, value));
        } else {
            lines.push(format!("{}:{}{}", label, pad, value));
        }
    }
    if color {
        // The 16 console colours, as two rows of swatches.
        lines.push(String::new());
        for base in [40, 100] {
            let mut row = String::new();
            for i in 0..8 {
                row.push_str(&format!("\x1b[{}m   ", base + i));
            }
            row.push_str(RESET);
            lines.push(row);
        }
    }
    let picture = if ascii {
        None
    } else {
        graphics_cell().and_then(|(cw, ch)| logo_image(lines.len(), cw, ch, cols as usize))
    };
    match picture {
        Some((sequence, width)) => {
            // The image keeps the cursor in place (C=1); each line then
            // skips the columns it covers.
            let _ = io::write_all(1, sequence.as_bytes());
            for l in &lines {
                println!("\x1b[{}C{}", width + 2, l);
            }
        }
        None => {
            for (i, l) in lines.iter().enumerate() {
                let logo = LOGO.get(i).copied().unwrap_or("              ");
                if color {
                    println!("{}{}{}  {}", ACCENT, logo, RESET, l);
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
/// the number of columns it covers.
///
/// The mark is drawn from its geometry, white with antialiased edges on a
/// transparent background, so it sits on the console background. It is
/// as high as the text minus half a row above and below, in the exact
/// pixel size of the cells, so the terminal shows it without scaling.
fn logo_image(rows: usize, cw: usize, ch: usize, screen_cols: usize) -> Option<(String, usize)> {
    let h = rows * ch;
    let mark = logo::mark(h - ch, 0xffffff);
    // Half a cell of space on each side.
    let cols = (mark.width + cw).div_ceil(cw);
    if cols + 2 + TEXT_COLS > screen_cols {
        return None;
    }
    let w = cols * cw;
    let mut canvas = Image::new(w, h);
    canvas.draw(&mark, (w - mark.width) / 2, (h - mark.height) / 2);
    let keys = format!("a=T,f=32,s={},v={},c={},r={},C=1,q=2", w, h, cols, rows);
    Some((kitty::transmit(&keys, &canvas.to_rgba()), cols))
}

//! velofetch: a system summary next to the Velora logo.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use coreutils::*;
use vlibc::process::{self, field_str};
use vlibc::{env, fs, println};

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

fn main() -> i32 {
    let s = match process::sysinfo() {
        Ok(s) => s,
        Err(e) => return fail("sysinfo", e),
    };
    let user = user_name(process::getuid().0);
    let shell = env::var("SHELL").unwrap_or("/bin/vsh");
    let disks: Vec<String> = fs::read_dir("/mnt")
        .map(|d| d.flatten().map(|e| e.name).collect())
        .unwrap_or_default();
    let (cols, rows) = vlibc::term::size();
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
    let color = colors();
    for (i, l) in lines.iter().enumerate() {
        let logo = LOGO.get(i).copied().unwrap_or("              ");
        if color {
            println!("\x1b[1;35m{}\x1b[0m  {}", logo, l);
        } else {
            println!("{}  {}", logo, l);
        }
    }
    0
}

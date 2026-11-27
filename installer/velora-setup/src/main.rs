//! velora-setup: installs Velora OS from the Live system onto a disk, in
//! the terminal.
//!
//! ```text
//! velora-setup                                   asks everything
//! velora-setup --auto --disk vd0 --user anna [--yes]
//! ```
//!
//! With `--auto` the passwords come from the environment variables
//! `SETUP_ROOT_PASSWORD` and `SETUP_USER_PASSWORD`, and `--yes` skips the
//! confirmation (used by the tests).
//!
//! The work is done by `velora-install`, which the graphical installer
//! uses too; this program only asks the questions. Appearance and locale
//! settings get their defaults (see `velora_install::settings`).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use velora_install::live::{self, LiveSource};
use velora_install::settings::Lang;
use velora_install::{MIN_DISK_MIB, Phase, Plan};
use vlibc::{env, io, print, println, process, term};

vlibc::main!(main);

fn read_line() -> Option<String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        let n = io::read(0, &mut buf).ok()?;
        if n == 0 {
            return (!out.is_empty()).then(|| String::from_utf8_lossy(&out).into_owned());
        }
        out.extend_from_slice(&buf[..n]);
        if out.ends_with(b"\n") {
            out.pop();
            return Some(String::from_utf8_lossy(&out).into_owned());
        }
    }
}

fn ask(q: &str) -> Option<String> {
    print!("{}", q);
    read_line().map(|s| String::from(s.trim()))
}

fn ask_password(who: &str) -> Option<String> {
    loop {
        print!("Password for {}: ", who);
        term::set_mode(term::NO_ECHO);
        let a = read_line();
        print!("Repeat the password: ");
        let b = read_line();
        term::set_mode(0);
        let (a, b) = (a?, b?);
        if a.is_empty() {
            println!("The password must not be empty.");
        } else if a != b {
            println!("The passwords differ, try again.");
        } else {
            return Some(a);
        }
    }
}

fn usage() -> i32 {
    println!("usage: velora-setup");
    println!("       velora-setup --auto --disk NAME --user NAME [--yes]");
    println!("       (--auto reads SETUP_ROOT_PASSWORD and SETUP_USER_PASSWORD)");
    2
}

fn main() -> i32 {
    let args = env::args();
    let argv: Vec<&str> = args.iter().skip(1).collect();
    if argv.contains(&"-h") || argv.contains(&"--help") {
        return usage();
    }
    if process::getuid().0 != 0 {
        println!("velora-setup: run it as root");
        return 1;
    }
    if !live::is_live() {
        println!("velora-setup: this is not the Live system; Velora OS is already installed");
        return 1;
    }
    let all = live::disks();
    let auto = argv.contains(&"--auto");
    let opt = |name: &str| {
        argv.iter()
            .position(|a| *a == name)
            .and_then(|i| argv.get(i + 1))
            .map(|s| String::from(*s))
    };

    println!("Velora OS {} installer", env!("CARGO_PKG_VERSION"));
    println!();
    if all.is_empty() {
        println!("No disks found.");
        return 1;
    }
    println!("Disks:");
    for d in &all {
        println!(
            "  {:<10} {:>8} MiB  {:<6} {}{}",
            d.name,
            d.size / (1024 * 1024),
            d.bus,
            d.model,
            if d.boot {
                "  (the system was started from it)"
            } else if d.in_use {
                "  (mounted under /mnt)"
            } else {
                ""
            }
        );
    }
    println!();

    let (disk, plan) = if auto {
        let (Some(disk), Some(user)) = (opt("--disk"), opt("--user")) else {
            return usage();
        };
        let (Some(rp), Some(up)) = (
            env::var("SETUP_ROOT_PASSWORD"),
            env::var("SETUP_USER_PASSWORD"),
        ) else {
            println!("velora-setup: set SETUP_ROOT_PASSWORD and SETUP_USER_PASSWORD");
            return 2;
        };
        let plan = Plan {
            user,
            root_password: rp.into(),
            user_password: up.into(),
            ..Plan::default()
        };
        (disk, plan)
    } else {
        let Some(disk) = ask("Disk to install on: ") else {
            return 1;
        };
        let user = loop {
            let Some(u) = ask("Name of the new user (lowercase letters, digits): ") else {
                return 1;
            };
            if velora_install::valid_user_name(&u) {
                break u;
            }
            println!("Not a valid user name.");
        };
        let Some(user_password) = ask_password(&user) else {
            return 1;
        };
        let Some(root_password) = ask_password("root") else {
            return 1;
        };
        let plan = Plan {
            user,
            root_password,
            user_password,
            ..Plan::default()
        };
        (disk, plan)
    };

    let Some(d) = all.iter().find(|d| d.name == disk) else {
        println!("velora-setup: no disk named {}", disk);
        return 1;
    };
    if d.boot {
        println!(
            "velora-setup: the system was started from {}; it cannot be erased",
            disk
        );
        return 1;
    }
    if let Err(p) = velora_install::check(&plan, d.size) {
        println!("velora-setup: {}", p.text(Lang::En));
        if d.size < MIN_DISK_MIB * 1024 * 1024 {
            println!("velora-setup: {} is too small", disk);
        }
        return 1;
    }
    println!(
        "Velora OS will be installed on {} ({} MiB) with the user {}.",
        disk,
        d.size / (1024 * 1024),
        plan.user
    );
    println!("ALL DATA ON {} WILL BE ERASED.", disk);
    if !argv.contains(&"--yes") {
        let answer = ask(&format!("Type 'yes' to erase {} and install: ", disk));
        if answer.as_deref() != Some("yes") {
            println!("Nothing was changed.");
            return 1;
        }
    }
    if d.in_use {
        println!("Unmounting the file systems of {}.", disk);
    }
    if let Err(e) = live::release(&disk) {
        println!("velora-setup: {}", e);
        return 1;
    }
    let dev = match live::open_disk(&disk) {
        Ok(d) => d,
        Err(e) => {
            println!("velora-setup: {}", e);
            return 1;
        }
    };
    let mut shown: Option<Phase> = None;
    let mut step = |p: velora_install::Progress| {
        if shown != Some(p.phase) {
            shown = Some(p.phase);
            let n = Phase::ALL.iter().position(|&x| x == p.phase).unwrap_or(0) + 1;
            println!("[{}/4] {}", n, p.phase.name(Lang::En));
        }
    };
    match velora_install::install(dev, &plan, &mut LiveSource, &mut step) {
        Ok(_) => {
            println!();
            println!("Installation complete.");
            println!("Remove the installation media and type 'reboot'.");
            0
        }
        Err(e) => {
            println!();
            println!("velora-setup: installation failed: {}", e);
            println!("The disk may be partly written; run velora-setup again.");
            1
        }
    }
}

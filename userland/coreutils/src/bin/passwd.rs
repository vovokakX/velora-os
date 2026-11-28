//! passwd [USER]: sets a password (root only: there are no setuid
//! programs yet, and /etc/shadow is readable and writable only by root).
//!
//! passwd --serve USER: changes USER's password for a program that has
//! dropped root. Lumi starts it as root before the user's session and
//! keeps its standard input and output: each line "OLD<tab>NEW" changes
//! the password if OLD is right, the answer is "ok" or "error: <why>".
//! It ends when its input closes (the session ends).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use coreutils::*;
use vlibc::{env, eprintln, io, print, println, process, term};

vlibc::main!(main);

fn read_secret(prompt: &str) -> Option<String> {
    print!("{}", prompt);
    term::set_mode(term::NO_ECHO);
    let mut buf = [0u8; 256];
    let mut out = String::new();
    let r = loop {
        match io::read(0, &mut buf) {
            Ok(0) | Err(_) => break None,
            Ok(n) => {
                out.push_str(&String::from_utf8_lossy(&buf[..n]));
                if out.ends_with('\n') {
                    out.pop();
                    break Some(out);
                }
            }
        }
    };
    term::set_mode(0);
    r
}

/// Writes a new /etc/shadow with `password` for `user`.
fn set_password(user: &str, password: &str) -> Result<(), String> {
    let shadow = vlibc::fs::read("/etc/shadow")
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| String::from(e.name()))?;
    if velora_auth::shadow_hash(&shadow, user).is_none() {
        return Err(format!("no user {}", user));
    }
    let mut salt = [0u8; 16];
    process::random(&mut salt);
    let hash = velora_auth::hash_password(password, &salt, velora_auth::DEFAULT_ITERATIONS);
    let mut out = String::new();
    for line in shadow.lines() {
        let mut f: alloc::vec::Vec<&str> = line.split(':').collect();
        if f.first() == Some(&user) && f.len() > 1 {
            f[1] = &hash;
        }
        out.push_str(&f.join(":"));
        out.push('\n');
    }
    // Write a new file and rename it over the old one.
    let tmp = "/etc/shadow.new";
    vlibc::fs::File::open_with(
        tmp,
        vlibc::fs::flags::WRITE | vlibc::fs::flags::CREATE | vlibc::fs::flags::TRUNCATE,
        0o600,
    )
    .and_then(|mut f| f.write_all(out.as_bytes()))
    .and_then(|_| vlibc::fs::rename(tmp, "/etc/shadow"))
    .and_then(|_| vlibc::fs::sync())
    .map_err(|e| String::from(e.name()))
}

/// Checks `old` against the stored hash, then sets `new`.
fn change(user: &str, old: &str, new: &str) -> Result<(), String> {
    if new.is_empty() {
        return Err(String::from("the new password is empty"));
    }
    let shadow = vlibc::fs::read("/etc/shadow")
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| String::from(e.name()))?;
    let hash = velora_auth::shadow_hash(&shadow, user).unwrap_or("!");
    if !velora_auth::verify_password(old, hash) {
        // As slow as login after a wrong password.
        vlibc::time::sleep_ms(600);
        return Err(String::from("wrong password"));
    }
    set_password(user, new)
}

fn serve(user: &str) -> i32 {
    let mut buf = [0u8; 512];
    let mut line = alloc::vec::Vec::new();
    loop {
        let n = match io::read(0, &mut buf) {
            Ok(0) | Err(_) => return 0,
            Ok(n) => n,
        };
        line.extend_from_slice(&buf[..n]);
        while let Some(end) = line.iter().position(|&b| b == b'\n') {
            let req: alloc::vec::Vec<u8> = line.drain(..=end).collect();
            let text = String::from_utf8_lossy(&req[..end]).into_owned();
            let answer = match text.split_once('\t') {
                Some((old, new)) => match change(user, old, new) {
                    Ok(()) => String::from("ok\n"),
                    Err(e) => format!("error: {}\n", e),
                },
                None => String::from("error: bad request\n"),
            };
            let _ = io::write_all(1, answer.as_bytes());
        }
        if line.len() > 4096 {
            line.clear();
        }
    }
}

fn main() -> i32 {
    if !need_root() {
        return 1;
    }
    let args = env::args();
    if args.get(1) == Some("--serve") {
        return match args.get(2) {
            Some(user) => serve(user),
            None => {
                eprintln!("usage: passwd --serve USER");
                2
            }
        };
    }
    let user = String::from(args.get(1).unwrap_or("root"));
    let shadow = match vlibc::fs::read("/etc/shadow") {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => return fail("/etc/shadow", e),
    };
    if velora_auth::shadow_hash(&shadow, &user).is_none() {
        eprintln!("passwd: no user {}", user);
        return 1;
    }
    let Some(a) = read_secret(&format!("New password for {}: ", user)) else {
        return 1;
    };
    let Some(b) = read_secret("Repeat the password: ") else {
        return 1;
    };
    if a.is_empty() || a != b {
        eprintln!("passwd: the passwords are empty or differ; nothing changed");
        return 1;
    }
    match set_password(&user, &a) {
        Ok(()) => {
            println!("passwd: password for {} changed", user);
            0
        }
        Err(e) => {
            eprintln!("passwd: /etc/shadow: {}", e);
            1
        }
    }
}

//! passwd [USER]: sets a password (root only: there are no setuid
//! programs yet, and /etc/shadow is readable and writable only by root).

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

fn main() -> i32 {
    if !need_root() {
        return 1;
    }
    let user = String::from(env::args().get(1).unwrap_or("root"));
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
    let mut salt = [0u8; 16];
    process::random(&mut salt);
    let hash = velora_auth::hash_password(&a, &salt, velora_auth::DEFAULT_ITERATIONS);
    let mut out = String::new();
    for line in shadow.lines() {
        let mut f: alloc::vec::Vec<&str> = line.split(':').collect();
        if f.first() == Some(&user.as_str()) && f.len() > 1 {
            f[1] = &hash;
        }
        out.push_str(&f.join(":"));
        out.push('\n');
    }
    // Write a new file and rename it over the old one.
    let tmp = "/etc/shadow.new";
    let r = vlibc::fs::File::open_with(
        tmp,
        vlibc::fs::flags::WRITE | vlibc::fs::flags::CREATE | vlibc::fs::flags::TRUNCATE,
        0o600,
    )
    .and_then(|mut f| f.write_all(out.as_bytes()))
    .and_then(|_| vlibc::fs::rename(tmp, "/etc/shadow"))
    .and_then(|_| vlibc::fs::sync());
    match r {
        Ok(()) => {
            println!("passwd: password for {} changed", user);
            0
        }
        Err(e) => fail("/etc/shadow", e),
    }
}

//! /bin/login: asks for a user name and a password, checks them against
//! /etc/passwd and /etc/shadow (PBKDF2 hashes) and starts the user's shell
//! as that user. The kernel starts login again when it exits.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use velora_auth::{User, parse_passwd, shadow_hash, verify_password};
use vlibc::{fs, io, print, println, process, term, time};

vlibc::main!(main);

const ATTEMPTS: u32 = 3;

/// Reads one line from the console (line mode); `None` at the end of input.
fn read_line() -> Option<String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        let n = io::read(0, &mut buf).ok()?;
        if n == 0 {
            return if out.is_empty() {
                None
            } else {
                Some(String::from_utf8_lossy(&out).into_owned())
            };
        }
        out.extend_from_slice(&buf[..n]);
        if out.ends_with(b"\n") {
            out.pop();
            return Some(String::from_utf8_lossy(&out).into_owned());
        }
    }
}

fn read_text(path: &str) -> String {
    fs::read(path)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default()
}

fn check(name: &str, password: &str) -> Option<User> {
    let user = parse_passwd(&read_text("/etc/passwd"))
        .into_iter()
        .find(|u| u.name == name);
    let shadow = read_text("/etc/shadow");
    let hash = shadow_hash(&shadow, name).unwrap_or("!");
    // Hash even for unknown users, so both cases take the same time.
    let ok = verify_password(password, hash);
    if ok { user } else { None }
}

fn session(u: &User) -> i32 {
    if !fs::exists(&u.home)
        && let Err(e) = fs::mkdir(&u.home, 0o700).and_then(|_| fs::chown(&u.home, u.uid, u.gid))
    {
        println!("login: cannot create {}: {}", u.home, e);
    }
    let motd = read_text("/etc/motd");
    if !motd.is_empty() {
        print!("{}", motd);
    }
    if let Err(e) = process::setuid(u.uid, u.gid) {
        println!("login: cannot switch to {}: {}", u.name, e);
        return 1;
    }
    if fs::chdir(&u.home).is_err() {
        let _ = fs::chdir("/");
    }
    let env = [
        format!("HOME={}", u.home),
        format!("USER={}", u.name),
        format!("LOGNAME={}", u.name),
        format!("SHELL={}", u.shell),
        String::from("PATH=/bin"),
        String::from("TERM=velora"),
    ];
    let env: Vec<&str> = env.iter().map(String::as_str).collect();
    match process::spawn(&u.shell, &[u.shell.as_str()], &env) {
        Ok(pid) => {
            let _ = process::wait(pid);
            0
        }
        Err(e) => {
            println!("login: cannot start {}: {}", u.shell, e);
            1
        }
    }
}

fn main() -> i32 {
    term::set_mode(0);
    let version = process::sysinfo()
        .map(|s| String::from(process::field_str(&s.version)))
        .unwrap_or_default();
    println!();
    println!("Velora OS {} (console)", version);
    println!();
    for _ in 0..ATTEMPTS {
        print!("velora login: ");
        let Some(name) = read_line() else { return 0 };
        let name = String::from(name.trim());
        if name.is_empty() {
            continue;
        }
        print!("Password: ");
        term::set_mode(term::NO_ECHO);
        let password = read_line().unwrap_or_default();
        term::set_mode(0);
        match check(&name, &password) {
            Some(u) => return session(&u),
            None => {
                time::sleep_ms(1000);
                println!("Login incorrect");
                println!();
            }
        }
    }
    0
}

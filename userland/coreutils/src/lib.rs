//! Helpers shared by the commands in `src/bin`.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use vlibc::fs::{self, Stat, mode};
use vlibc::{Errno, env, eprintln, process, term};

/// Program name for error messages (argv[0] without the directory).
pub fn prog() -> &'static str {
    let a = env::args().get(0).unwrap_or("?");
    a.rsplit('/').next().unwrap_or(a)
}

/// Prints "name: what: error" and returns status 1.
pub fn fail(what: &str, e: Errno) -> i32 {
    eprintln!("{}: {}: {}", prog(), what, describe(e));
    1
}

pub fn describe(e: Errno) -> &'static str {
    use vlibc::abi::errno::*;
    match e.0 {
        ENOENT => "no such file or directory",
        EEXIST => "already exists",
        ENOTDIR => "not a directory",
        EISDIR => "is a directory",
        ENOTEMPTY => "directory not empty",
        EACCES => "permission denied",
        EPERM => "operation not permitted",
        EROFS => "read-only file system",
        ENOSPC => "no space left on device",
        EXDEV => "cannot move between file systems",
        EBUSY => "busy (mount point)",
        ESRCH => "no such process",
        EINVAL => "invalid argument",
        EIO => "input/output error",
        ECONNREFUSED => "connection refused",
        ETIMEDOUT => "timed out",
        ENETUNREACH => "network unreachable",
        ENETDOWN => "no network",
        ECONNRESET => "connection reset",
        EADDRINUSE => "address in use",
        _ => e.name(),
    }
}

/// Splits arguments into flags (single letters after '-') and operands.
/// `--` ends the flags.
pub fn parse_args(allowed: &str) -> Result<(Vec<char>, Vec<&'static str>), String> {
    let mut flags = Vec::new();
    let mut ops = Vec::new();
    let mut only_ops = false;
    for a in env::args().iter().skip(1) {
        if only_ops || !a.starts_with('-') || a == "-" {
            ops.push(a);
        } else if a == "--" {
            only_ops = true;
        } else {
            for c in a[1..].chars() {
                if !allowed.contains(c) {
                    return Err(format!("unknown option -{}", c));
                }
                flags.push(c);
            }
        }
    }
    Ok((flags, ops))
}

/// True when standard output is the console (colours are used only then).
pub fn colors() -> bool {
    term::is_console(1)
}

pub fn is_dir(st: &Stat) -> bool {
    st.mode & mode::TYPE_MASK == mode::DIR
}

/// "drwxr-xr-x" style mode text.
pub fn mode_string(m: u32) -> String {
    let t = match m & mode::TYPE_MASK {
        mode::DIR => 'd',
        mode::CHAR_DEVICE => 'c',
        mode::BLOCK_DEVICE => 'b',
        mode::FIFO => 'p',
        _ => '-',
    };
    let mut s = String::from(t);
    for shift in [6, 3, 0] {
        let b = (m >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    s
}

fn read_text(path: &str) -> String {
    fs::read(path)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default()
}

/// User name for a uid (the number if unknown).
pub fn user_name(uid: u32) -> String {
    velora_auth::parse_passwd(&read_text("/etc/passwd"))
        .into_iter()
        .find(|u| u.uid == uid)
        .map_or_else(|| format!("{}", uid), |u| u.name)
}

pub fn group_name(gid: u32) -> String {
    velora_auth::parse_group(&read_text("/etc/group"))
        .into_iter()
        .find(|g| g.gid == gid)
        .map_or_else(|| format!("{}", gid), |g| g.name)
}

/// (year, month, day, hour, minute, second) for seconds since 1970 UTC.
pub fn civil(unix: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (unix / 86_400) as i64;
    let s = unix % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + (m <= 2) as i64;
    (
        y,
        m,
        d,
        (s / 3600) as u32,
        (s / 60 % 60) as u32,
        (s % 60) as u32,
    )
}

pub fn format_time(unix: u64) -> String {
    let (y, mo, d, h, mi, s) = civil(unix);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, mi, s)
}

/// "1.5 MiB" style size.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", v, UNITS[u])
    }
}

pub fn duration(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        format!("{} d {:02}:{:02}:{:02}", d, h, m, s)
    } else {
        format!("{:02}:{:02}:{:02}", h, m, s)
    }
}

/// Joins a directory and a name.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{}{}", dir, name)
    } else {
        format!("{}/{}", dir, name)
    }
}

/// Last path component.
pub fn base_name(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

/// Copies a file's bytes; the new file gets the source's permissions.
pub fn copy_file(from: &str, to: &str) -> Result<(), Errno> {
    let st = fs::stat(from)?;
    let data = fs::read(from)?;
    let mut f = fs::File::open_with(
        to,
        fs::flags::WRITE | fs::flags::CREATE | fs::flags::TRUNCATE,
        st.mode & 0o777,
    )?;
    f.write_all(&data)
}

/// Requires root; prints an error otherwise.
pub fn need_root() -> bool {
    if process::getuid().0 != 0 {
        eprintln!("{}: only root can do this", prog());
        return false;
    }
    true
}

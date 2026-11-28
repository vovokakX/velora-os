//! What Lumi needs from the system. The `lumi` program implements it with
//! vlibc; the host examples and tests use a fake one.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// The sound card's volume, mute and outputs (`/dev/mixer`).
pub use velora_hda::mixer::State as Mixer;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// Seconds since 1970 (UTC), 0 if unknown.
    pub mtime: u64,
    pub exec: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SysInfo {
    pub version: String,
    pub cpu: String,
    pub cpus: u32,
    pub mem_total: u64,
    pub mem_used: u64,
    pub uptime_s: u64,
    pub processes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetState {
    /// A network card is there.
    pub present: bool,
    /// It has an address (DHCP done).
    pub ip: Option<[u8; 4]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInfo {
    pub name: String,
    pub uid: u32,
    pub home: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Off,
    Restart,
}

/// A read from a terminal program's output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermRead {
    Data(usize),
    /// Nothing waiting.
    Empty,
    /// The program (and everything it started) has ended.
    Closed,
}

/// A program running in a pseudo-terminal.
pub trait Term {
    fn read(&mut self, buf: &mut [u8]) -> TermRead;
    fn write(&mut self, data: &[u8]);
    fn resize(&mut self, cols: u32, rows: u32);
    /// Ends the program (its window was closed).
    fn kill(&mut self);
    /// The exit status once [`TermRead::Closed`] was read, if known.
    fn status(&self) -> Option<i64> {
        None
    }
}

/// Why a program ended, from its exit status: `None` for an ordinary
/// end, else a short reason (a panic or a processor exception; 128 + 2
/// and 128 + 15 are what Lumi and `kill` use to end programs).
pub fn crash_reason(status: i64) -> Option<String> {
    const NAMES: [&str; 22] = [
        "divide error",
        "debug",
        "",
        "breakpoint",
        "overflow",
        "bound range",
        "invalid opcode",
        "no FPU",
        "double fault",
        "",
        "invalid TSS",
        "segment not present",
        "stack fault",
        "general protection",
        "page fault",
        "",
        "x87 error",
        "alignment check",
        "machine check",
        "SIMD error",
        "virtualization",
        "control protection",
    ];
    match status {
        101 => Some(String::from("panic")),
        128..=149 if status != 130 && status != 143 => {
            let v = (status - 128) as usize;
            Some(alloc::format!("exception {}: {}", v, NAMES[v]))
        }
        _ => None,
    }
}

pub trait Host {
    /// Milliseconds since boot.
    fn now_ms(&self) -> u64;
    /// Seconds since 1970 (UTC), if the clock is known.
    fn utc(&self) -> Option<i64>;
    fn read(&mut self, path: &str) -> Option<Vec<u8>>;
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), String>;
    fn list(&mut self, path: &str) -> Result<Vec<DirEntry>, String>;
    fn mkdir(&mut self, path: &str) -> Result<(), String>;
    /// Removes a file or an empty directory.
    fn remove(&mut self, path: &str) -> Result<(), String>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), String>;
    /// Starts `argv` in a new pseudo-terminal of `cols` x `rows`.
    fn spawn_term(&mut self, argv: &[&str], cols: u32, rows: u32) -> Result<Box<dyn Term>, String>;
    fn sysinfo(&mut self) -> SysInfo;
    fn net(&mut self) -> NetState;
    fn power(&mut self, p: Power) -> Result<(), String>;
    /// People who can log in (uid 1000 and above, then root).
    fn users(&mut self) -> Vec<UserInfo>;
    /// Checks the password; on success the program becomes that user
    /// (its later files and programs are theirs).
    fn login(&mut self, name: &str, password: &str) -> Result<UserInfo, String>;
    /// A progress line for the console (the QEMU tests follow them).
    fn log(&mut self, line: &str);
    /// Changes the logged-in user's password (`old` must be right).
    fn change_password(&mut self, old: &str, new: &str) -> Result<(), String>;
    /// The sound card's volume, mute and outputs; `None` without one.
    fn mixer(&mut self) -> Option<Mixer> {
        None
    }
    /// Sends a mixer command (`volume N`, `mute 0|1`, `output N`).
    fn set_mixer(&mut self, _cmd: &str) -> bool {
        false
    }
    /// Plays 48 kHz 16-bit stereo samples; returns the bytes taken (the
    /// rest did not fit and is dropped).
    fn play(&mut self, _pcm: &[u8]) -> usize {
        0
    }
    /// The screen's size in pixels and the bits per pixel.
    fn display_mode(&self) -> (u32, u32, u32) {
        (0, 0, 32)
    }
}

/// `a/b` without doubled slashes.
pub fn join(dir: &str, name: &str) -> String {
    let mut s = String::from(dir.trim_end_matches('/'));
    s.push('/');
    s.push_str(name.trim_start_matches('/'));
    s
}

/// The parent directory of a path ("/" for the root).
pub fn parent(path: &str) -> String {
    let t = path.trim_end_matches('/');
    match t.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(i) => String::from(&t[..i]),
    }
}

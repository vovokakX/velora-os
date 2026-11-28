//! /bin/lumi: the graphical shell. The kernel starts it instead of the
//! console login when there is a screen; it shows the login screen, then
//! the desktop of the user who logged in. Logging out ends the program
//! and the kernel starts it again. Without a screen or fonts it ends with
//! status 2 and the kernel falls back to the console login.
//!
//! Progress lines ("lumi: ready", "lumi: session user", ...) go to the
//! console, which also reaches the serial port; the QEMU tests follow
//! them. They also go to /var/log/lumi.log with the time, as do panics.
//! If Lumi crashes, the kernel starts it again with `--crashed STATUS`
//! (three crashes in a minute: the console login).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use velora_auth::{parse_passwd, shadow_hash, verify_password};
use velora_lumi::apps::Assets;
use velora_lumi::config::{self, Config};
use velora_lumi::host::{DirEntry, Host, NetState, Power, SysInfo, Term, TermRead, UserInfo};
use velora_lumi::{Desktop, Exit, Pointer as Shape};
use velora_ui::Event;
use velora_ui::image::{Picture, Pointer};
use velora_ui::screen::Screen;
use velora_ui::text::Fonts;
use velora_ui::ui::{Key, mods};
use vlibc::{Errno, display, fs, io, println, process, term, time};

vlibc::main!(main);

const FONT_DIR: &str = "/usr/share/fonts";
const LOG_FILE: &str = "/var/log/lumi.log";
const LOG_OLD: &str = "/var/log/lumi.log.1";
/// The log starts again (the old one kept as lumi.log.1) past this size.
const LOG_MAX: u64 = 256 * 1024;
/// Descriptor of the log, `u64::MAX` if it could not be opened.
static LOG_FD: AtomicU64 = AtomicU64::new(u64::MAX);

/// Opens the log for appending while Lumi still runs as root. The
/// descriptor stays open after login; programs Lumi starts get only
/// descriptors 0, 1 and 2.
fn open_log() {
    let _ = fs::mkdir("/var", 0o755);
    let _ = fs::mkdir("/var/log", 0o755);
    if fs::stat(LOG_FILE).is_ok_and(|s| s.size > LOG_MAX) {
        let _ = fs::remove(LOG_OLD);
        let _ = fs::rename(LOG_FILE, LOG_OLD);
    }
    let fl = fs::flags::WRITE | fs::flags::CREATE | fs::flags::APPEND;
    if let Ok(f) = fs::File::open_with(LOG_FILE, fl, 0o644) {
        LOG_FD.store(f.fd(), Ordering::Relaxed);
        core::mem::forget(f);
    }
}

/// A line of the log: "2026-11-28 12:42:01 UTC <text>".
fn log_file(text: &str) {
    let fd = LOG_FD.load(Ordering::Relaxed);
    if fd == u64::MAX {
        return;
    }
    let mut buf = Line::default();
    if let Ok(t) = time::realtime() {
        let l = velora_lumi::tz::local("UTC", t.as_secs() as i64);
        let _ = core::fmt::Write::write_fmt(
            &mut buf,
            format_args!(
                "{}-{:02}-{:02} {:02}:{:02}:{:02} UTC ",
                l.year, l.month, l.day, l.hour, l.minute, l.second
            ),
        );
    }
    let _ = core::fmt::Write::write_str(&mut buf, text);
    let _ = core::fmt::Write::write_str(&mut buf, "\n");
    let _ = io::write_all(fd, buf.bytes());
}

/// A line built without the heap (also for panics).
struct Line {
    buf: [u8; 512],
    len: usize,
}

impl Default for Line {
    fn default() -> Self {
        Line {
            buf: [0; 512],
            len: 0,
        }
    }
}

impl Line {
    fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // Keep room for the newline; cut long text at a char boundary.
        let room = self.buf.len() - 1 - self.len;
        let mut n = s.len().min(room);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Writes a panic to the log before Lumi ends (the kernel starts it
/// again).
fn panic_to_log(info: &core::panic::PanicInfo) {
    let mut msg = Line::default();
    let _ = core::fmt::Write::write_fmt(&mut msg, format_args!("lumi: panic: {}", info));
    let text = core::str::from_utf8(msg.bytes()).unwrap_or("lumi: panic");
    log_file(text);
}
/// Time between frames while something moves (60 per second), in us.
const FRAME_US: u64 = 16_667;

fn us() -> u64 {
    time::monotonic().map_or(0, |d| d.as_micros() as u64)
}

fn err(e: Errno) -> String {
    String::from(e.name())
}

/// A program in a pseudo-terminal.
struct PtyTerm {
    master: u64,
    pid: u64,
    done: bool,
    status: Option<i64>,
}

impl Term for PtyTerm {
    fn read(&mut self, buf: &mut [u8]) -> TermRead {
        if self.done {
            return TermRead::Closed;
        }
        match io::read(self.master, buf) {
            Ok(0) => {
                self.done = true;
                self.status = process::wait(self.pid).ok();
                TermRead::Closed
            }
            Ok(n) => TermRead::Data(n),
            Err(_) => TermRead::Empty,
        }
    }

    fn write(&mut self, data: &[u8]) {
        let _ = io::write_all(self.master, data);
    }

    fn resize(&mut self, cols: u32, rows: u32) {
        let _ = term::pty_resize(self.master, cols, rows);
    }

    fn kill(&mut self) {
        if !self.done {
            let _ = process::kill(self.pid, 130);
            self.done = true;
            self.status = process::wait(self.pid).ok();
        }
    }

    fn status(&self) -> Option<i64> {
        self.status
    }
}

impl Drop for PtyTerm {
    fn drop(&mut self) {
        let _ = fs::close(self.master);
    }
}

struct System {
    env: Vec<String>,
}

impl Host for System {
    fn now_ms(&self) -> u64 {
        time::monotonic().map_or(0, |d| d.as_millis() as u64)
    }

    fn utc(&self) -> Option<i64> {
        time::realtime().ok().map(|d| d.as_secs() as i64)
    }

    fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        fs::read(path).ok()
    }

    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        fs::write(path, data).map_err(err)
    }

    fn list(&mut self, path: &str) -> Result<Vec<DirEntry>, String> {
        let mut out = Vec::new();
        for e in fs::read_dir(path).map_err(err)? {
            let e = e.map_err(err)?;
            if e.name == "." || e.name == ".." {
                continue;
            }
            let full = velora_lumi::host::join(path, &e.name);
            let st = fs::stat(&full).ok();
            out.push(DirEntry {
                dir: e.is_dir,
                size: st.map_or(0, |s| s.size),
                mtime: st.map_or(0, |s| s.mtime),
                exec: st.is_some_and(|s| s.mode & 0o111 != 0) && !e.is_dir,
                name: e.name,
            });
        }
        Ok(out)
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        fs::mkdir(path, 0o755).map_err(err)
    }

    fn remove(&mut self, path: &str) -> Result<(), String> {
        fs::remove(path).map_err(err)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        fs::rename(from, to).map_err(err)
    }

    fn spawn_term(&mut self, argv: &[&str], cols: u32, rows: u32) -> Result<Box<dyn Term>, String> {
        let (master, slave) = term::pty_open(cols, rows).map_err(err)?;
        // The child gets the slave as descriptors 0, 1 and 2.
        let saved: Vec<Option<u64>> = (0..3).map(|fd| io::dup(fd).ok()).collect();
        for fd in 0..3 {
            let _ = io::dup2(slave, fd);
        }
        let env: Vec<&str> = self.env.iter().map(String::as_str).collect();
        let pid = process::spawn(argv[0], argv, &env);
        for (fd, s) in saved.iter().enumerate() {
            if let Some(s) = s {
                let _ = io::dup2(*s, fd as u64);
                let _ = fs::close(*s);
            }
        }
        let _ = fs::close(slave);
        match pid {
            Ok(pid) => Ok(Box::new(PtyTerm {
                master,
                pid,
                done: false,
                status: None,
            })),
            Err(e) => {
                let _ = fs::close(master);
                Err(err(e))
            }
        }
    }

    fn sysinfo(&mut self) -> SysInfo {
        match process::sysinfo() {
            Ok(s) => SysInfo {
                version: String::from(process::field_str(&s.version)),
                cpu: String::from(process::field_str(&s.cpu)),
                cpus: s.cpus as u32,
                mem_total: s.mem_total,
                mem_used: s.mem_used,
                uptime_s: s.uptime_ns / 1_000_000_000,
                processes: s.processes,
            },
            Err(_) => SysInfo::default(),
        }
    }

    fn net(&mut self) -> NetState {
        match vlibc::net::netinfo() {
            Ok(i) => NetState {
                present: i.flags & 1 != 0,
                ip: (i.flags & 2 != 0).then(|| i.ip.to_be_bytes()),
            },
            Err(_) => NetState::default(),
        }
    }

    fn power(&mut self, p: Power) -> Result<(), String> {
        let cmd = match p {
            Power::Off => vlibc::abi::reboot::POWER_OFF,
            Power::Restart => vlibc::abi::reboot::RESTART,
        };
        let _ = fs::sync();
        Err(err(process::reboot(cmd)))
    }

    fn users(&mut self) -> Vec<UserInfo> {
        let text = fs::read("/etc/passwd")
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        let mut v: Vec<UserInfo> = parse_passwd(&text)
            .into_iter()
            .filter(|u| u.uid >= 1000 || u.uid == 0)
            .map(|u| UserInfo {
                name: u.name,
                uid: u.uid,
                home: u.home,
            })
            .collect();
        v.sort_by_key(|u| if u.uid == 0 { u32::MAX } else { u.uid });
        v
    }

    fn login(&mut self, name: &str, password: &str) -> Result<UserInfo, String> {
        let text = fs::read("/etc/passwd")
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        let user = parse_passwd(&text).into_iter().find(|u| u.name == name);
        let shadow = fs::read("/etc/shadow")
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        let hash = shadow_hash(&shadow, name).unwrap_or("!");
        // Hash even for unknown users, so both cases take the same time.
        let ok = verify_password(password, hash);
        let Some(u) = user.filter(|_| ok) else {
            time::sleep_ms(600);
            return Err(String::from("wrong password"));
        };
        if !fs::exists(&u.home) {
            let _ = fs::mkdir(&u.home, 0o700).and_then(|_| fs::chown(&u.home, u.uid, u.gid));
        }
        process::setuid(u.uid, u.gid).map_err(err)?;
        if fs::chdir(&u.home).is_err() {
            let _ = fs::chdir("/");
        }
        self.env = alloc::vec![
            format!("HOME={}", u.home),
            format!("USER={}", u.name),
            format!("LOGNAME={}", u.name),
            format!("SHELL={}", u.shell),
            String::from("PATH=/bin"),
            String::from("TERM=velora"),
        ];
        Ok(UserInfo {
            name: u.name,
            uid: u.uid,
            home: u.home,
        })
    }

    fn log(&mut self, line: &str) {
        println!("{}", line);
        log_file(line);
    }
}

fn load_fonts() -> Option<Fonts> {
    let read = |f: &str| fs::read(&format!("{}/{}", FONT_DIR, f)).ok();
    let mut f = Fonts::new(
        &read("Inter-Regular.ttf")?,
        &read("Inter-Medium.ttf")?,
        &read("Inter-SemiBold.ttf")?,
    )
    .ok()?;
    if let Some(m) = read("JetBrainsMono-Regular.ttf") {
        let _ = f.add_mono(&m);
    }
    Some(f)
}

fn load_config() -> Config {
    let mut c = Config::default();
    if let Ok(t) = fs::read(config::SYSTEM_FILE) {
        c.apply(&String::from_utf8_lossy(&t));
    }
    if let Ok(t) = fs::read(config::LOCALE_FILE) {
        c.apply_locale(&String::from_utf8_lossy(&t));
    }
    c
}

/// Shows the pointer the desktop asks for, in the chosen style.
fn set_pointer(screen: &mut Screen, d: &mut Desktop, shown: &mut Option<(Shape, usize, bool)>) {
    let want = (d.pointer, d.cfg.pointer, d.cfg.large_pointer);
    if *shown == Some(want) {
        return;
    }
    *shown = Some(want);
    let base = (24 * d.scale / 256) as usize;
    let size = if d.cfg.large_pointer {
        base * 3 / 2
    } else {
        base
    };
    match d.pointer {
        Shape::Arrow => {
            let (img, hx, hy) = d.assets.pointers[d.cfg.pointer.min(2)].at(size.max(12));
            screen.set_pointer(Some((img, hx, hy)));
        }
        s => {
            let (img, hx, hy) = velora_lumi::cursor::resize(s, size.max(12));
            screen.set_pointer(Some((&img, hx, hy)));
        }
    }
    screen.speed = (d.scale as i32).max(256);
}

/// Counters for measurements: Ctrl+Alt+F12 prints them; the end of a
/// window drag prints its frames.
#[derive(Default)]
struct Stats {
    wakeups: u64,
    frames: u64,
    busy_us: u64,
    /// While a window is dragged: start, frames, busy time, longest frame.
    drag: Option<(u64, u64, u64, u64)>,
}

impl Stats {
    fn frame(&mut self, d: &Desktop, start: u64, end: u64, drew: bool) {
        let t = end - start;
        self.wakeups += 1;
        self.busy_us += t;
        if drew {
            self.frames += 1;
        }
        match (&mut self.drag, d.dragging()) {
            (None, true) => self.drag = Some((start, 0, 0, 0)),
            (Some(g), true) => {
                if drew {
                    g.1 += 1;
                    g.2 += t;
                    g.3 = g.3.max(t);
                }
            }
            (Some(g), false) => {
                let (t0, n, busy, max) = *g;
                self.drag = None;
                let ms = (end - t0) / 1000;
                println!(
                    "lumi: drag {} frames in {} ms ({} fps), frame avg {}.{} ms max {}.{} ms",
                    n,
                    ms,
                    n * 1000 / ms.max(1),
                    busy / n.max(1) / 1000,
                    busy / n.max(1) / 100 % 10,
                    max / 1000,
                    max / 100 % 10
                );
            }
            (None, false) => {}
        }
    }

    /// Takes Ctrl+Alt+F12 out of the input and prints the counters.
    fn key(&self, events: &mut Vec<Event>) {
        let before = events.len();
        events.retain(|e| {
            !matches!(e, Event::Chord { key: Key::F(12), mods: m }
                if m & mods::CTRL != 0 && m & mods::ALT != 0)
        });
        if events.len() != before {
            println!(
                "lumi: stats wakeups {} frames {} busy {} ms",
                self.wakeups,
                self.frames,
                self.busy_us / 1000
            );
        }
    }
}

fn main() -> i32 {
    let t0 = us();
    open_log();
    vlibc::set_panic_hook(panic_to_log);
    let args = vlibc::env::args();
    let crashed = args
        .iter()
        .position(|a| a == "--crashed")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<i64>().ok());
    match crashed {
        Some(st) => {
            let why =
                velora_lumi::host::crash_reason(st).unwrap_or_else(|| format!("status {}", st));
            let line = format!("lumi: started again after a crash (status {}: {})", st, why);
            println!("{}", line);
            log_file(&line);
        }
        None => log_file("lumi: started"),
    }
    let cfg = load_config();
    if cfg.session == "console" {
        println!("lumi: session=console in {}", config::SYSTEM_FILE);
        return 2;
    }
    let Some(mut fonts) = load_fonts() else {
        println!("lumi: no fonts in {}", FONT_DIR);
        return 2;
    };
    let mut screen = match Screen::open() {
        Ok(s) => s,
        Err(e) => {
            println!("lumi: no screen ({})", e.name());
            return 2;
        }
    };
    screen.extended_input = true;
    let p = |data: &[u8]| Pointer::from_png(data).expect("pointer");
    let assets = Assets {
        logo: Picture::from_png_on_background(include_bytes!("../../../assets/velora-logo.png"))
            .expect("logo"),
        pointers: [
            p(include_bytes!("../../../assets/cursors/classic.png")),
            p(include_bytes!("../../../assets/cursors/blade.png")),
            p(include_bytes!("../../../assets/cursors/drop.png")),
        ],
    };
    let mut host = System {
        env: alloc::vec![String::from("PATH=/bin")],
    };
    let (w, h) = (screen.width(), screen.height());
    let mut d = Desktop::new(w, h, cfg, assets, &mut host);
    println!("lumi: ready {}x{} in {} ms", w, h, (us() - t0) / 1000);
    let mut shown = None;
    let mut reported_login = false;
    let mut stats = Stats::default();
    loop {
        let start = us();
        let mut events = screen.poll();
        stats.key(&mut events);
        let rects = d.frame(&events, &mut screen.canvas, &mut fonts, &mut host);
        set_pointer(&mut screen, &mut d, &mut shown);
        // Input that came while drawing moves the pointer now.
        let mut more = screen.poll();
        stats.key(&mut more);
        let drew = !rects.is_empty();
        if rects.is_empty() {
            screen.show_pointer();
        }
        for r in &rects {
            screen.present(*r);
        }
        if !reported_login {
            reported_login = true;
            println!("lumi: login screen");
        }
        if !more.is_empty() {
            let rects = d.frame(&more, &mut screen.canvas, &mut fonts, &mut host);
            for r in &rects {
                screen.present(*r);
            }
        }
        d.frame_us = us() - start;
        stats.frame(&d, start, us(), drew);
        match d.exit.take() {
            Some(Exit::Logout) => {
                println!("lumi: logged out");
                return 0;
            }
            Some(Exit::Power(p)) => {
                println!(
                    "lumi: {}",
                    if p == Power::Off {
                        "power off"
                    } else {
                        "restart"
                    }
                );
                if let Err(e) = host.power(p) {
                    println!(
                        "lumi: cannot {}: {}",
                        if p == Power::Off {
                            "power off"
                        } else {
                            "restart"
                        },
                        e
                    );
                }
            }
            None => {}
        }
        // Wait for the next frame: soon while something moves, else
        // until input, terminal output or the next timer.
        let now = us();
        if d.animating() {
            let due = start + FRAME_US;
            while us() + 10_500 < due {
                display::input_wait(1);
            }
            while us() < due {
                process::yield_now();
            }
        } else {
            // Nothing moves: sleep until input, terminal output or the
            // next timer (the clock's minute at the latest).
            let wake = d.wake_at() * 1000;
            let ms = (wake.saturating_sub(now) / 1000).clamp(1, 60_000);
            display::input_wait(ms);
        }
    }
}

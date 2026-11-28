//! /bin/lumi: the graphical shell. The kernel starts it instead of the
//! console login when there is a screen; it shows the login screen, then
//! the desktop of the user who logged in. Logging out ends the program
//! and the kernel starts it again. Without a screen or fonts it ends with
//! status 2 and the kernel falls back to the console login.
//!
//! Progress lines ("lumi: ready", "lumi: session user", ...) go to the
//! console, which also reaches the serial port; the QEMU tests follow
//! them.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use velora_auth::{parse_passwd, shadow_hash, verify_password};
use velora_lumi::apps::Assets;
use velora_lumi::config::{self, Config};
use velora_lumi::host::{DirEntry, Host, NetState, Power, SysInfo, Term, TermRead, UserInfo};
use velora_lumi::{Desktop, Exit, Pointer as Shape};
use velora_ui::image::{Picture, Pointer};
use velora_ui::screen::Screen;
use velora_ui::text::Fonts;
use vlibc::{Errno, display, fs, io, println, process, term, time};

vlibc::main!(main);

const FONT_DIR: &str = "/usr/share/fonts";
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
}

impl Term for PtyTerm {
    fn read(&mut self, buf: &mut [u8]) -> TermRead {
        if self.done {
            return TermRead::Closed;
        }
        match io::read(self.master, buf) {
            Ok(0) => {
                self.done = true;
                let _ = process::wait(self.pid);
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
            let _ = process::wait(self.pid);
        }
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

fn main() -> i32 {
    let t0 = us();
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
    loop {
        let start = us();
        let events = screen.poll();
        let rects = d.frame(&events, &mut screen.canvas, &mut fonts, &mut host);
        set_pointer(&mut screen, &mut d, &mut shown);
        // Input that came while drawing moves the pointer now.
        let more = screen.poll();
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
            let wake = d.wake_at() * 1000;
            let ms = (wake.saturating_sub(now) / 1000).clamp(1, 1000);
            display::input_wait(ms);
        }
    }
}

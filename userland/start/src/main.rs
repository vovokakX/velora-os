//! /bin/velora-start: the start menu of the Live system and the graphical
//! installer. The kernel runs it once before the login prompt.
//!
//! The menu offers: 1) graphical installation (the default, started
//! after 10 seconds), 2) installation in the terminal (`velora-setup`),
//! 3) the Live system (the program ends and the login prompt appears).
//! Without a frame buffer the menu is printed on the console and the Live
//! system starts after 10 seconds.
//!
//! `velora-start --install` opens the graphical installer directly;
//! `--no-animations` turns animations off (the kernel passes it for
//! `velora.noanim`). Progress lines ("velora-start: screen welcome", ...)
//! go to the console, which also reaches the serial port; the QEMU tests
//! follow them. With `--uitest` (`velora.uitest`) the installer also
//! reports its interactive areas and a digest of its state after each
//! change, so a test can click every element and see that it reacted.
//!
//! Frames are drawn when something changes; while an animation runs they
//! come at about 60 per second.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use velora_install::live::{self, LiveSource};
use velora_installer_gui::chooser::{Chooser, Mode};
use velora_installer_gui::{Action, App, Assets, DiskEntry};
use velora_ui::image::{Picture, Pointer};
use velora_ui::screen::Screen;
use velora_ui::text::Fonts;
use velora_ui::ui::{Event, one_key_per_frame};
use velora_ui::{Rect, Ui};

/// Time between frames of an animation (about 60 per second).
const FRAME_MS: u64 = 16;
use vlibc::fs::File;
use vlibc::{env, fs, println, process, time};

vlibc::main!(main);

const FONT_DIR: &str = "/usr/share/fonts";

fn ms() -> u64 {
    time::monotonic().map_or(0, |d| d.as_millis() as u64)
}

fn load_fonts() -> Option<Fonts> {
    let read = |f: &str| fs::read(&alloc::format!("{}/{}", FONT_DIR, f)).ok();
    Fonts::new(
        &read("Inter-Regular.ttf")?,
        &read("Inter-Medium.ttf")?,
        &read("Inter-SemiBold.ttf")?,
    )
    .ok()
}

fn load_assets() -> Assets {
    let p = |data: &[u8]| Pointer::from_png(data).expect("pointer");
    Assets {
        logo: Picture::from_png_on_background(include_bytes!("../../../assets/velora-logo.png"))
            .expect("logo"),
        pointers: [
            p(include_bytes!("../../../assets/cursors/classic.png")),
            p(include_bytes!("../../../assets/cursors/blade.png")),
            p(include_bytes!("../../../assets/cursors/drop.png")),
        ],
    }
}

/// The screen with everything the pages need.
struct Gui {
    screen: Screen,
    ui: Ui,
    fonts: Fonts,
    assets: Assets,
    queue: alloc::collections::VecDeque<Vec<Event>>,
    pointer: Option<(usize, bool)>,
    animate: bool,
    uitest: bool,
    /// The last test report, printed again only when it changes.
    reported: String,
}

impl Gui {
    fn open(fonts: Fonts, assets: Assets, animate: bool, uitest: bool) -> Result<Gui, Fonts> {
        let screen = match Screen::open() {
            Ok(s) => s,
            Err(_) => return Err(fonts),
        };
        let mut ui = Ui::new(256);
        ui.set_animations(animate);
        let mut g = Gui {
            screen,
            ui,
            fonts,
            assets,
            queue: alloc::collections::VecDeque::new(),
            pointer: None,
            animate,
            uitest,
            reported: String::new(),
        };
        g.set_pointer(0, false);
        Ok(g)
    }

    /// Starts a frame: the clock for animations, then the input.
    fn begin(&mut self, events: &[Event]) {
        self.ui.set_time(ms());
        self.ui.begin(events);
    }

    /// Input for the next frame. Without an animation it waits for input;
    /// during one it waits until the next frame is due (`due`, in ms).
    fn wait(&mut self, animating: bool, due: u64) -> Vec<Event> {
        if animating {
            self.next_events_until(due)
        } else {
            self.next_events(u64::MAX / 4)
        }
    }

    /// Input until `due` (ms on the clock), or an empty frame then. The
    /// timer ticks every 10 ms, so the last part is waited by yielding.
    fn next_events_until(&mut self, due: u64) -> Vec<Event> {
        loop {
            if let Some(e) = self.queue.pop_front() {
                return e;
            }
            let polled = self.screen.poll();
            if !polled.is_empty() {
                self.queue.extend(one_key_per_frame(&polled));
                continue;
            }
            let now = ms();
            if now >= due {
                return Vec::new();
            }
            if due - now > 10 {
                time::sleep_ms(1);
            } else {
                process::yield_now();
            }
        }
    }

    /// For `--uitest`: the screen, a digest of the state, the focus, the
    /// open list, the last widget that took input and the interactive
    /// areas, printed when any of it changes.
    fn report(&mut self, screen: &str, digest: u32) {
        if !self.uitest {
            return;
        }
        let ui = &self.ui;
        let opt = |v: Option<u32>| v.map_or(String::from("-"), |v| alloc::format!("{}", v));
        let mut line = alloc::format!(
            "velora-start: ui {} state={:08x} focus={} popup={} hit={} scroll={},{} areas",
            screen,
            digest,
            opt(ui.focused()),
            opt(ui.open_popup_id()),
            opt(ui.last_hit),
            ui.scroll_of(20),
            ui.scroll_of(21),
        );
        for (id, r) in &ui.areas {
            line.push_str(&alloc::format!(" {}:{},{},{},{}", id, r.x, r.y, r.w, r.h));
        }
        if line != self.reported {
            println!("{}", line);
            self.reported = line;
        }
    }

    /// Shows pointer style `i` (large: one and a half times the size).
    fn set_pointer(&mut self, i: usize, large: bool) {
        if self.pointer == Some((i, large)) {
            return;
        }
        self.pointer = Some((i, large));
        let (_, scale) =
            velora_installer_gui::window_rect(self.screen.width(), self.screen.height());
        let base = 22 * scale / 256;
        let size = if large { base * 3 / 2 } else { base } as usize;
        let (img, hx, hy) = self.assets.pointers[i.min(2)].at(size.max(12));
        self.screen.set_pointer(Some((img, hx, hy)));
        self.screen.speed = (scale as i32).max(256);
    }

    /// The next input for a frame: waits up to `wait_ms` (0: do not wait).
    fn next_events(&mut self, wait_ms: u64) -> Vec<Event> {
        let end = ms() + wait_ms;
        loop {
            if let Some(e) = self.queue.pop_front() {
                return e;
            }
            let polled = self.screen.poll();
            if !polled.is_empty() {
                self.queue.extend(one_key_per_frame(&polled));
                continue;
            }
            if ms() >= end {
                return Vec::new();
            }
            time::sleep_ms(10);
        }
    }

    fn present(&mut self) {
        let full = Rect::new(0, 0, self.screen.width(), self.screen.height());
        self.screen.present(full);
    }
}

fn main() -> i32 {
    let args = env::args();
    let direct = args.iter().any(|a| a == "--install");
    let animate = !args.iter().any(|a| a == "--no-animations");
    let uitest = args.iter().any(|a| a == "--uitest");
    let t0 = ms();
    let assets = load_assets();
    let Some(fonts) = load_fonts() else {
        println!("velora-start: no fonts in {}; text menu", FONT_DIR);
        return text_menu();
    };
    let mut gui = match Gui::open(fonts, assets, animate, uitest) {
        Ok(g) => g,
        Err(_) => {
            println!("velora-start: no screen; text menu");
            return text_menu();
        }
    };
    println!(
        "velora-start: {}x{}, ready in {} ms",
        gui.screen.width(),
        gui.screen.height(),
        ms() - t0
    );
    if direct {
        installer(&mut gui);
        return 0;
    }
    loop {
        match menu(&mut gui) {
            Mode::Graphical => installer(&mut gui),
            Mode::Terminal => {
                // The console shows again while velora-setup runs.
                let fonts = gui.fonts;
                let assets = gui.assets;
                drop(gui.screen);
                terminal_install();
                gui = match Gui::open(fonts, assets, animate, uitest) {
                    Ok(g) => g,
                    Err(_) => return 0,
                };
            }
            Mode::Live => {
                println!("velora-start: live");
                return 0;
            }
        }
    }
}

/// The start menu until a mode is chosen.
fn menu(gui: &mut Gui) -> Mode {
    let mut ch = Chooser::default();
    let mut reported = false;
    gui.set_pointer(0, false);
    let mut events = Vec::new();
    let mut second = ms() + 1000;
    loop {
        let start = ms();
        gui.begin(&events);
        let (chosen, again) = ch.draw(
            &mut gui.ui,
            &mut gui.screen.canvas,
            &mut gui.fonts,
            &mut gui.assets,
        );
        gui.present();
        if !reported {
            reported = true;
            println!("velora-start: menu");
        }
        if let Some(m) = chosen {
            println!("velora-start: chose {}", m.name());
            return m;
        }
        if again {
            events.clear();
            continue;
        }
        let left = second.saturating_sub(ms());
        events = if gui.ui.animating() {
            gui.next_events_until((start + FRAME_MS).min(second))
        } else {
            gui.next_events(left)
        };
        if events.is_empty() && ms() >= second {
            second += 1000;
            if let Some(m) = ch.tick() {
                println!("velora-start: chose {} (countdown)", m.name());
                return m;
            }
        }
    }
}

fn disk_entries() -> Vec<DiskEntry> {
    live::disks()
        .into_iter()
        .filter(|d| !d.in_use)
        .map(|d| DiskEntry {
            name: d.name,
            size: d.size,
            model: d.model,
        })
        .collect()
}

/// The graphical installer; returns when the user leaves it.
fn installer(gui: &mut Gui) {
    let mut app = App::new(disk_entries());
    app.animate = gui.animate;
    let mut shown = None;
    let mut events = Vec::new();
    loop {
        let (p, large) = app.pointer();
        gui.set_pointer(p, large);
        let before = app.screen;
        let t = ms();
        gui.begin(&events);
        let (action, again) = app.draw(
            &mut gui.ui,
            &mut gui.screen.canvas,
            &mut gui.fonts,
            &mut gui.assets,
        );
        gui.present();
        let took = ms() - t;
        if took > 1000 {
            println!(
                "velora-start: slow frame {} ms ({})",
                took,
                app.screen.name()
            );
        }
        // Reported once a frame of the screen is on the display (a key
        // can change the screen at the end of a frame).
        if app.screen == before && shown != Some(app.screen) {
            shown = Some(app.screen);
            println!("velora-start: screen {}", app.screen.name());
        }
        let animating = gui.ui.animating();
        if !again && !animating && action == Action::None {
            gui.report(app.screen.name(), app.state_digest());
        }
        match action {
            Action::Quit => return,
            Action::Reboot => {
                println!("velora-start: restarting");
                let _ = process::reboot(vlibc::abi::reboot::RESTART);
                return;
            }
            Action::Install => {
                install(gui, &mut app);
                events.clear();
                continue;
            }
            Action::None => {}
        }
        if again {
            events.clear();
            continue;
        }
        events = gui.wait(animating, t + FRAME_MS);
    }
}

/// Runs the installation, drawing the progress page while it works.
fn install(gui: &mut Gui, app: &mut App) {
    let plan = app.plan();
    let Some(disk) = app.selected_disk().cloned() else {
        app.finish(Err(String::from("no disk")));
        return;
    };
    println!("velora-start: installing on {}", disk.name);
    let dev = match live::open_disk(&disk.name) {
        Ok(d) => d,
        Err(e) => {
            app.finish(Err(e));
            return;
        }
    };
    let mut last = (None, u32::MAX);
    let result = {
        let mut draw = |p: velora_install::Progress| {
            app.set_progress(p);
            if last.0 == Some(p.phase)
                && p.permille < last.1.saturating_add(10)
                && p.permille != 1000
            {
                return;
            }
            if last.0 != Some(p.phase) {
                println!("velora-start: phase {:?} {}%", p.phase, p.permille / 10);
            }
            last = (Some(p.phase), p.permille);
            gui.begin(&[]);
            app.draw(
                &mut gui.ui,
                &mut gui.screen.canvas,
                &mut gui.fonts,
                &mut gui.assets,
            );
            gui.present();
        };
        velora_install::install(dev, &plan, &mut LiveSource, &mut draw)
    };
    match result {
        Ok(_) => {
            println!("velora-start: installed");
            app.finish(Ok(()));
        }
        Err(e) => {
            println!("velora-start: failed: {}", e);
            app.finish(Err(e));
        }
    }
}

/// Runs velora-setup on the console and waits for Enter afterwards.
fn terminal_install() {
    println!("velora-start: terminal installation");
    let argv = ["/bin/velora-setup"];
    match process::spawn("/bin/velora-setup", &argv, &["PATH=/bin"]) {
        Ok(pid) => {
            let _ = process::wait(pid);
        }
        Err(e) => println!("velora-start: cannot start velora-setup: {:?}", e),
    }
    println!();
    println!("Press Enter to return to the menu.");
    let mut buf = [0u8; 16];
    let _ = vlibc::io::read(0, &mut buf);
}

/// The menu without a screen: keys 2 and 3 on the keyboard; the Live
/// system starts after 10 seconds.
fn text_menu() -> i32 {
    println!();
    println!("Velora OS");
    println!("  2  Installation in the terminal (velora-setup)");
    println!("  3  Live system");
    println!("The Live system starts in 10 seconds.");
    let mut kbd = File::open("/dev/keyboard").ok();
    let end = ms() + 10_000;
    let mut buf = [0u8; 64];
    while ms() < end {
        if let Some(k) = kbd.as_mut()
            && let Ok(n) = k.read(&mut buf)
        {
            for p in buf[..n].as_chunks::<8>().0 {
                let ch = u32::from_le_bytes([p[4], p[5], p[6], p[7]]);
                if p[2] & 1 == 0 {
                    continue;
                }
                match char::from_u32(ch) {
                    Some('2') => {
                        drop(kbd.take());
                        terminal_install();
                        return 0;
                    }
                    Some('3') => return 0,
                    _ => {}
                }
            }
        }
        time::sleep_ms(50);
    }
    0
}

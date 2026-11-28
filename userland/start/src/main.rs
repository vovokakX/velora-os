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

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use velora_install::live::{self, LiveSource};
use velora_installer_gui::chooser::{Chooser, Mode};
use velora_installer_gui::{Action, App, Assets, DiskEntry};
use velora_ui::image::{Picture, Pointer};
use velora_ui::screen::Screen;
use velora_ui::text::Fonts;
use velora_ui::ui::{Event, Key, Redraw};
use velora_ui::{Rect, Ui};

/// Time between frames (60 per second), in microseconds.
const FRAME_US: u64 = 16_667;
use vlibc::fs::File;
use vlibc::{display, env, fs, println, process, time};

vlibc::main!(main);

const FONT_DIR: &str = "/usr/share/fonts";

fn ms() -> u64 {
    time::monotonic().map_or(0, |d| d.as_millis() as u64)
}

fn us() -> u64 {
    time::monotonic().map_or(0, |d| d.as_micros() as u64)
}

/// The frame counter shown with F12: frames and pointer moves per
/// second, the average and longest frame. While it is on, a line goes to
/// the console every second ("velora-start: fps 60 frame 3.2 ms ...").
#[derive(Default)]
struct Fps {
    on: bool,
    /// Start of the current second (us).
    since: u64,
    frames: u32,
    /// Pointer moves shown and pointer reports read in the last second.
    pointer: (u32, u32),
    /// The screen's counts of both at the start of the second.
    pointer_base: (u32, u32),
    total_us: u64,
    max_us: u64,
    /// Start of the last frame (us) and whether it was animating: the
    /// gaps between frames of an animation show the frame rate it gets.
    last_start: u64,
    animating: bool,
    gaps: u32,
    gap_total: u64,
    gap_max: u64,
    /// The text of the last finished second.
    text: String,
}

impl Fps {
    fn toggle(&mut self, pointer_moves: (u32, u32)) {
        self.on = !self.on;
        *self = Fps {
            on: self.on,
            since: us(),
            pointer_base: pointer_moves,
            text: String::from("fps --"),
            ..Fps::default()
        };
        println!(
            "velora-start: fps counter {}",
            if self.on { "on" } else { "off" }
        );
    }

    /// Counts a frame that started at `start` and took `took` us;
    /// `animating`: another frame of an animation follows.
    fn frame(&mut self, start: u64, took: u64, animating: bool) {
        self.frames += 1;
        self.total_us += took;
        self.max_us = self.max_us.max(took);
        if self.animating && self.last_start > 0 {
            let gap = start - self.last_start;
            self.gaps += 1;
            self.gap_total += gap;
            self.gap_max = self.gap_max.max(gap);
        }
        self.animating = animating;
        self.last_start = start;
    }

    /// Closes the second if it is over; true when the text changed.
    /// `pointer_moves`: the screen's counts of pointer moves shown and
    /// reports read.
    fn tick(&mut self, pointer_moves: (u32, u32)) -> bool {
        let now = us();
        if !self.on || now < self.since + 1_000_000 {
            return false;
        }
        self.pointer = (
            pointer_moves.0.wrapping_sub(self.pointer_base.0),
            pointer_moves.1.wrapping_sub(self.pointer_base.1),
        );
        self.pointer_base = pointer_moves;
        let secs = (now - self.since) as f32 / 1e6;
        let avg = if self.frames > 0 {
            self.total_us as f32 / self.frames as f32 / 1000.0
        } else {
            0.0
        };
        let anim = if self.gaps > 0 {
            alloc::format!(
                "anim {:.1} fps  gap max {:.1} ms",
                self.gaps as f32 * 1e6 / self.gap_total as f32,
                self.gap_max as f32 / 1000.0
            )
        } else {
            String::from("anim --")
        };
        self.text = alloc::format!(
            "{:.0} fps  {:.1} ms  max {:.1} ms  pointer {:.0}/s  input {:.0}/s  {}",
            self.frames as f32 / secs,
            avg,
            self.max_us as f32 / 1000.0,
            self.pointer.0 as f32 / secs,
            self.pointer.1 as f32 / secs,
            anim,
        );
        println!("velora-start: fps {}", self.text);
        self.since = now;
        self.frames = 0;
        self.total_us = 0;
        self.max_us = 0;
        self.gaps = 0;
        self.gap_total = 0;
        self.gap_max = 0;
        true
    }

    /// The counter in the top left corner of the screen.
    fn draw(&self, c: &mut velora_ui::Canvas, fonts: &mut Fonts) -> Rect {
        let r = Self::area();
        if !self.on {
            return r;
        }
        c.fill_rrect(r, 8, 0xd0000000);
        fonts.draw(
            c,
            velora_ui::text::Weight::Medium,
            15,
            r.x + 10,
            r.y + 21,
            &self.text,
            0xffffffff,
        );
        r
    }

    fn area() -> Rect {
        Rect::new(8, 8, 720, 30)
    }
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
    /// Input waiting for frames: at most one key, click or wheel step per
    /// frame (widgets take keys while they draw); pointer moves within a
    /// frame are merged.
    queue: VecDeque<Vec<Event>>,
    pointer: Option<(usize, bool)>,
    animate: bool,
    uitest: bool,
    /// The last test report, printed again only when it changes.
    reported: String,
    fps: Fps,
}

/// How a frame is drawn: its area (the whole screen or a part) and when
/// it started (us).
struct Frame {
    area: Rect,
    start: u64,
    /// The frame counter was switched on or off: the next frame is whole.
    toggled: bool,
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
            queue: VecDeque::new(),
            pointer: None,
            animate,
            uitest,
            reported: String::new(),
            fps: Fps::default(),
        };
        g.set_pointer(0, false);
        Ok(g)
    }

    fn bounds(&self) -> Rect {
        self.screen.canvas.bounds()
    }

    /// Decides what the frame for `events` draws: `full` (or a plan of
    /// the whole screen) draws everything, else only the widgets that
    /// change. None when nothing changes: the frame is skipped (the
    /// pointer already moved).
    fn plan(&mut self, events: &[Event], full: bool) -> Option<Rect> {
        let counter = self.fps.tick(self.pointer_counts());
        let toggles = events.iter().any(|e| {
            matches!(
                e,
                Event::Key {
                    key: Key::F(12),
                    ..
                }
            )
        });
        let plan = if full || toggles {
            Redraw::All
        } else {
            self.ui.plan(events, ms())
        };
        let area = match plan {
            Redraw::All => return Some(self.bounds()),
            Redraw::Area(r) => r.intersect(self.bounds()),
            Redraw::Nothing => Rect::default(),
        };
        let area = if counter && self.fps.on {
            if area.is_empty() {
                Fps::area()
            } else {
                area.union(Fps::area())
            }
        } else {
            area
        };
        if area.is_empty() {
            self.ui.track_pointer(events);
            // A skipped frame ends what the counter takes for an animation.
            self.fps.animating = false;
            None
        } else {
            Some(area)
        }
    }

    /// Starts a frame that draws `area`: the clock for animations, then
    /// the input. F12 turns the frame counter on and off.
    fn begin(&mut self, events: &[Event], area: Rect) -> Frame {
        let mut toggled = false;
        for e in events {
            if let Event::Key {
                key: Key::F(12), ..
            } = e
            {
                self.fps.toggle(self.pointer_counts());
                toggled = true;
            }
        }
        self.screen.canvas.set_frame(area);
        self.ui.set_time(ms());
        self.ui.begin(events);
        Frame {
            area,
            start: us(),
            toggled,
        }
    }

    fn pointer_counts(&self) -> (u32, u32) {
        (self.screen.pointer_moves, self.screen.pointer_reports)
    }

    /// Reads the input that came while the frame was drawn: the pointer
    /// is then shown where the last report put it (its events wait for
    /// the next frame).
    fn catch_up(&mut self) {
        let polled = self.screen.poll();
        if !polled.is_empty() {
            self.queue_events(polled);
        }
    }

    /// For a frame that draws nothing: shows the pointer at its latest
    /// position. Returns when the next frame may start (us).
    fn skip(&mut self) -> u64 {
        let now = us();
        self.catch_up();
        self.screen.show_pointer();
        now + FRAME_US
    }

    /// Shows the frame (with the frame counter, if it is on).
    fn present(&mut self, f: &Frame) {
        self.fps.draw(&mut self.screen.canvas, &mut self.fonts);
        self.catch_up();
        self.screen.present(f.area);
        let b = self.bounds();
        self.screen.canvas.set_frame(b);
        let animating = self.ui.animating();
        self.fps.frame(f.start, us() - f.start, animating);
    }

    /// Adds input read from the devices to the queue.
    fn queue_events(&mut self, events: Vec<Event>) {
        let is_move = |e: &Event| matches!(e, Event::MouseMove { .. });
        for e in events {
            match self.queue.back_mut() {
                Some(batch) if is_move(&e) && batch.last().is_some_and(is_move) => {
                    *batch.last_mut().unwrap() = e;
                }
                Some(batch) if is_move(&e) || batch.iter().all(is_move) => batch.push(e),
                _ => self.queue.push_back(alloc::vec![e]),
            }
        }
    }

    /// Input for the next frame, at most 60 frames a second: it returns
    /// when input is waiting and the frame is due (`due`, us since boot),
    /// or empty at `due` while something moves. Otherwise it sleeps until
    /// input comes; the kernel wakes it at once (`input_wait`). The
    /// pointer is drawn by the frames (or [`Gui::skip`]), so it moves at
    /// most 60 times a second, to the position of the last report.
    fn next_frame(&mut self, animating: bool, due: u64) -> Vec<Event> {
        loop {
            let polled = self.screen.poll();
            if !polled.is_empty() {
                self.queue_events(polled);
            }
            let now = us();
            let busy = animating || !self.queue.is_empty();
            if busy {
                if now >= due {
                    return self.queue.pop_front().unwrap_or_default();
                }
                // The timer ticks every 10 ms: sleep while a tick fits
                // before the frame, then yield.
                if due - now > 10_500 {
                    display::input_wait(1);
                } else {
                    process::yield_now();
                }
                continue;
            }
            if self.fps.on {
                let second = self.fps.since + 1_000_000;
                if now >= second {
                    return Vec::new();
                }
                display::input_wait((second - now).div_ceil(1000));
            } else {
                display::input_wait(1000);
            }
        }
    }

    /// Input for the start menu: like [`Gui::next_frame`], but returns
    /// empty at `until` (us) for its countdown.
    fn next_frame_until(&mut self, animating: bool, due: u64, until: u64) -> Vec<Event> {
        loop {
            let polled = self.screen.poll();
            if !polled.is_empty() {
                self.queue_events(polled);
            }
            let now = us();
            let busy = animating || !self.queue.is_empty();
            if busy && now >= due {
                return self.queue.pop_front().unwrap_or_default();
            }
            if !busy && now >= until {
                return Vec::new();
            }
            let next = if busy { due } else { until };
            if next.saturating_sub(now) > 10_500 {
                display::input_wait(((next - now) / 1000).clamp(1, 1000));
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
    let mut second = us() + 1_000_000;
    // The countdown changes text outside the widgets: whole frames then.
    let mut full = true;
    loop {
        if let Some(area) = gui.plan(&events, full) {
            let f = gui.begin(&events, area);
            let (chosen, again) = ch.draw(
                &mut gui.ui,
                &mut gui.screen.canvas,
                &mut gui.fonts,
                &mut gui.assets,
            );
            gui.present(&f);
            if !reported {
                reported = true;
                println!("velora-start: menu");
            }
            if let Some(m) = chosen {
                println!("velora-start: chose {}", m.name());
                return m;
            }
            full = again || f.toggled;
            if again {
                events.clear();
                continue;
            }
            events = gui.next_frame_until(gui.ui.animating(), f.start + FRAME_US, second);
        } else {
            full = false;
            let due = gui.skip();
            events = gui.next_frame_until(false, due, second);
        }
        if events.is_empty() && us() >= second {
            second += 1_000_000;
            full = true;
            if let Some(m) = ch.tick() {
                println!("velora-start: chose {} (countdown)", m.name());
                return m;
            }
        }
    }
}

/// The disks to offer (all but the one the system was started from) and
/// that one's name.
fn disk_entries() -> (Vec<DiskEntry>, Option<String>) {
    let mut boot = None;
    let mut out = Vec::new();
    for d in live::disks() {
        if d.boot {
            boot = Some(d.name);
            continue;
        }
        out.push(DiskEntry {
            name: d.name,
            size: d.size,
            model: d.model,
            bus: String::from(d.bus),
            in_use: d.in_use,
        });
    }
    (out, boot)
}

/// The graphical installer; returns when the user leaves it.
fn installer(gui: &mut Gui) {
    let (disks, boot) = disk_entries();
    let mut app = App::new(disks);
    app.boot_disk = boot;
    app.animate = gui.animate;
    let mut shown = None;
    let mut events = Vec::new();
    let mut full = true;
    loop {
        let (p, large) = app.pointer();
        gui.set_pointer(p, large);
        let Some(area) = gui.plan(&events, full) else {
            let due = gui.skip();
            events = gui.next_frame(false, due);
            continue;
        };
        let before = app.screen;
        let f = gui.begin(&events, area);
        let (action, again) = app.draw(
            &mut gui.ui,
            &mut gui.screen.canvas,
            &mut gui.fonts,
            &mut gui.assets,
        );
        gui.present(&f);
        let took = us() - f.start;
        if took > 1_000_000 {
            println!(
                "velora-start: slow frame {} ms ({})",
                took / 1000,
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
        full = again || f.toggled || action != Action::None || app.screen != before;
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
            Action::Rescan => {
                let (disks, boot) = disk_entries();
                println!("velora-start: {} disk(s) found", disks.len());
                app.set_disks(disks);
                app.boot_disk = boot;
                events.clear();
                continue;
            }
            Action::None => {}
        }
        if again {
            events.clear();
            continue;
        }
        events = gui.next_frame(animating, f.start + FRAME_US);
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
    if let Err(e) = live::release(&disk.name) {
        println!("velora-start: {}", e);
        app.finish(Err(e));
        return;
    }
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
            let b = gui.bounds();
            let f = gui.begin(&[], b);
            app.draw(
                &mut gui.ui,
                &mut gui.screen.canvas,
                &mut gui.fonts,
                &mut gui.assets,
            );
            gui.present(&f);
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

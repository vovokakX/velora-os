//! Time Lumi takes for frames on the host, 1920x1080 by default:
//! `cargo run --release -p velora-lumi --example bench [-- WxH]`.
//! A window dragged in circles over two others, windows opening and
//! closing, the app menu opening, the wallpaper dimming slider moved, and
//! rest with and without dimming and blur. The time is spent composing
//! the screen (the program then copies the changed areas to the screen).

use std::time::Instant;
use velora_lumi::apps::{AppId, Assets};
use velora_lumi::config::Config;
use velora_lumi::desktop::Desktop;
use velora_lumi::fake::FakeHost;
use velora_ui::image::{Picture, Pointer};
use velora_ui::text::Fonts;
use velora_ui::ui::{Key, Modifier, mods};
use velora_ui::{Canvas, Event};

struct Bench {
    d: Desktop,
    c: Canvas,
    f: Fonts,
    h: FakeHost,
}

impl Bench {
    /// One frame 16 ms later; returns its time in microseconds.
    fn step(&mut self, ev: &[Event]) -> u128 {
        self.h.now += 16;
        let t = Instant::now();
        self.d.frame(ev, &mut self.c, &mut self.f, &mut self.h);
        t.elapsed().as_micros()
    }

    fn wait(&mut self, ms: u64) -> u128 {
        (0..ms / 16).map(|_| self.step(&[])).sum()
    }
}

fn report(what: &str, times: &[u128]) {
    let n = times.len().max(1) as u128;
    let sum: u128 = times.iter().sum();
    let max = times.iter().max().copied().unwrap_or(0);
    println!(
        "{:<28} {:>4} frames  avg {:>6.2} ms  max {:>6.2} ms",
        what,
        times.len(),
        sum as f64 / n as f64 / 1000.0,
        max as f64 / 1000.0
    );
}

fn main() {
    let size = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("1920x1080"));
    let (w, h) = size.split_once('x').expect("WxH");
    let (w, h): (i32, i32) = (w.parse().unwrap(), h.parse().unwrap());
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let read = |p: &str| std::fs::read(format!("{}/{}", root, p)).expect(p);
    let mut f = Fonts::new(
        &read("build/fonts/Inter-Regular.ttf"),
        &read("build/fonts/Inter-Medium.ttf"),
        &read("build/fonts/Inter-SemiBold.ttf"),
    )
    .unwrap();
    f.add_mono(&read("build/fonts/JetBrainsMono-Regular.ttf"))
        .unwrap();
    let p = |n: &str| Pointer::from_png(&read(&format!("assets/cursors/{}.png", n))).unwrap();
    let assets = Assets {
        logo: Picture::from_png_on_background(&read("assets/velora-logo.png")).unwrap(),
        pointers: [p("classic"), p("blade"), p("drop")],
    };
    let mut host = FakeHost::new();
    host.term_output =
        b"user@velora:~$ ls\r\nDocuments  Pictures  notes.txt  todo.md\r\nuser@velora:~$ ".to_vec();
    let d = Desktop::new(w, h, Config::default(), assets, &mut host);
    let mut b = Bench {
        d,
        c: Canvas::new(w as usize, h as usize),
        f,
        h: host,
    };
    b.wait(50);
    for ch in "velora".chars() {
        b.step(&[Event::Key {
            key: Key::Char(ch),
            shift: false,
        }]);
    }
    b.step(&[Event::Key {
        key: Key::Enter,
        shift: false,
    }]);
    b.wait(1500);
    for app in [AppId::Files, AppId::Settings, AppId::Terminal] {
        b.d.launch(app, &mut b.h, &mut b.f);
        b.wait(400);
    }
    // Drag the terminal by its title bar in circles for 4 s.
    let r = b.d.window_rect_of("Терминал").unwrap();
    let (x0, y0) = (r.x + r.w / 2, r.y + 12);
    b.step(&[Event::MouseMove { x: x0, y: y0 }]);
    b.step(&[Event::MouseButton { down: true }]);
    let mut times = Vec::new();
    for i in 0..240 {
        let a = i as f64 * 0.04;
        let x = x0 + (w as f64 * 0.2 * (a.cos() - 1.0)) as i32;
        let y = y0 + (h as f64 * 0.15 * a.sin()) as i32 + h / 6;
        times.push(b.step(&[Event::MouseMove { x, y }]));
    }
    b.step(&[Event::MouseButton { down: false }]);
    report("drag a window", &times);
    b.wait(300);
    // Windows opening and closing.
    let mut open = Vec::new();
    let mut close = Vec::new();
    for i in 0..10 {
        let app = if i % 2 == 0 {
            AppId::Files
        } else {
            AppId::About
        };
        b.d.launch(app, &mut b.h, &mut b.f);
        for _ in 0..20 {
            open.push(b.step(&[]));
        }
        close.push(b.step(&[Event::Chord {
            key: Key::F(4),
            mods: mods::ALT,
        }]));
        for _ in 0..20 {
            close.push(b.step(&[]));
        }
    }
    report("open a window (320 ms)", &open);
    report("close a window (320 ms)", &close);
    // The app menu with Super.
    let sup = |down| Event::Modifier {
        key: Modifier::Super,
        down,
        mods: if down { mods::SUPER } else { 0 },
    };
    let mut menu = Vec::new();
    for _ in 0..3 {
        menu.push(b.step(&[sup(true)]));
        menu.push(b.step(&[sup(false)]));
        for _ in 0..25 {
            menu.push(b.step(&[]));
        }
        menu.push(b.step(&[Event::Key {
            key: Key::Escape,
            shift: false,
        }]));
        for _ in 0..25 {
            menu.push(b.step(&[]));
        }
    }
    report("app menu open and close", &menu);
    rest(&mut b, "rest (1 s)");
    // The dimming slider moved by one step a frame, as when dragged.
    let mut slide = Vec::new();
    for dim in (0..=80).step_by(5).chain((0..80).step_by(5).rev()) {
        let mut cfg = b.d.cfg.clone();
        cfg.wallpaper_dim = dim;
        cfg.wallpaper_blur = 40;
        let t = Instant::now();
        b.d.apply_config(cfg, &mut b.h, false);
        let set = t.elapsed().as_micros();
        slide.push(set + b.step(&[]));
    }
    report("dimming slider", &slide);
    b.wait(500);
    rest(&mut b, "rest, dimmed and blurred");
}

fn rest(b: &mut Bench, what: &str) {
    let idle: Vec<u128> = (0..60).map(|_| b.step(&[])).collect();
    let drawn = idle.iter().filter(|&&t| t > 200).count();
    report(what, &idle);
    println!("frames at rest that drew something: {}", drawn);
}

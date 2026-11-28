//! Draws Lumi's screens on the host with a system in memory:
//! `cargo run --release -p velora-lumi --example screens -- OUT [WxH]
//! [dark|light|gamers]`. Writes PPM files (the tests convert them).

use velora_lumi::apps::{AppId, Assets};
use velora_lumi::config::{Accent, Config};
use velora_lumi::desktop::Desktop;
use velora_lumi::fake::FakeHost;
use velora_ui::image::{Picture, Pointer};
use velora_ui::text::Fonts;
use velora_ui::ui::{Key, Modifier, mods};
use velora_ui::{Canvas, Event};

const SESSION: &str = "\x1b[1;32muser@velora\x1b[0m:\x1b[1;34m~\x1b[0m$ velofetch\n\
\x1b[1;33m      __      __\x1b[0m       \x1b[1;33muser\x1b[0m@\x1b[1;33mvelora\x1b[0m\n\
\x1b[1;33m      \\ \\    / /\x1b[0m       ------------\n\
\x1b[1;33m       \\ \\  / /\x1b[0m        \x1b[1;33mOS\x1b[0m: Velora OS 0.16.0 x86_64\n\
\x1b[1;33m        \\ \\/ /\x1b[0m         \x1b[1;33mShell\x1b[0m: vsh\n\
\x1b[1;33m         \\  /\x1b[0m          \x1b[1;33mDesktop\x1b[0m: Lumi\n\
\x1b[1;33m          \\/\x1b[0m           \x1b[1;33mCPU\x1b[0m: QEMU Virtual CPU (4)\n\
                       \x1b[1;33mMemory\x1b[0m: 47 MiB / 512 MiB\n\
\n\
\x1b[1;32muser@velora\x1b[0m:\x1b[1;34m~\x1b[0m$ ls\n\
\x1b[1;34mDocuments\x1b[0m  \x1b[1;34mPictures\x1b[0m  notes.txt  todo.md\n\
\x1b[1;32muser@velora\x1b[0m:\x1b[1;34m~\x1b[0m$ echo Привет, мир\n\
Привет, мир\n\
\x1b[1;32muser@velora\x1b[0m:\x1b[1;34m~\x1b[0m$ ";

struct Run {
    d: Desktop,
    c: Canvas,
    f: Fonts,
    h: FakeHost,
    out: String,
}

impl Run {
    fn step(&mut self, events: &[Event]) {
        self.h.now += 16;
        self.h.utc = 1_795_869_720 + (self.h.now / 1000) as i64 - 10;
        self.d.frame(events, &mut self.c, &mut self.f, &mut self.h);
    }

    fn wait(&mut self, ms: u64) {
        for _ in 0..ms / 16 {
            self.step(&[]);
        }
    }

    fn key(&mut self, k: Key) {
        self.step(&[Event::Key {
            key: k,
            shift: false,
        }]);
    }

    fn shot(&mut self, name: &str) {
        let path = format!("{}/{}.ppm", self.out, name);
        std::fs::write(&path, self.c.to_ppm()).unwrap();
        println!("{}", path);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args
        .first()
        .cloned()
        .unwrap_or_else(|| "build/lumi-screens".into());
    let (w, h) = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap()))
        .unwrap_or((1920, 1080));
    let theme = args.get(2).map_or("dark", |s| s.as_str());
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
    std::fs::create_dir_all(&out).unwrap();
    let mut cfg = Config::default();
    cfg.theme = match theme {
        "light" => 1,
        "gamers" => 2,
        _ => 0,
    };
    cfg.accent = Accent::Preset(if cfg.theme == 2 { 7 } else { 0 });
    let mut host = FakeHost::new();
    host.term_output = SESSION.as_bytes().to_vec();
    let d = Desktop::new(w, h, cfg, assets, &mut host);
    let mut r = Run {
        d,
        c: Canvas::new(w as usize, h as usize),
        f,
        h: host,
        out,
    };
    r.wait(100);
    for ch in "velora".chars() {
        r.key(Key::Char(ch));
    }
    r.wait(50);
    r.shot("01-login");
    r.key(Key::Enter);
    r.wait(1200);
    r.shot("02-desktop");
    // The app menu with the Super key.
    let sup = |down| Event::Modifier {
        key: Modifier::Super,
        down,
        mods: if down { mods::SUPER } else { 0 },
    };
    r.step(&[sup(true)]);
    r.step(&[sup(false)]);
    r.wait(120);
    r.shot("03-app-menu-opening");
    r.wait(400);
    r.shot("04-app-menu");
    for ch in "наст".chars() {
        r.key(Key::Char(ch));
    }
    r.wait(200);
    r.shot("05-app-menu-search");
    r.key(Key::Escape);
    r.wait(300);
    // Windows.
    r.d.launch(AppId::Files, &mut r.h, &mut r.f);
    r.wait(400);
    r.shot("06-files");
    r.d.launch(AppId::Terminal, &mut r.h, &mut r.f);
    r.wait(400);
    r.shot("07-terminal");
    r.d.launch(AppId::Settings, &mut r.h, &mut r.f);
    r.wait(400);
    r.shot("08-settings");
    r.d.launch(AppId::About, &mut r.h, &mut r.f);
    r.wait(400);
    r.shot("09-about");
    // Alt+Tab.
    r.step(&[Event::Chord {
        key: Key::Tab,
        mods: mods::ALT,
    }]);
    r.wait(200);
    r.shot("10-alt-tab");
    r.step(&[Event::Modifier {
        key: Modifier::Alt,
        down: false,
        mods: 0,
    }]);
    r.wait(300);
    // The clock's small window: a click on the clock.
    let p = r.d.panel_rect();
    let (x, y) = (p.right() - p.h * 3 / 4, p.center_y());
    r.step(&[Event::MouseMove { x, y }]);
    r.step(&[Event::MouseButton { down: true }]);
    r.step(&[Event::MouseButton { down: false }]);
    r.wait(400);
    r.shot("11-clock");
    r.key(Key::Escape);
    r.wait(300);
    // Settings, page by page (a click on each row of the side bar).
    r.d.launch(AppId::Settings, &mut r.h, &mut r.f);
    r.wait(400);
    let win = r.d.window_rect_of("Настройки").unwrap();
    let sc = r.d.scale;
    let s = |d: i32| ((d as i64 * sc + 128) >> 8) as i32;
    let top = win.y + s(40);
    let row_h = ((win.bottom() - top - s(24)) / 10).min(s(42));
    let pages = [
        (0, "12-settings-appearance"),
        (1, "13-settings-wallpaper"),
        (2, "14-settings-panel"),
        (3, "15-settings-windows"),
        (4, "16-settings-input"),
        (5, "17-settings-display"),
        (6, "18-settings-time"),
        (8, "19-settings-user"),
    ];
    for (i, name) in pages {
        let (x, y) = (win.x + s(100), top + s(12) + i * row_h + row_h / 2);
        r.step(&[Event::MouseMove { x, y }]);
        r.step(&[Event::MouseButton { down: true }]);
        r.step(&[Event::MouseButton { down: false }]);
        r.step(&[Event::MouseMove {
            x: win.right() - s(20),
            y: win.bottom() - s(20),
        }]);
        r.wait(400);
        r.shot(name);
    }
    // A window maximized: smart hiding takes the panel away; the pointer
    // at the bottom edge brings it back.
    r.d.launch(AppId::Terminal, &mut r.h, &mut r.f);
    r.wait(300);
    r.step(&[Event::Chord {
        key: Key::Up,
        mods: mods::SUPER,
    }]);
    r.step(&[Event::MouseMove { x: w / 2, y: h / 3 }]);
    r.wait(1200);
    r.shot("20-panel-hidden");
    r.step(&[Event::MouseMove { x: w / 2, y: h - 3 }]);
    r.wait(500);
    r.shot("21-panel-shown-at-the-edge");
    r.step(&[Event::MouseMove { x: w / 2, y: h / 3 }]);
    while !r.d.window_titles().is_empty() {
        r.step(&[Event::Chord {
            key: Key::F(4),
            mods: mods::ALT,
        }]);
        r.wait(300);
    }
    r.d.launch(AppId::Files, &mut r.h, &mut r.f);
    r.wait(600);
    // Ctrl+Alt+D: every area drawn again is tinted (here a window moved
    // by its title bar and the pointer over the panel).
    r.step(&[Event::Chord {
        key: Key::Char('d'),
        mods: mods::CTRL | mods::ALT,
    }]);
    r.wait(100);
    let t = r.d.window_rect_of("Домашняя папка").unwrap();
    let (x, y) = (t.x + t.w / 2, t.y + s(14));
    r.step(&[Event::MouseMove { x, y }]);
    r.step(&[Event::MouseButton { down: true }]);
    for i in 1..8 {
        r.step(&[Event::MouseMove {
            x: x + i * s(14),
            y: y + i * s(6),
        }]);
    }
    r.step(&[Event::MouseButton { down: false }]);
    r.wait(300);
    r.shot("22-damage-debug");
}

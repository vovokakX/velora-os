//! The desktop on a system in memory: logging in, the app menu, windows,
//! the switcher, settings; and frames drawn by changed areas must equal
//! a full redraw. Needs the fonts in build/fonts (tools/fetch-fonts.sh).

use velora_lumi::apps::{AppId, Assets};
use velora_lumi::config::Config;
use velora_lumi::desktop::{Desktop, Mode};
use velora_lumi::fake::FakeHost;
use velora_ui::image::{Picture, Pointer};
use velora_ui::text::Fonts;
use velora_ui::ui::{Key, Modifier, mods};
use velora_ui::{Canvas, Event};

struct Run {
    d: Desktop,
    c: Canvas,
    f: Fonts,
    h: FakeHost,
}

impl Run {
    fn new(w: i32, h: i32) -> Run {
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
        host.term_output = b"user@velora:~$ ".to_vec();
        let d = Desktop::new(w, h, Config::default(), assets, &mut host);
        Run {
            d,
            c: Canvas::new(w as usize, h as usize),
            f,
            h: host,
        }
    }

    fn step(&mut self, ev: &[Event]) {
        self.h.now += 16;
        self.d.frame(ev, &mut self.c, &mut self.f, &mut self.h);
    }

    fn wait(&mut self, ms: u64) {
        for _ in 0..ms / 16 {
            self.step(&[]);
        }
    }

    fn typed(&mut self, s: &str) {
        for ch in s.chars() {
            self.step(&[Event::Key {
                key: Key::Char(ch),
                shift: false,
            }]);
        }
    }

    fn key(&mut self, k: Key) {
        self.step(&[Event::Key {
            key: k,
            shift: false,
        }]);
    }

    fn login(&mut self) {
        self.wait(50);
        self.typed("velora");
        self.key(Key::Enter);
        self.wait(1500);
        assert_eq!(self.d.mode, Mode::Session);
    }

    fn titles(&self) -> Vec<String> {
        self.d.window_titles()
    }
}

#[test]
fn login_menu_windows_and_settings() {
    let mut r = Run::new(1280, 720);
    // A wrong password keeps the login screen.
    r.wait(50);
    r.typed("nope");
    r.key(Key::Enter);
    r.wait(100);
    assert_eq!(r.d.mode, Mode::Login);
    assert!(r.h.log.iter().any(|l| l == "lumi: login failed"));
    r.login();
    assert_eq!(r.h.logged_in.as_deref(), Some("user"));
    // Super alone opens the app menu; typing finds the terminal.
    let sup = |down| Event::Modifier {
        key: Modifier::Super,
        down,
        mods: if down { mods::SUPER } else { 0 },
    };
    r.step(&[sup(true)]);
    r.step(&[sup(false)]);
    r.wait(300);
    r.typed("терм");
    r.key(Key::Enter);
    r.wait(400);
    assert_eq!(r.titles(), vec!["Терминал"]);
    assert_eq!(r.h.terms.len(), 1, "the terminal started a program");
    // Typing goes to the terminal's program.
    r.typed("ls");
    r.key(Key::Enter);
    r.wait(50);
    assert_eq!(r.h.terms[0].borrow().written, b"ls\n");
    // Ctrl+C is the control character.
    r.step(&[Event::Chord {
        key: Key::Char('c'),
        mods: mods::CTRL,
    }]);
    assert_eq!(r.h.terms[0].borrow().written.last(), Some(&3));
    // A second window, then Alt+Tab back to the terminal.
    r.d.launch(AppId::Files, &mut r.h, &mut r.f);
    r.wait(300);
    assert_eq!(r.d.focused_title().as_deref(), Some("Домашняя папка"));
    r.step(&[Event::Chord {
        key: Key::Tab,
        mods: mods::ALT,
    }]);
    r.wait(100);
    r.step(&[Event::Modifier {
        key: Modifier::Alt,
        down: false,
        mods: 0,
    }]);
    r.wait(100);
    assert_eq!(r.d.focused_title().as_deref(), Some("Терминал"));
    // Alt+F4 closes it and ends its program.
    r.step(&[Event::Chord {
        key: Key::F(4),
        mods: mods::ALT,
    }]);
    r.wait(400);
    assert_eq!(r.titles(), vec!["Домашняя папка"]);
    assert!(r.h.terms[0].borrow().killed);
    // New settings are saved in the user's file.
    let mut cfg = r.d.cfg.clone();
    cfg.theme = 1;
    cfg.wallpaper = String::from("aurora");
    r.d.apply_config(cfg, &mut r.h, true);
    r.wait(500);
    let saved =
        r.h.files
            .get("/home/user/.config/velora/desktop.conf")
            .cloned()
            .flatten()
            .unwrap();
    let text = String::from_utf8(saved).unwrap();
    assert!(
        text.contains("theme=velora-light\n") && text.contains("wallpaper=aurora\n"),
        "{}",
        text
    );
}

#[test]
fn changed_areas_match_a_full_redraw() {
    let mut r = Run::new(1024, 640);
    r.login();
    r.d.launch(AppId::Terminal, &mut r.h, &mut r.f);
    r.wait(300);
    r.d.launch(AppId::Settings, &mut r.h, &mut r.f);
    r.wait(300);
    // Drag the settings window by its title bar and hover the panel.
    let title = r.d.window_rect_of("Настройки").unwrap();
    let (x, y) = (title.x + title.w / 3, title.y + 15);
    r.step(&[Event::MouseMove { x, y }]);
    r.step(&[Event::MouseButton { down: true }]);
    for i in 1..10 {
        r.step(&[Event::MouseMove {
            x: x - i * 17,
            y: y + i * 9,
        }]);
    }
    r.step(&[Event::MouseButton { down: false }]);
    r.step(&[Event::MouseMove { x: 512, y: 610 }]);
    r.wait(1200);
    let incremental = r.c.pixels.clone();
    r.d.damage_all();
    r.step(&[]);
    let diff = incremental
        .iter()
        .zip(&r.c.pixels)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(diff, 0, "{} pixels differ from a full redraw", diff);
}

use super::*;
use crate::chooser::{Chooser, Mode};
use velora_ui::ui::Event;

fn read(dir: &str, f: &str) -> Vec<u8> {
    std::fs::read(format!("{}/{}", dir, f))
        .unwrap_or_else(|_| panic!("{}/{}: run tools/fetch-fonts.sh", dir, f))
}

pub(crate) fn fonts() -> Fonts {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../build/fonts");
    Fonts::new(
        &read(dir, "Inter-Regular.ttf"),
        &read(dir, "Inter-Medium.ttf"),
        &read(dir, "Inter-SemiBold.ttf"),
    )
    .unwrap()
}

pub(crate) fn assets() -> Assets {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
    let p = |n: &str| Pointer::from_png(&read(root, &format!("cursors/{}.png", n))).unwrap();
    Assets {
        logo: Picture::from_png_on_background(&read(root, "velora-logo.png")).unwrap(),
        pointers: [p("classic"), p("blade"), p("drop")],
    }
}

pub(crate) fn disks() -> Vec<DiskEntry> {
    alloc::vec![DiskEntry {
        name: String::from("nvme0n1"),
        size: 512_110_190_592,
        model: String::from("Samsung SSD 970 EVO"),
    }]
}

struct Harness {
    app: App,
    ui: Ui,
    c: Canvas,
    f: Fonts,
    a: Assets,
}

impl Harness {
    fn new() -> Harness {
        Harness {
            app: App::new(disks()),
            ui: Ui::new(256),
            c: Canvas::new(1280, 720),
            f: fonts(),
            a: assets(),
        }
    }

    /// Feeds events one key per frame, as the program does; returns the
    /// last action that was not None.
    fn input(&mut self, events: &[Event]) -> Action {
        let mut last = Action::None;
        let mut queue: Vec<Vec<Event>> = velora_ui::ui::one_key_per_frame(events);
        queue.insert(0, Vec::new());
        for batch in queue {
            let mut ev = batch;
            loop {
                self.ui.begin(&ev);
                let (action, again) =
                    self.app
                        .draw(&mut self.ui, &mut self.c, &mut self.f, &mut self.a);
                if action != Action::None {
                    last = action;
                }
                if !again {
                    break;
                }
                ev = Vec::new();
            }
        }
        last
    }

    fn keys(&mut self, keys: &[Key]) -> Action {
        let ev: Vec<Event> = keys
            .iter()
            .map(|&key| Event::Key { key, shift: false })
            .collect();
        self.input(&ev)
    }

    fn type_text(&mut self, s: &str) {
        let ev: Vec<Event> = s
            .chars()
            .map(|c| Event::Key {
                key: Key::Char(c),
                shift: false,
            })
            .collect();
        self.input(&ev);
    }
}

#[test]
fn every_screen_with_the_keyboard() {
    let mut h = Harness::new();
    assert_eq!(h.app.screen, Screen::Welcome);
    // Enter with nothing focused goes on.
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::Location);
    // Tab to the language list (after the theme switch), pick the next one.
    h.keys(&[Key::Tab, Key::Tab, Key::Down]);
    assert_eq!(h.app.settings.language, 1);
    // Escape goes back, Enter (from the Continue button) forward again.
    h.keys(&[Key::Escape]);
    assert_eq!(h.app.screen, Screen::Welcome);
    h.ui.set_focus(None);
    h.keys(&[Key::Enter, Key::Enter, Key::Enter]);
    // The Continue button kept the focus: Location -> Keyboard -> Appearance.
    assert_eq!(h.app.screen, Screen::Appearance, "{:?}", h.app.screen);
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::Disk);
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::User);
    // Empty name: an error, the screen stays.
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::User);
    assert!(h.app.error.is_some());
    // Fill in the fields with Tab and typing.
    h.ui.set_focus(Some(110));
    h.type_text("anna");
    h.ui.set_focus(Some(111));
    h.type_text("pw");
    h.ui.set_focus(Some(112));
    h.type_text("pw");
    h.ui.set_focus(Some(113));
    h.type_text("root");
    h.ui.set_focus(Some(114));
    h.type_text("roox");
    h.ui.set_focus(Some(901));
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::User);
    assert!(h.app.error.as_deref().unwrap().contains("Пароли"));
    h.ui.set_focus(Some(114));
    h.keys(&[Key::Backspace]);
    h.type_text("t");
    h.ui.set_focus(Some(901));
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::Confirm);
    let a = h.keys(&[Key::Enter]);
    assert_eq!(a, Action::Install);
    assert_eq!(h.app.screen, Screen::Installing);
    let plan = h.app.plan();
    assert_eq!(
        (plan.user.as_str(), plan.root_password.as_str()),
        ("anna", "root")
    );
    assert_eq!(plan.layout, Layout::default());
    // Keys do nothing while installing.
    assert_eq!(h.keys(&[Key::Escape, Key::Enter]), Action::None);
    h.app.set_progress(velora_install::Progress {
        phase: Phase::Configure,
        permille: 720,
    });
    h.app.finish(Ok(()));
    assert_eq!(h.app.screen, Screen::Done);
    assert_eq!(h.keys(&[Key::Enter]), Action::Reboot);
}

#[test]
fn manual_layout_and_mouse() {
    let mut h = Harness::new();
    h.app.screen = Screen::Disk;
    h.input(&[]);
    // Click the second radio row ("Manual").
    let (win, scale) = window_rect(1280, 720);
    let s = |u: i32| ((u as i64 * scale + 128) >> 8) as i32;
    let (mx, my) = (win.x + s(CONTENT_X + 40), win.y + s(78 + 48 + 20));
    h.input(&[
        Event::MouseMove { x: mx, y: my },
        Event::MouseButton { down: true },
        Event::MouseButton { down: false },
    ]);
    assert_eq!(h.app.mode, 1);
    h.ui.set_focus(None);
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::Partitions);
    h.app.esp = 1;
    h.app.root_share = 2;
    let l = h.app.layout();
    assert_eq!(l.esp_mib, 128);
    let total = 512_110_190_592u64 / (1024 * 1024);
    assert_eq!(l.root_mib, Some((total - 130) / 2));
    // Expert: the label is used and checked.
    h.app.mode = 2;
    h.app.label = String::from("bad label");
    h.ui.set_focus(None);
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::Partitions);
    assert!(h.app.error.is_some());
    h.app.label = String::from("main");
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::User);
    assert_eq!(h.app.layout().root_label, "main");
    // Back from the user screen returns to the partitions.
    h.keys(&[Key::Escape]);
    assert_eq!(h.app.screen, Screen::Partitions);
}

#[test]
fn no_disks_and_failure() {
    let mut h = Harness::new();
    h.app.disks.clear();
    h.app.screen = Screen::Disk;
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::Disk);
    assert!(h.app.error.is_some());
    h.app.screen = Screen::Installing;
    h.app.finish(Err(String::from("GPT: I/O error")));
    assert_eq!(h.app.screen, Screen::Failed);
    h.input(&[]);
    h.keys(&[Key::Escape]);
    assert_eq!(h.app.screen, Screen::Disk);
}

#[test]
fn english_and_light_theme_render() {
    let mut h = Harness::new();
    h.app.settings.installer_language = Lang::En;
    h.app.dark = false;
    for s in [
        Screen::Welcome,
        Screen::Location,
        Screen::Keyboard,
        Screen::Appearance,
        Screen::Disk,
        Screen::Partitions,
        Screen::User,
        Screen::Confirm,
        Screen::Installing,
        Screen::Done,
        Screen::Failed,
    ] {
        h.app.screen = s;
        h.input(&[]);
        // The window is light: most pixels in its middle are bright.
        let (win, _) = window_rect(1280, 720);
        let p = h.c.get(win.x + win.w * 3 / 4, win.y + 8);
        assert!((p & 0xff) > 180, "{:?}: {:06x}", s, p);
    }
}

#[test]
fn chooser_keys_and_countdown() {
    let f = &mut fonts();
    let a = &mut assets();
    let mut c = Canvas::new(1280, 720);
    let mut ui = Ui::new(256);
    let mut ch = Chooser::default();
    let mut run = |ch: &mut Chooser, ev: &[Event]| {
        ui.begin(ev);
        ch.draw(&mut ui, &mut c, f, a).0
    };
    assert_eq!(run(&mut ch, &[]), None);
    for _ in 0..9 {
        assert_eq!(ch.tick(), None);
    }
    assert_eq!(ch.tick(), Some(Mode::Graphical));
    let mut ch = Chooser::default();
    let key = |k| Event::Key {
        key: k,
        shift: false,
    };
    assert_eq!(run(&mut ch, &[key(Key::Right)]), None);
    assert_eq!(ch.countdown, None);
    assert_eq!(run(&mut ch, &[key(Key::Right)]), None);
    assert_eq!(run(&mut ch, &[key(Key::Enter)]), Some(Mode::Live));
    let mut ch = Chooser::default();
    assert_eq!(run(&mut ch, &[key(Key::Char('2'))]), Some(Mode::Terminal));
}

#[test]
fn sizes() {
    assert_eq!(size_text(512_110_190_592), "512 GB");
    assert_eq!(size_text(2 * 1024 * 1024 * 1024), "2.1 GB");
    assert_eq!(size_text(4_000_000_000), "4 GB");
    assert_eq!(size_text(128 * 1024 * 1024), "134 MB");
    assert_eq!(mib_text(64), "64 MiB");
    assert_eq!(mib_text(1536), "1.5 GiB");
}

#[test]
fn menu_then_installer_share_the_ui() {
    let mut f = fonts();
    let mut a = assets();
    let mut c = Canvas::new(1280, 720);
    let mut ui = Ui::new(256);
    let mut ch = Chooser::default();
    let key = |k| Event::Key {
        key: k,
        shift: false,
    };
    ui.begin(&[]);
    ch.draw(&mut ui, &mut c, &mut f, &mut a);
    ui.begin(&[key(Key::Char('1'))]);
    assert_eq!(
        ch.draw(&mut ui, &mut c, &mut f, &mut a).0,
        Some(Mode::Graphical)
    );
    let mut app = App::new(disks());
    ui.begin(&[]);
    app.draw(&mut ui, &mut c, &mut f, &mut a);
    ui.begin(&[key(Key::Enter)]);
    let (_, again) = app.draw(&mut ui, &mut c, &mut f, &mut a);
    assert_eq!(app.screen, Screen::Location);
    assert!(
        again,
        "a new screen must be drawn without waiting for input"
    );
}

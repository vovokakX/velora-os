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
        bus: String::from("NVMe"),
        in_use: false,
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
        let mut app = App::new(disks());
        // Animations are checked in their own test with a clock.
        app.animate = false;
        Harness {
            app,
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
    // Empty fields: Continue is disabled, Enter does nothing.
    assert!(!h.app.can_continue());
    h.keys(&[Key::Enter]);
    assert_eq!(h.app.screen, Screen::User);
    assert!(h.app.error.is_none());
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
    assert!(!h.app.can_continue());
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
    h.app.settings.theme = 1;
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

/// The theme and accent change the installer at once, with a cross-fade;
/// screens slide; the Animations switch turns it off.
#[test]
fn theme_accent_and_transitions_animate() {
    let mut h = Harness::new();
    h.app.animate = true;
    let mut now = 0u64;
    let frame = |h: &mut Harness, now: u64, ev: &[Event]| {
        h.ui.set_time(now);
        h.ui.begin(ev);
        h.app.draw(&mut h.ui, &mut h.c, &mut h.f, &mut h.a);
    };
    frame(&mut h, now, &[]);
    let (win, scale) = window_rect(1280, 720);
    let s = |u: i32| ((u as i64 * scale + 128) >> 8) as i32;
    // A pixel of the window background, away from the widgets.
    let (px, py) = (win.x + s(400), win.y + s(250));
    let dark = h.c.get(px, py);
    // The switch in the sidebar turns the light theme on.
    h.ui.set_focus(Some(800));
    frame(
        &mut h,
        now,
        &[Event::Key {
            key: Key::Enter,
            shift: false,
        }],
    );
    assert_eq!(h.app.settings.theme, 1);
    now += 16;
    frame(&mut h, now, &[]);
    assert!(h.app.in_transition());
    // The clock of a transition starts with its second frame.
    now += 16;
    frame(&mut h, now, &[]);
    now += 110;
    frame(&mut h, now, &[]);
    let mid = h.c.get(px, py);
    now += 400;
    frame(&mut h, now, &[]);
    frame(&mut h, now, &[]);
    assert!(!h.app.in_transition());
    let light = h.c.get(px, py);
    let lum = |c: u32| (c >> 16 & 0xff) + (c >> 8 & 0xff) + (c & 0xff);
    assert!(
        lum(dark) < lum(mid) && lum(mid) < lum(light),
        "{:x} {:x} {:x}",
        dark,
        mid,
        light
    );
    // A step forward slides the content.
    h.ui.set_focus(None);
    frame(
        &mut h,
        now,
        &[Event::Key {
            key: Key::Enter,
            shift: false,
        }],
    );
    assert_eq!(h.app.screen, Screen::Location);
    now += 16;
    frame(&mut h, now, &[]);
    assert!(h.app.in_transition() && h.ui.animating());
    now += 16;
    frame(&mut h, now, &[]);
    now += 600;
    frame(&mut h, now, &[]);
    frame(&mut h, now, &[]);
    assert!(!h.app.in_transition());
    // The accent colour is used at once; without "Animations" there is
    // no transition.
    h.app.settings.animations = false;
    h.app.settings.accent = 1;
    frame(&mut h, now, &[]);
    assert!(!h.app.in_transition());
    assert_eq!(h.ui.theme.accent, 0xff00_0000 | settings::ACCENTS[1].1);
}

#[test]
fn window_scales_are_steps() {
    assert_eq!(window_rect(1280, 720).1, 384);
    assert_eq!(window_rect(1920, 1080).1, 512);
    assert_eq!(window_rect(2560, 1440).1, 768);
    assert_eq!(window_rect(3840, 2160).1, 1024);
    assert_eq!(window_rect(1024, 768).1, 256);
    let (r, s) = window_rect(800, 600);
    assert!(s < 256 && r.w <= 800 - 48);
}

/// Frame times at 2560x1440 (run with --ignored --nocapture).
#[test]
#[ignore]
fn frame_times() {
    use std::time::Instant;
    let mut h = Harness::new();
    h.c = Canvas::new(2560, 1440);
    let (win, scale) = window_rect(2560, 1440);
    let t = Instant::now();
    velora_ui::decor::backdrop(&mut h.c, &velora_ui::theme::DARK);
    println!("backdrop {:?}", t.elapsed());
    let t = Instant::now();
    velora_ui::decor::window(
        &mut h.c,
        win,
        (16 * scale / 256) as i32,
        &velora_ui::theme::DARK,
        scale,
    );
    println!("window {:?}", t.elapsed());
    for s in [Screen::Welcome, Screen::Location, Screen::Appearance] {
        h.app.screen = s;
        let t = Instant::now();
        h.input(&[]);
        println!("{:?} first {:?}", s, t.elapsed());
        let t = Instant::now();
        h.input(&[]);
        println!("{:?} again {:?}", s, t.elapsed());
    }
}

/// One installer drawn with planned frames (parts of the screen, or none)
/// and one drawn whole every frame: after every step their pictures must
/// be the same, so nothing a widget shows is left out of a planned part.
#[test]
fn planned_frames_draw_what_whole_frames_draw() {
    use velora_ui::ui::Redraw;
    struct Side {
        app: App,
        ui: Ui,
        c: Canvas,
    }
    let new_side = || {
        let mut app = App::new(disks());
        app.animate = true;
        Side {
            app,
            ui: Ui::new(256),
            c: Canvas::new(1280, 720),
        }
    };
    let (mut planned, mut whole) = (new_side(), new_side());
    let mut f = fonts();
    let mut a = assets();
    let mut full = true;
    let (mut skipped, mut parts) = (0, 0);
    let mut step = |planned: &mut Side,
                    whole: &mut Side,
                    ev: &[Event],
                    now: u64,
                    f: &mut Fonts,
                    a: &mut Assets| {
        for s in [&mut *planned, &mut *whole] {
            s.ui.set_time(now);
        }
        let plan = if full {
            Redraw::All
        } else {
            planned.ui.plan(ev, now)
        };
        let b = planned.c.bounds();
        match plan {
            Redraw::Nothing => {
                planned.ui.track_pointer(ev);
                skipped += 1;
            }
            p => {
                let area = if let Redraw::Area(r) = p {
                    parts += 1;
                    r
                } else {
                    b
                };
                planned.c.set_frame(area);
                planned.ui.begin(ev);
                let (action, again) = planned.app.draw(&mut planned.ui, &mut planned.c, f, a);
                planned.c.set_frame(b);
                full = again || action != Action::None;
            }
        }
        whole.ui.begin(ev);
        whole.app.draw(&mut whole.ui, &mut whole.c, f, a);
        let diff = planned
            .c
            .pixels
            .iter()
            .zip(&whole.c.pixels)
            .position(|(p, q)| p != q);
        if let Some(i) = diff {
            panic!(
                "at {} ms ({:?}, plan {:?}): pixel {},{} differs",
                now,
                whole.app.screen,
                plan,
                i % 1280,
                i / 1280
            );
        }
    };
    let mut now = 1000;
    let mv = |x: i32, y: i32| Event::MouseMove { x, y };
    let enter = Event::Key {
        key: Key::Enter,
        shift: false,
    };
    // Welcome, then Location (a slide), then Appearance.
    for ev in [
        vec![],
        vec![enter],
        vec![],
        vec![enter],
        vec![],
        vec![enter],
        vec![],
    ] {
        step(&mut planned, &mut whole, &ev, now, &mut f, &mut a);
        for _ in 0..20 {
            now += 17;
            step(&mut planned, &mut whole, &[], now, &mut f, &mut a);
        }
    }
    assert_eq!(whole.app.screen, Screen::Appearance);
    // The pointer sweeps over the screen: hover effects start and end.
    for y in (40..700).step_by(41) {
        for x in (0..1280).step_by(41) {
            now += 9;
            step(&mut planned, &mut whole, &[mv(x, y)], now, &mut f, &mut a);
        }
    }
    // A drop-down list opens; the pointer moves over its rows, the wheel
    // scrolls it, a row is taken.
    let areas = whole.ui.areas.clone();
    let (_, field) = *areas
        .iter()
        .find(|(id, _)| *id == 53)
        .expect("the theme list");
    let (cx, cy) = (field.center_x(), field.center_y());
    let down = Event::MouseButton { down: true };
    let up = Event::MouseButton { down: false };
    for ev in [vec![mv(cx, cy)], vec![down], vec![up]] {
        now += 17;
        step(&mut planned, &mut whole, &ev, now, &mut f, &mut a);
    }
    for k in 0..40 {
        now += 17;
        step(
            &mut planned,
            &mut whole,
            &[mv(cx, cy + 10 + k * 4)],
            now,
            &mut f,
            &mut a,
        );
    }
    now += 17;
    step(
        &mut planned,
        &mut whole,
        &[Event::Wheel(1)],
        now,
        &mut f,
        &mut a,
    );
    for ev in [vec![down], vec![up]] {
        now += 17;
        step(&mut planned, &mut whole, &ev, now, &mut f, &mut a);
    }
    for _ in 0..30 {
        now += 17;
        step(&mut planned, &mut whole, &[], now, &mut f, &mut a);
    }
    // Planned frames did their job: many were parts or skipped.
    assert!(
        skipped > 40 && parts > 40,
        "skipped {}, parts {}",
        skipped,
        parts
    );
}

//! Opening and closing windows gives all their memory back: 50 windows
//! of every app, counted by an allocator that keeps the bytes in use.
//! Needs the fonts in build/fonts (tools/fetch-fonts.sh).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use velora_lumi::apps::{AppId, Assets};
use velora_lumi::config::Config;
use velora_lumi::desktop::{Desktop, Mode};
use velora_lumi::fake::FakeHost;
use velora_ui::image::{Picture, Pointer};
use velora_ui::text::Fonts;
use velora_ui::ui::{Key, mods};
use velora_ui::{Canvas, Event};

struct Counting;

static IN_USE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        IN_USE.fetch_add(l.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        IN_USE.fetch_sub(l.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        IN_USE.fetch_add(n as isize - l.size() as isize, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn fifty_windows_give_their_memory_back() {
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
    let mut h = FakeHost::new();
    h.term_output = b"user@velora:~$ ".to_vec();
    let mut d = Desktop::new(800, 500, Config::default(), assets, &mut h);
    let mut c = Canvas::new(800, 500);
    let mut step = |d: &mut Desktop, f: &mut Fonts, h: &mut FakeHost, ev: &[Event], ms: u64| {
        for i in 0..ms.div_ceil(16) {
            h.now += 16;
            d.frame(if i == 0 { ev } else { &[] }, &mut c, f, h);
        }
    };
    step(&mut d, &mut f, &mut h, &[], 50);
    for ch in "velora".chars() {
        let k = Event::Key {
            key: Key::Char(ch),
            shift: false,
        };
        step(&mut d, &mut f, &mut h, &[k], 16);
    }
    let enter = Event::Key {
        key: Key::Enter,
        shift: false,
    };
    step(&mut d, &mut f, &mut h, &[enter], 1500);
    assert_eq!(d.mode, Mode::Session);
    let apps = [
        AppId::Terminal,
        AppId::Files,
        AppId::Settings,
        AppId::About,
        AppId::Plume,
    ];
    let close = Event::Chord {
        key: Key::F(4),
        mods: mods::ALT,
    };
    let mut cycle = |d: &mut Desktop, f: &mut Fonts, h: &mut FakeHost, n: usize| {
        for i in 0..n {
            d.launch(apps[i % apps.len()], h, f);
            step(d, f, h, &[], 300);
            step(d, f, h, &[close], 400);
        }
        // What the fake system keeps for the test itself.
        h.log = Vec::new();
        h.terms = Vec::new();
    };
    // Once each: glyphs, pictures and the like are cached on first use.
    cycle(&mut d, &mut f, &mut h, apps.len());
    assert!(d.window_titles().is_empty());
    let before = IN_USE.load(Ordering::Relaxed);
    cycle(&mut d, &mut f, &mut h, 50);
    assert!(d.window_titles().is_empty());
    let after = IN_USE.load(Ordering::Relaxed);
    assert_eq!(
        after - before,
        0,
        "{} bytes more after 50 windows",
        after - before
    );
}

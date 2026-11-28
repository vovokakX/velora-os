//! Heap use of Lumi's parts on the host (a counting allocator):
//! `cargo run --release -p velora-lumi --example memory`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use velora_lumi::apps::{AppId, Assets};
use velora_lumi::config::Config;
use velora_lumi::desktop::Desktop;
use velora_lumi::fake::FakeHost;
use velora_ui::image::{Picture, Pointer};
use velora_ui::text::Fonts;
use velora_ui::ui::Key;
use velora_ui::{Canvas, Event};

struct Counting;
static NOW: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let n = NOW.fetch_add(l.size() as isize, Ordering::Relaxed) + l.size() as isize;
        PEAK.fetch_max(n, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        NOW.fetch_sub(l.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn mib() -> String {
    format!(
        "now {:.1} MiB, peak {:.1} MiB",
        NOW.load(Ordering::Relaxed) as f64 / 1048576.0,
        PEAK.load(Ordering::Relaxed) as f64 / 1048576.0
    )
}

fn main() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let read = |p: &str| std::fs::read(format!("{}/{}", root, p)).expect(p);
    let (r, m, s, j) = (
        read("build/fonts/Inter-Regular.ttf"),
        read("build/fonts/Inter-Medium.ttf"),
        read("build/fonts/Inter-SemiBold.ttf"),
        read("build/fonts/JetBrainsMono-Regular.ttf"),
    );
    let base = NOW.load(Ordering::Relaxed);
    let mut f = Fonts::new(&r, &m, &s).unwrap();
    f.add_mono(&j).unwrap();
    println!(
        "fonts: {:.1} MiB",
        (NOW.load(Ordering::Relaxed) - base) as f64 / 1048576.0
    );
    let p = |n: &str| Pointer::from_png(&read(&format!("assets/cursors/{}.png", n))).unwrap();
    let assets = Assets {
        logo: Picture::from_png_on_background(&read("assets/velora-logo.png")).unwrap(),
        pointers: [p("classic"), p("blade"), p("drop")],
    };
    println!("assets: {}", mib());
    let mut h = FakeHost::new();
    let mut c = Canvas::new(1920, 1080);
    let mut d = Desktop::new(1920, 1080, Config::default(), assets, &mut h);
    println!("desktop + one canvas: {}", mib());
    let step =
        |d: &mut Desktop, c: &mut Canvas, f: &mut Fonts, h: &mut FakeHost, ev: &[Event]| {
            h.now += 16;
            d.frame(ev, c, f, h);
        };
    for _ in 0..30 {
        step(&mut d, &mut c, &mut f, &mut h, &[]);
    }
    println!("login screen: {}", mib());
    for ch in "velora".chars() {
        step(
            &mut d,
            &mut c,
            &mut f,
            &mut h,
            &[Event::Key {
                key: Key::Char(ch),
                shift: false,
            }],
        );
    }
    step(
        &mut d,
        &mut c,
        &mut f,
        &mut h,
        &[Event::Key {
            key: Key::Enter,
            shift: false,
        }],
    );
    for _ in 0..100 {
        step(&mut d, &mut c, &mut f, &mut h, &[]);
    }
    println!("empty desktop: {}", mib());
    d.launch(AppId::Terminal, &mut h, &mut f);
    for _ in 0..60 {
        step(&mut d, &mut c, &mut f, &mut h, &[]);
    }
    println!("with a terminal: {}", mib());
}

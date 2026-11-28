//! /bin/uidemo: every widget of velora-ui on the real screen, controlled
//! with the mouse and the keyboard. Escape or "Выйти" ends it.
//!
//! `uidemo --frames N` draws N frames and exits (for tests); it reports
//! how long loading and drawing took on the serial console.

#![no_std]
#![no_main]

extern crate alloc;

use velora_ui::gallery::Gallery;
use velora_ui::image::{Picture, Pointer};
use velora_ui::screen::Screen;
use velora_ui::text::Fonts;
use velora_ui::{Rect, Ui};
use vlibc::{env, eprintln, fs, println, time};

vlibc::main!(main);

const FONT_DIR: &str = "/usr/share/fonts";

fn ms() -> u64 {
    time::monotonic().map_or(0, |d| d.as_millis() as u64)
}

/// The Inter faces from the image.
pub fn load_fonts() -> Option<Fonts> {
    let read = |f: &str| fs::read(&alloc::format!("{}/{}", FONT_DIR, f)).ok();
    Fonts::new(
        &read("Inter-Regular.ttf")?,
        &read("Inter-Medium.ttf")?,
        &read("Inter-SemiBold.ttf")?,
    )
    .ok()
}

fn main() -> i32 {
    let args = env::args();
    let frames = args
        .iter()
        .position(|a| a == "--frames")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<u32>().ok());
    let t0 = ms();
    let Some(mut fonts) = load_fonts() else {
        eprintln!("uidemo: cannot load the fonts from {}", FONT_DIR);
        return 1;
    };
    let t_fonts = ms();
    let mut logo =
        Picture::from_png_on_background(include_bytes!("../../../assets/velora-logo.png"))
            .expect("logo");
    let mut pointer =
        Pointer::from_png(include_bytes!("../../../assets/cursors/classic.png")).expect("pointer");
    let mut screen = match Screen::open() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("uidemo: no screen: {:?}", e);
            return 1;
        }
    };
    let mut ui = Ui::new(256);
    let mut g = Gallery::default();
    let (_, scale) = velora_ui::gallery::window_rect(screen.width(), screen.height());
    let psize = (22 * scale / 256) as usize;
    let (img, hx, hy) = pointer.at(psize);
    screen.set_pointer(Some((img, hx, hy)));
    screen.speed = (scale as i32).max(256);
    let mut events = alloc::vec::Vec::new();
    let mut queue: alloc::collections::VecDeque<alloc::vec::Vec<velora_ui::Event>> =
        alloc::collections::VecDeque::new();
    let mut drawn = 0u32;
    let mut first_frame = 0;
    loop {
        let t = ms();
        ui.begin(&events);
        let again = g.draw(&mut ui, &mut screen.canvas, &mut fonts, &mut logo);
        let full = Rect::new(0, 0, screen.width(), screen.height());
        screen.present(full);
        drawn += 1;
        if drawn == 1 {
            first_frame = ms() - t;
            println!(
                "uidemo: {}x{}, fonts {} ms, first frame {} ms",
                screen.width(),
                screen.height(),
                t_fonts - t0,
                first_frame
            );
        }
        if g.quit || frames.is_some_and(|n| drawn >= n) {
            break;
        }
        if again {
            events.clear();
            continue;
        }
        while queue.is_empty() {
            let polled = screen.poll();
            if polled.is_empty() {
                time::sleep_ms(10);
            }
            queue.extend(velora_ui::ui::one_key_per_frame(&polled));
        }
        events = queue.pop_front().unwrap_or_default();
    }
    println!(
        "uidemo: done after {} frames (first {} ms)",
        drawn, first_frame
    );
    0
}

//! Renders the widget gallery on the host into PPM files, for looking at
//! the toolkit without QEMU: `cargo run -p velora-ui --example render --
//! OUT_DIR [WIDTHxHEIGHT ...]`.

use velora_ui::gallery::Gallery;
use velora_ui::image::Picture;
use velora_ui::text::Fonts;
use velora_ui::{Canvas, Ui};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args
        .first()
        .cloned()
        .unwrap_or_else(|| String::from("build/ui-preview"));
    let sizes: Vec<(usize, usize)> = if args.len() > 1 {
        args[1..]
            .iter()
            .map(|s| {
                let (w, h) = s.split_once('x').expect("WIDTHxHEIGHT");
                (w.parse().unwrap(), h.parse().unwrap())
            })
            .collect()
    } else {
        vec![(1280, 720), (1920, 1080)]
    };
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../build/fonts");
    let read = |f: &str| std::fs::read(format!("{}/{}", dir, f)).expect("run tools/fetch-fonts.sh");
    let mut fonts = Fonts::new(
        &read("Inter-Regular.ttf"),
        &read("Inter-Medium.ttf"),
        &read("Inter-SemiBold.ttf"),
    )
    .unwrap();
    let mut logo =
        Picture::from_png_on_background(include_bytes!("../../../assets/velora-logo.png")).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    for (w, h) in sizes {
        for dark in [true, false] {
            let mut c = Canvas::new(w, h);
            let mut ui = Ui::new(256);
            let mut g = Gallery {
                dark,
                ..Gallery::default()
            };
            ui.begin(&[]);
            g.draw(&mut ui, &mut c, &mut fonts, &mut logo);
            let name = format!(
                "{}/gallery-{}x{}-{}.ppm",
                out,
                w,
                h,
                if dark { "dark" } else { "light" }
            );
            std::fs::write(&name, c.to_ppm()).unwrap();
            println!("{}", name);
        }
    }
}

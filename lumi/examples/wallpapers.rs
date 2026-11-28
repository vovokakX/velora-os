//! Draws the built-in wallpapers on the host as PPM files:
//! `cargo run --release -p velora-lumi --example wallpapers -- OUT [WxH]`.

use velora_lumi::config::BUILTIN_WALLPAPERS;
use velora_lumi::wallpaper;
use velora_ui::Canvas;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args
        .first()
        .cloned()
        .unwrap_or_else(|| "build/lumi-wallpapers".into());
    let (w, h) = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap()))
        .unwrap_or((1920, 1080));
    std::fs::create_dir_all(&out).unwrap();
    for id in BUILTIN_WALLPAPERS {
        let mut c = Canvas::new(w, h);
        let t = std::time::Instant::now();
        wallpaper::draw_builtin(&mut c, id);
        println!("{}: {} ms", id, t.elapsed().as_millis());
        std::fs::write(format!("{}/{}.ppm", out, id), c.to_ppm()).unwrap();
    }
}

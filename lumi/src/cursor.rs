//! Pointer pictures for resizing windows: white double arrows with a dark
//! outline, drawn at any size. The arrow pointer comes from
//! `assets/cursors` (the style chosen in the settings).

use crate::desktop::Pointer;
use velora_gfx::image::Image;
use velora_ui::canvas::{Canvas, FX};

/// Lines of each picture on a 24-unit grid: the shaft, then the heads.
fn lines(p: Pointer) -> [((i64, i64), (i64, i64)); 5] {
    match p {
        Pointer::ResizeV => [
            ((12, 3), (12, 21)),
            ((12, 3), (8, 7)),
            ((12, 3), (16, 7)),
            ((12, 21), (8, 17)),
            ((12, 21), (16, 17)),
        ],
        Pointer::ResizeNwse => [
            ((5, 5), (19, 19)),
            ((5, 5), (5, 10)),
            ((5, 5), (10, 5)),
            ((19, 19), (19, 14)),
            ((19, 19), (14, 19)),
        ],
        Pointer::ResizeNesw => [
            ((19, 5), (5, 19)),
            ((19, 5), (14, 5)),
            ((19, 5), (19, 10)),
            ((5, 19), (5, 14)),
            ((5, 19), (10, 19)),
        ],
        _ => [
            ((3, 12), (21, 12)),
            ((3, 12), (7, 8)),
            ((3, 12), (7, 16)),
            ((21, 12), (17, 8)),
            ((21, 12), (17, 16)),
        ],
    }
}

/// The picture for `p` (not the arrow) at `size` pixels, and its hot spot.
pub fn resize(p: Pointer, size: usize) -> (Image, i32, i32) {
    let s = size.max(12);
    let f = |v: i64| v * s as i64 * FX / 24;
    let draw = |c: &mut Canvas, width: i64| {
        for (a, b) in lines(p) {
            c.line_fx(f(a.0), f(a.1), f(b.0), f(b.1), width, 0xffffffff);
        }
    };
    let w = (s as i64) * FX / 12;
    let mut outline = Canvas::new(s, s);
    draw(&mut outline, w * 2 + FX * 2);
    let mut fill = Canvas::new(s, s);
    draw(&mut fill, w);
    let mut img = Image::new(s, s);
    for i in 0..s * s {
        let o = outline.pixels[i] & 0xff;
        let fv = fill.pixels[i] & 0xff;
        // Premultiplied: white where the fill is, dark around it.
        let v = fv as u8;
        img.data[i * 4..i * 4 + 4].copy_from_slice(&[v, v, v, o.max(fv) as u8]);
    }
    (img, s as i32 / 2, s as i32 / 2)
}

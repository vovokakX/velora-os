//! Wallpapers. The built-in ones are drawn from a description (large
//! curved sheets of light on a dark gradient, with a fine grain) right in
//! the screen's resolution, so they are sharp on any screen and take no
//! space. Pictures (PNG) are scaled to cover the screen without changing
//! their proportions: the middle part is kept, scaled down by averaging
//! areas or up by bilinear filtering.
//!
//! The frosted copy ([`frost`]) is a strongly blurred, darkened version
//! for the login screen, the app menu and the panel, made at an eighth of
//! the resolution and scaled up.

use crate::config::BUILTIN_WALLPAPERS;
use alloc::vec;
use alloc::vec::Vec;
use velora_gfx::image::Image;
use velora_ui::Rect;
use velora_ui::canvas::{Canvas, Color};

/// A curved sheet: the inside of a large circle (in a 1600 x 900 design
/// space) lit differently from the outside, with a light rim.
struct Sheet {
    cx: i64,
    cy: i64,
    r: i64,
    /// Added to the colour inside (signed, per channel).
    fill: [i32; 3],
    /// Colour of the rim light.
    rim: [i32; 3],
    /// Width of the glow inside and outside the edge, design units.
    inner: i64,
    outer: i64,
    /// Strength of the thin line on the edge (0..=256).
    line: i32,
}

struct Design {
    /// Gradient from the top-left to the bottom-right corner.
    top: [i32; 3],
    bottom: [i32; 3],
    /// A soft light: centre, radius (design units) and colour.
    glow: (i64, i64, i64, [i32; 3]),
    sheets: Vec<Sheet>,
    grain: i32,
}

fn design(id: &str) -> Design {
    let s = |cx, cy, r, fill, rim, inner, outer, line| Sheet {
        cx,
        cy,
        r,
        fill,
        rim,
        inner,
        outer,
        line,
    };
    match id {
        "graphite" => Design {
            top: [30, 30, 33],
            bottom: [12, 12, 14],
            glow: (300, -100, 900, [14, 14, 16]),
            sheets: vec![
                s(-300, 1900, 1500, [6, 6, 7], [70, 70, 76], 260, 60, 110),
                s(-200, 2150, 1500, [-6, -6, -6], [40, 40, 44], 160, 40, 70),
                s(2300, -900, 1500, [5, 5, 6], [60, 60, 66], 200, 50, 90),
            ],
            grain: 3,
        },
        "aurora" => Design {
            top: [14, 22, 36],
            bottom: [8, 8, 18],
            glow: (1200, 100, 900, [10, 30, 40]),
            sheets: vec![
                s(500, 1700, 1150, [4, 10, 16], [40, 170, 190], 230, 70, 150),
                s(1900, 1500, 1050, [10, 4, 18], [140, 80, 220], 260, 60, 140),
                s(800, 2050, 1150, [-4, -4, -6], [60, 90, 200], 160, 40, 90),
            ],
            grain: 3,
        },
        "ember" => Design {
            top: [34, 28, 26],
            bottom: [14, 12, 12],
            glow: (1350, 820, 800, [40, 22, 8]),
            sheets: vec![
                s(1700, 1450, 900, [10, 5, 2], [255, 170, 80], 260, 60, 170),
                s(1900, 1800, 1050, [-6, -5, -4], [200, 110, 50], 200, 40, 110),
                s(-500, -600, 1300, [4, 3, 3], [120, 90, 70], 220, 50, 80),
            ],
            grain: 4,
        },
        // "waves", the concept's picture: graphite with light sheets.
        _ => Design {
            top: [52, 52, 54],
            bottom: [26, 26, 28],
            glow: (1350, -50, 1000, [16, 16, 17]),
            sheets: vec![
                // The hill from the left edge to the middle.
                s(250, 2050, 1680, [-9, -9, -9], [120, 120, 124], 200, 90, 70),
                // The large sheet sweeping from the bottom to the top right.
                s(2350, 1250, 1500, [8, 8, 8], [150, 150, 155], 300, 70, 130),
                // The arc over the bottom right.
                s(
                    1250,
                    1700,
                    1000,
                    [10, 10, 11],
                    [170, 170, 175],
                    260,
                    60,
                    150,
                ),
                // A last curve low on the right.
                s(2000, 1350, 700, [6, 6, 6], [130, 130, 135], 200, 50, 110),
            ],
            grain: 4,
        },
    }
}

/// Integer square root from a nearby guess (the previous pixel's root):
/// one or two Newton steps.
#[inline]
fn sqrt_near(n: u64, guess: u64) -> u64 {
    let mut x = guess.max(1);
    for _ in 0..6 {
        let y = (x + n / x) / 2;
        if y == x || y + 1 == x || x + 1 == y {
            x = y.min(x);
            break;
        }
        x = y;
    }
    while x * x > n {
        x -= 1;
    }
    while (x + 1) * (x + 1) <= n {
        x += 1;
    }
    x
}

/// A small hash for the grain.
#[inline]
fn hash(x: u32, y: u32) -> u32 {
    let mut h = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h
}

/// Glow falloff: 256 at distance 0, 0 at `w`, squared.
#[inline]
fn falloff(d: i64, w: i64) -> i64 {
    if d >= w || w <= 0 {
        return 0;
    }
    let t = (w - d) * 256 / w;
    t * t / 256
}

/// Draws built-in wallpaper `id` over the whole canvas.
pub fn draw_builtin(c: &mut Canvas, id: &str) {
    let d = design(id);
    let (w, h) = (c.width as i64, c.height as i64);
    // Map the 1600 x 900 design to cover the canvas, in 1/16 pixel.
    let scale = (w * 16 * 1000 / 1600).max(h * 16 * 1000 / 900); // px/16 per 1000 du
    let ox = (w * 16 - 1600 * scale / 1000) / 2;
    let oy = (h * 16 - 900 * scale / 1000) / 2;
    let du = |v: i64| v * scale / 1000; // design units to 1/16 px
    let sheets: Vec<(i64, i64, i64, &Sheet, i64, i64)> = d
        .sheets
        .iter()
        .map(|s| {
            (
                du(s.cx) + ox,
                du(s.cy) + oy,
                du(s.r),
                s,
                du(s.inner),
                du(s.outer),
            )
        })
        .collect();
    let (gx, gy, gr) = (du(d.glow.0) + ox, du(d.glow.1) + oy, du(d.glow.2));
    let mut guesses = vec![0u64; sheets.len()];
    let mut glow_guess: u64;
    let diag = (w + h).max(1);
    let line_w = 24i64; // 1.5 px in 1/16 px
    for y in 0..h {
        let py = y * 16 + 8;
        for g in guesses.iter_mut() {
            *g = 0;
        }
        glow_guess = 0;
        let row = &mut c.pixels[(y * w) as usize..((y + 1) * w) as usize];
        for (x, px_out) in row.iter_mut().enumerate() {
            let x = x as i64;
            let px = x * 16 + 8;
            // Base gradient along the diagonal.
            let t = ((x + y) * 256 / diag) as i32;
            let mut col = [0i32; 3];
            for k in 0..3 {
                col[k] = (d.top[k] * (256 - t) + d.bottom[k] * t) * 4; // x1024
            }
            // Soft light.
            let (dx, dy) = ((px - gx) as i128, (py - gy) as i128);
            let n = (dx * dx + dy * dy).min(u64::MAX as i128) as u64;
            let dist = if glow_guess == 0 {
                velora_ui::canvas::isqrt(n)
            } else {
                sqrt_near(n, glow_guess)
            };
            glow_guess = dist;
            let gl = falloff(dist as i64, gr) as i32;
            for k in 0..3 {
                col[k] += d.glow.3[k] * gl * 4;
            }
            for (i, &(cx, cy, r, s, inner, outer)) in sheets.iter().enumerate() {
                let (dx, dy) = ((px - cx) as i128, (py - cy) as i128);
                let n = (dx * dx + dy * dy).min(u64::MAX as i128) as u64;
                let dist = if guesses[i] == 0 {
                    velora_ui::canvas::isqrt(n)
                } else {
                    sqrt_near(n, guesses[i])
                };
                guesses[i] = dist;
                let sd = dist as i64 - r; // negative inside
                // Coverage of the inside with a 1.5 px soft edge.
                let cov = if sd <= -line_w / 2 {
                    256
                } else if sd >= line_w / 2 {
                    0
                } else {
                    (line_w / 2 - sd) * 256 / line_w
                } as i32;
                let glow = if sd < 0 {
                    falloff(-sd, inner)
                } else {
                    falloff(sd, outer) / 3
                } as i32;
                let line = (falloff(sd.abs(), line_w * 2) as i32 * s.line) / 256;
                let light = glow / 2 + line;
                for k in 0..3 {
                    col[k] += s.fill[k] * cov * 4 + s.rim[k] * light;
                }
            }
            // Grain: a little noise, also against banding.
            let n = (hash(x as u32, y as u32) & 0xff) as i32 - 128;
            let g = n * d.grain * 1024 / 128 / 2;
            let ch = |v: i32| ((v + g) / 1024).clamp(0, 255) as u32;
            *px_out = ch(col[0]) << 16 | ch(col[1]) << 8 | ch(col[2]);
        }
    }
}

/// The colours a wallpaper preview shows before it is drawn (and for
/// small marks): dark, mid and light.
pub fn colors(id: &str) -> (Color, Color, Color) {
    let d = design(id);
    let c = |v: [i32; 3]| {
        0xff00_0000
            | (v[0].clamp(0, 255) as u32) << 16
            | (v[1].clamp(0, 255) as u32) << 8
            | v[2].clamp(0, 255) as u32
    };
    let rim = d.sheets.first().map_or([128; 3], |s| s.rim);
    (c(d.bottom), c(d.top), c(rim))
}

pub fn is_builtin(id: &str) -> bool {
    BUILTIN_WALLPAPERS.contains(&id)
}

/// A picture scaled to cover `w` x `h` (cropped in the middle, never
/// stretched).
pub fn cover(img: &Image, w: usize, h: usize) -> Canvas {
    let mut out = Canvas::new(w.max(1), h.max(1));
    let (sw, sh) = (img.width.max(1), img.height.max(1));
    // Source area used: the largest part with the screen's proportions.
    let (cw, ch) = if sw * h > sh * w {
        (sh * w / h, sh)
    } else {
        (sw, sw * h / w)
    };
    let (cx0, cy0) = ((sw - cw) / 2, (sh - ch) / 2);
    let px = |x: usize, y: usize| -> [u32; 3] {
        let i = (y * sw + x) * 4;
        let d = &img.data[i..i + 4];
        // Premultiplied: transparent parts become black.
        [d[0] as u32, d[1] as u32, d[2] as u32]
    };
    if cw >= w {
        // Down: average the source pixels each target pixel covers.
        for ty in 0..h {
            let y0 = cy0 + ty * ch / h;
            let y1 = (cy0 + (ty + 1) * ch / h).max(y0 + 1).min(sh);
            for tx in 0..w {
                let x0 = cx0 + tx * cw / w;
                let x1 = (cx0 + (tx + 1) * cw / w).max(x0 + 1).min(sw);
                let mut acc = [0u32; 3];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let p = px(sx, sy);
                        acc[0] += p[0];
                        acc[1] += p[1];
                        acc[2] += p[2];
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                out.pixels[ty * w + tx] = (acc[0] / n) << 16 | (acc[1] / n) << 8 | (acc[2] / n);
            }
        }
    } else {
        // Up: bilinear, in 1/256 source pixel.
        for ty in 0..h {
            let fy = ((ty * 2 + 1) * ch * 128 / h) as i64 - 128 + (cy0 * 256) as i64;
            let fy = fy.clamp(0, ((sh - 1) * 256) as i64);
            let (y0, wy) = ((fy >> 8) as usize, (fy & 255) as u32);
            let y1 = (y0 + 1).min(sh - 1);
            for tx in 0..w {
                let fx = ((tx * 2 + 1) * cw * 128 / w) as i64 - 128 + (cx0 * 256) as i64;
                let fx = fx.clamp(0, ((sw - 1) * 256) as i64);
                let (x0, wx) = ((fx >> 8) as usize, (fx & 255) as u32);
                let x1 = (x0 + 1).min(sw - 1);
                let (a, b, c2, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
                let mut o = 0u32;
                for k in 0..3 {
                    let top = a[k] * (256 - wx) + b[k] * wx;
                    let bot = c2[k] * (256 - wx) + d[k] * wx;
                    let v = (top * (256 - wy) + bot * wy) >> 16;
                    o = o << 8 | v.min(255);
                }
                out.pixels[ty * w + tx] = o;
            }
        }
    }
    out
}

/// Box blur of a small RGB image in place (horizontal and vertical).
fn box_blur(px: &mut [u32], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0u32; px.len()];
    let pass =
        |src: &[u32], dst: &mut [u32], len: usize, count: usize, step: usize, stride: usize| {
            for line in 0..count {
                let base = line * stride;
                for i in 0..len {
                    let lo = i.saturating_sub(r);
                    let hi = (i + r).min(len - 1);
                    let mut acc = [0u32; 3];
                    for j in lo..=hi {
                        let p = src[base + j * step];
                        acc[0] += (p >> 16) & 0xff;
                        acc[1] += (p >> 8) & 0xff;
                        acc[2] += p & 0xff;
                    }
                    let n = (hi - lo + 1) as u32;
                    dst[base + i * step] = (acc[0] / n) << 16 | (acc[1] / n) << 8 | (acc[2] / n);
                }
            }
        };
    pass(px, &mut tmp, w, h, 1, w);
    pass(&tmp, px, h, w, w, 1);
}

/// The desktop's wallpaper softened by the settings: blurred (`blur`
/// 0..=100: averaged down by 2 to 12 times, blurred and scaled up
/// bilinearly) and darkened by `dim` per cent.
pub fn soften(src: &Canvas, blur: u32, dim: u32) -> Canvas {
    let (w, h) = (src.width, src.height);
    let mut out = if blur == 0 {
        let mut c = Canvas::new(w, h);
        c.pixels.copy_from_slice(&src.pixels);
        c
    } else {
        let k = 2 + blur.min(100) as usize / 10;
        let (sw, sh) = (w.div_ceil(k), h.div_ceil(k));
        let mut small = vec![0u32; sw * sh];
        for sy in 0..sh {
            for sx in 0..sw {
                let mut acc = [0u32; 3];
                let mut n = 0;
                for y in sy * k..((sy + 1) * k).min(h) {
                    for x in sx * k..((sx + 1) * k).min(w) {
                        let p = src.pixels[y * w + x];
                        acc[0] += (p >> 16) & 0xff;
                        acc[1] += (p >> 8) & 0xff;
                        acc[2] += p & 0xff;
                        n += 1;
                    }
                }
                let n = n.max(1);
                small[sy * sw + sx] = (acc[0] / n) << 16 | (acc[1] / n) << 8 | (acc[2] / n);
            }
        }
        for _ in 0..2 {
            box_blur(&mut small, sw, sh, 1);
        }
        let mut c = Canvas::new(w, h);
        // Bilinear: pixel centres of the small grid at (i + 0.5) * k.
        let pos = |v: usize, n: usize| -> (usize, usize, u32) {
            let f = ((v * 2 + 1) * 256 / (2 * k)) as i64 - 128;
            let f = f.clamp(0, ((n - 1) * 256) as i64) as usize;
            let i = f >> 8;
            (i, (i + 1).min(n - 1), (f & 0xff) as u32)
        };
        let lerp = |a: u32, b: u32, t: u32| -> u32 {
            let mut o = 0;
            for sh_ in [16u32, 8, 0] {
                let (x, y) = ((a >> sh_) & 0xff, (b >> sh_) & 0xff);
                o |= ((x * (256 - t) + y * t) >> 8) << sh_;
            }
            o
        };
        let xs: Vec<(usize, usize, u32)> = (0..w).map(|x| pos(x, sw)).collect();
        for y in 0..h {
            let (y0, y1, ty) = pos(y, sh);
            let (r0, r1) = (&small[y0 * sw..y0 * sw + sw], &small[y1 * sw..y1 * sw + sw]);
            let row = &mut c.pixels[y * w..y * w + w];
            for (x, d) in row.iter_mut().enumerate() {
                let (x0, x1, tx) = xs[x];
                let top = lerp(r0[x0], r0[x1], tx);
                let bot = lerp(r1[x0], r1[x1], tx);
                *d = lerp(top, bot, ty);
            }
        }
        c
    };
    if dim > 0 {
        let keep = 100 - dim.min(100);
        for p in out.pixels.iter_mut() {
            let mut o = *p & 0xff00_0000;
            for sh_ in [16u32, 8, 0] {
                o |= (((*p >> sh_) & 0xff) * keep / 100) << sh_;
            }
            *p = o;
        }
    }
    out
}

/// The frosted copy of a wallpaper: strongly blurred and mixed with a
/// tint, kept at an eighth of the resolution (130 KiB for 1920 x 1080
/// instead of 8 MiB) and scaled up bilinearly, with a little grain, as it
/// is drawn.
pub struct Frost {
    pub w: usize,
    pub h: usize,
    sw: usize,
    sh: usize,
    px: Vec<u32>,
}

const K: usize = 8;

/// Makes the frosted copy of `src` with `tint` (alpha in the top byte).
pub fn frost(src: &Canvas, tint: Color) -> Frost {
    let (w, h) = (src.width, src.height);
    let (sw, sh) = (w.div_ceil(K), h.div_ceil(K));
    let mut small = vec![0u32; sw * sh];
    for sy in 0..sh {
        for sx in 0..sw {
            let mut acc = [0u32; 3];
            let mut n = 0;
            for y in sy * K..((sy + 1) * K).min(h) {
                for x in sx * K..((sx + 1) * K).min(w) {
                    let p = src.pixels[y * w + x];
                    acc[0] += (p >> 16) & 0xff;
                    acc[1] += (p >> 8) & 0xff;
                    acc[2] += p & 0xff;
                    n += 1;
                }
            }
            let n = n.max(1);
            small[sy * sw + sx] = (acc[0] / n) << 16 | (acc[1] / n) << 8 | (acc[2] / n);
        }
    }
    for _ in 0..3 {
        box_blur(&mut small, sw, sh, 3);
    }
    let ta = tint >> 24;
    let tc = [(tint >> 16) & 0xff, (tint >> 8) & 0xff, tint & 0xff];
    for p in small.iter_mut() {
        let mut o = 0;
        for (k, sh_) in [16u32, 8, 0].iter().enumerate() {
            let v = (*p >> sh_) & 0xff;
            o = (o << 8) | ((v * (255 - ta) + tc[k] * ta) / 255);
        }
        *p = o;
    }
    Frost {
        w,
        h,
        sw,
        sh,
        px: small,
    }
}

impl Frost {
    /// Pixels `x0..x1` of row `y`.
    fn span(&self, y: usize, x0: usize, x1: usize, out: &mut [u32]) {
        let (sw, sh) = (self.sw, self.sh);
        let fy = (((y * 2 + 1) * 128 / K) as i64 - 128).clamp(0, ((sh - 1) * 256) as i64);
        let (y0, wy) = ((fy >> 8) as usize, (fy & 255) as u32);
        let y1 = (y0 + 1).min(sh - 1);
        let (r0, r1) = (
            &self.px[y0 * sw..y0 * sw + sw],
            &self.px[y1 * sw..y1 * sw + sw],
        );
        for (i, x) in (x0..x1).enumerate() {
            let fx = (((x * 2 + 1) * 128 / K) as i64 - 128).clamp(0, ((sw - 1) * 256) as i64);
            let (a0, wx) = ((fx >> 8) as usize, (fx & 255) as u32);
            let a1 = (a0 + 1).min(sw - 1);
            let (a, b, c, d) = (r0[a0], r0[a1], r1[a0], r1[a1]);
            let g = hash(x as u32, y as u32);
            let mut o = 0u32;
            for (k, s) in [16u32, 8, 0].iter().enumerate() {
                let ch = |p: u32| (p >> s) & 0xff;
                let top = ch(a) * (256 - wx) + ch(b) * wx;
                let bot = ch(c) * (256 - wx) + ch(d) * wx;
                let v = (top * (256 - wy) + bot * wy) >> 16;
                // A touch of grain hides the steps of the gradient.
                let n = (g >> (k * 8)) & 3;
                o = o << 8 | (v + n).saturating_sub(1).min(255);
            }
            out[i] = o;
        }
    }

    /// One pixel (for tests).
    pub fn get(&self, x: usize, y: usize) -> u32 {
        let mut o = [0u32; 1];
        self.span(y, x, x + 1, &mut o);
        o[0]
    }

    /// Copies `r` into `dst` (a canvas of the same size), inside its clip.
    pub fn copy_to(&self, dst: &mut Canvas, r: Rect) {
        self.mix_to(dst, r, 255);
    }

    /// Mixes `r` over `dst` by `t` / 255.
    pub fn mix_to(&self, dst: &mut Canvas, r: Rect, t: u32) {
        let r = r
            .intersect(dst.clip())
            .intersect(Rect::new(0, 0, self.w as i32, self.h as i32));
        if r.is_empty() || t == 0 {
            return;
        }
        let mut row = vec![0u32; r.w as usize];
        for y in r.y..r.bottom() {
            self.span(y as usize, r.x as usize, r.right() as usize, &mut row);
            let base = y as usize * dst.width + r.x as usize;
            let d = &mut dst.pixels[base..base + r.w as usize];
            if t >= 255 {
                d.copy_from_slice(&row);
            } else {
                for (p, s) in d.iter_mut().zip(&row) {
                    *p = velora_ui::canvas::blend(*p, *s, t);
                }
            }
        }
    }

    /// Copies the rounded rectangle `r` into `dst`: frosted glass under
    /// the panel and the menus.
    pub fn rounded_to(&self, dst: &mut Canvas, r: Rect, corner: &crate::gfx::Corner, alpha: u32) {
        let area = r
            .intersect(dst.clip())
            .intersect(Rect::new(0, 0, self.w as i32, self.h as i32));
        if area.is_empty() || alpha == 0 {
            return;
        }
        let mut row = vec![0u32; area.w as usize];
        for y in area.y..area.bottom() {
            self.span(y as usize, area.x as usize, area.right() as usize, &mut row);
            let base = y as usize * dst.width;
            for (i, x) in (area.x..area.right()).enumerate() {
                let cov = corner.at(x - r.x, y - r.y, r.w, r.h) * alpha / 255;
                if cov == 0 {
                    continue;
                }
                let d = &mut dst.pixels[base + x as usize];
                *d = if cov >= 255 {
                    row[i]
                } else {
                    velora_ui::canvas::blend(*d, row[i], cov)
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_drawn_and_pictures_keep_proportions() {
        for id in BUILTIN_WALLPAPERS {
            let mut c = Canvas::new(160, 90);
            draw_builtin(&mut c, id);
            let lum: Vec<u32> = c.pixels.iter().map(|p| (p >> 8) & 0xff).collect();
            let (min, max) = (lum.iter().min().unwrap(), lum.iter().max().unwrap());
            assert!(max - min > 12, "{} is flat", id);
            assert!(*max < 230, "{} is too bright", id);
        }
        // A 4:3 picture, left half red and right half blue, on a 16:9
        // screen: the middle is kept, nothing is stretched.
        let mut img = Image::new(400, 300);
        for y in 0..300 {
            for x in 0..400 {
                let i = (y * 400 + x) * 4;
                let c = if x < 200 {
                    [200, 0, 0, 255]
                } else {
                    [0, 0, 200, 255]
                };
                img.data[i..i + 4].copy_from_slice(&c);
            }
        }
        let small = cover(&img, 160, 90);
        assert_eq!(small.pixels[45 * 160 + 10], 200 << 16);
        assert_eq!(small.pixels[45 * 160 + 150], 200);
        let big = cover(&img, 1600, 900);
        assert_eq!(big.pixels[450 * 1600 + 700], 200 << 16);
        assert_eq!(big.pixels[450 * 1600 + 900], 200);
        let f = frost(&big, 0x80000000);
        assert!((f.get(10, 450) >> 16) & 0xff < 120, "frost darkens");
        let mut c = Canvas::new(1600, 900);
        let b = c.bounds();
        f.copy_to(&mut c, b);
        assert_eq!(c.pixels[450 * 1600 + 10], f.get(10, 450));
    }
}

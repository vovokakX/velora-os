//! A pixel buffer and the drawing operations of the toolkit.
//!
//! Pixels are opaque `0x00RRGGBB`. Colours given to the drawing calls are
//! `0xAARRGGBB` with straight (not premultiplied) alpha. Shapes are
//! anti-aliased with signed distance functions in fixed point (1/256 of a
//! pixel), so no floating point is needed: programs are built without SSE.

use crate::geom::Rect;
use alloc::vec;
use alloc::vec::Vec;
use velora_gfx::image::Image;

/// `0xAARRGGBB`, straight alpha.
pub type Color = u32;

/// Fixed point: 1 pixel = 256 units.
pub const FX: i64 = 256;

pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    0xff00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
    (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// `c` with its alpha multiplied by `a / 255`.
pub const fn fade(c: Color, a: u8) -> Color {
    let ca = c >> 24;
    let na = (ca * a as u32 + 127) / 255;
    (na << 24) | (c & 0x00ff_ffff)
}

/// Mixes two colours: `t` = 0 gives `a`, 255 gives `b` (alpha too).
pub fn lerp(a: Color, b: Color, t: u8) -> Color {
    let t = t as u32;
    let mut out = 0;
    for shift in [0, 8, 16, 24] {
        let x = (a >> shift) & 0xff;
        let y = (b >> shift) & 0xff;
        let v = (x * (255 - t) + y * t + 127) / 255;
        out |= v << shift;
    }
    out
}

#[inline]
fn div255(x: u32) -> u32 {
    (x + 1 + (x >> 8)) >> 8
}

/// Blends `src` (0x..RRGGBB) over the opaque pixel `dst` with alpha `a`.
#[inline]
pub fn blend(dst: u32, src: u32, a: u32) -> u32 {
    if a >= 255 {
        return src & 0x00ff_ffff;
    }
    if a == 0 {
        return dst;
    }
    let inv = 255 - a;
    let r = div255(((src >> 16) & 0xff) * a + ((dst >> 16) & 0xff) * inv + 128);
    let g = div255(((src >> 8) & 0xff) * a + ((dst >> 8) & 0xff) * inv + 128);
    let b = div255((src & 0xff) * a + (dst & 0xff) * inv + 128);
    (r << 16) | (g << 8) | b
}

/// Integer square root (floor).
pub fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Coverage 0..=255 of a point at signed distance `d` from an edge
/// (negative inside), with an edge `feather` units wide.
#[inline]
fn coverage(d: i64, feather: i64) -> u32 {
    let half = feather / 2;
    if d <= -half {
        255
    } else if d >= feather - half {
        0
    } else {
        ((feather - half - d) * 255 / feather) as u32
    }
}

/// A rounded rectangle in fixed point.
#[derive(Clone, Copy, Debug)]
struct RRect {
    cx: i64,
    cy: i64,
    hw: i64,
    hh: i64,
    r: i64,
}

impl RRect {
    fn new(x0: i64, y0: i64, x1: i64, y1: i64, r: i64) -> Self {
        let hw = (x1 - x0).max(0) / 2;
        let hh = (y1 - y0).max(0) / 2;
        RRect {
            cx: (x0 + x1) / 2,
            cy: (y0 + y1) / 2,
            hw,
            hh,
            r: r.clamp(0, hw.min(hh)),
        }
    }

    fn from_rect(r: Rect, radius: i64) -> Self {
        Self::new(
            r.x as i64 * FX,
            r.y as i64 * FX,
            r.right() as i64 * FX,
            r.bottom() as i64 * FX,
            radius,
        )
    }

    /// Signed distance of the point (fixed point) from the outline.
    #[inline]
    fn dist(&self, px: i64, py: i64) -> i64 {
        let qx = (px - self.cx).abs() - (self.hw - self.r);
        let qy = (py - self.cy).abs() - (self.hh - self.r);
        if qx > 0 && qy > 0 {
            isqrt((qx * qx + qy * qy) as u64) as i64 - self.r
        } else {
            qx.max(qy) - self.r
        }
    }
}

/// Integer atan2 in turns: 0 points up, growing clockwise, 0..65536.
pub fn angle(dx: i64, dy: i64) -> u32 {
    // Screen coordinates: y grows downwards. Clockwise from "up" means
    // the angle of (dx, -dy) measured from the y axis towards x.
    let (x, y) = (dx, -dy);
    if x == 0 && y == 0 {
        return 0;
    }
    let ax = x.abs();
    let ay = y.abs();
    // atan(t) for t in [0, 1], in 1/65536 turns (an eighth is 8192):
    // atan(t) ~ t * (pi/4 + 0.273 * (1 - t)), max error about 0.04 degrees.
    let (small, big) = if ax < ay { (ax, ay) } else { (ay, ax) };
    let t = (small << 16) / big; // 0..=65536
    let a = ((t * (51472 + ((17891 * (65536 - t)) >> 16))) >> 16) * 65536 / 411775;
    // a: angle from the nearer axis, 0..=8192.
    let from_y = if ax < ay { a } else { 16384 - a };
    let turn = match (x >= 0, y >= 0) {
        (true, true) => from_y,
        (true, false) => 32768 - from_y,
        (false, false) => 32768 + from_y,
        (false, true) => 65536 - from_y,
    };
    (turn as u32) & 0xffff
}

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
    clip: Rect,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        Canvas {
            width,
            height,
            pixels: vec![0; width * height],
            clip: Rect::new(0, 0, width as i32, height as i32),
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    /// Limits drawing to `r` (within the canvas). Returns the old clip.
    pub fn set_clip(&mut self, r: Rect) -> Rect {
        let old = self.clip;
        self.clip = r.intersect(self.bounds());
        old
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return 0;
        }
        self.pixels[y as usize * self.width + x as usize]
    }

    #[inline]
    fn put(&mut self, x: i32, y: i32, c: Color, cov: u32) {
        let a = div255((c >> 24) * cov);
        if a == 0 {
            return;
        }
        let i = y as usize * self.width + x as usize;
        self.pixels[i] = blend(self.pixels[i], c, a);
    }

    /// Calls `f(x, y)` for every pixel of `r` inside the clip.
    fn area(&self, r: Rect) -> Option<(i32, i32, i32, i32)> {
        let r = r.intersect(self.clip);
        if r.w <= 0 || r.h <= 0 {
            None
        } else {
            Some((r.x, r.y, r.right(), r.bottom()))
        }
    }

    pub fn clear(&mut self, c: Color) {
        self.pixels.fill(c & 0x00ff_ffff);
    }

    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        let Some((x0, y0, x1, y1)) = self.area(r) else {
            return;
        };
        let a = c >> 24;
        for y in y0..y1 {
            let row = y as usize * self.width;
            for p in &mut self.pixels[row + x0 as usize..row + x1 as usize] {
                *p = blend(*p, c, a);
            }
        }
    }

    /// A gradient from `c0` to `c1`: top to bottom if `vertical`, else
    /// left to right. Ordered dithering hides 8-bit banding.
    pub fn fill_gradient(&mut self, r: Rect, c0: Color, c1: Color, vertical: bool) {
        let Some((x0, y0, x1, y1)) = self.area(r) else {
            return;
        };
        let len = if vertical { r.h } else { r.w }.max(1) as i64;
        for y in y0..y1 {
            for x in x0..x1 {
                let pos = if vertical { y - r.y } else { x - r.x } as i64;
                // t in 1/65536.
                let t = (pos * 65536 / len).clamp(0, 65536);
                let c = dither_lerp(c0, c1, t, x, y);
                let i = y as usize * self.width + x as usize;
                self.pixels[i] = blend(self.pixels[i], c, c >> 24);
            }
        }
    }

    /// A radial glow: `c` at the centre fading to nothing at `radius`.
    pub fn radial_glow(&mut self, cx: i32, cy: i32, radius: i32, c: Color) {
        let r = Rect::new(cx - radius, cy - radius, radius * 2, radius * 2);
        let Some((x0, y0, x1, y1)) = self.area(r) else {
            return;
        };
        let r2 = (radius as i64 * radius as i64).max(1);
        let a0 = (c >> 24) as i64;
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = (x - cx) as i64;
                let dy = (y - cy) as i64;
                let d2 = dx * dx + dy * dy;
                if d2 >= r2 {
                    continue;
                }
                // Smooth falloff: (1 - d^2/r^2)^2, with dithering.
                let f = 65536 - d2 * 65536 / r2;
                let f = (f * f) >> 16;
                let a16 = a0 * f; // alpha * 65536
                let a = ((a16 + BAYER[(y & 3) as usize][(x & 3) as usize] as i64 * 4096) >> 16)
                    .clamp(0, 255) as u32;
                let i = y as usize * self.width + x as usize;
                self.pixels[i] = blend(self.pixels[i], c, a);
            }
        }
    }

    fn fill_rrect_fx(&mut self, rr: RRect, feather: i64, c: Color) {
        let ext = feather;
        let bx0 = ((rr.cx - rr.hw - ext) >> 8) as i32;
        let by0 = ((rr.cy - rr.hh - ext) >> 8) as i32;
        let bx1 = ((rr.cx + rr.hw + ext + 255) >> 8) as i32;
        let by1 = ((rr.cy + rr.hh + ext + 255) >> 8) as i32;
        let Some((x0, y0, x1, y1)) = self.area(Rect::new(bx0, by0, bx1 - bx0, by1 - by0)) else {
            return;
        };
        let ca = c >> 24;
        for y in y0..y1 {
            let py = y as i64 * FX + 128;
            for x in x0..x1 {
                let px = x as i64 * FX + 128;
                let cov = coverage(rr.dist(px, py), feather);
                if cov == 0 {
                    continue;
                }
                let i = y as usize * self.width + x as usize;
                self.pixels[i] = blend(self.pixels[i], c, div255(ca * cov));
            }
        }
    }

    /// A filled rounded rectangle, anti-aliased.
    pub fn fill_rrect(&mut self, r: Rect, radius: i32, c: Color) {
        self.fill_rrect_fx(RRect::from_rect(r, radius as i64 * FX), FX, c);
    }

    /// A soft shadow of a rounded rectangle: the edge fades over `blur`
    /// pixels on each side.
    pub fn shadow(&mut self, r: Rect, radius: i32, blur: i32, c: Color) {
        let f = (blur.max(1) as i64) * 2 * FX;
        self.fill_rrect_fx(RRect::from_rect(r, radius as i64 * FX), f, c);
    }

    /// The outline of a rounded rectangle, `width` pixels wide inside `r`.
    /// `width` is in 1/256 pixel so thin borders scale smoothly.
    pub fn stroke_rrect_fx(&mut self, r: Rect, radius: i32, width: i64, c: Color) {
        let outer = RRect::from_rect(r, radius as i64 * FX);
        let w = width.max(FX / 4);
        let inner = RRect {
            hw: outer.hw - w,
            hh: outer.hh - w,
            r: (outer.r - w).max(0),
            ..outer
        };
        let bx0 = r.x;
        let by0 = r.y;
        let Some((x0, y0, x1, y1)) = self.area(Rect::new(bx0 - 1, by0 - 1, r.w + 2, r.h + 2))
        else {
            return;
        };
        let ca = c >> 24;
        for y in y0..y1 {
            let py = y as i64 * FX + 128;
            for x in x0..x1 {
                let px = x as i64 * FX + 128;
                let co = coverage(outer.dist(px, py), FX);
                if co == 0 {
                    continue;
                }
                let ci = if inner.hw > 0 && inner.hh > 0 {
                    coverage(inner.dist(px, py), FX)
                } else {
                    0
                };
                let cov = co.saturating_sub(ci);
                if cov == 0 {
                    continue;
                }
                let i = y as usize * self.width + x as usize;
                self.pixels[i] = blend(self.pixels[i], c, div255(ca * cov));
            }
        }
    }

    pub fn stroke_rrect(&mut self, r: Rect, radius: i32, width: i32, c: Color) {
        self.stroke_rrect_fx(r, radius, width as i64 * FX, c);
    }

    /// A soft-edged disc (edge `feather` units wide) that only shows
    /// inside the rounded rectangle `inside` (radius `inside_r` pixels).
    /// Used for the large waves on window backgrounds.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_circle_inside(
        &mut self,
        cx: i64,
        cy: i64,
        r: i64,
        feather: i64,
        inside: Rect,
        inside_r: i32,
        c: Color,
    ) {
        let disc = RRect {
            cx,
            cy,
            hw: r,
            hh: r,
            r,
        };
        let shape = RRect::from_rect(inside, inside_r as i64 * FX);
        let Some((x0, y0, x1, y1)) = self.area(inside) else {
            return;
        };
        let ca = c >> 24;
        for y in y0..y1 {
            let py = y as i64 * FX + 128;
            // Rows the disc does not reach.
            let dy = (py - cy).abs();
            if dy > r + feather {
                continue;
            }
            for x in x0..x1 {
                let px = x as i64 * FX + 128;
                let a = coverage(disc.dist(px, py), feather);
                if a == 0 {
                    continue;
                }
                let b = coverage(shape.dist(px, py), FX);
                let cov = a.min(b);
                if cov == 0 {
                    continue;
                }
                let i = y as usize * self.width + x as usize;
                self.pixels[i] = blend(self.pixels[i], c, div255(ca * cov));
            }
        }
    }

    /// A filled circle; centre and radius in 1/256 pixel.
    pub fn fill_circle_fx(&mut self, cx: i64, cy: i64, r: i64, c: Color) {
        let rr = RRect {
            cx,
            cy,
            hw: r,
            hh: r,
            r,
        };
        self.fill_rrect_fx(rr, FX, c);
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, c: Color) {
        self.fill_circle_fx(cx as i64 * FX, cy as i64 * FX, r as i64 * FX, c);
    }

    /// A ring (circle outline) of radius `r` to the middle of the line.
    pub fn stroke_circle_fx(&mut self, cx: i64, cy: i64, r: i64, width: i64, c: Color) {
        self.arc_fx(cx, cy, r, width, 0, 65536, false, c);
    }

    /// An arc of a ring from turn `start` for `len` (1/65536 turns,
    /// clockwise from the top), with round ends if `caps`.
    #[allow(clippy::too_many_arguments)]
    pub fn arc_fx(
        &mut self,
        cx: i64,
        cy: i64,
        r: i64,
        width: i64,
        start: u32,
        len: u32,
        caps: bool,
        c: Color,
    ) {
        if len == 0 {
            return;
        }
        let half = width / 2;
        let outer = r + half + FX;
        let bx0 = ((cx - outer) >> 8) as i32;
        let by0 = ((cy - outer) >> 8) as i32;
        let bx1 = ((cx + outer + 255) >> 8) as i32;
        let by1 = ((cy + outer + 255) >> 8) as i32;
        let Some((x0, y0, x1, y1)) = self.area(Rect::new(bx0, by0, bx1 - bx0, by1 - by0)) else {
            return;
        };
        let full = len >= 65536;
        let ca = c >> 24;
        for y in y0..y1 {
            let py = y as i64 * FX + 128;
            for x in x0..x1 {
                let px = x as i64 * FX + 128;
                let dx = px - cx;
                let dy = py - cy;
                let dist = isqrt((dx * dx + dy * dy) as u64) as i64;
                let cov = coverage((dist - r).abs() - half, FX);
                if cov == 0 {
                    continue;
                }
                if !full {
                    let a = angle(dx, dy).wrapping_sub(start) & 0xffff;
                    if a >= len {
                        continue;
                    }
                }
                let i = y as usize * self.width + x as usize;
                self.pixels[i] = blend(self.pixels[i], c, div255(ca * cov));
            }
        }
        if caps && !full {
            for t in [start, start.wrapping_add(len)] {
                let (sx, sy) = point_on_circle(cx, cy, r, t);
                self.fill_circle_fx(sx, sy, half, c);
            }
        }
    }

    /// A line with round ends; coordinates and width in 1/256 pixel.
    pub fn line_fx(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, width: i64, c: Color) {
        let half = width / 2;
        let ext = half + FX;
        let bx0 = ((x0.min(x1) - ext) >> 8) as i32;
        let by0 = ((y0.min(y1) - ext) >> 8) as i32;
        let bx1 = ((x0.max(x1) + ext + 255) >> 8) as i32;
        let by1 = ((y0.max(y1) + ext + 255) >> 8) as i32;
        let Some((ax0, ay0, ax1, ay1)) = self.area(Rect::new(bx0, by0, bx1 - bx0, by1 - by0))
        else {
            return;
        };
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len2 = dx * dx + dy * dy;
        let ca = c >> 24;
        for y in ay0..ay1 {
            let py = y as i64 * FX + 128;
            for x in ax0..ax1 {
                let px = x as i64 * FX + 128;
                // Closest point on the segment, t in 1/65536.
                let t = if len2 == 0 {
                    0
                } else {
                    (((px - x0) * dx + (py - y0) * dy) * 65536 / len2).clamp(0, 65536)
                };
                let qx = x0 + dx * t / 65536 - px;
                let qy = y0 + dy * t / 65536 - py;
                let d = isqrt((qx * qx + qy * qy) as u64) as i64 - half;
                let cov = coverage(d, FX);
                if cov == 0 {
                    continue;
                }
                let i = y as usize * self.width + x as usize;
                self.pixels[i] = blend(self.pixels[i], c, div255(ca * cov));
            }
        }
    }

    /// A line in whole pixels with a width in 1/256 pixel.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, width: i64, c: Color) {
        self.line_fx(
            x0 as i64 * FX,
            y0 as i64 * FX,
            x1 as i64 * FX,
            y1 as i64 * FX,
            width,
            c,
        );
    }

    /// A filled polygon (even-odd rule), points in 1/256 pixel. Each
    /// pixel is sampled 4x4 times; meant for small shapes such as icons.
    pub fn fill_polygon_fx(&mut self, pts: &[(i64, i64)], c: Color) {
        if pts.len() < 3 {
            return;
        }
        let minx = pts.iter().map(|p| p.0).min().unwrap_or(0) >> 8;
        let miny = pts.iter().map(|p| p.1).min().unwrap_or(0) >> 8;
        let maxx = (pts.iter().map(|p| p.0).max().unwrap_or(0) + 255) >> 8;
        let maxy = (pts.iter().map(|p| p.1).max().unwrap_or(0) + 255) >> 8;
        let Some((x0, y0, x1, y1)) = self.area(Rect::new(
            minx as i32,
            miny as i32,
            (maxx - minx) as i32,
            (maxy - miny) as i32,
        )) else {
            return;
        };
        let ca = c >> 24;
        for y in y0..y1 {
            for x in x0..x1 {
                let mut hits = 0u32;
                for sy in 0..4 {
                    let py = y as i64 * FX + 32 + sy * 64;
                    for sx in 0..4 {
                        let px = x as i64 * FX + 32 + sx * 64;
                        if inside(pts, px, py) {
                            hits += 1;
                        }
                    }
                }
                if hits == 0 {
                    continue;
                }
                let cov = hits * 255 / 16;
                let i = y as usize * self.width + x as usize;
                self.pixels[i] = blend(self.pixels[i], c, div255(ca * cov));
            }
        }
    }

    /// A coverage mask (glyph): `mask[w * h]`, drawn in colour `c`.
    ///
    /// Text is blended in linear light (gamma 2), so the edges of light
    /// text on a dark background and of dark text on a light one keep the
    /// same weight, and the coverage gets a slight contrast boost so small
    /// sizes stay crisp.
    pub fn draw_mask(&mut self, x: i32, y: i32, w: usize, h: usize, mask: &[u8], c: Color) {
        let Some((x0, y0, x1, y1)) = self.area(Rect::new(x, y, w as i32, h as i32)) else {
            return;
        };
        let ca = c >> 24;
        let s = [
            SQUARE[((c >> 16) & 0xff) as usize],
            SQUARE[((c >> 8) & 0xff) as usize],
            SQUARE[(c & 0xff) as usize],
        ];
        for py in y0..y1 {
            let mrow = (py - y) as usize * w;
            for px in x0..x1 {
                let cov = mask[mrow + (px - x) as usize] as usize;
                if cov == 0 {
                    continue;
                }
                let a = div255(ca * CONTRAST[cov] as u32);
                if a == 0 {
                    continue;
                }
                let i = py as usize * self.width + px as usize;
                if a >= 255 {
                    self.pixels[i] = c & 0x00ff_ffff;
                    continue;
                }
                let d = self.pixels[i];
                let inv = 255 - a;
                let ch = |sh: u32, sl: u32| {
                    let dl = SQUARE[((d >> sh) & 0xff) as usize];
                    let l = (sl * a + dl * inv) / 255;
                    (root(l) as u32) << sh
                };
                self.pixels[i] = ch(16, s[0]) | ch(8, s[1]) | ch(0, s[2]);
            }
        }
    }

    /// Draws a premultiplied RGBA image at (x, y) with `opacity`.
    pub fn draw_image(&mut self, img: &Image, x: i32, y: i32, opacity: u8) {
        let Some((x0, y0, x1, y1)) =
            self.area(Rect::new(x, y, img.width as i32, img.height as i32))
        else {
            return;
        };
        let o = opacity as u32;
        for py in y0..y1 {
            for px in x0..x1 {
                let si = ((py - y) as usize * img.width + (px - x) as usize) * 4;
                let s = &img.data[si..si + 4];
                let a = div255(s[3] as u32 * o);
                if a == 0 {
                    continue;
                }
                let i = py as usize * self.width + px as usize;
                let d = self.pixels[i];
                let inv = 255 - a;
                let ch = |sh: u32, sv: u8| {
                    (div255(sv as u32 * o) + div255(((d >> sh) & 0xff) * inv)).min(255)
                };
                self.pixels[i] = ch(16, s[0]) << 16 | ch(8, s[1]) << 8 | ch(0, s[2]);
            }
        }
    }

    /// Draws an image tinted with `c`: its alpha is used as a mask.
    pub fn draw_image_tinted(&mut self, img: &Image, x: i32, y: i32, c: Color) {
        let Some((x0, y0, x1, y1)) =
            self.area(Rect::new(x, y, img.width as i32, img.height as i32))
        else {
            return;
        };
        for py in y0..y1 {
            for px in x0..x1 {
                let si = ((py - y) as usize * img.width + (px - x) as usize) * 4;
                let cov = img.data[si + 3] as u32;
                if cov != 0 {
                    self.put(px, py, c, cov);
                }
            }
        }
    }

    /// Copies `r` of `src` (same size as this canvas) into this canvas.
    pub fn copy_from(&mut self, src: &Canvas, r: Rect) {
        let Some((x0, y0, x1, y1)) = self.area(r) else {
            return;
        };
        if src.width != self.width || src.height != self.height {
            return;
        }
        for y in y0..y1 {
            let a = y as usize * self.width + x0 as usize;
            let b = y as usize * self.width + x1 as usize;
            self.pixels[a..b].copy_from_slice(&src.pixels[a..b]);
        }
    }

    /// Writes the canvas as binary PPM (for tests and screenshots).
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = alloc::format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.reserve(self.width * self.height * 3);
        for p in &self.pixels {
            out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        out
    }
}

/// Point of a circle at turn `t` (1/65536, clockwise from the top).
pub fn point_on_circle(cx: i64, cy: i64, r: i64, t: u32) -> (i64, i64) {
    let (s, c) = sin_cos(t);
    (cx + r * s / 65536, cy - r * c / 65536)
}

/// Sine and cosine of `t` turns (1/65536), both scaled by 65536.
pub fn sin_cos(t: u32) -> (i64, i64) {
    let t = t & 0xffff;
    let quarter = t / 16384;
    let f = (t % 16384) as i64; // 0..16384 within the quarter
    let (s, c) = (sin_q(f), sin_q(16384 - f));
    match quarter {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

/// sin of `f` / 16384 of a quarter turn, scaled by 65536 (Bhaskara-free
/// polynomial, error below 0.001).
fn sin_q(f: i64) -> i64 {
    // x in [0, pi/2] mapped from f; sin x ~ x - x^3/6 + x^5/120 - x^7/5040.
    let x = f * 102944 / 16384; // pi/2 * 65536 = 102944
    let x2 = (x * x) >> 16;
    let mut term = x;
    let mut sum = x;
    for div in [6, 20, 42] {
        term = -((term * x2) >> 16) / div;
        sum += term;
    }
    sum.clamp(-65536, 65536)
}

/// `v * v` for 0..=255: the light of an sRGB value with gamma 2.
static SQUARE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = (i * i) as u32;
        i += 1;
    }
    t
};

/// Square root of 0..=65025 (back from light to an sRGB value), rounded.
#[inline]
fn root(l: u32) -> u8 {
    let mut lo = 0u32;
    let mut hi = 255u32;
    // Largest v with v * v <= l, then round to the nearer square.
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if mid * mid <= l {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo < 255 && l - lo * lo > lo {
        (lo + 1) as u8
    } else {
        lo as u8
    }
}

/// Glyph coverage with a slight contrast boost: thin stems at small sizes
/// get a little darker, full coverage stays full.
static CONTRAST: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        // c + 0.35 * c * (1 - c), in 1/255.
        let c = i as u32;
        let v = c + 35 * c * (255 - c) / (100 * 255);
        t[i] = if v > 255 { 255 } else { v as u8 };
        i += 1;
    }
    t
};

const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Mixes two colours at `t` / 65536 and dithers to 8 bits per channel.
fn dither_lerp(a: Color, b: Color, t: i64, x: i32, y: i32) -> Color {
    let d = BAYER[(y & 3) as usize][(x & 3) as usize] as i64 * 16 + 8; // 0..256
    let mut out = 0u32;
    for shift in [0, 8, 16, 24] {
        let p = ((a >> shift) & 0xff) as i64;
        let q = ((b >> shift) & 0xff) as i64;
        let v = p * 256 + (q - p) * 256 * t / 65536; // value * 256
        let v = ((v + d - 128) >> 8).clamp(0, 255) as u32;
        out |= v << shift;
    }
    out
}

fn inside(pts: &[(i64, i64)], px: i64, py: i64) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > py) != (yj > py) && px < xi + (xj - xi) * (py - yi) / (yj - yi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blending_and_colours() {
        assert_eq!(blend(0x000000, 0xffffff, 255), 0xffffff);
        assert_eq!(blend(0x000000, 0xffffff, 0), 0);
        assert_eq!(blend(0x000000, 0xffffff, 128), 0x808080);
        assert_eq!(fade(0xff102030, 128), 0x80102030);
        assert_eq!(lerp(0xff000000, 0xff0000ff, 255), 0xff0000ff);
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(99), 9);
        assert_eq!(isqrt(1 << 40), 1 << 20);
    }

    #[test]
    fn angles_and_trigonometry() {
        assert_eq!(angle(0, -10), 0);
        assert!((angle(10, 0) as i64 - 16384).abs() < 20);
        assert!((angle(0, 10) as i64 - 32768).abs() < 20);
        assert!((angle(-10, 0) as i64 - 49152).abs() < 20);
        assert!((angle(10, -10) as i64 - 8192).abs() < 20);
        for t in (0..65536).step_by(1000) {
            let (s, c) = sin_cos(t);
            let r = isqrt((s * s + c * c) as u64) as i64;
            assert!((r - 65536).abs() < 100, "t {} r {}", t, r);
            let (x, y) = point_on_circle(0, 0, 100 * FX, t);
            let back = angle(x, y) as i64;
            let diff = (back - t as i64).rem_euclid(65536);
            assert!(
                !(100..=65536 - 100).contains(&diff),
                "t {} back {}",
                t,
                back
            );
        }
    }

    #[test]
    fn rounded_rectangle_is_smooth_and_inside() {
        let mut c = Canvas::new(40, 40);
        c.fill_rrect(Rect::new(5, 5, 30, 30), 10, 0xffffffff);
        // Middle and straight edges fully covered.
        assert_eq!(c.get(20, 20), 0xffffff);
        assert_eq!(c.get(5, 20), 0xffffff);
        assert_eq!(c.get(4, 20), 0);
        // The corner pixel is outside, the arc is partly covered.
        assert_eq!(c.get(5, 5), 0);
        let partial = (0..40)
            .flat_map(|y| (0..40).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let v = c.get(x, y) & 0xff;
                v != 0 && v != 0xff
            })
            .count();
        assert!(partial > 20, "anti-aliased pixels: {}", partial);
        // Symmetric.
        for y in 0..40 {
            for x in 0..40 {
                assert_eq!(c.get(x, y), c.get(39 - x, y));
                assert_eq!(c.get(x, y), c.get(x, 39 - y));
            }
        }
    }

    #[test]
    fn outline_leaves_the_inside() {
        let mut c = Canvas::new(30, 30);
        c.stroke_rrect(Rect::new(0, 0, 30, 30), 8, 2, 0xffffffff);
        assert_eq!(c.get(15, 0), 0xffffff);
        assert_eq!(c.get(15, 1), 0xffffff);
        assert_eq!(c.get(15, 2), 0);
        assert_eq!(c.get(15, 15), 0);
    }

    #[test]
    fn arc_covers_its_part_of_the_ring() {
        let mut c = Canvas::new(100, 100);
        // A quarter: from the top to the right, clockwise.
        c.arc_fx(
            50 * FX,
            50 * FX,
            40 * FX,
            6 * FX,
            0,
            16384,
            false,
            0xffffffff,
        );
        assert_eq!(c.get(70, 16), 0xffffff); // top right
        assert_eq!(c.get(30, 16), 0); // top left
        assert_eq!(c.get(50, 90), 0); // bottom
        assert_eq!(c.get(50, 50), 0); // centre
    }

    #[test]
    fn lines_polygons_and_gradients() {
        let mut c = Canvas::new(40, 40);
        c.line(5, 20, 35, 20, 4 * FX, 0xffff0000);
        assert_eq!(c.get(20, 20), 0xff0000);
        assert_eq!(c.get(20, 25), 0);
        let tri = [(0, 0), (40 * FX, 0), (0, 40 * FX)];
        let mut p = Canvas::new(40, 40);
        p.fill_polygon_fx(&tri, 0xff00ff00);
        assert_eq!(p.get(5, 5), 0x00ff00);
        assert_eq!(p.get(35, 35), 0);
        let mut g = Canvas::new(64, 4);
        g.fill_gradient(Rect::new(0, 0, 64, 4), 0xff000000, 0xffffffff, false);
        let left = (0..4).map(|y| g.get(0, y) & 0xff).sum::<u32>();
        let right = (0..4).map(|y| g.get(63, y) & 0xff).sum::<u32>();
        assert!(left < 20 && right > 980);
    }

    #[test]
    fn clip_limits_drawing() {
        let mut c = Canvas::new(20, 20);
        c.set_clip(Rect::new(0, 0, 10, 20));
        c.fill_rect(Rect::new(0, 0, 20, 20), 0xffffffff);
        assert_eq!(c.get(9, 5), 0xffffff);
        assert_eq!(c.get(10, 5), 0);
    }
}

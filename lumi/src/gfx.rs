//! Compositing helpers: window pictures copied with rounded corners and
//! opacity, scaled for animations, and soft shadows from a cached
//! nine-part picture. Everything stays inside the canvas clip.

use alloc::vec;
use alloc::vec::Vec;
use velora_ui::Rect;
use velora_ui::canvas::{Canvas, Color, blend, isqrt};

/// Coverage of the top-left corner of a rounded rectangle, `r` x `r`.
pub struct Corner {
    pub r: i32,
    mask: Vec<u8>,
}

impl Corner {
    pub fn new(r: i32) -> Corner {
        let r = r.max(0);
        let mut mask = vec![255u8; (r * r) as usize];
        // Centre of the arc at (r, r); pixel centres at +0.5; in 1/16 px.
        let rr = r as i64 * 16;
        for y in 0..r {
            for x in 0..r {
                let dx = rr - (x as i64 * 16 + 8);
                let dy = rr - (y as i64 * 16 + 8);
                let d = isqrt((dx * dx + dy * dy) as u64) as i64;
                let cov = ((rr - d) * 255 / 16 + 128).clamp(0, 255);
                mask[(y * r + x) as usize] = cov as u8;
            }
        }
        Corner { r, mask }
    }

    /// Coverage of pixel (x, y) of a `w` x `h` rounded rectangle.
    #[inline]
    pub fn at(&self, x: i32, y: i32, w: i32, h: i32) -> u32 {
        let r = self.r;
        let cx = if x < r {
            x
        } else if x >= w - r {
            w - 1 - x
        } else {
            return 255;
        };
        let cy = if y < r {
            y
        } else if y >= h - r {
            h - 1 - y
        } else {
            return 255;
        };
        if cx < 0 || cy < 0 {
            return 0;
        }
        self.mask[(cy * r + cx) as usize] as u32
    }
}

/// Copies `src` to (x, y) of `dst` with rounded corners and `alpha`.
pub fn blit(dst: &mut Canvas, src: &Canvas, x: i32, y: i32, corner: &Corner, alpha: u32) {
    let (w, h) = (src.width as i32, src.height as i32);
    let area = Rect::new(x, y, w, h).intersect(dst.clip());
    if area.is_empty() || alpha == 0 {
        return;
    }
    let r = corner.r.min(w / 2).min(h / 2);
    for dy in area.y..area.bottom() {
        let sy = dy - y;
        let srow = sy as usize * src.width;
        let drow = dy as usize * dst.width;
        let in_band = sy < r || sy >= h - r;
        let (x0, x1) = (area.x, area.right());
        if !in_band && alpha >= 255 {
            let s0 = srow + (x0 - x) as usize;
            let n = (x1 - x0) as usize;
            dst.pixels[drow + x0 as usize..drow + x1 as usize]
                .copy_from_slice(&src.pixels[s0..s0 + n]);
            continue;
        }
        for dx in x0..x1 {
            let sx = dx - x;
            let cov = if in_band && (sx < r || sx >= w - r) {
                corner.at(sx, sy, w, h)
            } else {
                255
            };
            let a = cov * alpha / 255;
            if a == 0 {
                continue;
            }
            let p = src.pixels[srow + sx as usize];
            let d = &mut dst.pixels[drow + dx as usize];
            *d = if a >= 255 { p } else { blend(*d, p, a) };
        }
    }
}

/// Draws `src` scaled into `to` (nearest pixel) with rounded corners
/// (`corner` made for the scaled size) and `alpha`. For animations.
pub fn blit_scaled(dst: &mut Canvas, src: &Canvas, to: Rect, corner: &Corner, alpha: u32) {
    if to.w <= 0 || to.h <= 0 || alpha == 0 {
        return;
    }
    if to.w == src.width as i32 && to.h == src.height as i32 {
        blit(dst, src, to.x, to.y, corner, alpha);
        return;
    }
    let area = to.intersect(dst.clip());
    if area.is_empty() {
        return;
    }
    let sx_step = ((src.width as i64) << 16) / to.w as i64;
    let sy_step = ((src.height as i64) << 16) / to.h as i64;
    for dy in area.y..area.bottom() {
        let ly = dy - to.y;
        let sy = (((ly as i64 * 2 + 1) * sy_step) >> 17).min(src.height as i64 - 1) as usize;
        let srow = sy * src.width;
        let drow = dy as usize * dst.width;
        for dx in area.x..area.right() {
            let lx = dx - to.x;
            let sx = (((lx as i64 * 2 + 1) * sx_step) >> 17).min(src.width as i64 - 1) as usize;
            let cov = corner.at(lx, ly, to.w, to.h);
            let a = cov * alpha / 255;
            if a == 0 {
                continue;
            }
            let p = src.pixels[srow + sx];
            let d = &mut dst.pixels[drow + dx as usize];
            *d = if a >= 255 { p } else { blend(*d, p, a) };
        }
    }
}

/// A soft shadow as nine parts: corners from a small picture, edges
/// repeated from its middle row and column.
pub struct Shadow {
    /// Shadow strength per pixel (0..=255), `size` x `size`.
    mask: Vec<u8>,
    size: i32,
    /// How far the shadow reaches past the shape.
    pub pad: i32,
    /// Half the size of the shape the picture was made for.
    k: i32,
    key: (i32, i32),
}

impl Shadow {
    pub fn new(radius: i32, blur: i32) -> Shadow {
        let pad = blur * 2 + 2;
        let k = radius + blur + 2;
        let size = 2 * pad + 2 * k;
        let mut c = Canvas::new(size as usize, size as usize);
        c.clear(0xffffff);
        c.shadow(Rect::new(pad, pad, 2 * k, 2 * k), radius, blur, 0xff000000);
        let mask = c.pixels.iter().map(|p| 255 - (p & 0xff) as u8).collect();
        Shadow {
            mask,
            size,
            pad,
            k,
            key: (radius, blur),
        }
    }

    pub fn matches(&self, radius: i32, blur: i32) -> bool {
        self.key == (radius, blur)
    }

    /// Draws the shadow of `r` in colour `c` (its alpha is the strength),
    /// without the inside of `covered` except its corners (`radius`): the
    /// shape there hides it.
    pub fn draw(&self, dst: &mut Canvas, r: Rect, radius: i32, c: Color, covered: Rect) {
        let (pad, k, size) = (self.pad, self.k, self.size);
        let outer = r.inset(-pad);
        let area = outer.intersect(dst.clip());
        if area.is_empty() {
            return;
        }
        let strength = c >> 24;
        let rgb = c & 0x00ff_ffff;
        let tw = outer.w;
        let th = outer.h;
        let map = |u: i32, t: i32| -> i32 {
            if u < pad + k {
                u
            } else if u >= t - pad - k {
                u - (t - size)
            } else {
                pad + k
            }
        };
        let radius = radius.max(0);

        let r = covered;
        for y in area.y..area.bottom() {
            let v = map(y - outer.y, th).clamp(0, size - 1);
            let row = (v * size) as usize;
            let drow = y as usize * dst.width;
            let covered_row = y >= r.y && y < r.bottom();
            let corner_row = y < r.y + radius || y >= r.bottom() - radius;
            let mut x = area.x;
            while x < area.right() {
                if covered_row && x >= r.x && x < r.right() {
                    // Inside the shape: only its corner squares show.
                    let corner_col = x < r.x + radius || x >= r.right() - radius;
                    if !(corner_row && corner_col) {
                        x = if corner_row {
                            r.right() - radius
                        } else {
                            r.right()
                        };
                        continue;
                    }
                }
                let u = map(x - outer.x, tw).clamp(0, size - 1);
                let m = self.mask[row + u as usize] as u32;
                let a = m * strength / 255;
                if a > 0 {
                    let d = &mut dst.pixels[drow + x as usize];
                    *d = blend(*d, rgb, a);
                }
                x += 1;
            }
        }
    }
}

/// Copies `r` of `src` into `dst` (same size canvases), inside the clip.
pub fn copy(dst: &mut Canvas, src: &Canvas, r: Rect) {
    let r = r.intersect(dst.clip()).intersect(src.bounds());
    if r.is_empty() {
        return;
    }
    for y in r.y..r.bottom() {
        let row = y as usize * dst.width;
        let (a, b) = (row + r.x as usize, row + r.right() as usize);
        dst.pixels[a..b].copy_from_slice(&src.pixels[a..b]);
    }
}

/// Mixes `r` of `src` over `dst` by `t` / 255 (same size canvases).
pub fn mix(dst: &mut Canvas, src: &Canvas, r: Rect, t: u32) {
    if t >= 255 {
        copy(dst, src, r);
        return;
    }
    let r = r.intersect(dst.clip()).intersect(src.bounds());
    if r.is_empty() || t == 0 {
        return;
    }
    for y in r.y..r.bottom() {
        let row = y as usize * dst.width;
        for i in row + r.x as usize..row + r.right() as usize {
            dst.pixels[i] = blend(dst.pixels[i], src.pixels[i], t);
        }
    }
}

/// Copies the rounded rectangle `r` of `src` into `dst` (same size
/// canvases): frosted glass under panels and menus.
pub fn copy_rounded(dst: &mut Canvas, src: &Canvas, r: Rect, corner: &Corner, alpha: u32) {
    let area = r.intersect(dst.clip()).intersect(src.bounds());
    if area.is_empty() {
        return;
    }
    for y in area.y..area.bottom() {
        let row = y as usize * dst.width;
        for x in area.x..area.right() {
            let cov = corner.at(x - r.x, y - r.y, r.w, r.h) * alpha / 255;
            if cov == 0 {
                continue;
            }
            let i = row + x as usize;
            dst.pixels[i] = if cov >= 255 {
                src.pixels[i]
            } else {
                blend(dst.pixels[i], src.pixels[i], cov)
            };
        }
    }
}

/// Linear interpolation of rectangles, `t` 0..=1000.
pub fn lerp_rect(a: Rect, b: Rect, t: u32) -> Rect {
    let l = |x: i32, y: i32| x + ((y - x) as i64 * t as i64 / 1000) as i32;
    Rect::new(l(a.x, b.x), l(a.y, b.y), l(a.w, b.w), l(a.h, b.h))
}

/// `r` scaled about its centre by `s` / 1000.
pub fn scale_rect(r: Rect, s: u32) -> Rect {
    let w = (r.w as i64 * s as i64 / 1000) as i32;
    let h = (r.h as i64 * s as i64 / 1000) as i32;
    Rect::new(r.center_x() - w / 2, r.center_y() - h / 2, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_copy_and_shadow() {
        let mut dst = Canvas::new(40, 40);
        dst.clear(0x000000);
        let mut src = Canvas::new(20, 20);
        src.clear(0xffffff);
        let corner = Corner::new(6);
        blit(&mut dst, &src, 10, 10, &corner, 255);
        assert_eq!(dst.pixels[20 * 40 + 20], 0xffffff);
        assert_eq!(dst.pixels[10 * 40 + 10], 0, "the corner is cut");
        assert_eq!(dst.pixels[10 * 40 + 20], 0xffffff, "the top edge is kept");
        // Half opacity, then a scaled copy keeps the middle.
        let mut d2 = Canvas::new(40, 40);
        blit(&mut d2, &src, 10, 10, &corner, 128);
        assert_eq!(d2.pixels[20 * 40 + 20] & 0xff, 128);
        let mut d3 = Canvas::new(40, 40);
        blit_scaled(&mut d3, &src, Rect::new(5, 5, 30, 30), &Corner::new(9), 255);
        assert_eq!(d3.pixels[20 * 40 + 20], 0xffffff);
        // The shadow darkens outside the shape and leaves its inside.
        let mut bg = Canvas::new(60, 60);
        bg.clear(0xffffff);
        let sh = Shadow::new(4, 6);
        sh.draw(
            &mut bg,
            Rect::new(20, 20, 20, 20),
            4,
            0xff000000,
            Rect::new(20, 20, 20, 20),
        );
        assert_eq!(bg.pixels[30 * 60 + 30], 0xffffff, "inside untouched");
        assert!(
            bg.pixels[30 * 60 + 41] & 0xff < 200,
            "dark next to the edge"
        );
        assert!(bg.pixels[30 * 60 + 41] & 0xff < bg.pixels[30 * 60 + 50] & 0xff);
    }
}

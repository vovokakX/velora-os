//! The Velora mark drawn from its geometry: two blades and the cursor
//! bar, defined on a 200 x 200 grid, filled with antialiased edges on a
//! transparent background at any size. Programs draw it at the exact
//! pixel size they need instead of scaling a picture.

use crate::image::Image;
use alloc::vec::Vec;

/// The size of the design grid.
pub const GRID: i64 = 200;

/// The left blade, the right blade (grid points, clockwise).
pub const BLADES: [[(i64, i64); 4]; 2] = [
    [(20, 34), (58, 34), (104, 150), (94, 176)],
    [(180, 34), (142, 34), (106, 124), (112, 140)],
];

/// The cursor bar: x, y, width, height on the grid.
pub const CURSOR: (i64, i64, i64, i64) = (93, 40, 14, 58);

/// The part of the grid the mark covers: x0, y0, x1, y1.
pub const BOUNDS: (i64, i64, i64, i64) = (20, 34, 180, 176);

/// Samples per pixel along each axis (16 per pixel).
const SS: i64 = 4;

/// The shapes of the mark as polygons.
fn shapes() -> [Vec<(i64, i64)>; 3] {
    let (x, y, w, h) = CURSOR;
    [
        BLADES[0].to_vec(),
        BLADES[1].to_vec(),
        alloc::vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)],
    ]
}

/// Whether the point (`px` / `den`, `py` / `den`) on the grid is inside
/// `poly` (crossing number, exact in integers).
fn inside(poly: &[(i64, i64)], px: i64, py: i64, den: i64) -> bool {
    let mut odd = false;
    let n = poly.len();
    for i in 0..n {
        let (ax, ay) = (poly[i].0 * den, poly[i].1 * den);
        let (bx, by) = (poly[(i + 1) % n].0 * den, poly[(i + 1) % n].1 * den);
        if (ay > py) != (by > py) {
            // x of the edge at py, compared without division.
            let lhs = (px - ax) * (by - ay);
            let rhs = (bx - ax) * (py - ay);
            if (by > ay && lhs < rhs) || (by < ay && lhs > rhs) {
                odd = !odd;
            }
        }
    }
    odd
}

/// The mark `height` pixels high (its width follows the proportions,
/// 160 : 142), in `rgb` (0xRRGGBB) on a transparent background, cropped to
/// the mark.
pub fn mark(height: usize, rgb: u32) -> Image {
    let (x0, y0, x1, y1) = BOUNDS;
    let height = height.max(1) as i64;
    let width = ((x1 - x0) * height + (y1 - y0) / 2) / (y1 - y0);
    let mut img = Image::new(width as usize, height as usize);
    let shapes = shapes();
    // Pixel (x, y) covers grid [x0 + x * gh / height, ...) with gh the
    // grid height; a sample at sub-position (i + 0.5) / SS of the pixel.
    // On a common denominator den = height * SS * 2 everything is whole.
    let gh = y1 - y0;
    let den = height * SS * 2;
    let rgb = [(rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff];
    for py in 0..height {
        for px in 0..width {
            let mut hits = 0u32;
            for sy in 0..SS {
                let gy = y0 * den + (py * SS * 2 + sy * 2 + 1) * gh;
                for sx in 0..SS {
                    let gx = x0 * den + (px * SS * 2 + sx * 2 + 1) * gh;
                    if shapes.iter().any(|s| inside(s, gx, gy, den)) {
                        hits += 1;
                    }
                }
            }
            if hits == 0 {
                continue;
            }
            let a = (hits * 255 + (SS * SS) as u32 / 2) / (SS * SS) as u32;
            let i = (py as usize * width as usize + px as usize) * 4;
            for c in 0..3 {
                img.data[i + c] = ((rgb[c] * a + 127) / 255) as u8;
            }
            img.data[i + 3] = a as u8;
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pixel of `img` at grid point (gx, gy).
    fn at(img: &Image, gx: i64, gy: i64) -> [u8; 4] {
        let (x0, y0, _, y1) = BOUNDS;
        let s = img.height as i64;
        img.pixel(
            ((gx - x0) * s / (y1 - y0)) as usize,
            ((gy - y0) * s / (y1 - y0)) as usize,
        )
    }

    #[test]
    fn the_mark_has_its_shape_at_any_size() {
        for h in [64, 128, 400] {
            let m = mark(h, 0xffffff);
            assert_eq!(m.height, h);
            assert_eq!(m.width, (160 * h + 71) / 142);
            // Strokes: the cursor bar, both blades, the tip.
            for (gx, gy) in [(100, 60), (40, 45), (160, 45), (93, 160)] {
                assert_eq!(at(&m, gx, gy), [255; 4], "{} px: ({}, {})", h, gx, gy);
            }
            // Gaps: under the cursor, between the blades, outside the tip.
            for (gx, gy) in [(100, 112), (75, 40), (125, 40), (40, 150), (160, 150)] {
                assert_eq!(at(&m, gx, gy)[3], 0, "{} px: ({}, {})", h, gx, gy);
            }
            // The edges are antialiased: partly covered pixels exist.
            assert!(m.data.chunks(4).any(|p| p[3] > 0 && p[3] < 255));
            // Nothing is cut off and there is no margin: the mark reaches
            // the sides (the sharp corners may cover less than a sample).
            let t = m.trim();
            assert!(m.width - t.width <= 1 && m.height - t.height <= 1);
        }
        // Colour is premultiplied by coverage.
        let m = mark(64, 0x80ff00);
        let p = m.data.chunks(4).find(|p| p[3] == 255).unwrap();
        assert_eq!(p, [0x80, 0xff, 0x00, 0xff]);
    }
}

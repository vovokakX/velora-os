//! RGBA images with premultiplied alpha: building them from protocol
//! data, area-average scaling and blending over an opaque background.
//!
//! Premultiplied alpha keeps scaling correct at the edges of transparent
//! areas (no dark fringes) and makes blending one multiply per channel.

use alloc::vec;
use alloc::vec::Vec;

/// An image of `width * height` pixels, 4 bytes each: R, G, B premultiplied
/// by A, then A.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

#[inline]
fn mul(c: u8, a: u8) -> u8 {
    // Rounded c * a / 255.
    let t = c as u32 * a as u32 + 128;
    ((t + (t >> 8)) >> 8) as u8
}

impl Image {
    /// A fully transparent image.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0; width * height * 4],
        }
    }

    /// From packed 8-bit RGB (opaque).
    pub fn from_rgb(width: usize, height: usize, rgb: &[u8]) -> Option<Self> {
        if rgb.len() != width.checked_mul(height)?.checked_mul(3)? {
            return None;
        }
        let mut data = Vec::with_capacity(width * height * 4);
        for p in rgb.as_chunks::<3>().0 {
            data.extend_from_slice(&[p[0], p[1], p[2], 255]);
        }
        Some(Self {
            width,
            height,
            data,
        })
    }

    /// From packed 8-bit RGBA with straight (not premultiplied) alpha.
    pub fn from_rgba(width: usize, height: usize, rgba: &[u8]) -> Option<Self> {
        if rgba.len() != width.checked_mul(height)?.checked_mul(4)? {
            return None;
        }
        let mut data = Vec::with_capacity(rgba.len());
        for p in rgba.as_chunks::<4>().0 {
            let a = p[3];
            data.extend_from_slice(&[mul(p[0], a), mul(p[1], a), mul(p[2], a), a]);
        }
        Some(Self {
            width,
            height,
            data,
        })
    }

    /// Packed RGBA with straight alpha (what the Kitty format 32 carries).
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len());
        for p in self.data.as_chunks::<4>().0 {
            let a = p[3] as u32;
            if a == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let un = |c: u8| ((c as u32 * 255 + a / 2) / a).min(255) as u8;
                out.extend_from_slice(&[un(p[0]), un(p[1]), un(p[2]), p[3]]);
            }
        }
        out
    }

    /// Premultiplied pixel at (x, y).
    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Scaled to `width * height` by area averaging: every target pixel is
    /// the mean of the source area it covers, weighted by exact overlap.
    /// Good for shrinking; enlarging by a whole factor repeats pixels.
    pub fn resize(&self, width: usize, height: usize) -> Image {
        if (width, height) == (self.width, self.height) {
            return self.clone();
        }
        if width == 0 || height == 0 || self.width == 0 || self.height == 0 {
            return Image::new(width, height);
        }
        let wide = resample(&self.data, self.width, self.height, width, true);
        let data = resample(&wide, width, self.height, height, false);
        Image {
            width,
            height,
            data,
        }
    }

    /// Draws `src` with its top-left corner at (x, y), over this image
    /// (source-over); the parts outside are cut off.
    pub fn draw(&mut self, src: &Image, x: usize, y: usize) {
        for sy in 0..src.height {
            let ty = y + sy;
            if ty >= self.height {
                break;
            }
            for sx in 0..src.width {
                let tx = x + sx;
                if tx >= self.width {
                    break;
                }
                let s = src.pixel(sx, sy);
                let i = (ty * self.width + tx) * 4;
                let keep = 255 - s[3];
                for (c, sc) in s.iter().enumerate() {
                    self.data[i + c] = sc.saturating_add(mul(self.data[i + c], keep));
                }
            }
        }
    }
}

/// One pass of the area-average filter along x (`horizontal`) or y.
/// Source pixel k covers [k * dst, (k + 1) * dst) and target pixel t
/// covers [t * src, (t + 1) * src) on a common grid, so all weights are
/// whole numbers and each target pixel's weights add up to `src`.
fn resample(data: &[u8], w: usize, h: usize, dst: usize, horizontal: bool) -> Vec<u8> {
    let src = if horizontal { w } else { h };
    let (ow, oh) = if horizontal { (dst, h) } else { (w, dst) };
    let mut out = vec![0u8; ow * oh * 4];
    let lines = if horizontal { h } else { w };
    for line in 0..lines {
        for t in 0..dst {
            let start = t * src;
            let end = start + src;
            let mut sum = [0u64; 4];
            let mut k = start / dst;
            while k * dst < end && k < src {
                let lo = (k * dst).max(start);
                let hi = ((k + 1) * dst).min(end);
                let weight = (hi - lo) as u64;
                let (x, y) = if horizontal { (k, line) } else { (line, k) };
                let i = (y * w + x) * 4;
                for c in 0..4 {
                    sum[c] += data[i + c] as u64 * weight;
                }
                k += 1;
            }
            let (x, y) = if horizontal { (t, line) } else { (line, t) };
            let o = (y * ow + x) * 4;
            for c in 0..4 {
                out[o + c] = ((sum[c] + src as u64 / 2) / src as u64) as u8;
            }
        }
    }
    out
}

/// The largest size with the aspect ratio of `w * h` that fits into
/// `box_w * box_h` (at least 1x1).
pub fn fit(w: usize, h: usize, box_w: usize, box_h: usize) -> (usize, usize) {
    if w == 0 || h == 0 {
        return (box_w, box_h);
    }
    if w * box_h <= h * box_w {
        (((w * box_h + h / 2) / h).clamp(1, box_w), box_h)
    } else {
        (box_w, ((h * box_w + w / 2) / w).clamp(1, box_h))
    }
}

/// A premultiplied pixel over an opaque 0xRRGGBB colour.
#[inline]
pub fn blend(px: [u8; 4], under: u32) -> u32 {
    let keep = 255 - px[3];
    let ch =
        |shift: u32, c: u8| (c.saturating_add(mul((under >> shift) as u8, keep)) as u32) << shift;
    ch(16, px[0]) | ch(8, px[1]) | ch(0, px[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_round_trip() {
        let rgba = [255, 0, 0, 255, 10, 200, 30, 128, 1, 2, 3, 0];
        let img = Image::from_rgba(3, 1, &rgba).unwrap();
        assert_eq!(img.pixel(1, 0), [5, 100, 15, 128]);
        let back = img.to_rgba();
        assert_eq!(&back[0..4], &[255, 0, 0, 255]);
        // Within rounding of the 7-bit alpha.
        assert!((back[5] as i32 - 200).abs() <= 2 && back[7] == 128);
        assert_eq!(&back[8..12], &[0, 0, 0, 0]);
        assert!(Image::from_rgba(2, 2, &rgba).is_none());
        assert!(Image::from_rgb(1, 1, &[1, 2, 3]).is_some());
    }

    #[test]
    fn enlarging_by_whole_factor_repeats_pixels() {
        let img = Image::from_rgb(2, 1, &[255, 0, 0, 0, 0, 255]).unwrap();
        let big = img.resize(4, 2);
        assert_eq!(big.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(big.pixel(1, 1), [255, 0, 0, 255]);
        assert_eq!(big.pixel(2, 0), [0, 0, 255, 255]);
        assert_eq!(big.pixel(3, 1), [0, 0, 255, 255]);
    }

    #[test]
    fn shrinking_averages_areas() {
        // A 4x4 checkerboard of black and white becomes mid grey.
        let mut rgb = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                rgb.extend_from_slice(&[v, v, v]);
            }
        }
        let img = Image::from_rgb(4, 4, &rgb).unwrap();
        let small = img.resize(2, 2);
        for p in small.data.chunks(4) {
            assert_eq!(p, &[128, 128, 128, 255]);
        }
        // A non-integer factor: 3 pixels into 2. Weights 2:1 and 1:2.
        let row = Image::from_rgb(3, 1, &[0, 0, 0, 255, 255, 255, 30, 30, 30]).unwrap();
        let two = row.resize(2, 1);
        assert_eq!(two.pixel(0, 0)[0], 85);
        assert_eq!(two.pixel(1, 0)[0], 105);
    }

    #[test]
    fn fit_keeps_aspect() {
        assert_eq!(fit(1024, 1024, 160, 160), (160, 160));
        assert_eq!(fit(200, 100, 160, 160), (160, 80));
        assert_eq!(fit(100, 200, 160, 160), (80, 160));
        assert_eq!(fit(1000, 1, 10, 10), (10, 1));
    }

    #[test]
    fn blending_and_drawing() {
        assert_eq!(blend([0, 0, 0, 0], 0x123456), 0x123456);
        assert_eq!(blend([255, 255, 255, 255], 0x123456), 0xffffff);
        // Half-transparent white over black is mid grey.
        assert_eq!(blend([128, 128, 128, 128], 0), 0x808080);
        let mut canvas = Image::new(3, 3);
        let dot = Image::from_rgb(1, 1, &[9, 8, 7]).unwrap();
        canvas.draw(&dot, 1, 1);
        canvas.draw(&dot, 2, 5); // outside: ignored
        assert_eq!(canvas.pixel(1, 1), [9, 8, 7, 255]);
        assert_eq!(canvas.pixel(0, 0), [0, 0, 0, 0]);
    }
}

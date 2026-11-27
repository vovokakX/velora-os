//! Anti-aliased text with TrueType fonts (fontdue), with a glyph cache.

use crate::canvas::{Canvas, Color};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use fontdue::{Font, FontSettings};

/// Font weights the toolkit loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Weight {
    Regular = 0,
    Medium = 1,
    SemiBold = 2,
}

struct Glyph {
    xmin: i32,
    ymin: i32,
    width: usize,
    height: usize,
    advance: f32,
    bitmap: Vec<u8>,
}

pub struct Fonts {
    fonts: Vec<Font>,
    /// The x-height of each face per pixel of size.
    x_height: Vec<f32>,
    cache: BTreeMap<(u8, char, u32), Glyph>,
}

impl Fonts {
    /// Loads the regular, medium and semibold faces.
    pub fn new(regular: &[u8], medium: &[u8], semibold: &[u8]) -> Result<Self, &'static str> {
        let mut fonts = Vec::new();
        for data in [regular, medium, semibold] {
            let settings = FontSettings {
                scale: 32.0,
                ..FontSettings::default()
            };
            fonts.push(Font::from_bytes(data, settings)?);
        }
        let x_height = fonts
            .iter()
            .map(|f| {
                let m = f.metrics('x', 1000.0);
                (m.height as i32 + m.ymin) as f32 / 1000.0
            })
            .collect();
        Ok(Fonts {
            fonts,
            x_height,
            cache: BTreeMap::new(),
        })
    }

    /// The size actually used for `px`: changed by a few percent at most
    /// so that the x-height is a whole number of pixels. With the baseline
    /// on a pixel row too, the tops and bottoms of lowercase letters fall
    /// on pixel edges and stay sharp (a light form of hinting; fontdue has
    /// no hinter).
    pub fn snapped_size(&self, w: Weight, px: u32) -> f32 {
        let p = px as f32;
        let r = self.x_height[w as usize];
        if px < 8 || r <= 0.1 {
            return p;
        }
        let target = round(p * r) as f32;
        let s = target / r;
        if s < p * 0.94 {
            p * 0.94
        } else if s > p * 1.06 {
            p * 1.06
        } else {
            s
        }
    }

    fn font(&self, w: Weight) -> &Font {
        &self.fonts[w as usize]
    }

    fn glyph(&mut self, w: Weight, ch: char, px: u32) -> &Glyph {
        let size = self.snapped_size(w, px);
        let font = &self.fonts[w as usize];
        self.cache.entry((w as u8, ch, px)).or_insert_with(|| {
            let (m, bitmap) = font.rasterize(ch, size);
            Glyph {
                xmin: m.xmin,
                ymin: m.ymin,
                width: m.width,
                height: m.height,
                advance: m.advance_width,
                bitmap,
            }
        })
    }

    /// Distance from the top of a line to the baseline.
    pub fn ascent(&self, w: Weight, px: u32) -> i32 {
        self.font(w)
            .horizontal_line_metrics(self.snapped_size(w, px))
            .map_or(px as i32, |m| round(m.ascent))
    }

    /// Height of a line of text.
    pub fn line_height(&self, w: Weight, px: u32) -> i32 {
        self.font(w)
            .horizontal_line_metrics(self.snapped_size(w, px))
            .map_or(px as i32 * 5 / 4, |m| round(m.new_line_size))
    }

    /// Height of capital letters above the baseline (for centring).
    pub fn cap_height(&mut self, w: Weight, px: u32) -> i32 {
        let g = self.glyph(w, 'H', px);
        g.height as i32 + g.ymin
    }

    /// Width of `text` in pixels.
    pub fn measure(&mut self, w: Weight, px: u32, text: &str) -> i32 {
        let mut pen = 0.0f32;
        let mut prev = None;
        for ch in text.chars() {
            if let Some(p) = prev {
                pen += self
                    .font(w)
                    .horizontal_kern(p, ch, self.snapped_size(w, px))
                    .unwrap_or(0.0);
            }
            pen += self.glyph(w, ch, px).advance;
            prev = Some(ch);
        }
        round(pen)
    }

    /// Draws `text` with its baseline at `baseline`; returns the width.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        c: &mut Canvas,
        w: Weight,
        px: u32,
        x: i32,
        baseline: i32,
        text: &str,
        color: Color,
    ) -> i32 {
        let mut pen = 0.0f32;
        let mut prev = None;
        for ch in text.chars() {
            if let Some(p) = prev {
                pen += self
                    .font(w)
                    .horizontal_kern(p, ch, self.snapped_size(w, px))
                    .unwrap_or(0.0);
            }
            let g = self.glyph(w, ch, px);
            let gx = x + round(pen) + g.xmin;
            let gy = baseline - g.height as i32 - g.ymin;
            c.draw_mask(gx, gy, g.width, g.height, &g.bitmap, color);
            pen += g.advance;
            prev = Some(ch);
        }
        round(pen)
    }

    /// Draws `text` vertically centred on `cy` (by the capital height).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_centered_y(
        &mut self,
        c: &mut Canvas,
        w: Weight,
        px: u32,
        x: i32,
        cy: i32,
        text: &str,
        color: Color,
    ) -> i32 {
        let cap = self.cap_height(w, px);
        self.draw(c, w, px, x, cy + cap / 2, text, color)
    }

    /// Splits `text` into lines no wider than `max` (at spaces; explicit
    /// newlines are kept).
    pub fn wrap(&mut self, w: Weight, px: u32, text: &str, max: i32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split(' ') {
                let candidate = if line.is_empty() {
                    String::from(word)
                } else {
                    alloc::format!("{} {}", line, word)
                };
                if !line.is_empty() && self.measure(w, px, &candidate) > max {
                    lines.push(core::mem::take(&mut line));
                    line = String::from(word);
                } else {
                    line = candidate;
                }
            }
            lines.push(line);
        }
        lines
    }
}

fn round(v: f32) -> i32 {
    if v >= 0.0 {
        (v + 0.5) as i32
    } else {
        (v - 0.5) as i32
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The Inter faces fetched by tools/fetch-fonts.sh.
    pub(crate) fn fonts() -> Fonts {
        let dir = alloc::format!("{}/../../build/fonts", env!("CARGO_MANIFEST_DIR"));
        let read = |f: &str| {
            std::fs::read(alloc::format!("{}/{}", dir, f))
                .unwrap_or_else(|_| panic!("{}/{} missing: run tools/fetch-fonts.sh", dir, f))
        };
        Fonts::new(
            &read("Inter-Regular.ttf"),
            &read("Inter-Medium.ttf"),
            &read("Inter-SemiBold.ttf"),
        )
        .unwrap()
    }

    #[test]
    fn measures_draws_and_wraps() {
        let mut f = fonts();
        let a = f.measure(Weight::Regular, 16, "Velora");
        let b = f.measure(Weight::Regular, 32, "Velora");
        // Sizes snap by a few percent (x-height on whole pixels).
        assert!(a > 30 && (b - 2 * a).abs() <= a / 8, "{} {}", a, b);
        assert!(f.measure(Weight::SemiBold, 16, "Добро") > f.measure(Weight::Regular, 16, "Добро"));
        assert!(f.ascent(Weight::Regular, 20) > 14 && f.line_height(Weight::Regular, 20) >= 20);
        let mut c = Canvas::new(200, 40);
        let w = f.draw(&mut c, Weight::Medium, 20, 4, 28, "Привет", 0xffffffff);
        assert!(w > 50);
        // Anti-aliased: full and partial coverage both appear.
        let full = c.pixels.iter().filter(|&&p| p == 0xffffff).count();
        let part = c
            .pixels
            .iter()
            .filter(|&&p| p != 0 && p != 0xffffff)
            .count();
        assert!(full > 20 && part > 20, "{} {}", full, part);
        // Nothing above the line box or left of x.
        assert!((0..40).all(|y| c.get(0, y) == 0));
        let lines = f.wrap(
            Weight::Regular,
            14,
            "Современная, быстрая и удобная система",
            150,
        );
        assert!(lines.len() >= 2);
        assert!(
            lines
                .iter()
                .all(|l| f.measure(Weight::Regular, 14, l) <= 150)
        );
        assert_eq!(lines.join(" "), "Современная, быстрая и удобная система");
    }

    #[test]
    fn x_height_lands_on_whole_pixels() {
        let mut f = fonts();
        for px in [11u32, 13, 15, 16, 17, 20, 24, 33] {
            let s = f.snapped_size(Weight::Regular, px);
            assert!(
                (s - px as f32).abs() <= px as f32 * 0.06 + 0.01,
                "{} {}",
                px,
                s
            );
            // The flat top of 'x' fills its top row (sharp, not a half row).
            let mut c = Canvas::new(60, 60);
            f.draw(&mut c, Weight::Regular, px, 2, 50, "x", 0xffffffff);
            let rows: Vec<u32> = (0..60)
                .map(|y| (0..60).map(|x| c.get(x, y) & 0xff).max().unwrap())
                .collect();
            let top = rows.iter().position(|&v| v > 0).unwrap();
            assert!(rows[top] > 150, "size {}: top row {}", px, rows[top]);
        }
    }
}

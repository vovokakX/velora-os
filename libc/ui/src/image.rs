//! Pictures from PNG files, scaled once per size and kept.

use alloc::vec::Vec;
use velora_gfx::image::{self, Image};
use velora_gfx::png;

/// A picture with its scaled copies.
pub struct Picture {
    pub source: Image,
    scaled: Vec<Image>,
}

impl Picture {
    /// Decodes a PNG (at most 4 megapixels).
    pub fn from_png(data: &[u8]) -> Option<Picture> {
        let source = png::decode(data, 4 << 20).ok()?;
        Some(Picture {
            source,
            scaled: Vec::new(),
        })
    }

    /// Decodes a PNG with a solid background: the colour of its top-left
    /// corner becomes transparent and the empty border is cut away.
    pub fn from_png_on_background(data: &[u8]) -> Option<Picture> {
        let mut img = png::decode(data, 4 << 20).ok()?;
        let c = img.pixel(0, 0);
        img.color_to_alpha((c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32);
        Some(Picture {
            source: img.trim(),
            scaled: Vec::new(),
        })
    }

    /// The picture fitted into `w` x `h`, keeping its proportions.
    pub fn fit(&mut self, w: usize, h: usize) -> &Image {
        let (fw, fh) = image::fit(self.source.width, self.source.height, w.max(1), h.max(1));
        self.sized(fw, fh)
    }

    /// The picture at exactly `w` x `h`.
    pub fn sized(&mut self, w: usize, h: usize) -> &Image {
        if let Some(i) = self
            .scaled
            .iter()
            .position(|s| s.width == w && s.height == h)
        {
            return &self.scaled[i];
        }
        // Keep a few sizes (the screen and the pointer sizes).
        if self.scaled.len() >= 6 {
            self.scaled.remove(0);
        }
        self.scaled.push(self.source.resize(w.max(1), h.max(1)));
        self.scaled.last().unwrap()
    }
}

/// A mouse pointer picture and its hot spot (the tip), in source pixels.
pub struct Pointer {
    pub picture: Picture,
    pub hot_x: usize,
    pub hot_y: usize,
}

impl Pointer {
    /// The pointer from a PNG with a transparent background; the hot spot
    /// is the first opaque pixel from the top-left (the tip).
    pub fn from_png(data: &[u8]) -> Option<Pointer> {
        let picture = Picture::from_png(data)?;
        let img = &picture.source;
        let mut hot = (0, 0);
        'find: for s in 0..img.width + img.height {
            for y in 0..=s.min(img.height - 1) {
                let x = s - y;
                if x < img.width && img.pixel(x, y)[3] > 128 {
                    hot = (x, y);
                    break 'find;
                }
            }
        }
        Some(Pointer {
            picture,
            hot_x: hot.0,
            hot_y: hot.1,
        })
    }

    /// The pointer `size` pixels tall (of the source square) and its hot
    /// spot at that size.
    pub fn at(&mut self, size: usize) -> (&Image, i32, i32) {
        let src = self.picture.source.width.max(1);
        let (hx, hy) = (self.hot_x * size / src, self.hot_y * size / src);
        (self.picture.sized(size, size), hx as i32, hy as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_and_pointers_load() {
        let logo = include_bytes!("../../../assets/velora-logo.png");
        let mut p = Picture::from_png_on_background(logo).unwrap();
        // The mark is wider than tall and its background is gone.
        assert!(p.source.width > p.source.height);
        assert_eq!(p.source.pixel(0, p.source.height - 1)[3], 0);
        let small = p.fit(100, 100);
        assert_eq!(small.width, 100);
        for name in ["classic", "blade", "drop"] {
            let data = std::fs::read(alloc::format!(
                "{}/../../assets/cursors/{}.png",
                env!("CARGO_MANIFEST_DIR"),
                name
            ))
            .unwrap();
            let mut ptr = Pointer::from_png(&data).unwrap();
            assert!(
                ptr.hot_x < 60 && ptr.hot_y < 60,
                "{} {:?}",
                name,
                (ptr.hot_x, ptr.hot_y)
            );
            let (img, hx, hy) = ptr.at(32);
            assert_eq!(img.width, 32);
            assert!(hx < 8 && hy < 8);
        }
    }
}

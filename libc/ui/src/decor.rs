//! The look shared by Velora's graphical programs: a dark graphite
//! backdrop and rounded windows with soft waves on their background.

use crate::canvas::{Canvas, FX, fade};
use crate::geom::Rect;
use crate::theme::Theme;

/// Fills the whole canvas with the backdrop: a slow diagonal gradient and
/// two faint glows.
pub fn backdrop(c: &mut Canvas, t: &Theme) {
    let b = c.bounds();
    let top = crate::canvas::lerp(t.backdrop, if t.dark { 0xff2a2a2e } else { 0xffe6e6ea }, 90);
    c.fill_gradient(b, top, t.backdrop, true);
    let r = (b.w.max(b.h) as i64 * 6 / 10) as i32;
    c.radial_glow(b.w / 5, b.h / 6, r, t.backdrop_glow);
    c.radial_glow(b.w * 9 / 10, b.h, r, fade(t.backdrop_glow, 160));
}

/// A window: shadow, background with waves and a thin light border.
/// `scale` is pixels per design unit times 256 (for the shadow size).
pub fn window(c: &mut Canvas, r: Rect, radius: i32, t: &Theme, scale: i64) {
    let s = |du: i64| (du * scale / 256) as i32;
    c.shadow(r.offset(0, s(10)), radius, s(28), t.shadow);
    c.fill_rrect(r, radius, t.window);
    waves(c, r, radius, t);
    c.stroke_rrect_fx(r, radius, (scale / 2).max(FX), t.window_border);
}

/// The soft waves: large discs, lighter and darker, cut by the window
/// shape. Their edges sweep from the bottom middle to the top right, as
/// in the design.
pub fn waves(c: &mut Canvas, r: Rect, radius: i32, t: &Theme) {
    waves_side(c, r, radius, t, false);
}

/// Waves from the bottom right (`mirror` false) or the bottom left.
pub fn waves_side(c: &mut Canvas, r: Rect, radius: i32, t: &Theme, mirror: bool) {
    let w = r.w as i64;
    let h = r.h as i64;
    let x = r.x as i64;
    let y = r.y as i64;
    let at = |fx: i64, fy: i64| {
        let fx = if mirror { 1000 - fx } else { fx };
        (x * FX + w * FX * fx / 1000, y * FX + h * FX * fy / 1000)
    };
    let soft = (w * FX / 300).max(FX);
    let shade = if t.dark { 0xff000000 } else { 0xffffffff };
    // A broad lighter sweep.
    let (cx, cy) = at(1180, 1500);
    c.fill_circle_inside(cx, cy, w * FX * 78 / 100, soft * 3, r, radius, t.wave);
    // A darker cut inside it, leaving a band.
    let (cx, cy) = at(1330, 1640);
    c.fill_circle_inside(
        cx,
        cy,
        w * FX * 70 / 100,
        soft * 2,
        r,
        radius,
        fade(shade, if t.dark { 40 } else { 50 }),
    );
    // A second, brighter band near the bottom right corner.
    let (cx, cy) = at(1250, 1720);
    c.fill_circle_inside(
        cx,
        cy,
        w * FX * 55 / 100,
        soft * 2,
        r,
        radius,
        fade(t.wave, 200),
    );
    let (cx, cy) = at(1400, 1830);
    c.fill_circle_inside(
        cx,
        cy,
        w * FX * 50 / 100,
        soft * 2,
        r,
        radius,
        fade(shade, if t.dark { 36 } else { 40 }),
    );
    // A faint light from the top right.
    let (cx, cy) = at(900, -350);
    c.fill_circle_inside(
        cx,
        cy,
        w * FX * 40 / 100,
        soft * 40,
        r,
        radius,
        fade(t.wave, 140),
    );
}

/// The parts of a screen that do not change between frames (backdrop,
/// window, waves, panels), drawn once and copied into each frame. Large
/// soft shapes are the slowest thing to draw, and programs are built
/// without SSE.
#[derive(Default)]
pub struct Cache {
    key: u64,
    canvas: Option<Canvas>,
}

impl Cache {
    /// Copies the cached picture for `key` into `c`, drawing it with
    /// `draw` first if the key or the canvas size changed.
    pub fn paint(&mut self, c: &mut Canvas, key: u64, draw: impl FnOnce(&mut Canvas)) {
        let fresh = self.key == key
            && self
                .canvas
                .as_ref()
                .is_some_and(|b| b.width == c.width && b.height == c.height);
        if !fresh {
            let mut b = Canvas::new(c.width, c.height);
            draw(&mut b);
            self.canvas = Some(b);
            self.key = key;
        }
        let b = self.canvas.as_ref().unwrap();
        c.pixels.copy_from_slice(&b.pixels);
    }
}

/// A side panel at the left of a window, its own rounded shape.
pub fn sidebar(c: &mut Canvas, r: Rect, radius: i32, t: &Theme) {
    c.fill_rrect(r, radius, t.sidebar);
}

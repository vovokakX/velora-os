//! Line icons drawn from primitives, so they scale to any size. Each
//! takes the square it fills: top-left corner and side in pixels.

use crate::canvas::{Canvas, Color, FX};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Globe,
    Clock,
    Keyboard,
    Pointer,
    ChevronDown,
    ArrowLeft,
    ArrowRight,
    Check,
    Moon,
    Sun,
    Refresh,
    Picture,
}

/// Point (u, v) of a 24-unit icon grid, in 1/256 pixel.
struct Grid {
    x: i64,
    y: i64,
    size: i64,
}

impl Grid {
    fn p(&self, u: i64, v: i64) -> (i64, i64) {
        (
            self.x * FX + u * self.size * FX / 24,
            self.y * FX + v * self.size * FX / 24,
        )
    }

    fn len(&self, u: i64) -> i64 {
        u * self.size * FX / 24
    }
}

/// Draws `icon` in the square (x, y, size). `stroke` is the line width
/// in 1/256 pixel (0: a width that suits the size).
pub fn draw(c: &mut Canvas, icon: Icon, x: i32, y: i32, size: i32, color: Color, stroke: i64) {
    let g = Grid {
        x: x as i64,
        y: y as i64,
        size: size as i64,
    };
    let w = if stroke > 0 {
        stroke
    } else {
        (size as i64 * FX / 13).max(FX)
    };
    let line =
        |c: &mut Canvas, a: (i64, i64), b: (i64, i64)| c.line_fx(a.0, a.1, b.0, b.1, w, color);
    match icon {
        Icon::Globe => {
            let (cx, cy) = g.p(12, 12);
            let r = g.len(9);
            c.stroke_circle_fx(cx, cy, r, w, color);
            // Meridian as an ellipse: two arcs approximated by polylines.
            let rx = g.len(4);
            let mut prev = (cx, cy - r);
            for i in 1..=24 {
                let t = i * 65536 / 24;
                let (s, co) = crate::canvas::sin_cos(t as u32);
                let p = (cx + rx * s / 65536, cy - r * co / 65536);
                line(c, prev, p);
                prev = p;
            }
            line(c, g.p(3, 12), g.p(21, 12));
            line(c, g.p(5, 7), g.p(19, 7));
            line(c, g.p(5, 17), g.p(19, 17));
        }
        Icon::Clock => {
            let (cx, cy) = g.p(12, 12);
            c.stroke_circle_fx(cx, cy, g.len(9), w, color);
            line(c, g.p(12, 12), g.p(12, 7));
            line(c, g.p(12, 12), g.p(15, 14));
        }
        Icon::Keyboard => {
            let (x0, y0) = g.p(2, 6);
            let (x1, y1) = g.p(22, 18);
            // Outline from four lines with round joins.
            line(c, (x0, y0), (x1, y0));
            line(c, (x1, y0), (x1, y1));
            line(c, (x1, y1), (x0, y1));
            line(c, (x0, y1), (x0, y0));
            for row in 0..2 {
                for col in 0..5 {
                    let (kx, ky) = g.p(5 + col * 3 + 1, 9 + row * 3 + 1);
                    c.fill_circle_fx(kx, ky, w * 6 / 10, color);
                }
            }
            line(c, g.p(8, 15), g.p(16, 15));
        }
        Icon::Pointer => {
            let pts = [
                g.p(6, 3),
                g.p(6, 19),
                g.p(10, 15),
                g.p(13, 21),
                g.p(15, 20),
                g.p(12, 14),
                g.p(18, 14),
            ];
            for i in 0..pts.len() {
                line(c, pts[i], pts[(i + 1) % pts.len()]);
            }
        }
        Icon::ChevronDown => {
            line(c, g.p(6, 9), g.p(12, 15));
            line(c, g.p(12, 15), g.p(18, 9));
        }
        Icon::ArrowLeft => {
            line(c, g.p(19, 12), g.p(5, 12));
            line(c, g.p(5, 12), g.p(11, 6));
            line(c, g.p(5, 12), g.p(11, 18));
        }
        Icon::ArrowRight => {
            line(c, g.p(5, 12), g.p(19, 12));
            line(c, g.p(19, 12), g.p(13, 6));
            line(c, g.p(19, 12), g.p(13, 18));
        }
        Icon::Check => {
            line(c, g.p(5, 12), g.p(10, 17));
            line(c, g.p(10, 17), g.p(19, 7));
        }
        Icon::Moon => {
            // A crescent: a circle with a shifted circle cut out, drawn as a
            // polygon of the outer arc and the inner arc.
            let (cx, cy) = g.p(12, 12);
            let r = g.len(8);
            let (ix, iy) = g.p(16, 9);
            let ri = g.len(7);
            let mut pts = alloc::vec::Vec::new();
            for i in 0..=40 {
                let t = (16000 + i * 50000 / 40) as u32;
                let (s, co) = crate::canvas::sin_cos(t);
                let p = (cx + r * s / 65536, cy - r * co / 65536);
                let (dx, dy) = (p.0 - ix, p.1 - iy);
                if dx * dx + dy * dy >= ri * ri {
                    pts.push(p);
                }
            }
            for i in (0..=40).rev() {
                let t = (i * 65536 / 40) as u32;
                let (s, co) = crate::canvas::sin_cos(t);
                let p = (ix + ri * s / 65536, iy - ri * co / 65536);
                let (dx, dy) = (p.0 - cx, p.1 - cy);
                if dx * dx + dy * dy <= r * r {
                    pts.push(p);
                }
            }
            c.fill_polygon_fx(&pts, color);
        }
        Icon::Sun => {
            let (cx, cy) = g.p(12, 12);
            c.fill_circle_fx(cx, cy, g.len(5), color);
            for i in 0..8u32 {
                let t = i * 8192;
                let a = crate::canvas::point_on_circle(cx, cy, g.len(8), t);
                let b = crate::canvas::point_on_circle(cx, cy, g.len(10), t);
                line(c, a, b);
            }
        }
        Icon::Refresh => {
            let (cx, cy) = g.p(12, 12);
            let r = g.len(7);
            c.arc_fx(cx, cy, r, w, 12000, 50000, false, color);
            let tip = crate::canvas::point_on_circle(cx, cy, r, 12000);
            line(c, tip, (tip.0 - g.len(5), tip.1 - g.len(1)));
            line(c, tip, (tip.0 + g.len(1), tip.1 - g.len(5)));
        }
        Icon::Picture => {
            let pts = [
                g.p(4, 18),
                g.p(10, 9),
                g.p(14, 15),
                g.p(16, 12),
                g.p(20, 18),
            ];
            c.fill_polygon_fx(&pts, color);
            let (sx, sy) = g.p(16, 7);
            c.fill_circle_fx(sx, sy, g.len(2), color);
        }
    }
}

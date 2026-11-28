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
    ChevronUp,
    ArrowLeft,
    ArrowRight,
    Check,
    Moon,
    Sun,
    Refresh,
    Picture,
    Warning,
    User,
    Lock,
    /// Four dots in a square: the app menu.
    Apps,
    /// A quill pen: the Plume editor.
    Feather,
    Folder,
    /// A prompt `>_`: the terminal.
    Terminal,
    Gear,
    Info,
    Speaker,
    /// A speaker with a cross: no sound.
    SpeakerMute,
    /// Signal arcs: the network.
    Network,
    Power,
    Search,
    Close,
    Minimize,
    Maximize,
    /// Two overlapping squares: a maximized window comes back.
    Restore,
    File,
    FileText,
    ArrowUp,
    Plus,
    Trash,
    Pencil,
    LogOut,
    Home,
    FolderPlus,
    Copy,
    Disk,
    /// A monitor on a stand.
    Display,
    /// A window with its title bar.
    Window,
    /// The screen with a bar at the bottom (the panel).
    Dock,
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
        Icon::ChevronUp => {
            line(c, g.p(6, 15), g.p(12, 9));
            line(c, g.p(12, 9), g.p(18, 15));
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
        Icon::Warning => {
            let pts = [g.p(12, 3), g.p(22, 20), g.p(2, 20)];
            for i in 0..3 {
                line(c, pts[i], pts[(i + 1) % 3]);
            }
            line(c, g.p(12, 9), g.p(12, 14));
            let (dx, dy) = g.p(12, 17);
            c.fill_circle_fx(dx, dy, w * 7 / 10, color);
        }
        Icon::User => {
            let (cx, cy) = g.p(12, 8);
            c.stroke_circle_fx(cx, cy, g.len(4), w, color);
            let (bx, by) = g.p(12, 22);
            c.arc_fx(bx, by, g.len(8), w, 49152, 32768, true, color);
        }
        Icon::Lock => {
            let (x0, y0) = g.p(5, 11);
            let (x1, y1) = g.p(19, 21);
            line(c, (x0, y0), (x1, y0));
            line(c, (x1, y0), (x1, y1));
            line(c, (x1, y1), (x0, y1));
            line(c, (x0, y1), (x0, y0));
            let (cx, cy) = g.p(12, 10);
            c.arc_fx(cx, cy, g.len(4), w, 49152, 32768, false, color);
            line(c, g.p(8, 10), g.p(8, 11));
            line(c, g.p(16, 10), g.p(16, 11));
        }
        Icon::Apps => {
            for (u, v) in [(8, 8), (16, 8), (8, 16), (16, 16)] {
                let (x, y) = g.p(u, v);
                c.fill_circle_fx(x, y, g.len(5) / 2 + w / 4, color);
            }
        }
        Icon::Feather => {
            // The vane: a leaf shape from the tip at the top right.
            let pts = [
                g.p(20, 3),
                g.p(17, 4),
                g.p(13, 6),
                g.p(9, 10),
                g.p(7, 14),
                g.p(7, 17),
                g.p(10, 17),
                g.p(14, 15),
                g.p(17, 12),
                g.p(19, 8),
            ];
            c.fill_polygon_fx(&pts, color);
            // The shaft down to the bottom left.
            line(c, g.p(15, 9), g.p(4, 21));
        }
        Icon::Folder => {
            let pts = [
                g.p(2, 6),
                g.p(9, 6),
                g.p(11, 8),
                g.p(22, 8),
                g.p(22, 19),
                g.p(2, 19),
            ];
            c.fill_polygon_fx(&pts, color);
        }
        Icon::FolderPlus => {
            let pts = [
                g.p(2, 6),
                g.p(9, 6),
                g.p(11, 8),
                g.p(22, 8),
                g.p(22, 19),
                g.p(2, 19),
            ];
            for i in 0..pts.len() {
                line(c, pts[i], pts[(i + 1) % pts.len()]);
            }
            line(c, g.p(12, 11), g.p(12, 17));
            line(c, g.p(9, 14), g.p(15, 14));
        }
        Icon::Terminal => {
            line(c, g.p(4, 7), g.p(10, 12));
            line(c, g.p(10, 12), g.p(4, 17));
            line(c, g.p(13, 18), g.p(20, 18));
        }
        Icon::Gear => {
            let (cx, cy) = g.p(12, 12);
            for i in 0..8u32 {
                let t = i * 8192;
                let a = crate::canvas::point_on_circle(cx, cy, g.len(6), t);
                let b = crate::canvas::point_on_circle(cx, cy, g.len(10), t);
                c.line_fx(a.0, a.1, b.0, b.1, w * 9 / 5, color);
            }
            c.stroke_circle_fx(cx, cy, g.len(6), w * 2, color);
        }
        Icon::Info => {
            let (cx, cy) = g.p(12, 12);
            c.stroke_circle_fx(cx, cy, g.len(9), w, color);
            line(c, g.p(12, 11), g.p(12, 17));
            let (dx, dy) = g.p(12, 7);
            c.fill_circle_fx(dx, dy, w * 7 / 10, color);
        }
        Icon::Speaker => {
            let pts = [
                g.p(3, 9),
                g.p(7, 9),
                g.p(12, 4),
                g.p(12, 20),
                g.p(7, 15),
                g.p(3, 15),
            ];
            c.fill_polygon_fx(&pts, color);
            let (cx, cy) = g.p(12, 12);
            c.arc_fx(cx, cy, g.len(4), w, 10000, 12768, true, color);
            c.arc_fx(cx, cy, g.len(8), w, 9000, 14768, true, color);
        }
        Icon::SpeakerMute => {
            let pts = [
                g.p(3, 9),
                g.p(7, 9),
                g.p(12, 4),
                g.p(12, 20),
                g.p(7, 15),
                g.p(3, 15),
            ];
            c.fill_polygon_fx(&pts, color);
            line(c, g.p(16, 9), g.p(22, 15));
            line(c, g.p(22, 9), g.p(16, 15));
        }
        Icon::Network => {
            let (cx, cy) = g.p(12, 19);
            for r in [5, 10, 15] {
                c.arc_fx(cx, cy, g.len(r), w, 57344, 16384, true, color);
            }
            c.fill_circle_fx(cx, cy, w, color);
        }
        Icon::Power => {
            let (cx, cy) = g.p(12, 13);
            c.arc_fx(cx, cy, g.len(8), w, 6000, 53536, true, color);
            line(c, g.p(12, 3), g.p(12, 12));
        }
        Icon::Search => {
            let (cx, cy) = g.p(10, 10);
            c.stroke_circle_fx(cx, cy, g.len(6), w, color);
            line(c, g.p(15, 15), g.p(21, 21));
        }
        Icon::Display => {
            let pts = [g.p(3, 4), g.p(21, 4), g.p(21, 16), g.p(3, 16)];
            for i in 0..4 {
                line(c, pts[i], pts[(i + 1) % 4]);
            }
            line(c, g.p(12, 16), g.p(12, 20));
            line(c, g.p(8, 20), g.p(16, 20));
        }
        Icon::Window => {
            let pts = [g.p(3, 5), g.p(21, 5), g.p(21, 19), g.p(3, 19)];
            for i in 0..4 {
                line(c, pts[i], pts[(i + 1) % 4]);
            }
            line(c, g.p(3, 9), g.p(21, 9));
            let (dx, dy) = g.p(17, 7);
            c.fill_circle_fx(dx, dy, w * 6 / 10, color);
        }
        Icon::Dock => {
            let pts = [g.p(3, 4), g.p(21, 4), g.p(21, 20), g.p(3, 20)];
            for i in 0..4 {
                line(c, pts[i], pts[(i + 1) % 4]);
            }
            line(c, g.p(8, 16), g.p(16, 16));
        }
        Icon::Close => {
            line(c, g.p(6, 6), g.p(18, 18));
            line(c, g.p(18, 6), g.p(6, 18));
        }
        Icon::Minimize => {
            line(c, g.p(6, 12), g.p(18, 12));
        }
        Icon::Maximize => {
            let pts = [g.p(6, 6), g.p(18, 6), g.p(18, 18), g.p(6, 18)];
            for i in 0..4 {
                line(c, pts[i], pts[(i + 1) % 4]);
            }
        }
        Icon::Restore => {
            let pts = [g.p(5, 9), g.p(15, 9), g.p(15, 19), g.p(5, 19)];
            for i in 0..4 {
                line(c, pts[i], pts[(i + 1) % 4]);
            }
            line(c, g.p(9, 5), g.p(19, 5));
            line(c, g.p(19, 5), g.p(19, 15));
        }
        Icon::File | Icon::FileText => {
            let pts = [g.p(6, 3), g.p(14, 3), g.p(19, 8), g.p(19, 21), g.p(6, 21)];
            for i in 0..pts.len() {
                line(c, pts[i], pts[(i + 1) % pts.len()]);
            }
            line(c, g.p(14, 3), g.p(14, 8));
            line(c, g.p(14, 8), g.p(19, 8));
            if icon == Icon::FileText {
                line(c, g.p(9, 13), g.p(16, 13));
                line(c, g.p(9, 17), g.p(16, 17));
            }
        }
        Icon::ArrowUp => {
            line(c, g.p(12, 19), g.p(12, 5));
            line(c, g.p(12, 5), g.p(6, 11));
            line(c, g.p(12, 5), g.p(18, 11));
        }
        Icon::Plus => {
            line(c, g.p(12, 5), g.p(12, 19));
            line(c, g.p(5, 12), g.p(19, 12));
        }
        Icon::Trash => {
            line(c, g.p(4, 6), g.p(20, 6));
            line(c, g.p(9, 6), g.p(10, 3));
            line(c, g.p(10, 3), g.p(14, 3));
            line(c, g.p(14, 3), g.p(15, 6));
            let pts = [g.p(6, 6), g.p(18, 6), g.p(17, 21), g.p(7, 21)];
            for i in 1..4 {
                line(c, pts[i], pts[(i + 1) % 4]);
            }
            line(c, g.p(10, 10), g.p(10, 17));
            line(c, g.p(14, 10), g.p(14, 17));
        }
        Icon::Pencil => {
            line(c, g.p(4, 20), g.p(5, 15));
            line(c, g.p(4, 20), g.p(9, 19));
            line(c, g.p(5, 15), g.p(16, 4));
            line(c, g.p(9, 19), g.p(20, 8));
            line(c, g.p(16, 4), g.p(20, 8));
            line(c, g.p(13, 7), g.p(17, 11));
        }
        Icon::LogOut => {
            line(c, g.p(10, 4), g.p(4, 4));
            line(c, g.p(4, 4), g.p(4, 20));
            line(c, g.p(4, 20), g.p(10, 20));
            line(c, g.p(9, 12), g.p(21, 12));
            line(c, g.p(21, 12), g.p(16, 7));
            line(c, g.p(21, 12), g.p(16, 17));
        }
        Icon::Home => {
            line(c, g.p(3, 11), g.p(12, 3));
            line(c, g.p(12, 3), g.p(21, 11));
            let pts = [g.p(6, 9), g.p(6, 20), g.p(18, 20), g.p(18, 9)];
            for i in 0..3 {
                line(c, pts[i], pts[i + 1]);
            }
            line(c, g.p(10, 20), g.p(10, 14));
            line(c, g.p(10, 14), g.p(14, 14));
            line(c, g.p(14, 14), g.p(14, 20));
        }
        Icon::Copy => {
            let pts = [g.p(8, 8), g.p(20, 8), g.p(20, 20), g.p(8, 20)];
            for i in 0..4 {
                line(c, pts[i], pts[(i + 1) % 4]);
            }
            line(c, g.p(4, 16), g.p(4, 4));
            line(c, g.p(4, 4), g.p(16, 4));
        }
        Icon::Disk => {
            let pts = [g.p(3, 7), g.p(21, 7), g.p(21, 17), g.p(3, 17)];
            for i in 0..4 {
                line(c, pts[i], pts[(i + 1) % 4]);
            }
            let (x, y) = g.p(17, 12);
            c.fill_circle_fx(x, y, w, color);
            line(c, g.p(6, 12), g.p(12, 12));
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

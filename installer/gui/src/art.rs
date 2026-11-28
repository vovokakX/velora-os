//! Pictures drawn for the installer: the disk drive, theme and wallpaper
//! previews, the step marks of the sidebar, the light/dark switch.

use velora_ui::Rect;
use velora_ui::canvas::{Canvas, Color, FX, fade, lerp, rgb};
use velora_ui::icons::{self, Icon};
use velora_ui::theme::Theme;

/// A solid state drive seen from above at an angle, in `r`.
pub fn drive(c: &mut Canvas, r: Rect, t: &Theme) {
    let body = if t.dark {
        rgb(92, 92, 98)
    } else {
        rgb(150, 150, 158)
    };
    let top = if t.dark {
        rgb(112, 112, 119)
    } else {
        rgb(176, 176, 184)
    };
    let edge = if t.dark {
        rgb(58, 58, 63)
    } else {
        rgb(118, 118, 126)
    };
    let rad = r.h / 6;
    // Soft shadow under the drive.
    c.shadow(
        Rect::new(r.x + r.w / 10, r.bottom() - r.h / 5, r.w * 8 / 10, r.h / 6),
        r.h / 12,
        r.h / 8,
        fade(0xff000000, 120),
    );
    // Front face.
    let front = Rect::new(r.x, r.y + r.h * 55 / 100, r.w, r.h * 35 / 100);
    c.fill_rrect(front, rad, edge);
    // Top face as a trapezoid (narrower at the back).
    let inset = (r.w as i64) * FX / 10;
    let x0 = r.x as i64 * FX;
    let x1 = r.right() as i64 * FX;
    let y0 = r.y as i64 * FX;
    let y1 = (r.y + r.h * 70 / 100) as i64 * FX;
    let round = (rad as i64) * FX;
    let pts = [
        (x0 + inset + round, y0),
        (x1 - inset - round, y0),
        (x1 - inset, y0 + round),
        (x1, y1 - round),
        (x1 - round, y1),
        (x0 + round, y1),
        (x0, y1 - round),
        (x0 + inset, y0 + round),
    ];
    c.fill_polygon_fx(&pts, body);
    // A highlight on the top face.
    let hl = [
        (x0 + inset + round, y0 + FX),
        (x1 - inset - round, y0 + FX),
        (x1 - inset * 3 / 2, (y0 + y1) / 2),
        (x0 + inset * 3 / 2, (y0 + y1) / 2),
    ];
    c.fill_polygon_fx(&hl, fade(top, 120));
    // Activity light on the front.
    let lx = r.right() - r.w / 9;
    let ly = r.y + r.h * 72 / 100;
    c.fill_circle(lx, ly, (r.h / 22).max(2), lerp(body, 0xffffffff, 120));
}

/// Gradient colours of the theme previews (dark, light, Velora Gamers).
pub fn theme_colors(i: usize) -> (Color, Color, Color) {
    match i {
        1 => (rgb(236, 236, 240), rgb(160, 160, 168), rgb(250, 250, 252)),
        2 => (
            rgb(0x15, 0x13, 0x1f),
            rgb(0xa8, 0x55, 0xf7),
            rgb(0xf4, 0xf2, 0xff),
        ),
        _ => (rgb(26, 26, 30), rgb(70, 70, 76), rgb(230, 230, 234)),
    }
}

/// A theme or wallpaper preview card: a diagonal gradient with a light
/// sweep and the picture icon.
pub fn preview(c: &mut Canvas, r: Rect, radius: i32, colors: (Color, Color, Color)) {
    let (a, b, mark) = colors;
    c.fill_rrect(r, radius, a);
    let old = c.set_clip(r.inset(1));
    // A light sweep across the card, cut to its shape.
    let sweep = fade(b, 200);
    c.fill_circle_inside(
        (r.x as i64 + r.w as i64 * 13 / 10) * FX,
        (r.y as i64 - r.h as i64 / 2) * FX,
        r.w as i64 * FX,
        r.w as i64 * FX / 3,
        r,
        radius,
        sweep,
    );
    c.fill_circle_inside(
        (r.x as i64 - r.w as i64 / 4) * FX,
        (r.bottom() as i64 + r.h as i64) * FX,
        r.w as i64 * FX * 9 / 10,
        r.w as i64 * FX / 4,
        r,
        radius,
        fade(b, 90),
    );
    c.set_clip(old);
    let s = r.h * 2 / 5;
    icons::draw(
        c,
        Icon::Picture,
        r.center_x() - s / 2,
        r.center_y() - s / 2,
        s,
        mark,
        0,
    );
}

/// The wallpaper previews (waves, graphite, aurora, ember).
pub fn wallpaper_colors(i: usize) -> (Color, Color, Color) {
    match i {
        1 => (rgb(44, 44, 48), rgb(90, 90, 96), rgb(220, 220, 226)),
        2 => (rgb(14, 40, 52), rgb(40, 150, 140), rgb(220, 250, 240)),
        3 => (rgb(24, 18, 16), rgb(150, 86, 40), rgb(255, 200, 140)),
        _ => (rgb(12, 12, 14), rgb(80, 80, 88), rgb(236, 236, 240)),
    }
}

/// A step mark of the sidebar: done (an accent check), the current step
/// (an accent disc with a check and a soft glow) or ahead (a dim disc).
pub fn step_mark(c: &mut Canvas, cx: i32, cy: i32, d: i32, state: StepState, t: &Theme) {
    let r = d as i64 * FX / 2;
    let (fx, fy) = (cx as i64 * FX, cy as i64 * FX);
    let check = |c: &mut Canvas, color: Color| {
        let s = d * 7 / 10;
        icons::draw(
            c,
            Icon::Check,
            cx - s / 2,
            cy - s / 2,
            s,
            color,
            (d as i64 * FX / 9).max(FX),
        );
    };
    match state {
        StepState::Current => {
            c.radial_glow(cx, cy, d * 3 / 2, fade(t.accent, 70));
            c.fill_circle_fx(fx, fy, r, t.accent);
            check(c, t.on_accent);
        }
        StepState::Done => {
            c.fill_circle_fx(fx, fy, r, fade(t.accent, 46));
            check(c, t.accent);
        }
        StepState::Ahead => {
            c.fill_circle_fx(fx, fy, r, fade(t.text_faint, 90));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Done,
    Current,
    Ahead,
}

/// The light/dark switch at the bottom of the sidebar: a sun disc and a
/// moon on a track; a pill slides under the active one (`dark` 0 for the
/// sun ... 1000 for the moon, animated). `hover` 0..=255.
pub fn theme_switch(c: &mut Canvas, r: Rect, dark: i32, hover: u8, t: &Theme) {
    let track = lerp(t.field, t.field_hover, hover);
    c.fill_rrect(r, r.h / 2, track);
    let d = r.h - r.h / 4;
    let pad = (r.h - d) / 2;
    let sun = Rect::new(r.x + pad, r.y + pad, d, d);
    let moon = Rect::new(r.right() - pad - d, r.y + pad, d, d);
    let px = sun.x + (moon.x - sun.x) * dark.clamp(0, 1000) / 1000;
    let pill = Rect::new(px, sun.y, d, d);
    c.fill_rrect(pill, d / 2, if t.dark { t.selected } else { t.knob });
    // The sun as a plain light disc, the moon as a crescent.
    let on = (dark.clamp(0, 1000) * 255 / 1000) as u8;
    c.fill_circle(
        sun.center_x(),
        sun.center_y(),
        d * 34 / 100,
        lerp(t.accent, t.text_dim, on),
    );
    let ms = d * 8 / 10;
    icons::draw(
        c,
        Icon::Moon,
        moon.center_x() - ms / 2,
        moon.center_y() - ms / 2,
        ms,
        lerp(t.text_dim, t.accent, on),
        0,
    );
}

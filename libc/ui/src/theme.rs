//! Colours of the themes: dark (graphite), light and Velora Gamers, each
//! with an accent colour for switches that are on, selections, the
//! keyboard focus, progress and the step marks, and a detail colour for
//! small marks (the text cursor, the spinner of a running step).

use crate::canvas::{Color, lerp, rgb, rgba};

/// The default accent: warm amber, like the light of a street lamp.
pub const AMBER: Color = rgb(0xff, 0xb5, 0x47);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub dark: bool,
    /// Behind the window.
    pub backdrop: Color,
    pub backdrop_glow: Color,
    pub window: Color,
    pub window_border: Color,
    pub shadow: Color,
    pub sidebar: Color,
    /// Soft wave shapes on the window background.
    pub wave: Color,
    pub text: Color,
    pub text_dim: Color,
    pub text_faint: Color,
    /// Input fields, lists and secondary buttons.
    pub field: Color,
    pub field_hover: Color,
    pub field_pressed: Color,
    pub field_border: Color,
    /// A neutral highlight (the hovered row of a list).
    pub selected: Color,
    pub scrollbar: Color,
    /// The main button (a light pill on the dark theme).
    pub primary: Color,
    pub primary_hover: Color,
    pub primary_pressed: Color,
    pub on_primary: Color,
    pub track_off: Color,
    pub knob: Color,
    pub knob_off: Color,
    pub progress_track: Color,
    pub error: Color,
    pub warning: Color,
    /// The accent and the colour of marks drawn on it.
    pub accent: Color,
    pub on_accent: Color,
    /// Small details: the text cursor, the spinner of a running step.
    /// It follows the accent unless the theme has its own.
    pub detail: Color,
}

impl Theme {
    /// The theme with another accent colour (opaque `0xRRGGBB` or
    /// `0xAARRGGBB`). Marks on it are dark on light accents.
    pub fn with_accent(mut self, accent: Color) -> Theme {
        let a = 0xff00_0000 | accent;
        if self.detail == self.accent {
            self.detail = a;
        }
        self.accent = a;
        self.on_accent = if luminance(a) > 140 {
            rgb(24, 22, 20)
        } else {
            rgb(255, 255, 255)
        };
        self
    }

    /// The theme's own neutral accent (used when the accent is off).
    pub fn neutral(self) -> Theme {
        let n = if self.dark {
            rgb(214, 213, 218)
        } else {
            rgb(44, 44, 50)
        };
        self.with_accent(n)
    }

    /// The colour of a highlight with the accent: the accent mixed into
    /// `base` by `amount` / 255.
    pub fn tint(&self, base: Color, amount: u8) -> Color {
        lerp(base, self.accent, amount)
    }

    /// The keyboard focus ring.
    pub fn focus(&self) -> Color {
        (self.accent & 0x00ff_ffff) | 0xe6 << 24
    }

    /// Every colour mixed between `a` and `b` (`t` 0..=255); `dark` flips
    /// in the middle.
    pub fn mix(a: &Theme, b: &Theme, t: u8) -> Theme {
        let m = |x: Color, y: Color| lerp(x, y, t);
        Theme {
            dark: if t < 128 { a.dark } else { b.dark },
            backdrop: m(a.backdrop, b.backdrop),
            backdrop_glow: m(a.backdrop_glow, b.backdrop_glow),
            window: m(a.window, b.window),
            window_border: m(a.window_border, b.window_border),
            shadow: m(a.shadow, b.shadow),
            sidebar: m(a.sidebar, b.sidebar),
            wave: m(a.wave, b.wave),
            text: m(a.text, b.text),
            text_dim: m(a.text_dim, b.text_dim),
            text_faint: m(a.text_faint, b.text_faint),
            field: m(a.field, b.field),
            field_hover: m(a.field_hover, b.field_hover),
            field_pressed: m(a.field_pressed, b.field_pressed),
            field_border: m(a.field_border, b.field_border),
            selected: m(a.selected, b.selected),
            scrollbar: m(a.scrollbar, b.scrollbar),
            primary: m(a.primary, b.primary),
            primary_hover: m(a.primary_hover, b.primary_hover),
            primary_pressed: m(a.primary_pressed, b.primary_pressed),
            on_primary: m(a.on_primary, b.on_primary),
            track_off: m(a.track_off, b.track_off),
            knob: m(a.knob, b.knob),
            knob_off: m(a.knob_off, b.knob_off),
            progress_track: m(a.progress_track, b.progress_track),
            error: m(a.error, b.error),
            warning: m(a.warning, b.warning),
            accent: m(a.accent, b.accent),
            on_accent: m(a.on_accent, b.on_accent),
            detail: m(a.detail, b.detail),
        }
    }
}

/// Perceived brightness 0..=255 of a colour.
pub fn luminance(c: Color) -> u32 {
    let r = (c >> 16) & 0xff;
    let g = (c >> 8) & 0xff;
    let b = c & 0xff;
    (r * 299 + g * 587 + b * 114) / 1000
}

pub const DARK: Theme = Theme {
    dark: true,
    backdrop: rgb(20, 20, 22),
    backdrop_glow: rgba(255, 255, 255, 10),
    window: rgb(36, 36, 39),
    window_border: rgba(255, 255, 255, 34),
    shadow: rgba(0, 0, 0, 150),
    sidebar: rgb(31, 31, 34),
    wave: rgba(255, 255, 255, 10),
    text: rgb(238, 238, 240),
    text_dim: rgb(196, 196, 202),
    text_faint: rgb(128, 127, 134),
    field: rgb(45, 45, 49),
    field_hover: rgb(54, 54, 59),
    field_pressed: rgb(40, 40, 44),
    field_border: rgba(255, 255, 255, 26),
    selected: rgb(66, 65, 71),
    scrollbar: rgb(119, 120, 125),
    primary: rgb(232, 232, 235),
    primary_hover: rgb(248, 248, 250),
    primary_pressed: rgb(206, 206, 210),
    on_primary: rgb(26, 26, 30),
    track_off: rgb(70, 69, 75),
    knob: rgb(255, 255, 255),
    knob_off: rgb(168, 167, 173),
    progress_track: rgb(62, 61, 66),
    error: rgb(255, 138, 128),
    warning: rgb(255, 181, 71),
    accent: AMBER,
    on_accent: rgb(24, 22, 20),
    detail: AMBER,
};

pub const LIGHT: Theme = Theme {
    dark: false,
    backdrop: rgb(206, 207, 212),
    backdrop_glow: rgba(255, 255, 255, 60),
    window: rgb(240, 240, 243),
    window_border: rgba(0, 0, 0, 30),
    shadow: rgba(40, 40, 60, 70),
    sidebar: rgb(228, 228, 233),
    wave: rgba(0, 0, 0, 9),
    text: rgb(26, 26, 30),
    text_dim: rgb(70, 70, 78),
    text_faint: rgb(132, 132, 140),
    field: rgb(252, 252, 253),
    field_hover: rgb(244, 244, 247),
    field_pressed: rgb(232, 232, 237),
    field_border: rgba(0, 0, 0, 34),
    selected: rgb(222, 222, 228),
    scrollbar: rgb(170, 170, 178),
    primary: rgb(34, 34, 38),
    primary_hover: rgb(56, 56, 62),
    primary_pressed: rgb(20, 20, 24),
    on_primary: rgb(244, 244, 246),
    track_off: rgb(204, 204, 210),
    knob: rgb(255, 255, 255),
    knob_off: rgb(255, 255, 255),
    progress_track: rgb(214, 214, 220),
    error: rgb(196, 48, 40),
    warning: rgb(176, 98, 0),
    accent: AMBER,
    on_accent: rgb(24, 22, 20),
    detail: AMBER,
};

/// Velora Gamers: near-black with a violet tint, a neon violet accent
/// and a cold cyan for small details.
pub const GAMERS: Theme = Theme {
    dark: true,
    backdrop: rgb(0x0b, 0x0a, 0x12),
    backdrop_glow: rgba(0xa8, 0x55, 0xf7, 12),
    window: rgb(0x15, 0x13, 0x1f),
    window_border: rgb(0x2a, 0x25, 0x40),
    shadow: rgba(0, 0, 0, 170),
    // Between the backdrop and the panels.
    sidebar: rgb(0x10, 0x0e, 0x18),
    wave: rgba(0xa8, 0x55, 0xf7, 8),
    text: rgb(0xf4, 0xf2, 0xff),
    text_dim: rgb(0xa3, 0x9d, 0xbd),
    text_faint: rgb(0x8f, 0x89, 0xab),
    field: rgb(0x1e, 0x1b, 0x2c),
    field_hover: rgb(0x26, 0x22, 0x38),
    field_pressed: rgb(0x1a, 0x17, 0x27),
    field_border: rgb(0x2a, 0x25, 0x40),
    selected: rgb(0x2c, 0x27, 0x45),
    scrollbar: rgb(0x4e, 0x47, 0x70),
    primary: rgb(0xf4, 0xf2, 0xff),
    primary_hover: rgb(0xff, 0xff, 0xff),
    primary_pressed: rgb(0xd8, 0xd3, 0xee),
    on_primary: rgb(0x15, 0x13, 0x1f),
    track_off: rgb(0x33, 0x2d, 0x4d),
    knob: rgb(0xff, 0xff, 0xff),
    knob_off: rgb(0xa3, 0x9d, 0xbd),
    progress_track: rgb(0x2a, 0x25, 0x40),
    error: rgb(0xff, 0x6b, 0x81),
    warning: rgb(0xff, 0xb5, 0x47),
    accent: NEON_VIOLET,
    on_accent: rgb(255, 255, 255),
    detail: CYAN,
};

/// The accent of Velora Gamers.
pub const NEON_VIOLET: Color = rgb(0xa8, 0x55, 0xf7);
/// The detail colour of Velora Gamers.
pub const CYAN: Color = rgb(0x38, 0xe1, 0xff);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_and_mixing() {
        let t = DARK.with_accent(0x3f7ee8);
        assert_eq!(t.accent, 0xff3f7ee8);
        assert_eq!(t.on_accent, rgb(255, 255, 255));
        assert_eq!(DARK.on_accent, rgb(24, 22, 20));
        let m = Theme::mix(&DARK, &LIGHT, 0);
        assert_eq!(m, DARK);
        assert_eq!(Theme::mix(&DARK, &LIGHT, 255), LIGHT);
        let half = Theme::mix(&DARK, &LIGHT, 128);
        assert!(!half.dark);
        assert!(luminance(half.window) > luminance(DARK.window));
        assert!(luminance(half.window) < luminance(LIGHT.window));
        assert_ne!(DARK.neutral().accent, AMBER);
        // The detail colour follows the accent, unless the theme has its own.
        assert_eq!(DARK.with_accent(0x3f7ee8).detail, 0xff3f7ee8);
        assert_eq!(GAMERS.with_accent(0x3f7ee8).detail, CYAN);
    }

    /// WCAG contrast ratio of two opaque colours.
    fn contrast(a: Color, b: Color) -> f64 {
        let lum = |c: Color| {
            let ch = |v: u32| {
                let v = v as f64 / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch((c >> 16) & 0xff) + 0.7152 * ch((c >> 8) & 0xff) + 0.0722 * ch(c & 0xff)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn gamers_palette_and_text_contrast() {
        let t = GAMERS;
        assert_eq!(t.backdrop, 0xff0b0a12);
        assert_eq!(t.window, 0xff15131f);
        assert_eq!(t.window_border, 0xff2a2540);
        assert_eq!(t.text, 0xfff4f2ff);
        assert_eq!(t.text_dim, 0xffa39dbd);
        assert_eq!(t.accent, 0xffa855f7);
        assert_eq!(t.detail, 0xff38e1ff);
        // Every text colour on every surface text is drawn on: 4.5:1 or more.
        for (name, fg) in [
            ("text", t.text),
            ("text_dim", t.text_dim),
            ("text_faint", t.text_faint),
            ("error", t.error),
            ("warning", t.warning),
        ] {
            for (bg_name, bg) in [
                ("window", t.window),
                ("sidebar", t.sidebar),
                ("field", t.field),
                ("field_hover", t.field_hover),
                ("backdrop", t.backdrop),
            ] {
                let r = contrast(fg, bg);
                assert!(r >= 4.5, "{} on {}: {:.2}", name, bg_name, r);
            }
        }
        assert!(contrast(t.text, t.selected) >= 4.5);
        assert!(contrast(t.text_dim, t.selected) >= 4.5);
        assert!(contrast(t.on_primary, t.primary) >= 4.5);
        assert!(contrast(t.on_primary, t.primary_hover) >= 4.5);
    }
}

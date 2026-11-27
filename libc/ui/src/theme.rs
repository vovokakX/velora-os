//! Colours of the themes: dark (graphite), light and midnight, each with
//! an accent colour for switches that are on, selections, the keyboard
//! focus, progress and the step marks.

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
}

impl Theme {
    /// The theme with another accent colour (opaque `0xRRGGBB` or
    /// `0xAARRGGBB`). Marks on it are dark on light accents.
    pub fn with_accent(mut self, accent: Color) -> Theme {
        let a = 0xff00_0000 | accent;
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
};

/// Dark blue-violet.
pub const MIDNIGHT: Theme = Theme {
    dark: true,
    backdrop: rgb(13, 14, 28),
    backdrop_glow: rgba(140, 150, 255, 14),
    window: rgb(26, 28, 48),
    window_border: rgba(170, 180, 255, 40),
    shadow: rgba(0, 0, 10, 160),
    sidebar: rgb(22, 24, 42),
    wave: rgba(150, 160, 255, 12),
    text: rgb(236, 238, 250),
    text_dim: rgb(190, 194, 220),
    text_faint: rgb(122, 126, 160),
    field: rgb(36, 39, 64),
    field_hover: rgb(45, 48, 76),
    field_pressed: rgb(31, 34, 56),
    field_border: rgba(170, 180, 255, 30),
    selected: rgb(56, 60, 94),
    scrollbar: rgb(112, 118, 160),
    primary: rgb(232, 234, 248),
    primary_hover: rgb(248, 249, 255),
    primary_pressed: rgb(204, 208, 230),
    on_primary: rgb(22, 24, 42),
    track_off: rgb(62, 66, 100),
    knob: rgb(255, 255, 255),
    knob_off: rgb(160, 164, 196),
    progress_track: rgb(52, 56, 90),
    error: rgb(255, 138, 140),
    warning: rgb(255, 181, 71),
    accent: AMBER,
    on_accent: rgb(24, 22, 20),
};

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
    }
}

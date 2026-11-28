//! The desktop settings: `/etc/velora/desktop.conf` (written by the
//! installer) and the user's own file, `~/.config/lumi/settings.conf`,
//! which Lumi writes when the settings change (an older
//! `~/.config/velora/desktop.conf` is read if it is not there yet).
//! Unknown keys and comments are kept when the file is written again;
//! the installer's yes/no keys are read and written in the new form.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use velora_ui::canvas::Color;
use velora_ui::theme::{self, Theme};

pub const SYSTEM_FILE: &str = "/etc/velora/desktop.conf";
pub const LOCALE_FILE: &str = "/etc/velora/locale.conf";
pub const KEYBOARD_FILE: &str = "/etc/velora/keyboard.conf";
/// The user's settings directory and file, under the home directory.
pub const USER_DIR: &str = ".config/lumi";
pub const USER_FILE: &str = ".config/lumi/settings.conf";
/// Where Lumi kept them before.
pub const OLD_USER_FILE: &str = ".config/velora/desktop.conf";

/// Theme ids, as the installer writes them.
pub const THEMES: [&str; 3] = ["velora-dark", "velora-light", "velora-gamers"];
/// Wallpaper ids drawn by Lumi; pictures are `file:<path>`.
pub const BUILTIN_WALLPAPERS: [&str; 4] = ["waves", "graphite", "aurora", "ember"];
pub const POINTERS: [&str; 3] = ["classic", "blade", "drop"];
/// Pointer sizes: per cent of the normal size.
pub const POINTER_SIZES: [u32; 4] = [100, 125, 150, 200];
/// Pointer speeds (relative mice): per cent.
pub const POINTER_SPEEDS: [u32; 5] = [50, 75, 100, 140, 200];
/// Animation speeds: off, slow, normal, fast (durations in per cent).
pub const ANIMATIONS: [&str; 4] = ["off", "slow", "normal", "fast"];
const ANIM_PERCENT: [u32; 4] = [0, 160, 100, 60];
/// Panel modes.
pub const PANEL_MODES: [&str; 3] = ["always", "autohide", "smart"];
pub const PANEL_ALWAYS: usize = 0;
pub const PANEL_AUTOHIDE: usize = 1;
pub const PANEL_SMART: usize = 2;
/// Panel sizes: the item size in design units.
pub const PANEL_SIZES: [&str; 3] = ["small", "medium", "large"];
pub const PANEL_ITEM: [i32; 3] = [36, 42, 50];
/// Interface scales: 0 is automatic (from the screen height), else per cent.
pub const UI_SCALES: [u32; 7] = [0, 100, 125, 150, 175, 200, 250];
/// Apps that can be pinned to the panel, in their ids.
pub const DEFAULT_PINNED: [&str; 4] = ["plume", "files", "terminal", "settings"];
/// Accent names and colours (the installer's list).
pub const ACCENTS: [(&str, u32); 8] = [
    ("amber", 0xffb547),
    ("blue", 0x3f7ee8),
    ("violet", 0x724ecc),
    ("magenta", 0xaf44a4),
    ("pink", 0xeb8cd0),
    ("periwinkle", 0x6f89c4),
    ("mint", 0x78bea2),
    ("neon-violet", 0xa855f7),
];

/// The accent colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accent {
    /// The theme's neutral colour.
    None,
    /// One of [`ACCENTS`].
    Preset(usize),
    /// Any colour, 0xRRGGBB.
    Custom(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Index into [`THEMES`].
    pub theme: usize,
    pub accent: Accent,
    /// A built-in id or `file:<path>`.
    pub wallpaper: String,
    /// Darkening of the desktop's wallpaper, per cent (0..=80).
    pub wallpaper_dim: u32,
    /// Blur of the desktop's wallpaper, 0..=100.
    pub wallpaper_blur: u32,
    /// Panel and menus: how much of the wallpaper shows through,
    /// per cent (0 = opaque).
    pub transparency: u32,
    /// Window corner radius in design units (0 = square).
    pub radius: u32,
    /// Index into [`ANIMATIONS`].
    pub animations: usize,
    /// Index into [`UI_SCALES`].
    pub ui_scale: usize,
    /// Index into [`PANEL_MODES`].
    pub panel_mode: usize,
    /// Index into [`PANEL_SIZES`].
    pub panel_size: usize,
    /// Ids of the apps pinned to the panel, in order.
    pub pinned: Vec<String>,
    pub clock_24h: bool,
    pub show_date: bool,
    /// Windows snap to the screen's edges when dragged there.
    pub snap: bool,
    /// Title bar buttons on the left (close first) instead of the right.
    pub buttons_left: bool,
    /// Index into [`POINTERS`].
    pub pointer: usize,
    /// Index into [`POINTER_SIZES`].
    pub pointer_size: usize,
    /// Index into [`POINTER_SPEEDS`].
    pub pointer_speed: usize,
    /// The content moves with the fingers (wheel direction reversed).
    pub natural_scroll: bool,
    /// A region id (`UA`, `DE`, ...) for the time zone.
    pub region: String,
    /// `ru` or `en` (from the system language).
    pub language: String,
    /// `lumi` or `console` (the console login instead of Lumi).
    pub session: String,
    /// The lines of the file it was read from (to keep other keys).
    lines: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: 0,
            accent: Accent::Preset(0),
            wallpaper: String::from("waves"),
            wallpaper_dim: 0,
            wallpaper_blur: 0,
            transparency: 50,
            radius: 12,
            animations: 2,
            ui_scale: 0,
            panel_mode: PANEL_SMART,
            panel_size: 1,
            pinned: DEFAULT_PINNED.iter().map(|s| s.to_string()).collect(),
            clock_24h: true,
            show_date: false,
            snap: true,
            buttons_left: false,
            pointer: 0,
            pointer_size: 0,
            pointer_speed: 2,
            natural_scroll: false,
            region: String::from("UA"),
            language: String::from("ru"),
            session: String::from("lumi"),
            lines: Vec::new(),
        }
    }
}

fn yes(v: &str) -> Option<bool> {
    match v {
        "yes" | "true" | "1" | "on" => Some(true),
        "no" | "false" | "0" | "off" => Some(false),
        _ => None,
    }
}

fn pairs(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter_map(|l| {
        let l = l.trim();
        if l.starts_with('#') {
            return None;
        }
        let (k, v) = l.split_once('=')?;
        Some((k.trim(), v.trim()))
    })
}

fn index_of(list: &[&str], v: &str) -> Option<usize> {
    list.iter().position(|x| *x == v)
}

fn number(v: &str, max: u32) -> Option<u32> {
    v.trim_end_matches('%')
        .parse::<u32>()
        .ok()
        .map(|n| n.min(max))
}

/// `#rrggbb` (or `rrggbb`) as 0xRRGGBB.
pub fn parse_color(v: &str) -> Option<u32> {
    let h = v.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    u32::from_str_radix(h, 16).ok()
}

/// Keys written by older versions that the new ones replace.
const OLD_KEYS: [&str; 2] = ["large_pointer", "rounded_corners"];

impl Config {
    /// Applies the keys of a settings file on top of these settings.
    pub fn apply(&mut self, text: &str) {
        self.lines = text.lines().map(String::from).collect();
        let mut accent_color = None;
        let mut custom = false;
        for (k, v) in pairs(text) {
            match k {
                "theme" => self.theme = index_of(&THEMES, v).unwrap_or(self.theme),
                "wallpaper" if !v.is_empty() => self.wallpaper = String::from(v),
                "wallpaper_dim" => self.wallpaper_dim = number(v, 80).unwrap_or(self.wallpaper_dim),
                "wallpaper_blur" => {
                    self.wallpaper_blur = number(v, 100).unwrap_or(self.wallpaper_blur)
                }
                "pointer" => self.pointer = index_of(&POINTERS, v).unwrap_or(self.pointer),
                "pointer_size" => {
                    if let Some(n) = number(v, 400) {
                        self.pointer_size =
                            POINTER_SIZES.iter().rposition(|&s| s <= n).unwrap_or(0);
                    }
                }
                "large_pointer" => {
                    if let Some(b) = yes(v) {
                        self.pointer_size = if b { 2 } else { 0 };
                    }
                }
                "pointer_speed" => {
                    if let Some(n) = number(v, 1000) {
                        self.pointer_speed =
                            POINTER_SPEEDS.iter().rposition(|&s| s <= n).unwrap_or(0);
                    }
                }
                "natural_scroll" => self.natural_scroll = yes(v).unwrap_or(self.natural_scroll),
                "transparency" => {
                    self.transparency = match yes(v) {
                        Some(true) => 50,
                        Some(false) => 0,
                        None => number(v, 100).unwrap_or(self.transparency),
                    }
                }
                "rounded_corners" => {
                    if let Some(b) = yes(v) {
                        self.radius = if b { 12 } else { 0 };
                    }
                }
                "corner_radius" => self.radius = number(v, 24).unwrap_or(self.radius),
                "animations" => {
                    self.animations = match yes(v) {
                        Some(true) => 2,
                        Some(false) => 0,
                        None => index_of(&ANIMATIONS, v).unwrap_or(self.animations),
                    }
                }
                "ui_scale" => {
                    self.ui_scale = if v == "auto" {
                        0
                    } else {
                        number(v, 400)
                            .and_then(|n| UI_SCALES.iter().position(|&s| s == n))
                            .unwrap_or(self.ui_scale)
                    }
                }
                "panel" => self.panel_mode = index_of(&PANEL_MODES, v).unwrap_or(self.panel_mode),
                "panel_size" => {
                    self.panel_size = index_of(&PANEL_SIZES, v).unwrap_or(self.panel_size)
                }
                "pinned" => {
                    self.pinned = v
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect()
                }
                "clock" => self.clock_24h = v != "12h",
                "show_date" => self.show_date = yes(v).unwrap_or(self.show_date),
                "snap" => self.snap = yes(v).unwrap_or(self.snap),
                "title_buttons" => self.buttons_left = v == "left",
                "accent" => {
                    custom = v == "custom";
                    if v == "none" {
                        self.accent = Accent::None;
                    } else if let Some(i) = ACCENTS.iter().position(|a| a.0 == v) {
                        self.accent = Accent::Preset(i);
                    }
                }
                "accent_color" => accent_color = parse_color(v),
                "region" if !v.is_empty() => self.region = String::from(v),
                "session" if !v.is_empty() => self.session = String::from(v),
                _ => {}
            }
        }
        if custom && let Some(c) = accent_color {
            self.accent = Accent::Custom(c);
        }
    }

    /// Takes the region and language from locale.conf.
    pub fn apply_locale(&mut self, text: &str) {
        for (k, v) in pairs(text) {
            match k {
                "region" if !v.is_empty() => self.region = String::from(v),
                "language" => {
                    self.language = String::from(if v.starts_with("ru") || v.starts_with("uk") {
                        "ru"
                    } else {
                        "en"
                    })
                }
                _ => {}
            }
        }
    }

    /// The accent colour, 0xRRGGBB, if there is one.
    pub fn accent_rgb(&self) -> Option<u32> {
        match self.accent {
            Accent::None => None,
            Accent::Preset(i) => Some(ACCENTS[i.min(ACCENTS.len() - 1)].1),
            Accent::Custom(c) => Some(c & 0xff_ffff),
        }
    }

    /// The text of the settings file: known keys are replaced in place,
    /// other lines kept, old keys dropped.
    pub fn to_text(&self) -> String {
        let accent_name = match self.accent {
            Accent::None => String::from("none"),
            Accent::Preset(i) => ACCENTS[i.min(ACCENTS.len() - 1)].0.to_string(),
            Accent::Custom(_) => String::from("custom"),
        };
        let scale = match UI_SCALES[self.ui_scale.min(UI_SCALES.len() - 1)] {
            0 => String::from("auto"),
            n => n.to_string(),
        };
        let values: Vec<(&str, String)> = alloc::vec![
            ("theme", THEMES[self.theme.min(2)].to_string()),
            ("accent", accent_name),
            (
                "accent_color",
                format!("#{:06x}", self.accent_rgb().unwrap_or(ACCENTS[0].1)),
            ),
            ("wallpaper", self.wallpaper.clone()),
            ("wallpaper_dim", self.wallpaper_dim.to_string()),
            ("wallpaper_blur", self.wallpaper_blur.to_string()),
            ("transparency", self.transparency.to_string()),
            ("corner_radius", self.radius.to_string()),
            ("animations", ANIMATIONS[self.animations.min(3)].to_string()),
            ("ui_scale", scale),
            ("panel", PANEL_MODES[self.panel_mode.min(2)].to_string()),
            (
                "panel_size",
                PANEL_SIZES[self.panel_size.min(2)].to_string()
            ),
            ("pinned", self.pinned.join(",")),
            (
                "clock",
                String::from(if self.clock_24h { "24h" } else { "12h" })
            ),
            ("show_date", yn(self.show_date)),
            ("snap", yn(self.snap)),
            (
                "title_buttons",
                String::from(if self.buttons_left { "left" } else { "right" })
            ),
            ("pointer", POINTERS[self.pointer.min(2)].to_string()),
            (
                "pointer_size",
                POINTER_SIZES[self.pointer_size.min(3)].to_string()
            ),
            (
                "pointer_speed",
                POINTER_SPEEDS[self.pointer_speed.min(4)].to_string()
            ),
            ("natural_scroll", yn(self.natural_scroll)),
            ("region", self.region.clone()),
        ];
        let mut done = alloc::vec![false; values.len()];
        let mut out = String::new();
        if self.lines.is_empty() {
            out.push_str("# Settings of Lumi, the Velora OS desktop.\n");
        }
        for l in &self.lines {
            let comment = l.trim_start().starts_with('#');
            let key = l.split_once('=').map(|(k, _)| k.trim());
            if !comment && key.is_some_and(|k| OLD_KEYS.contains(&k)) {
                continue;
            }
            match key.and_then(|k| values.iter().position(|(n, _)| *n == k)) {
                Some(i) if !comment => {
                    if !done[i] {
                        out.push_str(&format!("{}={}\n", values[i].0, values[i].1));
                        done[i] = true;
                    }
                }
                _ => {
                    // The installer's note that Lumi does not exist yet.
                    if !l.contains("not part of the system yet") {
                        out.push_str(l);
                        out.push('\n');
                    }
                }
            }
        }
        for (i, (k, v)) in values.iter().enumerate() {
            if !done[i] {
                out.push_str(&format!("{}={}\n", k, v));
            }
        }
        out
    }

    /// The theme with its accent.
    pub fn theme(&self) -> Theme {
        let base = match self.theme {
            1 => theme::LIGHT,
            2 => theme::GAMERS,
            _ => theme::DARK,
        };
        match self.accent_rgb() {
            Some(c) => base.with_accent(c as Color),
            None => base.neutral(),
        }
    }

    pub fn ru(&self) -> bool {
        self.language == "ru"
    }

    pub fn animations_on(&self) -> bool {
        self.animations != 0
    }

    /// An animation's duration at the chosen speed (0 when off).
    pub fn dur(&self, ms: u32) -> u32 {
        ms * ANIM_PERCENT[self.animations.min(3)] / 100
    }

    /// A corner radius of an element whose radius is `base` with the
    /// normal setting (12): scaled with the chosen radius; square
    /// corners keep a hint of rounding.
    pub fn round(&self, base: i32) -> i32 {
        if self.radius == 0 {
            base / 4
        } else {
            (base * self.radius as i32 + 6) / 12
        }
    }

    /// Pixels per design unit times 256 for a screen of height `h`.
    pub fn scale_for(&self, h: i32) -> i64 {
        match UI_SCALES[self.ui_scale.min(UI_SCALES.len() - 1)] {
            0 => auto_scale(h),
            n => n as i64 * 256 / 100,
        }
    }

    /// Opacity (0..=255) of the panel and menus over the frosted glass.
    pub fn panel_alpha(&self) -> u32 {
        255 - self.transparency.min(100) * 21 / 10
    }
}

/// The installer's steps: about 1x at 720 lines, 1.25x at 1080, 2.5x at
/// 2160.
pub fn auto_scale(h: i32) -> i64 {
    const STEPS: [i64; 9] = [256, 320, 384, 448, 512, 640, 768, 896, 1024];
    let fit = h as i64 * 256 / 860;
    STEPS
        .iter()
        .rev()
        .find(|&&s| s <= fit)
        .copied()
        .unwrap_or(256)
}

fn yn(b: bool) -> String {
    String::from(if b { "yes" } else { "no" })
}

/// The keyboard layouts and the switch keys from keyboard.conf:
/// ("ru,us", "alt+shift").
pub fn keyboard(text: &str) -> (String, String) {
    let mut layouts = String::from("us,ru");
    let mut switch = String::from("alt+shift");
    for (k, v) in pairs(text) {
        match k {
            "layouts" if !v.is_empty() => layouts = String::from(v),
            "switch" if !v.is_empty() => switch = String::from(v),
            _ => {}
        }
    }
    (layouts, switch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTALLER: &str = "# Written by the Velora OS installer.\n\
        # For the graphical shell (Lumi), which is not part of the system yet.\n\
        theme=velora-gamers\nicons=velora\nwallpaper=aurora\npointer=drop\n\
        large_pointer=yes\ntransparency=no\nrounded_corners=yes\nanimations=no\n\
        accent=mint\naccent_color=#78bea2\n";

    #[test]
    fn reads_the_installer_file_and_writes_it_back() {
        let mut c = Config::default();
        c.apply(INSTALLER);
        c.apply_locale("language=en_US\nregion=DE\n");
        assert_eq!(c.theme, 2);
        assert_eq!(c.wallpaper, "aurora");
        assert_eq!(c.pointer, 2);
        assert_eq!(c.pointer_size, 2);
        assert_eq!((c.transparency, c.radius, c.animations), (0, 12, 0));
        assert_eq!(c.accent, Accent::Preset(6));
        assert_eq!((c.region.as_str(), c.ru()), ("DE", false));
        assert_eq!(c.theme().accent & 0xffffff, 0x78bea2);
        c.theme = 1;
        c.accent = Accent::None;
        c.wallpaper = String::from("file:/usr/share/wallpapers/sea.png");
        let text = c.to_text();
        assert!(text.contains("theme=velora-light\n"));
        assert!(text.contains("accent=none\n"));
        assert!(text.contains("wallpaper=file:/usr/share/wallpapers/sea.png\n"));
        assert!(text.contains("icons=velora\n"), "unknown keys are kept");
        assert!(text.contains("animations=off\n") && text.contains("transparency=0\n"));
        assert!(!text.contains("large_pointer") && !text.contains("rounded_corners"));
        assert!(!text.contains("not part of the system"));
        let mut again = Config::default();
        again.apply(&text);
        assert_eq!(again.theme, 1);
        assert_eq!(again.accent, Accent::None);
        assert_eq!(again.wallpaper, c.wallpaper);
        assert_eq!(again.pointer_size, 2);
    }

    #[test]
    fn new_settings_survive_a_round_trip() {
        let mut c = Config {
            accent: Accent::Custom(0x12ab34),
            wallpaper_dim: 30,
            wallpaper_blur: 40,
            transparency: 70,
            radius: 6,
            animations: 3,
            ui_scale: 3,
            panel_mode: PANEL_AUTOHIDE,
            panel_size: 2,
            pinned: alloc::vec![String::from("terminal"), String::from("files")],
            clock_24h: false,
            show_date: true,
            snap: false,
            buttons_left: true,
            pointer_size: 3,
            pointer_speed: 4,
            natural_scroll: true,
            ..Config::default()
        };
        let text = c.to_text();
        let mut again = Config::default();
        again.apply(&text);
        c.lines = again.lines.clone();
        assert_eq!(again, c, "{}", text);
        assert_eq!(again.theme().accent & 0xffffff, 0x12ab34);
        assert_eq!(again.scale_for(1080), 150 * 256 / 100);
        assert_eq!(Config::default().scale_for(1080), 320);
        assert_eq!(c.dur(100), 60);
        assert_eq!(Config::default().panel_mode, PANEL_SMART);
    }

    #[test]
    fn keyboard_conf() {
        let (l, s) = keyboard("# x\nlayouts=ru,us\nswitch=alt+shift\n");
        assert_eq!((l.as_str(), s.as_str()), ("ru,us", "alt+shift"));
    }
}

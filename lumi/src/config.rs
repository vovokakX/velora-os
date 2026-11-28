//! The desktop settings: `/etc/velora/desktop.conf` (written by the
//! installer) and the user's own copy, `~/.config/velora/desktop.conf`,
//! which Lumi writes when the settings change. Unknown keys and comments
//! are kept when the file is written again.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use velora_ui::canvas::Color;
use velora_ui::theme::{self, Theme};

pub const SYSTEM_FILE: &str = "/etc/velora/desktop.conf";
pub const LOCALE_FILE: &str = "/etc/velora/locale.conf";
/// The user's file, under the home directory.
pub const USER_FILE: &str = ".config/velora/desktop.conf";

/// Theme ids, as the installer writes them.
pub const THEMES: [&str; 3] = ["velora-dark", "velora-light", "velora-gamers"];
/// Wallpaper ids drawn by Lumi; pictures are `file:<path>`.
pub const BUILTIN_WALLPAPERS: [&str; 4] = ["waves", "graphite", "aurora", "ember"];
pub const POINTERS: [&str; 3] = ["classic", "blade", "drop"];
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Index into [`THEMES`].
    pub theme: usize,
    /// A built-in id or `file:<path>`.
    pub wallpaper: String,
    /// Index into [`POINTERS`].
    pub pointer: usize,
    pub large_pointer: bool,
    pub transparency: bool,
    pub rounded_corners: bool,
    pub animations: bool,
    /// Index into [`ACCENTS`], or None for the theme's neutral accent.
    pub accent: Option<usize>,
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
            wallpaper: String::from("waves"),
            pointer: 0,
            large_pointer: false,
            transparency: true,
            rounded_corners: true,
            animations: true,
            accent: Some(0),
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

impl Config {
    /// Applies the keys of a desktop.conf on top of these settings.
    pub fn apply(&mut self, text: &str) {
        self.lines = text.lines().map(String::from).collect();
        for (k, v) in pairs(text) {
            match k {
                "theme" => {
                    if let Some(i) = THEMES.iter().position(|t| *t == v) {
                        self.theme = i;
                    }
                }
                "wallpaper" if !v.is_empty() => self.wallpaper = String::from(v),
                "pointer" => {
                    if let Some(i) = POINTERS.iter().position(|p| *p == v) {
                        self.pointer = i;
                    }
                }
                "large_pointer" => self.large_pointer = yes(v).unwrap_or(self.large_pointer),
                "transparency" => self.transparency = yes(v).unwrap_or(self.transparency),
                "rounded_corners" => self.rounded_corners = yes(v).unwrap_or(self.rounded_corners),
                "animations" => self.animations = yes(v).unwrap_or(self.animations),
                "accent" => {
                    self.accent = if v == "none" {
                        None
                    } else {
                        ACCENTS.iter().position(|a| a.0 == v).or(self.accent)
                    }
                }
                "region" if !v.is_empty() => self.region = String::from(v),
                "session" if !v.is_empty() => self.session = String::from(v),
                _ => {}
            }
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

    /// The text of desktop.conf with these settings: known keys are
    /// replaced in place, other lines kept.
    pub fn to_text(&self) -> String {
        let accent = self.accent.map(|i| ACCENTS[i.min(7)]);
        let values: [(&str, String); 10] = [
            ("theme", THEMES[self.theme.min(2)].to_string()),
            ("wallpaper", self.wallpaper.clone()),
            ("pointer", POINTERS[self.pointer.min(2)].to_string()),
            ("large_pointer", yn(self.large_pointer)),
            ("transparency", yn(self.transparency)),
            ("rounded_corners", yn(self.rounded_corners)),
            ("animations", yn(self.animations)),
            (
                "accent",
                accent.map_or(String::from("none"), |a| a.0.to_string()),
            ),
            (
                "accent_color",
                format!("#{:06x}", accent.map_or(ACCENTS[0].1, |a| a.1)),
            ),
            ("region", self.region.clone()),
        ];
        let mut done = [false; 10];
        let mut out = String::new();
        if self.lines.is_empty() {
            out.push_str("# Desktop settings of Lumi.\n");
        }
        for l in &self.lines {
            let key = l.split_once('=').map(|(k, _)| k.trim());
            match key.and_then(|k| values.iter().position(|(n, _)| *n == k)) {
                Some(i) if !l.trim_start().starts_with('#') => {
                    out.push_str(&format!("{}={}\n", values[i].0, values[i].1));
                    done[i] = true;
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
        match self.accent {
            Some(i) => base.with_accent(ACCENTS[i.min(7)].1 as Color),
            None => base.neutral(),
        }
    }

    pub fn ru(&self) -> bool {
        self.language == "ru"
    }
}

fn yn(b: bool) -> String {
    String::from(if b { "yes" } else { "no" })
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
        assert!(c.large_pointer && !c.transparency && !c.animations);
        assert_eq!(c.accent, Some(6));
        assert_eq!((c.region.as_str(), c.ru()), ("DE", false));
        assert_eq!(c.theme().accent & 0xffffff, 0x78bea2);
        c.theme = 1;
        c.accent = None;
        c.wallpaper = String::from("file:/usr/share/wallpapers/sea.png");
        let text = c.to_text();
        assert!(text.contains("theme=velora-light\n"));
        assert!(text.contains("accent=none\n"));
        assert!(text.contains("wallpaper=file:/usr/share/wallpapers/sea.png\n"));
        assert!(text.contains("icons=velora\n"), "unknown keys are kept");
        assert!(!text.contains("not part of the system"));
        let mut again = Config::default();
        again.apply(&text);
        assert_eq!(again.theme, 1);
        assert_eq!(again.accent, None);
        assert_eq!(again.wallpaper, c.wallpaper);
    }
}

//! Choices made in the installer and the configuration files they become.
//!
//! The appearance, pointer and locale choices are written to
//! `/etc/velora/*.conf`. The graphical shell Lumi reads desktop.conf and
//! the region in locale.conf. NOT IMPLEMENTED: keyboard.conf and the
//! system language are not read yet (the keyboard switches EN/RU with
//! Alt+Shift as before; Lumi speaks Russian or English).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A choice with a stable id (written to the files) and names in Russian
/// and English (shown by the installer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: &'static str,
    pub ru: &'static str,
    pub en: &'static str,
}

const fn c(id: &'static str, ru: &'static str, en: &'static str) -> Choice {
    Choice { id, ru, en }
}

impl Choice {
    pub fn name(&self, lang: Lang) -> &'static str {
        match lang {
            Lang::Ru => self.ru,
            Lang::En => self.en,
        }
    }
}

/// Languages the installer itself is translated into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    Ru,
    En,
}

pub const INSTALLER_LANGUAGES: [Choice; 2] =
    [c("ru", "Русский", "Русский"), c("en", "English", "English")];

/// Languages of the system (written as the user's choice; names in their
/// own language). Inter has no CJK glyphs, so no such languages yet.
pub const LANGUAGES: [Choice; 10] = [
    c("ru_RU", "Русский", "Русский"),
    c("en_US", "English", "English"),
    c("uk_UA", "Українська", "Українська"),
    c("de_DE", "Deutsch", "Deutsch"),
    c("fr_FR", "Français", "Français"),
    c("es_ES", "Español", "Español"),
    c("pl_PL", "Polski", "Polski"),
    c("it_IT", "Italiano", "Italiano"),
    c("pt_BR", "Português", "Português"),
    c("tr_TR", "Türkçe", "Türkçe"),
];

pub const REGIONS: [Choice; 12] = [
    c("UA", "Украина", "Ukraine"),
    c("RU", "Россия", "Russia"),
    c("BY", "Беларусь", "Belarus"),
    c("KZ", "Казахстан", "Kazakhstan"),
    c("PL", "Польша", "Poland"),
    c("DE", "Германия", "Germany"),
    c("US", "США", "United States"),
    c("GB", "Великобритания", "United Kingdom"),
    c("FR", "Франция", "France"),
    c("ES", "Испания", "Spain"),
    c("IT", "Италия", "Italy"),
    c("TR", "Турция", "Türkiye"),
];

/// Keyboard layouts the system has (drivers/input): US and Russian.
pub const LAYOUTS: [Choice; 3] = [
    c("ru,us", "Русская и английская", "Russian and English"),
    c("us,ru", "Английская и русская", "English and Russian"),
    c("us", "Английская (США)", "English (US)"),
];

/// Mouse pointer styles: the pictures in assets/cursors.
pub const POINTERS: [Choice; 3] = [
    c("classic", "Классический", "Classic"),
    c("blade", "Клинок", "Blade"),
    c("drop", "Капля", "Drop"),
];

pub const THEMES: [Choice; 3] = [
    c("velora-dark", "Velora Dark", "Velora Dark"),
    c("velora-light", "Velora Light", "Velora Light"),
    c("velora-gamers", "Velora Gamers", "Velora Gamers"),
];

pub const ICONS: [Choice; 2] = [
    c("velora", "Velora Icons", "Velora Icons"),
    c("velora-line", "Velora Line", "Velora Line"),
];

pub const WALLPAPERS: [Choice; 4] = [
    c("waves", "Волны", "Waves"),
    c("graphite", "Графит", "Graphite"),
    c("aurora", "Сияние", "Aurora"),
    c("ember", "Угли", "Ember"),
];

/// Accent colours (0xRRGGBB), as in the design, and the neon violet of
/// Velora Gamers.
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

/// The accent each theme starts with (an index into [`ACCENTS`]): amber
/// for dark and light, neon violet for Velora Gamers.
pub const THEME_ACCENTS: [usize; 3] = [0, 0, 7];

/// Everything chosen on the installer's screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub installer_language: Lang,
    pub language: usize,
    pub region: usize,
    pub layout: usize,
    pub system_hotkeys: bool,
    pub pointer: usize,
    pub large_pointer: bool,
    pub theme: usize,
    pub icons: usize,
    pub wallpaper: usize,
    pub transparency: bool,
    pub rounded_corners: bool,
    pub animations: bool,
    pub use_accent: bool,
    pub accent: usize,
    /// Start with the console login instead of the Lumi desktop.
    pub console: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            installer_language: Lang::Ru,
            language: 0,
            region: 0,
            layout: 0,
            system_hotkeys: true,
            pointer: 0,
            large_pointer: false,
            theme: 0,
            icons: 0,
            wallpaper: 0,
            transparency: true,
            rounded_corners: true,
            animations: true,
            use_accent: true,
            accent: 0,
            console: false,
        }
    }
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn pick(list: &[Choice], i: usize) -> &'static str {
    list.get(i).unwrap_or(&list[0]).id
}

impl Settings {
    /// The files under /etc/velora: (path, contents).
    pub fn files(&self) -> Vec<(&'static str, String)> {
        let accent = ACCENTS.get(self.accent).unwrap_or(&ACCENTS[0]);
        let header = "# Written by the Velora OS installer.\n";
        alloc::vec![
            (
                "/etc/velora/locale.conf",
                format!(
                    "{}language={}\nregion={}\n",
                    header,
                    pick(&LANGUAGES, self.language),
                    pick(&REGIONS, self.region)
                ),
            ),
            (
                "/etc/velora/keyboard.conf",
                format!(
                    "{}layouts={}\nswitch=alt+shift\nsystem_hotkeys={}\n",
                    header,
                    pick(&LAYOUTS, self.layout),
                    yes(self.system_hotkeys)
                ),
            ),
            (
                "/etc/velora/desktop.conf",
                format!(
                    "{}# For the graphical shell Lumi (session=console: the console login).\n\
                     session={}\n\
                     theme={}\nicons={}\nwallpaper={}\npointer={}\nlarge_pointer={}\n\
                     transparency={}\nrounded_corners={}\nanimations={}\n\
                     accent={}\naccent_color=#{:06x}\n",
                    header,
                    if self.console { "console" } else { "lumi" },
                    pick(&THEMES, self.theme),
                    pick(&ICONS, self.icons),
                    pick(&WALLPAPERS, self.wallpaper),
                    pick(&POINTERS, self.pointer),
                    yes(self.large_pointer),
                    yes(self.transparency),
                    yes(self.rounded_corners),
                    yes(self.animations),
                    if self.use_accent { accent.0 } else { "none" },
                    accent.1
                ),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_files() {
        let mut s = Settings {
            theme: 2,
            accent: 6,
            large_pointer: true,
            ..Settings::default()
        };
        let files = s.files();
        assert_eq!(files.len(), 3);
        let desktop = &files
            .iter()
            .find(|f| f.0 == "/etc/velora/desktop.conf")
            .unwrap()
            .1;
        assert!(desktop.contains("theme=velora-gamers\n"));
        assert!(desktop.contains("accent=mint\naccent_color=#78bea2\n"));
        assert!(desktop.contains("large_pointer=yes\n"));
        assert!(desktop.contains("session=lumi\n"));
        let locale = &files[0].1;
        assert!(locale.contains("language=ru_RU\nregion=UA\n"));
        s.use_accent = false;
        assert!(s.files()[2].1.contains("accent=none\n"));
        // Out of range indices fall back to the first choice.
        s.theme = 99;
        assert!(s.files()[2].1.contains("theme=velora-dark\n"));
        assert_eq!(REGIONS[0].name(Lang::En), "Ukraine");
    }
}

//! The apps Lumi runs in its windows, their names and icons.

pub mod about;
pub mod files;
pub mod settings;
pub mod terminal;
pub mod viewer;

use crate::config::Config;
use crate::host::Host;
use alloc::string::String;
use alloc::vec::Vec;
use velora_ui::canvas::{Canvas, Color, FX, fade, rgb};
use velora_ui::icons::{self, Icon};
use velora_ui::image::{Picture, Pointer};
use velora_ui::text::Fonts;
use velora_ui::theme::Theme;
use velora_ui::{Event, Rect, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppId {
    Terminal,
    Files,
    Plume,
    Settings,
    About,
    Viewer,
}

/// The apps of the app menu, in its order.
pub const MENU: [AppId; 5] = [
    AppId::Terminal,
    AppId::Files,
    AppId::Plume,
    AppId::Settings,
    AppId::About,
];

/// The apps pinned to the panel.
pub const PINNED: [AppId; 4] = [AppId::Plume, AppId::Files, AppId::Terminal, AppId::Settings];

pub fn tr(ru: bool, r: &'static str, e: &'static str) -> &'static str {
    if ru { r } else { e }
}

impl AppId {
    pub fn name(self, ru: bool) -> &'static str {
        match self {
            AppId::Terminal => tr(ru, "Терминал", "Terminal"),
            AppId::Files => tr(ru, "Файлы", "Files"),
            AppId::Plume => "Plume",
            AppId::Settings => tr(ru, "Настройки", "Settings"),
            AppId::About => tr(ru, "О системе", "About"),
            AppId::Viewer => tr(ru, "Просмотр", "Viewer"),
        }
    }

    /// Words the app menu search also finds it by.
    pub fn keywords(self) -> &'static str {
        match self {
            AppId::Terminal => "terminal console shell vsh терминал консоль командная строка",
            AppId::Files => "files folders explorer файлы папки проводник диск",
            AppId::Plume => "plume editor text редактор текст блокнот",
            AppId::Settings => {
                "settings theme wallpaper accent pointer настройки тема обои акцент указатель"
            }
            AppId::About => "about system version memory cpu о системе версия память процессор",
            AppId::Viewer => "viewer image picture просмотр картинка",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            AppId::Terminal => Icon::Terminal,
            AppId::Files => Icon::Folder,
            AppId::Plume => Icon::Feather,
            AppId::Settings => Icon::Gear,
            AppId::About => Icon::Info,
            AppId::Viewer => Icon::Picture,
        }
    }

    /// Colours of the app's tile: top and bottom.
    fn tile_colors(self) -> (Color, Color) {
        match self {
            AppId::Terminal => (rgb(78, 78, 86), rgb(30, 30, 34)),
            AppId::Files => (rgb(96, 150, 240), rgb(44, 92, 200)),
            AppId::Plume => (rgb(150, 120, 236), rgb(92, 64, 186)),
            AppId::Settings => (rgb(150, 150, 160), rgb(84, 84, 94)),
            AppId::About => (rgb(88, 196, 172), rgb(38, 132, 112)),
            AppId::Viewer => (rgb(240, 170, 90), rgb(196, 110, 40)),
        }
    }

    /// Does the search text find this app?
    pub fn matches(self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        let hay = alloc::format!(
            "{} {} {}",
            self.name(true).to_lowercase(),
            self.name(false).to_lowercase(),
            self.keywords()
        );
        q.split_whitespace().all(|w| hay.contains(w))
    }
}

/// Draws an app's tile (a rounded square with a light sheen and the
/// icon) in `r`; `alpha` 0..=255.
pub fn draw_tile(c: &mut Canvas, app: AppId, r: Rect, rounded: bool, alpha: u32) {
    if alpha == 0 || r.w <= 0 {
        return;
    }
    let a = |col: Color| fade(col, (alpha * ((col >> 24) & 0xff) / 255) as u8);
    let (top, bottom) = app.tile_colors();
    let rad = if rounded { r.w * 26 / 100 } else { r.w / 10 };
    c.shadow(r.offset(0, r.h / 20), rad, r.w / 10, a(0x50000000));
    c.fill_rrect(r, rad, a(bottom));
    let x = r.x as i64;
    let w = r.w as i64;
    c.fill_circle_inside(
        (x + w / 2) * FX,
        (r.y as i64 - w * 9 / 10) * FX,
        w * FX * 13 / 10,
        w * FX / 3,
        r,
        rad,
        a(top),
    );
    // A thin light edge on the top.
    c.stroke_rrect_fx(r, rad, FX, a(0x30ffffff));
    let s = r.w * 52 / 100;
    icons::draw(
        c,
        app.icon(),
        r.center_x() - s / 2,
        r.center_y() - s / 2,
        s,
        a(0xf2ffffff),
        (s as i64 * FX / 11).max(FX),
    );
}

/// Pictures the shell uses.
pub struct Assets {
    pub logo: Picture,
    /// Classic, blade, drop (the order of `config::POINTERS`).
    pub pointers: [Pointer; 3],
}

/// What an app asks the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Launch(AppId),
    /// Opens a file or folder with the app that fits it.
    Open(String),
    /// Opens Files at a folder.
    Browse(String),
    /// Runs a program in a new terminal window.
    Run {
        argv: Vec<String>,
        app: AppId,
    },
    /// New settings: applied with a transition and saved.
    Config(Config),
    /// Puts text on the clipboard.
    Copy(String),
    Close,
}

/// Everything an app draws with.
pub struct Cx<'a> {
    pub ui: &'a mut Ui,
    pub c: &'a mut Canvas,
    pub f: &'a mut Fonts,
    pub host: &'a mut dyn Host,
    pub assets: &'a mut Assets,
    pub cfg: &'a Config,
    pub theme: Theme,
    /// This frame's input in window coordinates (also shortcuts).
    pub events: &'a [Event],
    pub focused: bool,
    pub now: u64,
    pub ru: bool,
    pub out: &'a mut Vec<Action>,
    pub clipboard: &'a str,
}

impl Cx<'_> {
    pub fn tr(&self, r: &'static str, e: &'static str) -> &'static str {
        tr(self.ru, r, e)
    }
}

/// An app in a window.
pub trait App {
    fn id(&self) -> AppId;
    fn title(&self, ru: bool) -> String;
    /// Size the window opens with and its smallest size, design units.
    fn size(&self) -> ((i32, i32), (i32, i32));
    /// Draws the content in `r` (window picture coordinates). Apps with
    /// widgets ([`App::uses_ui`]) get their input through `cx.ui`.
    fn draw(&mut self, cx: &mut Cx, r: Rect);
    /// True if the app reads its input itself ([`App::input`]) and says
    /// what to draw again ([`App::damage`]), instead of the widgets.
    fn uses_ui(&self) -> bool {
        true
    }
    /// Input for apps that do not use widgets (window coordinates).
    fn input(&mut self, _events: &[Event], _cx: &mut Cx, _r: Rect) {}
    /// Part of the content to draw again (apps without widgets).
    fn damage(&mut self, _r: Rect) -> Option<Rect> {
        None
    }
    /// Called every frame; true if something changed (the whole content
    /// is drawn again, or [`App::damage`] for apps without widgets).
    fn poll(&mut self, _host: &mut dyn Host, _now: u64) -> bool {
        false
    }
    /// When the app wants a frame without input (ms since boot).
    fn wake_at(&self, _now: u64) -> Option<u64> {
        None
    }
    /// False once the app has ended by itself (a terminal's program).
    fn alive(&self) -> bool {
        true
    }
    /// The window is closing.
    fn close(&mut self, _host: &mut dyn Host) {}
    /// The window got a new size; `r` is the content rectangle.
    fn resized(&mut self, _host: &mut dyn Host, _f: &mut Fonts, _ui: &Ui, _r: Rect) {}
    /// Called before drawing with new settings.
    fn settings_changed(&mut self) {}
}

//! Settings: appearance, wallpaper, the panel, windows, mouse and
//! keyboard, the screen, date and time, sound, the user and the system.
//! Every change applies at once (with a transition) and is saved to the
//! user's settings file (`~/.config/lumi/settings.conf`).

use super::{Action, App, AppId, Cx, PINNABLE, tr};
use crate::config::{
    self, ACCENTS, Accent, BUILTIN_WALLPAPERS, Config, PANEL_ALWAYS, PANEL_AUTOHIDE, PANEL_SMART,
    THEMES, UI_SCALES,
};
use crate::host::{Host, SysInfo, join};
use crate::{tz, wallpaper};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use velora_ui::canvas::{Canvas, FX, fade, lerp, rgb};
use velora_ui::icons::{self, Icon};
use velora_ui::text::Weight;
use velora_ui::theme::{self, Theme};
use velora_ui::ui::{ButtonKind, Ctx, Key, Response};
use velora_ui::{Event, Rect};

/// Where pictures for the wallpaper are looked for.
pub const WALLPAPER_DIR: &str = "/usr/share/wallpapers";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Appearance,
    Wallpaper,
    Panel,
    Windows,
    Input,
    Display,
    Time,
    Sound,
    User,
    About,
}

const PAGES: [Page; 10] = [
    Page::Appearance,
    Page::Wallpaper,
    Page::Panel,
    Page::Windows,
    Page::Input,
    Page::Display,
    Page::Time,
    Page::Sound,
    Page::User,
    Page::About,
];

impl Page {
    fn name(self, ru: bool) -> &'static str {
        match self {
            Page::Appearance => tr(ru, "Оформление", "Appearance"),
            Page::Wallpaper => tr(ru, "Обои", "Wallpaper"),
            Page::Panel => tr(ru, "Панель", "Panel"),
            Page::Windows => tr(ru, "Окна", "Windows"),
            Page::Input => tr(ru, "Мышь и клавиатура", "Mouse and keyboard"),
            Page::Display => tr(ru, "Экран", "Display"),
            Page::Time => tr(ru, "Дата и время", "Date and time"),
            Page::Sound => tr(ru, "Звук", "Sound"),
            Page::User => tr(ru, "Пользователь", "User"),
            Page::About => tr(ru, "О системе", "About"),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Page::Appearance => Icon::Sun,
            Page::Wallpaper => Icon::Picture,
            Page::Panel => Icon::Dock,
            Page::Windows => Icon::Window,
            Page::Input => Icon::Pointer,
            Page::Display => Icon::Display,
            Page::Time => Icon::Clock,
            Page::Sound => Icon::Speaker,
            Page::User => Icon::User,
            Page::About => Icon::Info,
        }
    }
}

pub struct Settings {
    page: Page,
    /// Wallpaper choices: id and a small picture.
    walls: Vec<(String, Option<Canvas>)>,
    thumb: (usize, usize),
    scroll: i32,
    home: String,
    /// The pinned app being dragged in the panel's list.
    dragging: Option<AppId>,
    /// Text fields: a custom accent, a picture's path, passwords.
    hex: String,
    path: String,
    old_pw: String,
    new_pw: String,
    new_pw2: String,
    /// The last result of a form: success and the text.
    note: Option<(bool, String)>,
    info: SysInfo,
    keyboard: (String, String),
    /// The volume slider clicks again from this time.
    click_at: u64,
}

impl Settings {
    pub fn new(host: &mut dyn Host, home: &str) -> Settings {
        let mut walls: Vec<(String, Option<Canvas>)> = BUILTIN_WALLPAPERS
            .iter()
            .map(|id| (String::from(*id), None))
            .collect();
        for dir in [String::from(WALLPAPER_DIR), join(home, "Pictures")] {
            if let Ok(list) = host.list(&dir) {
                let mut names: Vec<String> = list
                    .into_iter()
                    .filter(|e| !e.dir && e.name.to_lowercase().ends_with(".png"))
                    .map(|e| e.name)
                    .collect();
                names.sort();
                for n in names {
                    walls.push((format!("file:{}", join(&dir, &n)), None));
                }
            }
        }
        let keyboard = host
            .read(config::KEYBOARD_FILE)
            .map(|t| config::keyboard(&String::from_utf8_lossy(&t)))
            .unwrap_or_else(|| config::keyboard(""));
        Settings {
            page: Page::Appearance,
            walls,
            thumb: (0, 0),
            scroll: 0,
            home: String::from(home),
            dragging: None,
            hex: String::new(),
            path: String::new(),
            old_pw: String::new(),
            new_pw: String::new(),
            new_pw2: String::new(),
            note: None,
            info: host.sysinfo(),
            keyboard,
            click_at: 0,
        }
    }

    /// Opens on the wallpaper page.
    pub fn show_wallpapers(&mut self) {
        self.page = Page::Wallpaper;
    }

    fn thumbs(&mut self, host: &mut dyn Host, w: usize, h: usize) {
        if self.thumb != (w, h) {
            self.thumb = (w, h);
            for t in self.walls.iter_mut() {
                t.1 = None;
            }
        }
        for (id, pic) in self.walls.iter_mut() {
            if pic.is_some() {
                continue;
            }
            let mut c = Canvas::new(w, h);
            if let Some(path) = id.strip_prefix("file:") {
                match host
                    .read(path)
                    .and_then(|d| velora_gfx::png::decode(&d, 32 << 20).ok())
                {
                    Some(img) => c = wallpaper::cover(&img, w, h),
                    None => c.clear(0x202024),
                }
            } else {
                wallpaper::draw_builtin(&mut c, id);
            }
            *pic = Some(c);
        }
    }
}

/// A small picture of a theme: backdrop, a window and the panel.
fn theme_preview(c: &mut Canvas, r: Rect, t: &Theme, rad: i32) {
    c.fill_rrect(r, rad, t.backdrop);
    let win = Rect::new(r.x + r.w / 7, r.y + r.h / 6, r.w * 5 / 7, r.h * 5 / 9);
    c.fill_rrect(win, rad / 2, t.window);
    let bar = (r.h / 22).max(2);
    c.fill_rect(
        Rect::new(win.x + win.w / 8, win.y + win.h / 3, win.w / 2, bar),
        t.text_faint,
    );
    c.fill_rect(
        Rect::new(win.x + win.w / 8, win.y + win.h / 2, win.w / 3, bar),
        t.text_faint,
    );
    let dot = (r.h / 14).max(3);
    c.fill_circle(
        win.right() - dot * 2,
        win.y + dot * 2,
        dot / 2 + 1,
        t.accent,
    );
    let panel = Rect::new(
        r.x + r.w * 3 / 10,
        r.bottom() - r.h / 6,
        r.w * 4 / 10,
        r.h / 9,
    );
    c.fill_rrect(panel, panel.h / 2, fade(t.window, 230));
}

/// A colour from a hue (0..360) with the saturation and brightness of
/// the preset accents.
pub fn hue_color(h: i32) -> u32 {
    let h = h.rem_euclid(360);
    let (v, s) = (230i32, 150i32);
    let c = v * s / 255;
    let x = c * (60 - ((h % 120) - 60).abs()) / 60;
    let m = v - c;
    let (r, g, b) = match h / 60 {
        0 => (c, x, 0),
        1 => (x, c, 0),
        2 => (0, c, x),
        3 => (0, x, c),
        4 => (x, 0, c),
        _ => (c, 0, x),
    };
    (((r + m) as u32) << 16) | (((g + m) as u32) << 8) | (b + m) as u32
}

/// The hue of a colour (0..360).
fn hue_of(c: u32) -> i32 {
    let (r, g, b) = (
        ((c >> 16) & 0xff) as i32,
        ((c >> 8) & 0xff) as i32,
        (c & 0xff) as i32,
    );
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    if d == 0 {
        return 0;
    }
    let h = if max == r {
        60 * (g - b) / d
    } else if max == g {
        120 + 60 * (b - r) / d
    } else {
        240 + 60 * (r - g) / d
    };
    h.rem_euclid(360)
}

/// Where a page draws: the visible area and the scale.
#[derive(Clone, Copy)]
struct Area {
    view: Rect,
    x: i32,
    w: i32,
    sc: i64,
}

impl Area {
    fn s(&self, d: i32) -> i32 {
        ((d as i64 * self.sc + 128) >> 8) as i32
    }

    /// A widget is fully in view (only then it takes input).
    fn shown(&self, r: Rect) -> bool {
        r.intersect(self.view) == r
    }
}

/// Registers a widget only where it can be seen.
fn hit(cx: &mut Cx, a: &Area, id: u32, r: Rect) -> Response {
    let vis = r.intersect(a.view);
    if vis.is_empty() {
        return Response::default();
    }
    cx.ui.interact(id, vis)
}

/// Runs a widget that takes input only when it is fully in view.
fn in_view<R>(cx: &mut Cx, a: &Area, r: Rect, f: impl FnOnce(&mut Ctx) -> R) -> R {
    let on = a.shown(r);
    let was = cx.ui.is_enabled();
    if !on {
        cx.ui.set_enabled(false);
    }
    let out = {
        let mut ctx = Ctx {
            ui: cx.ui,
            c: cx.c,
            f: cx.f,
        };
        f(&mut ctx)
    };
    cx.ui.set_enabled(was);
    out
}

fn section(cx: &mut Cx, a: &Area, y: i32, text: &str) -> i32 {
    let t = cx.theme;
    cx.f.draw(
        cx.c,
        Weight::Medium,
        cx.ui.px(12),
        a.x,
        y + a.s(12),
        text,
        t.text_dim,
    );
    y + a.s(24)
}

/// A setting's row: a card with a title and a second line; returns the
/// card.
fn row(cx: &mut Cx, a: &Area, y: i32, title: &str, sub: &str, cfg: &Config) -> Rect {
    let t = cx.theme;
    let r = Rect::new(a.x, y, a.w, a.s(if sub.is_empty() { 44 } else { 54 }));
    cx.c.fill_rrect(r, a.s(cfg.round(12)), t.field);
    let p13 = cx.ui.px(13);
    if sub.is_empty() {
        cx.f.draw_centered_y(
            cx.c,
            Weight::Medium,
            p13,
            r.x + a.s(18),
            r.center_y(),
            title,
            t.text,
        );
    } else {
        cx.f.draw(
            cx.c,
            Weight::Medium,
            p13,
            r.x + a.s(18),
            r.y + a.s(23),
            title,
            t.text,
        );
        cx.f.draw(
            cx.c,
            Weight::Regular,
            cx.ui.px(11),
            r.x + a.s(18),
            r.y + a.s(41),
            sub,
            t.text_faint,
        );
    }
    r
}

/// A row with a switch on the right.
#[allow(clippy::too_many_arguments)]
fn toggle_row(
    cx: &mut Cx,
    a: &Area,
    y: &mut i32,
    id: u32,
    title: &str,
    sub: &str,
    cfg: &Config,
    v: &mut bool,
) -> bool {
    let r = row(cx, a, *y, title, sub, cfg);
    let tg = Rect::new(
        r.right() - a.s(62),
        r.center_y() - a.s(12),
        a.s(44),
        a.s(24),
    );
    *y = r.bottom() + a.s(8);
    in_view(cx, a, tg, |c| c.toggle(id, tg, v))
}

/// A row with a slider and its value as text.
#[allow(clippy::too_many_arguments)]
fn slider_row(
    cx: &mut Cx,
    a: &Area,
    y: &mut i32,
    id: u32,
    title: &str,
    value_text: &str,
    cfg: &Config,
    v: &mut i32,
    range: (i32, i32, i32),
) -> bool {
    let r = row(cx, a, *y, title, "", cfg);
    let t = cx.theme;
    let p12 = cx.ui.px(12);
    let tw = cx.f.measure(Weight::Medium, p12, value_text);
    cx.f.draw_centered_y(
        cx.c,
        Weight::Medium,
        p12,
        r.right() - a.s(18) - tw,
        r.center_y(),
        value_text,
        t.text_dim,
    );
    let sw = (r.w / 2).min(a.s(300));
    let sr = Rect::new(
        r.right() - a.s(76) - sw,
        r.center_y() - a.s(11),
        sw,
        a.s(22),
    );
    *y = r.bottom() + a.s(8);
    in_view(cx, a, sr, |c| {
        c.slider(id, sr, v, range.0, range.1, range.2)
    })
}

/// Buttons side by side, one chosen; true when another was chosen.
fn segmented(
    cx: &mut Cx,
    a: &Area,
    id: u32,
    r: Rect,
    items: &[&str],
    sel: &mut usize,
    cfg: &Config,
) -> bool {
    let t = cx.theme;
    let n = items.len().max(1) as i32;
    let rad = a.s(cfg.round(10));
    cx.c.fill_rrect(r, rad, t.field);
    let gap = a.s(3);
    let w = (r.w - gap * (n + 1)) / n;
    let mut changed = false;
    for (i, label) in items.iter().enumerate() {
        let cell = Rect::new(
            r.x + gap + i as i32 * (w + gap),
            r.y + gap,
            w,
            r.h - 2 * gap,
        );
        let uid = id + i as u32;
        let resp = hit(cx, a, uid, cell);
        let on = *sel == i;
        let hov = cx.ui.anim(uid, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
        if on {
            cx.c.fill_rrect(cell, (rad - gap).max(0), lerp(t.selected, t.accent, 70));
        } else if hov > 0 {
            cx.c.fill_rrect(cell, (rad - gap).max(0), fade(t.field_hover, hov));
        }
        let px = cx.ui.px(12);
        let tw = cx.f.measure(Weight::Medium, px, label);
        cx.f.draw_centered_y(
            cx.c,
            Weight::Medium,
            px,
            cell.center_x() - tw / 2,
            cell.center_y(),
            label,
            if on { t.text } else { t.text_dim },
        );
        if resp.focused {
            Ctx {
                ui: cx.ui,
                c: cx.c,
                f: cx.f,
            }
            .focus_ring(uid, cell, (rad - gap).max(0), true);
        }
        if resp.activated && !on {
            *sel = i;
            changed = true;
        }
    }
    changed
}

/// A row with a title on the left and buttons to choose from.
#[allow(clippy::too_many_arguments)]
fn choice_row(
    cx: &mut Cx,
    a: &Area,
    y: &mut i32,
    id: u32,
    title: &str,
    items: &[&str],
    sel: &mut usize,
    cfg: &Config,
) -> bool {
    let r = row(cx, a, *y, title, "", cfg);
    let sw = (a.s(86) * items.len() as i32).min(r.w * 6 / 10);
    let sr = Rect::new(r.right() - a.s(8) - sw, r.y + a.s(6), sw, r.h - a.s(12));
    *y = r.bottom() + a.s(8);
    segmented(cx, a, id, sr, items, sel, cfg)
}

/// Plain text lines (notes, lists); returns the next y.
fn note(cx: &mut Cx, a: &Area, y: i32, text: &str) -> i32 {
    let t = cx.theme;
    let px = cx.ui.px(12);
    let mut y = y;
    for line in wrap(cx, px, text, a.w) {
        cx.f.draw(
            cx.c,
            Weight::Regular,
            px,
            a.x,
            y + a.s(14),
            &line,
            t.text_faint,
        );
        y += a.s(19);
    }
    y + a.s(4)
}

/// Splits text into lines that fit `w` pixels.
fn wrap(cx: &mut Cx, px: u32, text: &str, w: i32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let try_ = if line.is_empty() {
            String::from(word)
        } else {
            format!("{} {}", line, word)
        };
        if !line.is_empty() && cx.f.measure(Weight::Regular, px, &try_) > w {
            out.push(core::mem::replace(&mut line, String::from(word)));
        } else {
            line = try_;
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

impl App for Settings {
    fn id(&self) -> AppId {
        AppId::Settings
    }

    fn title(&self, ru: bool) -> String {
        String::from(AppId::Settings.name(ru))
    }

    fn size(&self) -> ((i32, i32), (i32, i32)) {
        ((900, 640), (720, 520))
    }

    fn draw(&mut self, cx: &mut Cx, r: Rect) {
        let t = cx.theme;
        let ru = cx.ru;
        let sc = cx.ui.scale;
        let s = move |d: i32| ((d as i64 * sc + 128) >> 8) as i32;
        cx.c.fill_rect(r, t.window);
        let side = Rect::new(r.x, r.y, s(210), r.h);
        cx.c.fill_rect(side, t.sidebar);
        let row_h = ((r.h - s(24)) / PAGES.len() as i32).min(s(42));
        let mut y = r.y + s(12);
        for (i, p) in PAGES.iter().enumerate() {
            let row = Rect::new(side.x + s(10), y, side.w - s(20), row_h - s(4));
            let id = 10 + i as u32;
            let resp = cx.ui.interact(id, row);
            let here = self.page == *p;
            let hov = cx.ui.anim(id, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
            let rad = s(cx.cfg.round(9));
            if here {
                cx.c.fill_rrect(row, rad, lerp(t.selected, t.accent, 46));
            } else if hov > 0 {
                cx.c.fill_rrect(row, rad, fade(t.selected, hov));
            }
            let col = if here { t.text } else { t.text_dim };
            icons::draw(
                cx.c,
                p.icon(),
                row.x + s(12),
                row.center_y() - s(9),
                s(18),
                col,
                0,
            );
            cx.f.draw_centered_y(
                cx.c,
                Weight::Medium,
                cx.ui.px(13),
                row.x + s(42),
                row.center_y(),
                p.name(ru),
                col,
            );
            if resp.activated && !here {
                // The rows above were drawn for the old page: all again.
                cx.ui.request_redraw();
                self.page = *p;
                self.scroll = 0;
                self.note = None;
                self.dragging = None;
                if *p == Page::About {
                    self.info = cx.host.sysinfo();
                }
            }
            y += row_h;
        }
        let body = Rect::new(
            side.right() + s(32),
            r.y + s(20),
            r.right() - side.right() - s(56),
            r.h - s(20),
        );
        cx.f.draw(
            cx.c,
            Weight::SemiBold,
            cx.ui.px(22),
            body.x,
            body.y + s(24),
            self.page.name(ru),
            t.text,
        );
        let view = Rect::new(
            body.x - s(6),
            body.y + s(44),
            body.w + s(12),
            body.h - s(44),
        );
        // Scrolling with the wheel over the page.
        if view.contains(cx.ui.mouse.0, cx.ui.mouse.1) {
            for ev in cx.events {
                if let Event::Wheel(d) = *ev {
                    self.scroll = (self.scroll + d * s(48)).max(0);
                }
            }
        }
        let a = Area {
            view,
            x: body.x,
            w: body.w,
            sc,
        };
        let old = cx.c.set_clip(view.intersect(cx.c.clip()));
        let y0 = view.y + s(6) - self.scroll;
        let mut cfg = cx.cfg.clone();
        let (changed, bottom) = match self.page {
            Page::Appearance => self.appearance(cx, &a, y0, &mut cfg),
            Page::Wallpaper => self.wallpaper(cx, &a, y0, &mut cfg),
            Page::Panel => self.panel(cx, &a, y0, &mut cfg),
            Page::Windows => self.windows(cx, &a, y0, &mut cfg),
            Page::Input => self.input(cx, &a, y0, &mut cfg),
            Page::Display => self.display(cx, &a, y0, &mut cfg),
            Page::Time => self.time(cx, &a, y0, &mut cfg),
            Page::Sound => (false, self.sound(cx, &a, y0)),
            Page::User => (false, self.user(cx, &a, y0)),
            Page::About => (false, self.about(cx, &a, y0)),
        };
        cx.c.set_clip(old);
        // Keep the page in view; a scroll bar when it is longer.
        let content = bottom + self.scroll - y0 + s(12);
        let max = (content - view.h).max(0);
        if self.scroll > max {
            self.scroll = max;
            cx.ui.request_redraw();
        }
        if max > 0 {
            let bar_h = (view.h * view.h / content.max(1)).max(s(30));
            let by = view.y + (view.h - bar_h) * self.scroll / max;
            cx.c.fill_rrect(
                Rect::new(view.right() + s(4), by, s(4), bar_h),
                s(2),
                t.scrollbar,
            );
        }
        if changed && cfg != *cx.cfg {
            cx.out.push(Action::Config(alloc::boxed::Box::new(cfg)));
        }
    }
}

impl Settings {
    fn appearance(&mut self, cx: &mut Cx, a: &Area, y: i32, cfg: &mut Config) -> (bool, i32) {
        let t = cx.theme;
        let ru = cx.ru;
        let mut changed = false;
        let mut y = section(cx, a, y, tr(ru, "Тема", "Theme"));
        let gap = a.s(16);
        let cw = ((a.w - 2 * gap) / 3).min(a.s(190));
        let ch = cw * 10 / 16;
        for (i, id) in THEMES.iter().enumerate() {
            let card = Rect::new(a.x + i as i32 * (cw + gap), y, cw, ch);
            let base = match i {
                1 => theme::LIGHT,
                2 => theme::GAMERS,
                _ => theme::DARK,
            };
            let preview = match cfg.accent_rgb() {
                Some(c) => base.with_accent(c),
                None => base.neutral(),
            };
            let resp = hit(cx, a, 20 + i as u32, card);
            let rad = a.s(cfg.round(12));
            theme_preview(cx.c, card, &preview, rad);
            let sel = cfg.theme == i;
            let hov = cx.ui.anim(
                20 + i as u32,
                1,
                if resp.hovered || sel { 255 } else { 0 },
                120,
            ) as u8;
            let ring = if sel {
                t.accent
            } else {
                fade(t.text_faint, hov)
            };
            if sel || hov > 0 {
                cx.c.stroke_rrect_fx(card.inset(-a.s(3)), rad + a.s(3), a.s(2) as i64 * FX, ring);
            }
            let name = match *id {
                "velora-light" => "Velora Light",
                "velora-gamers" => "Velora Gamers",
                _ => "Velora Dark",
            };
            cx.f.draw(
                cx.c,
                Weight::Medium,
                cx.ui.px(12),
                card.x,
                card.bottom() + a.s(20),
                name,
                if sel { t.text } else { t.text_dim },
            );
            if resp.activated && !sel {
                cfg.theme = i;
                // Each theme comes with its accent, as in the installer.
                if !matches!(cfg.accent, Accent::Custom(_)) {
                    cfg.accent = Accent::Preset(if i == 2 { 7 } else { 0 });
                }
                changed = true;
            }
        }
        y += ch + a.s(34);
        y = section(cx, a, y, tr(ru, "Акцентный цвет", "Accent colour"));
        let d = a.s(30);
        let step = d + a.s(12);
        // The presets, the theme's neutral colour and a colour of one's own.
        for i in 0..ACCENTS.len() + 2 {
            let c = Rect::new(a.x + i as i32 * step, y + a.s(4), d, d);
            let id = 40 + i as u32;
            let resp = hit(cx, a, id, c);
            let choice = match i {
                n if n < ACCENTS.len() => Accent::Preset(n),
                n if n == ACCENTS.len() => Accent::None,
                _ => match cfg.accent {
                    Accent::Custom(c) => Accent::Custom(c),
                    _ => Accent::Custom(hue_color(200)),
                },
            };
            let col = match choice {
                Accent::Preset(n) => 0xff00_0000 | ACCENTS[n].1,
                Accent::None => t.neutral().accent,
                Accent::Custom(c) => 0xff00_0000 | c,
            };
            let sel = match (cfg.accent, choice) {
                (Accent::Custom(_), Accent::Custom(_)) => true,
                (x, y) => x == y,
            };
            let grow = cx.ui.anim(id, 1, if resp.hovered { 255 } else { 0 }, 120);
            let rr = c.inset(-(a.s(2) * grow / 255));
            if matches!(choice, Accent::Custom(_)) && !sel {
                // A ring of hues with a plus: choose your own.
                for k in 0..12 {
                    let ang = k * 30;
                    let (sn, cs) = velora_ui::canvas::sin_cos((ang * 65536 / 360) as u32);
                    let px = rr.center_x() as i64 * FX + (rr.w as i64 * FX * 3 / 8) * sn / 65536;
                    let py = rr.center_y() as i64 * FX - (rr.w as i64 * FX * 3 / 8) * cs / 65536;
                    cx.c.fill_circle_fx(px, py, rr.w as i64 * FX / 7, 0xff00_0000 | hue_color(ang));
                }
                icons::draw(
                    cx.c,
                    Icon::Plus,
                    rr.center_x() - a.s(7),
                    rr.center_y() - a.s(7),
                    a.s(14),
                    t.text,
                    0,
                );
            } else {
                cx.c.fill_circle(rr.center_x(), rr.center_y(), rr.w / 2, col);
            }
            if choice == Accent::None {
                // "No accent": the theme's own neutral colour, crossed.
                cx.c.line(
                    c.x + a.s(7),
                    c.bottom() - a.s(7),
                    c.right() - a.s(7),
                    c.y + a.s(7),
                    FX * 2,
                    t.window,
                );
            }
            if sel {
                cx.c.stroke_circle_fx(
                    c.center_x() as i64 * FX,
                    c.center_y() as i64 * FX,
                    (d / 2 + a.s(4)) as i64 * FX,
                    a.s(2) as i64 * FX,
                    col,
                );
            }
            if resp.activated && !sel {
                cfg.accent = choice;
                if let Accent::Custom(c) = choice {
                    self.hex = format!("#{:06x}", c);
                }
                changed = true;
            }
        }
        y += d + a.s(20);
        if let Accent::Custom(c) = cfg.accent {
            // The hue and the code of one's own colour.
            let mut h = hue_of(c);
            let text = format!("{}°", h);
            if slider_row(
                cx,
                a,
                &mut y,
                55,
                tr(ru, "Свой цвет: оттенок", "Own colour: hue"),
                &text,
                cfg,
                &mut h,
                (0, 359, 1),
            ) {
                let c = hue_color(h);
                cfg.accent = Accent::Custom(c);
                self.hex = format!("#{:06x}", c);
                changed = true;
            }
            let r = row(cx, a, y, tr(ru, "Код цвета", "Colour code"), "", cfg);
            let f = Rect::new(r.right() - a.s(178), r.y + a.s(6), a.s(160), r.h - a.s(12));
            if self.hex.is_empty() {
                self.hex = format!("#{:06x}", c);
            }
            let typed = in_view(cx, a, f, |x| {
                x.text_field(56, f, &mut self.hex, "#3f7ee8", false, 7)
            });
            if typed
                && let Some(n) = config::parse_color(&self.hex)
                && n != c
            {
                cfg.accent = Accent::Custom(n);
                changed = true;
            }
            y = r.bottom() + a.s(8);
        }
        y += a.s(10);
        y = section(cx, a, y, tr(ru, "Панель и окна", "Panel and windows"));
        let mut v = cfg.transparency as i32;
        let text = format!("{} %", v);
        if slider_row(
            cx,
            a,
            &mut y,
            60,
            tr(ru, "Прозрачность", "Transparency"),
            &text,
            cfg,
            &mut v,
            (0, 100, 5),
        ) {
            cfg.transparency = v as u32;
            changed = true;
        }
        let mut v = cfg.radius as i32;
        let text = format!("{} px", v);
        if slider_row(
            cx,
            a,
            &mut y,
            61,
            tr(ru, "Скругление углов", "Corner radius"),
            &text,
            cfg,
            &mut v,
            (0, 20, 1),
        ) {
            cfg.radius = v as u32;
            changed = true;
        }
        let names = [
            tr(ru, "Нет", "Off"),
            tr(ru, "Медленно", "Slow"),
            tr(ru, "Обычно", "Normal"),
            tr(ru, "Быстро", "Fast"),
        ];
        let mut sel = cfg.animations;
        if choice_row(
            cx,
            a,
            &mut y,
            62,
            tr(ru, "Анимации", "Animations"),
            &names,
            &mut sel,
            cfg,
        ) {
            cfg.animations = sel;
            changed = true;
        }
        y = note(
            cx,
            a,
            y,
            tr(
                ru,
                "Масштаб интерфейса — на странице «Экран».",
                "The interface scale is on the Display page.",
            ),
        );
        (changed, y)
    }

    fn wallpaper(&mut self, cx: &mut Cx, a: &Area, y: i32, cfg: &mut Config) -> (bool, i32) {
        let t = cx.theme;
        let ru = cx.ru;
        let mut changed = false;
        let gap = a.s(16);
        let cols = 3;
        let tw = (a.w - gap * (cols - 1)) / cols;
        let th = tw * 9 / 16;
        self.thumbs(cx.host, tw as usize, th as usize);
        let row_h = th + a.s(40);
        let rad = a.s(cfg.round(12));
        let mut bottom = y;
        for (i, (id, pic)) in self.walls.iter().enumerate() {
            let (col, row) = (i as i32 % cols, i as i32 / cols);
            let cell = Rect::new(a.x + col * (tw + gap), y + row * row_h + a.s(4), tw, th);
            bottom = cell.bottom() + a.s(30);
            let uid = 100 + i as u32;
            let resp = hit(cx, a, uid, cell);
            if let Some(p) = pic {
                let corner = crate::gfx::Corner::new(rad);
                crate::gfx::blit(cx.c, p, cell.x, cell.y, &corner, 255);
            }
            let sel = cfg.wallpaper == *id;
            let hov = cx.ui.anim(uid, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
            if sel {
                cx.c.stroke_rrect_fx(
                    cell.inset(-a.s(3)),
                    rad + a.s(3),
                    a.s(2) as i64 * FX,
                    t.accent,
                );
            } else if hov > 0 {
                cx.c.stroke_rrect_fx(
                    cell.inset(-a.s(3)),
                    rad + a.s(3),
                    a.s(2) as i64 * FX,
                    fade(t.text_faint, hov),
                );
            }
            let name = match id.as_str() {
                "waves" => String::from(tr(ru, "Волны", "Waves")),
                "graphite" => String::from(tr(ru, "Графит", "Graphite")),
                "aurora" => String::from(tr(ru, "Сияние", "Aurora")),
                "ember" => String::from(tr(ru, "Угли", "Ember")),
                other => String::from(other.rsplit('/').next().unwrap_or(other)),
            };
            cx.f.draw(
                cx.c,
                Weight::Medium,
                cx.ui.px(12),
                cell.x,
                cell.bottom() + a.s(20),
                &name,
                if sel { t.text } else { t.text_dim },
            );
            if resp.activated && !sel {
                cfg.wallpaper = id.clone();
                changed = true;
            }
        }
        let mut y = bottom + a.s(6);
        let mut v = cfg.wallpaper_dim as i32;
        let text = format!("{} %", v);
        if slider_row(
            cx,
            a,
            &mut y,
            70,
            tr(ru, "Затемнение", "Dimming"),
            &text,
            cfg,
            &mut v,
            (0, 80, 5),
        ) {
            cfg.wallpaper_dim = v as u32;
            changed = true;
        }
        let mut v = cfg.wallpaper_blur as i32;
        let text = format!("{} %", v);
        if slider_row(
            cx,
            a,
            &mut y,
            71,
            tr(ru, "Размытие", "Blur"),
            &text,
            cfg,
            &mut v,
            (0, 100, 10),
        ) {
            cfg.wallpaper_blur = v as u32;
            changed = true;
        }
        // A picture of one's own by its path.
        let r = row(cx, a, y, tr(ru, "Своя картинка", "Own picture"), "", cfg);
        let bw = a.s(110);
        let f = Rect::new(
            r.x + a.s(150),
            r.y + a.s(6),
            r.w - a.s(150) - bw - a.s(18),
            r.h - a.s(12),
        );
        let b = Rect::new(f.right() + a.s(8), f.y, bw, f.h);
        let ph = format!("{}/Pictures/…png", self.home);
        in_view(cx, a, f, |x| {
            x.text_field(72, f, &mut self.path, &ph, false, 200)
        });
        let enter = cx.ui.focused() == Some(72) && cx.ui.take_key(Key::Enter);
        let pressed = in_view(cx, a, b, |x| {
            x.button(
                73,
                b,
                tr(ru, "Поставить", "Set"),
                ButtonKind::Tonal,
                None,
                None,
            )
        });
        if pressed || enter {
            let path = String::from(self.path.trim());
            let ok = cx
                .host
                .read(&path)
                .is_some_and(|d| velora_gfx::png::decode(&d, 40 << 20).is_ok());
            if ok {
                let id = format!("file:{}", path);
                if !self.walls.iter().any(|w| w.0 == id) {
                    self.walls.push((id.clone(), None));
                }
                cfg.wallpaper = id;
                changed = true;
                self.note = None;
            } else {
                self.note = Some((
                    false,
                    String::from(tr(
                        ru,
                        "Не получилось открыть: нужен файл PNG.",
                        "Could not open it: a PNG file is needed.",
                    )),
                ));
            }
        }
        y = r.bottom() + a.s(8);
        if let Some((_, text)) = self.note.clone() {
            y = note(cx, a, y, &text);
        }
        let hint = if ru {
            format!(
                "Картинки PNG из {} и {}/Pictures появляются в списке сами; они заполняют экран без искажений.",
                WALLPAPER_DIR, self.home
            )
        } else {
            format!(
                "PNG pictures in {} and {}/Pictures appear in the list; they fill the screen without distortion.",
                WALLPAPER_DIR, self.home
            )
        };
        y = note(cx, a, y, &hint);
        (changed, y)
    }

    fn panel(&mut self, cx: &mut Cx, a: &Area, y: i32, cfg: &mut Config) -> (bool, i32) {
        let t = cx.theme;
        let ru = cx.ru;
        let mut changed = false;
        let mut y = section(cx, a, y, tr(ru, "Скрытие", "Hiding"));
        let modes = [
            (
                PANEL_SMART,
                tr(ru, "Умное скрытие", "Smart hiding"),
                tr(
                    ru,
                    "Прячется, когда окно её перекрывает",
                    "Hides when a window covers it",
                ),
            ),
            (
                PANEL_AUTOHIDE,
                tr(ru, "Автоскрытие", "Autohide"),
                tr(
                    ru,
                    "Появляется у нижнего края экрана",
                    "Comes up at the bottom edge of the screen",
                ),
            ),
            (
                PANEL_ALWAYS,
                tr(ru, "Всегда видна", "Always shown"),
                tr(
                    ru,
                    "Окна не заходят на её место",
                    "Windows keep out of its place",
                ),
            ),
        ];
        for (i, (m, title, sub)) in modes.iter().enumerate() {
            let r = Rect::new(a.x, y, a.w, a.s(52));
            let rr = r;
            let on = cfg.panel_mode == *m;
            let picked = in_view(cx, a, rr, |x| x.radio(200 + i as u32, rr, on, title, sub));
            if picked && !on {
                cfg.panel_mode = *m;
                changed = true;
            }
            y += a.s(58);
        }
        y += a.s(6);
        y = section(cx, a, y, tr(ru, "Вид", "Look"));
        let sizes = [
            tr(ru, "Мелкие", "Small"),
            tr(ru, "Средние", "Medium"),
            tr(ru, "Крупные", "Large"),
        ];
        let mut sel = cfg.panel_size;
        if choice_row(
            cx,
            a,
            &mut y,
            210,
            tr(ru, "Размер значков", "Icon size"),
            &sizes,
            &mut sel,
            cfg,
        ) {
            cfg.panel_size = sel;
            changed = true;
        }
        let mut sel = if cfg.clock_24h { 0 } else { 1 };
        if choice_row(
            cx,
            a,
            &mut y,
            215,
            tr(ru, "Часы", "Clock"),
            &[tr(ru, "24 часа", "24-hour"), tr(ru, "12 часов", "12-hour")],
            &mut sel,
            cfg,
        ) {
            cfg.clock_24h = sel == 0;
            changed = true;
        }
        let mut v = cfg.show_date;
        if toggle_row(
            cx,
            a,
            &mut y,
            218,
            tr(ru, "Дата рядом с часами", "Date next to the clock"),
            "",
            cfg,
            &mut v,
        ) {
            cfg.show_date = v;
            changed = true;
        }
        y += a.s(6);
        y = section(cx, a, y, tr(ru, "Закреплённые приложения", "Pinned apps"));
        y = note(
            cx,
            a,
            y,
            tr(
                ru,
                "Порядок меняется перетаскиванием за левый край строки.",
                "Drag a row by its left edge to change the order.",
            ),
        );
        // Pinned first, in their order; then the others.
        let mut list: Vec<AppId> = cfg
            .pinned
            .iter()
            .filter_map(|k| AppId::from_key(k))
            .collect();
        let pinned_n = list.len();
        for app in PINNABLE {
            if !list.contains(&app) {
                list.push(app);
            }
        }
        let rh = a.s(46);
        let top = y;
        // A drag ends when the button goes up.
        if cx
            .events
            .iter()
            .any(|e| matches!(e, Event::MouseButton { down: false }))
        {
            self.dragging = None;
        }
        if let Some(app) = self.dragging
            && let Some(from) = list.iter().position(|&x| x == app)
            && from < pinned_n
        {
            let to = ((cx.ui.mouse.1 - top) / rh).clamp(0, pinned_n as i32 - 1) as usize;
            if to != from {
                let k = cfg.pinned.iter().position(|s| s == app.key());
                if let Some(k) = k {
                    let s = cfg.pinned.remove(k);
                    let to = to.min(cfg.pinned.len());
                    cfg.pinned.insert(to, s);
                    changed = true;
                    let x = list.remove(from);
                    list.insert(to, x);
                }
            }
            cx.ui.request_redraw();
        }
        for (i, app) in list.iter().enumerate() {
            let r = Rect::new(a.x, top + i as i32 * rh, a.w, rh - a.s(6));
            let pinned = i < pinned_n;
            let dragged = self.dragging == Some(*app);
            let fill = if dragged {
                lerp(t.field, t.accent, 50)
            } else {
                t.field
            };
            cx.c.fill_rrect(r, a.s(cfg.round(10)), fill);
            // The handle.
            let handle = Rect::new(r.x, r.y, a.s(40), r.h);
            if pinned {
                let resp = hit(cx, a, 230 + i as u32, handle);
                if resp.pressed && self.dragging.is_none() {
                    self.dragging = Some(*app);
                }
                let hc = if resp.hovered || dragged {
                    t.text
                } else {
                    t.text_faint
                };
                for k in 0..3 {
                    cx.c.fill_rect(
                        Rect::new(
                            r.x + a.s(14),
                            r.center_y() - a.s(5) + k * a.s(5),
                            a.s(14),
                            a.s(2).max(1),
                        ),
                        hc,
                    );
                }
            }
            icons::draw(
                cx.c,
                app.icon(),
                r.x + a.s(44),
                r.center_y() - a.s(9),
                a.s(18),
                t.text_dim,
                0,
            );
            cx.f.draw_centered_y(
                cx.c,
                Weight::Medium,
                cx.ui.px(13),
                r.x + a.s(74),
                r.center_y(),
                app.name(ru),
                if pinned { t.text } else { t.text_dim },
            );
            let tg = Rect::new(
                r.right() - a.s(62),
                r.center_y() - a.s(12),
                a.s(44),
                a.s(24),
            );
            let mut v = pinned;
            if in_view(cx, a, tg, |x| x.toggle(250 + i as u32, tg, &mut v)) {
                if v {
                    cfg.pinned.push(app.key().to_string());
                } else {
                    cfg.pinned.retain(|k| k != app.key());
                }
                changed = true;
            }
        }
        y = top + list.len() as i32 * rh + a.s(4);
        (changed, y)
    }

    fn windows(&mut self, cx: &mut Cx, a: &Area, y: i32, cfg: &mut Config) -> (bool, i32) {
        let ru = cx.ru;
        let mut changed = false;
        let mut y = y;
        let mut v = cfg.snap;
        if toggle_row(
            cx,
            a,
            &mut y,
            300,
            tr(ru, "Привязка к краям", "Snap to edges"),
            tr(
                ru,
                "Окно у левого или правого края занимает половину экрана, у верхнего — весь",
                "A window at the left or right edge takes half the screen, at the top all of it",
            ),
            cfg,
            &mut v,
        ) {
            cfg.snap = v;
            changed = true;
        }
        let mut sel = if cfg.buttons_left { 0 } else { 1 };
        if choice_row(
            cx,
            a,
            &mut y,
            305,
            tr(ru, "Кнопки заголовка", "Title bar buttons"),
            &[tr(ru, "Слева", "Left"), tr(ru, "Справа", "Right")],
            &mut sel,
            cfg,
        ) {
            cfg.buttons_left = sel == 0;
            changed = true;
        }
        y = note(
            cx,
            a,
            y + a.s(4),
            tr(
                ru,
                "Двойной щелчок по заголовку разворачивает окно; Super+стрелки — к краям, развернуть и свернуть.",
                "A double click on the title maximizes a window; Super and the arrows snap, maximize and minimize.",
            ),
        );
        (changed, y)
    }

    fn input(&mut self, cx: &mut Cx, a: &Area, y: i32, cfg: &mut Config) -> (bool, i32) {
        let t = cx.theme;
        let ru = cx.ru;
        let mut changed = false;
        let mut y = section(cx, a, y, tr(ru, "Указатель", "Pointer"));
        let names = [
            tr(ru, "Классический", "Classic"),
            tr(ru, "Клинок", "Blade"),
            tr(ru, "Капля", "Drop"),
        ];
        let card_w = ((a.w - 2 * a.s(16)) / 3).min(a.s(150));
        for (i, name) in names.iter().enumerate() {
            let card = Rect::new(a.x + i as i32 * (card_w + a.s(16)), y, card_w, a.s(104));
            let id = 400 + i as u32;
            let resp = hit(cx, a, id, card);
            let sel = cfg.pointer == i;
            let hov = cx.ui.anim(id, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
            let rad = a.s(cfg.round(12));
            cx.c.fill_rrect(card, rad, lerp(t.field, t.field_hover, hov));
            if sel {
                cx.c.stroke_rrect_fx(
                    card.inset(-a.s(3)),
                    rad + a.s(3),
                    a.s(2) as i64 * FX,
                    t.accent,
                );
            }
            let pct = config::POINTER_SIZES[cfg.pointer_size.min(3)] as i32;
            let size = (a.s(32) * pct / 100).min(a.s(56)) as usize;
            let (img, _, _) = cx.assets.pointers[i].at(size);
            let (w, h) = (img.width as i32, img.height as i32);
            cx.c.draw_image(
                img,
                card.center_x() - w / 2,
                card.y + a.s(10) + (a.s(58) - h) / 2,
                255,
            );
            let tw = cx.f.measure(Weight::Medium, cx.ui.px(12), name);
            cx.f.draw(
                cx.c,
                Weight::Medium,
                cx.ui.px(12),
                card.center_x() - tw / 2,
                card.bottom() - a.s(14),
                name,
                if sel { t.text } else { t.text_dim },
            );
            if resp.activated && !sel {
                cfg.pointer = i;
                changed = true;
            }
        }
        y += a.s(116);
        let sizes = ["100 %", "125 %", "150 %", "200 %"];
        let mut sel = cfg.pointer_size;
        if choice_row(
            cx,
            a,
            &mut y,
            410,
            tr(ru, "Размер", "Size"),
            &sizes,
            &mut sel,
            cfg,
        ) {
            cfg.pointer_size = sel;
            changed = true;
        }
        let mut v = cfg.pointer_speed as i32;
        let speeds = [
            tr(ru, "очень медленно", "very slow"),
            tr(ru, "медленно", "slow"),
            tr(ru, "обычно", "normal"),
            tr(ru, "быстро", "fast"),
            tr(ru, "очень быстро", "very fast"),
        ];
        let text = speeds[(v as usize).min(4)];
        if slider_row(
            cx,
            a,
            &mut y,
            415,
            tr(ru, "Скорость (мышь)", "Speed (mouse)"),
            text,
            cfg,
            &mut v,
            (0, 4, 1),
        ) {
            cfg.pointer_speed = v as usize;
            changed = true;
        }
        let mut v = cfg.natural_scroll;
        if toggle_row(
            cx,
            a,
            &mut y,
            418,
            tr(ru, "Естественная прокрутка", "Natural scrolling"),
            tr(
                ru,
                "Содержимое движется за колесом, как на сенсорном экране",
                "The content follows the wheel, as on a touch screen",
            ),
            cfg,
            &mut v,
        ) {
            cfg.natural_scroll = v;
            changed = true;
        }
        y += a.s(6);
        y = section(cx, a, y, tr(ru, "Клавиатура", "Keyboard"));
        let layouts: Vec<&str> = self
            .keyboard
            .0
            .split(',')
            .map(|l| match l.trim() {
                "ru" => tr(ru, "русская", "Russian"),
                "us" => tr(ru, "английская (США)", "English (US)"),
                other => other,
            })
            .collect();
        let sub = format!(
            "{}: {}",
            tr(ru, "переключение", "switch"),
            self.keyboard
                .1
                .replace("alt", "Alt")
                .replace("shift", "Shift")
        );
        let r = row(cx, a, y, &layouts.join(", "), &sub, cfg);
        y = r.bottom() + a.s(6);
        y = note(
            cx,
            a,
            y,
            tr(
                ru,
                "Раскладки задаёт установщик (/etc/velora/keyboard.conf). NOT IMPLEMENTED: смена раскладок отсюда — ядро пока знает только английскую и русскую с Alt+Shift.",
                "The installer sets the layouts (/etc/velora/keyboard.conf). NOT IMPLEMENTED: changing them here; the kernel knows only English and Russian with Alt+Shift.",
            ),
        );
        y = section(cx, a, y + a.s(4), tr(ru, "Горячие клавиши", "Shortcuts"));
        let keys: [(&str, &str, &str); 10] = [
            ("Super", "меню приложений", "app menu"),
            ("Alt+Tab", "переключить окно", "switch windows"),
            ("Alt+F4", "закрыть окно", "close the window"),
            ("Super+↑ ↓", "развернуть, свернуть", "maximize, minimize"),
            (
                "Super+← →",
                "к левому или правому краю",
                "to the left or right half",
            ),
            ("Ctrl+Alt+T", "терминал", "terminal"),
            ("Super+E", "файлы", "files"),
            ("Alt+Shift", "раскладка", "keyboard layout"),
            (
                "Ctrl+Shift+C, V",
                "копировать, вставить в терминале",
                "copy, paste in the terminal",
            ),
            ("Ctrl+Alt+D", "показать перерисовку", "show what is redrawn"),
        ];
        let p12 = cx.ui.px(12);
        for (k, rn, en) in keys {
            let r = Rect::new(a.x, y, a.w, a.s(30));
            let kw = cx.f.measure(Weight::Medium, p12, k) + a.s(16);
            let chip = Rect::new(r.x, r.y + a.s(3), kw, a.s(24));
            cx.c.fill_rrect(chip, a.s(cfg.round(6)), t.field);
            cx.f.draw_centered_y(
                cx.c,
                Weight::Medium,
                p12,
                chip.x + a.s(8),
                chip.center_y(),
                k,
                t.text,
            );
            cx.f.draw_centered_y(
                cx.c,
                Weight::Regular,
                p12,
                r.x + a.s(170),
                chip.center_y(),
                if ru { rn } else { en },
                t.text_dim,
            );
            y += a.s(32);
        }
        y = note(
            cx,
            a,
            y + a.s(4),
            tr(
                ru,
                "NOT IMPLEMENTED: свои сочетания клавиш.",
                "NOT IMPLEMENTED: shortcuts of one's own.",
            ),
        );
        (changed, y)
    }

    fn display(&mut self, cx: &mut Cx, a: &Area, y: i32, cfg: &mut Config) -> (bool, i32) {
        let ru = cx.ru;
        let mut changed = false;
        let (w, h, bpp) = cx.host.display_mode();
        let mut y = y;
        let r = row(
            cx,
            a,
            y,
            &format!("{} × {}", w, h),
            &format!(
                "{}, {} {}",
                tr(ru, "разрешение экрана", "screen resolution"),
                bpp,
                tr(ru, "бита на точку", "bits per pixel")
            ),
            cfg,
        );
        y = r.bottom() + a.s(6);
        y = note(
            cx,
            a,
            y,
            tr(
                ru,
                "Разрешение выбирает прошивка UEFI при загрузке. NOT IMPLEMENTED: смена разрешения из Lumi.",
                "The UEFI firmware chooses the resolution at boot. NOT IMPLEMENTED: changing it from Lumi.",
            ),
        );
        let auto = config::auto_scale(h as i32) * 100 / 256;
        let names: Vec<String> = UI_SCALES
            .iter()
            .map(|&s| {
                if s == 0 {
                    format!("{} ({} %)", tr(ru, "Авто", "Auto"), auto)
                } else {
                    format!("{} %", s)
                }
            })
            .collect();
        let items: Vec<&str> = names.iter().map(String::as_str).collect();
        let r = row(
            cx,
            a,
            y + a.s(6),
            tr(ru, "Масштаб интерфейса", "Interface scale"),
            tr(
                ru,
                "Окна, панель, текст и указатель",
                "Windows, the panel, text and the pointer",
            ),
            cfg,
        );
        let dd = Rect::new(r.right() - a.s(178), r.y + a.s(8), a.s(160), r.h - a.s(16));
        let mut sel = cfg.ui_scale.min(UI_SCALES.len() - 1);
        let rad = a.s(cfg.round(10));
        if in_view(cx, a, dd, |x| {
            x.dropdown(500, dd, rad, &items, &mut sel, None, 13)
        }) {
            cfg.ui_scale = sel;
            changed = true;
        }
        y = r.bottom() + a.s(8);
        (changed, y)
    }

    fn time(&mut self, cx: &mut Cx, a: &Area, y: i32, cfg: &mut Config) -> (bool, i32) {
        let t = cx.theme;
        let ru = cx.ru;
        let mut changed = false;
        let mut y = y;
        if let Some(utc) = cx.host.utc() {
            let l = tz::local(&cfg.region, utc);
            let big = if cfg.clock_24h {
                l.hhmm()
            } else {
                let h12 = if l.hour.is_multiple_of(12) {
                    12
                } else {
                    l.hour % 12
                };
                format!(
                    "{}:{:02} {}",
                    h12,
                    l.minute,
                    if l.hour < 12 { "AM" } else { "PM" }
                )
            };
            cx.f.draw(
                cx.c,
                Weight::SemiBold,
                cx.ui.px(36),
                a.x,
                y + a.s(34),
                &big,
                t.text,
            );
            let off = tz::offset_minutes(&cfg.region, utc);
            let sub = format!(
                "{}  ·  {}  ·  UTC{}{:02}:{:02}",
                l.date(ru),
                tz::zone_name(&cfg.region),
                if off < 0 { '-' } else { '+' },
                off.abs() / 60,
                off.abs() % 60
            );
            cx.f.draw(
                cx.c,
                Weight::Regular,
                cx.ui.px(12),
                a.x,
                y + a.s(58),
                &sub,
                t.text_dim,
            );
            y += a.s(74);
        }
        let mut sel = if cfg.clock_24h { 0 } else { 1 };
        if choice_row(
            cx,
            a,
            &mut y,
            600,
            tr(ru, "Формат времени", "Time format"),
            &[tr(ru, "24 часа", "24-hour"), tr(ru, "12 часов", "12-hour")],
            &mut sel,
            cfg,
        ) {
            cfg.clock_24h = sel == 0;
            changed = true;
        }
        let mut v = cfg.show_date;
        if toggle_row(
            cx,
            a,
            &mut y,
            603,
            tr(ru, "Дата на панели", "Date on the panel"),
            "",
            cfg,
            &mut v,
        ) {
            cfg.show_date = v;
            changed = true;
        }
        y = section(
            cx,
            a,
            y + a.s(4),
            tr(ru, "Регион и часовой пояс", "Region and time zone"),
        );
        let cols = 3;
        let cw = (a.w - a.s(12) * (cols - 1)) / cols;
        let mut last = y;
        for (i, (id, rn, en)) in tz::REGIONS.iter().enumerate() {
            let (c, rw) = (i as i32 % cols, i as i32 / cols);
            let cell = Rect::new(a.x + c * (cw + a.s(12)), y + rw * a.s(44), cw, a.s(36));
            last = cell.bottom();
            let uid = 620 + i as u32;
            let resp = hit(cx, a, uid, cell);
            let sel = cfg.region == *id;
            let hov = cx.ui.anim(uid, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
            let fill = if sel {
                lerp(t.field, t.accent, 60)
            } else {
                lerp(t.field, t.field_hover, hov)
            };
            cx.c.fill_rrect(cell, a.s(cfg.round(9)), fill);
            cx.f.draw_centered_y(
                cx.c,
                Weight::Medium,
                cx.ui.px(12),
                cell.x + a.s(14),
                cell.center_y(),
                if ru { rn } else { en },
                if sel { t.text } else { t.text_dim },
            );
            if resp.activated && !sel {
                cfg.region = String::from(*id);
                changed = true;
            }
        }
        (changed, last + a.s(8))
    }

    fn sound(&mut self, cx: &mut Cx, a: &Area, y: i32) -> i32 {
        let t = cx.theme;
        let ru = cx.ru;
        let Some(m) = cx.host.mixer().filter(|m| !m.devices.is_empty()) else {
            let card = Rect::new(a.x, y, a.w, a.s(120));
            cx.c.fill_rrect(card, a.s(cx.cfg.round(12)), t.field);
            icons::draw(
                cx.c,
                Icon::Speaker,
                card.x + a.s(24),
                card.y + a.s(26),
                a.s(32),
                t.text_dim,
                0,
            );
            cx.f.draw(
                cx.c,
                Weight::Medium,
                cx.ui.px(15),
                card.x + a.s(76),
                card.y + a.s(40),
                tr(ru, "Звуковых устройств нет", "No sound devices"),
                t.text,
            );
            let sub = tr(
                ru,
                "Velora OS поддерживает звуковые карты Intel HDA. Такой карты в этом компьютере нет.",
                "Velora OS supports Intel HDA sound cards. This computer has none.",
            );
            let inner = Area {
                x: card.x + a.s(76),
                w: card.w - a.s(96),
                ..*a
            };
            note(cx, &inner, card.y + a.s(52), sub);
            return card.bottom() + a.s(8);
        };
        let cfg = cx.cfg.clone();
        let mut y = section(cx, a, y, tr(ru, "Вывод", "Output"));
        let mut v = m.volume as i32;
        let text = format!("{} %", v);
        if slider_row(
            cx,
            a,
            &mut y,
            90,
            tr(ru, "Громкость", "Volume"),
            &text,
            &cfg,
            &mut v,
            (0, 100, 5),
        ) {
            cx.host.set_mixer(&format!(
                "volume {}
",
                v
            ));
            // A quiet click confirms the new volume, at most every 150 ms.
            if cx.now >= self.click_at {
                cx.host.play(&crate::sound::click());
                self.click_at = cx.now + 150;
            }
        }
        let mut mute = m.mute;
        if toggle_row(
            cx,
            a,
            &mut y,
            91,
            tr(ru, "Без звука", "Mute"),
            "",
            &cfg,
            &mut mute,
        ) {
            cx.host.set_mixer(&format!(
                "mute {}
",
                mute as u8
            ));
        }
        let names: Vec<&str> = m
            .devices
            .iter()
            .map(|d| {
                let (r, e) = d.name();
                tr(ru, r, e)
            })
            .collect();
        let mut sel = m.output;
        if choice_row(
            cx,
            a,
            &mut y,
            92,
            tr(ru, "Устройство вывода", "Output device"),
            &names,
            &mut sel,
            &cfg,
        ) {
            cx.host.set_mixer(&format!(
                "output {}
",
                sel
            ));
        }
        let r = row(
            cx,
            a,
            y,
            tr(ru, "Проверка звука", "Sound test"),
            tr(
                ru,
                "Короткая мелодия на выбранном устройстве",
                "A short tune on the chosen device",
            ),
            &cfg,
        );
        let bw = a.s(150);
        let b = Rect::new(r.right() - bw - a.s(12), r.y + a.s(10), bw, r.h - a.s(20));
        let pressed = in_view(cx, a, b, |x| {
            x.button(
                99,
                b,
                tr(ru, "Проверить", "Play"),
                ButtonKind::Tonal,
                Some(Icon::Speaker),
                None,
            )
        });
        if pressed {
            cx.host.play(&crate::sound::chime());
        }
        r.bottom() + a.s(8)
    }

    fn user(&mut self, cx: &mut Cx, a: &Area, y: i32) -> i32 {
        let t = cx.theme;
        let ru = cx.ru;
        let name = String::from(self.home.rsplit('/').next().unwrap_or("user"));
        let d = a.s(64);
        let (cx0, cy0) = (a.x + d / 2, y + d / 2 + a.s(4));
        cx.c.fill_circle(cx0, cy0, d / 2, lerp(t.field, t.accent, 90));
        let initial: String = name
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default();
        let p = cx.ui.px(26);
        let w = cx.f.measure(Weight::SemiBold, p, &initial);
        cx.f.draw_centered_y(
            cx.c,
            Weight::SemiBold,
            p,
            cx0 - w / 2,
            cy0,
            &initial,
            t.text,
        );
        cx.f.draw(
            cx.c,
            Weight::SemiBold,
            cx.ui.px(18),
            a.x + d + a.s(18),
            y + a.s(30),
            &name,
            t.text,
        );
        cx.f.draw(
            cx.c,
            Weight::Regular,
            cx.ui.px(12),
            a.x + d + a.s(18),
            y + a.s(52),
            &format!("{}: {}", tr(ru, "домашняя папка", "home"), self.home),
            t.text_dim,
        );
        let mut y = section(
            cx,
            a,
            y + d + a.s(24),
            tr(ru, "Смена пароля", "Change the password"),
        );
        let labels = [
            tr(ru, "Текущий пароль", "Current password"),
            tr(ru, "Новый пароль", "New password"),
            tr(ru, "Ещё раз", "Again"),
        ];
        let fw = a.w.min(a.s(420));
        for (i, label) in labels.iter().enumerate() {
            let f = Rect::new(a.x, y, fw, a.s(40));
            let text = match i {
                0 => &mut self.old_pw,
                1 => &mut self.new_pw,
                _ => &mut self.new_pw2,
            };
            in_view(cx, a, f, |x| {
                x.text_field(700 + i as u32, f, text, label, true, 64)
            });
            y += a.s(48);
        }
        let b = Rect::new(a.x, y + a.s(4), a.s(190), a.s(40));
        let go = in_view(cx, a, b, |x| {
            x.button(
                710,
                b,
                tr(ru, "Сменить пароль", "Change password"),
                ButtonKind::Primary,
                Some(Icon::Lock),
                None,
            )
        });
        if go {
            self.note = Some(if self.new_pw.is_empty() {
                (
                    false,
                    String::from(tr(ru, "Новый пароль пустой.", "The new password is empty.")),
                )
            } else if self.new_pw != self.new_pw2 {
                (
                    false,
                    String::from(tr(
                        ru,
                        "Новые пароли не совпадают.",
                        "The new passwords differ.",
                    )),
                )
            } else {
                match cx.host.change_password(&self.old_pw, &self.new_pw) {
                    Ok(()) => {
                        cx.host.log("lumi: password changed");
                        self.old_pw.clear();
                        self.new_pw.clear();
                        self.new_pw2.clear();
                        (
                            true,
                            String::from(tr(ru, "Пароль изменён.", "The password was changed.")),
                        )
                    }
                    Err(e) => {
                        let why = if e == "wrong password" {
                            String::from(tr(
                                ru,
                                "Текущий пароль неверный.",
                                "The current password is wrong.",
                            ))
                        } else {
                            format!("{}: {}", tr(ru, "Не удалось", "Failed"), e)
                        };
                        (false, why)
                    }
                }
            });
        }
        y = b.bottom() + a.s(10);
        if let Some((ok, text)) = self.note.clone() {
            let col = if ok { t.accent } else { t.error };
            cx.f.draw(
                cx.c,
                Weight::Medium,
                cx.ui.px(13),
                a.x,
                y + a.s(14),
                &text,
                col,
            );
            y += a.s(26);
        }
        y
    }

    fn about(&mut self, cx: &mut Cx, a: &Area, y: i32) -> i32 {
        let t = cx.theme;
        let ru = cx.ru;
        let i = self.info.clone();
        let mib = |b: u64| format!("{} MiB", (b + (1 << 19)) >> 20);
        let (w, h, _) = cx.host.display_mode();
        let rows = [
            (
                tr(ru, "Система", "System"),
                format!("Velora OS {}", i.version),
            ),
            (
                tr(ru, "Процессор", "Processor"),
                format!("{} × {}", i.cpus, i.cpu),
            ),
            (
                tr(ru, "Память", "Memory"),
                format!(
                    "{} {} {}",
                    mib(i.mem_used),
                    tr(ru, "из", "of"),
                    mib(i.mem_total)
                ),
            ),
            (tr(ru, "Экран", "Screen"), format!("{} × {}", w, h)),
            (tr(ru, "Оболочка", "Desktop"), String::from("Lumi")),
        ];
        let mut y = y;
        for (k, v) in rows {
            let r = Rect::new(a.x, y, a.w, a.s(40));
            cx.c.fill_rrect(r, a.s(cx.cfg.round(10)), t.field);
            cx.f.draw_centered_y(
                cx.c,
                Weight::Regular,
                cx.ui.px(12),
                r.x + a.s(16),
                r.center_y(),
                k,
                t.text_dim,
            );
            cx.f.draw_centered_y(
                cx.c,
                Weight::Medium,
                cx.ui.px(13),
                r.x + a.s(150),
                r.center_y(),
                &v,
                t.text,
            );
            y += a.s(46);
        }
        let b = Rect::new(a.x, y + a.s(6), a.s(200), a.s(40));
        if in_view(cx, a, b, |x| {
            x.button(
                800,
                b,
                tr(ru, "Открыть «О системе»", "Open About"),
                ButtonKind::Tonal,
                Some(Icon::Info),
                None,
            )
        }) {
            cx.out.push(Action::Launch(AppId::About));
        }
        let _ = rgb;
        b.bottom() + a.s(8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hues_go_round() {
        for h in (0..360).step_by(15) {
            let c = hue_color(h);
            let back = hue_of(c);
            assert!((back - h).abs() <= 2, "{} -> {:06x} -> {}", h, c, back);
        }
    }
}

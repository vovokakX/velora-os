//! Settings: theme, accent colour, transparency, rounded corners and
//! animations; the wallpaper; the pointer; the region for the clock.
//! Every change applies at once (with a transition) and is saved to the
//! user's desktop.conf.

use super::{Action, App, AppId, Cx, tr};
use crate::config::{ACCENTS, BUILTIN_WALLPAPERS, Config, THEMES};
use crate::host::{Host, join};
use crate::{tz, wallpaper};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use velora_ui::canvas::{Canvas, FX, fade, lerp};
use velora_ui::icons::{self, Icon};
use velora_ui::text::Weight;
use velora_ui::theme::{self, Theme};
use velora_ui::ui::Ctx;
use velora_ui::{Event, Rect};

/// Where pictures for the wallpaper are looked for.
pub const WALLPAPER_DIR: &str = "/usr/share/wallpapers";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Appearance,
    Wallpaper,
    Pointer,
    Time,
}

const PAGES: [Page; 4] = [Page::Appearance, Page::Wallpaper, Page::Pointer, Page::Time];

impl Page {
    fn name(self, ru: bool) -> &'static str {
        match self {
            Page::Appearance => tr(ru, "Оформление", "Appearance"),
            Page::Wallpaper => tr(ru, "Обои", "Wallpaper"),
            Page::Pointer => tr(ru, "Указатель", "Pointer"),
            Page::Time => tr(ru, "Дата и время", "Date and time"),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Page::Appearance => Icon::Sun,
            Page::Wallpaper => Icon::Picture,
            Page::Pointer => Icon::Pointer,
            Page::Time => Icon::Clock,
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
        Settings {
            page: Page::Appearance,
            walls,
            thumb: (0, 0),
            scroll: 0,
            home: String::from(home),
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
    c.fill_rect(
        Rect::new(
            win.x + win.w / 8,
            win.y + win.h / 3,
            win.w / 2,
            (r.h / 22).max(2),
        ),
        t.text_faint,
    );
    c.fill_rect(
        Rect::new(
            win.x + win.w / 8,
            win.y + win.h / 2,
            win.w / 3,
            (r.h / 22).max(2),
        ),
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

impl App for Settings {
    fn id(&self) -> AppId {
        AppId::Settings
    }

    fn title(&self, ru: bool) -> String {
        String::from(AppId::Settings.name(ru))
    }

    fn size(&self) -> ((i32, i32), (i32, i32)) {
        ((820, 600), (680, 520))
    }

    fn draw(&mut self, cx: &mut Cx, r: Rect) {
        let t = cx.theme;
        let ru = cx.ru;
        let sc = cx.ui.scale;
        let s = move |d: i32| ((d as i64 * sc + 128) >> 8) as i32;
        cx.c.fill_rect(r, t.window);
        let side = Rect::new(r.x, r.y, s(200), r.h);
        cx.c.fill_rect(side, t.sidebar);
        let mut y = r.y + s(16);
        for (i, p) in PAGES.iter().enumerate() {
            let row = Rect::new(side.x + s(10), y, side.w - s(20), s(38));
            let id = 10 + i as u32;
            let resp = cx.ui.interact(id, row);
            let here = self.page == *p;
            let hov = cx.ui.anim(id, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
            if here {
                cx.c.fill_rrect(row, s(9), lerp(t.selected, t.accent, 46));
            } else if hov > 0 {
                cx.c.fill_rrect(row, s(9), fade(t.selected, hov));
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
                self.page = *p;
                self.scroll = 0;
            }
            y += s(42);
        }
        let body = Rect::new(
            side.right() + s(32),
            r.y + s(28),
            r.right() - side.right() - s(64),
            r.h - s(40),
        );
        let title = self.page.name(ru);
        cx.f.draw(
            cx.c,
            Weight::SemiBold,
            cx.ui.px(22),
            body.x,
            body.y + s(20),
            title,
            t.text,
        );
        let content = Rect::new(body.x, body.y + s(48), body.w, body.h - s(48));
        let mut cfg = cx.cfg.clone();
        let changed = match self.page {
            Page::Appearance => self.appearance(cx, content, &mut cfg),
            Page::Wallpaper => self.wallpaper(cx, content, &mut cfg),
            Page::Pointer => self.pointer(cx, content, &mut cfg),
            Page::Time => self.time(cx, content, &mut cfg),
        };
        if changed && cfg != *cx.cfg {
            cx.out.push(Action::Config(cfg));
        }
    }
}

impl Settings {
    fn section(cx: &mut Cx, x: i32, y: i32, text: &str) {
        let t = cx.theme;
        cx.f.draw(cx.c, Weight::Medium, cx.ui.px(12), x, y, text, t.text_dim);
    }

    fn appearance(&mut self, cx: &mut Cx, r: Rect, cfg: &mut Config) -> bool {
        let t = cx.theme;
        let ru = cx.ru;
        let sc = cx.ui.scale;
        let s = move |d: i32| ((d as i64 * sc + 128) >> 8) as i32;
        let mut changed = false;
        Self::section(cx, r.x, r.y + s(12), tr(ru, "Тема", "Theme"));
        let gap = s(16);
        let cw = ((r.w - 2 * gap) / 3).min(s(180));
        let ch = cw * 10 / 16;
        let y = r.y + s(26);
        for (i, id) in THEMES.iter().enumerate() {
            let card = Rect::new(r.x + i as i32 * (cw + gap), y, cw, ch);
            let base = match i {
                1 => theme::LIGHT,
                2 => theme::GAMERS,
                _ => theme::DARK,
            };
            let preview = match cfg.accent {
                Some(a) => base.with_accent(ACCENTS[a].1),
                None => base.neutral(),
            };
            let resp = cx.ui.interact(20 + i as u32, card);
            let rad = if cfg.rounded_corners { s(12) } else { s(3) };
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
                cx.c.stroke_rrect_fx(card.inset(-s(3)), rad + s(3), s(2) as i64 * FX, ring);
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
                card.bottom() + s(22),
                name,
                if sel { t.text } else { t.text_dim },
            );
            if resp.activated && !sel {
                cfg.theme = i;
                // Each theme comes with its accent, as in the installer.
                cfg.accent = Some(if i == 2 { 7 } else { 0 });
                changed = true;
            }
        }
        let mut y = y + ch + s(56);
        Self::section(cx, r.x, y, tr(ru, "Акцентный цвет", "Accent colour"));
        y += s(16);
        let d = s(30);
        for i in 0..=ACCENTS.len() {
            let c = Rect::new(r.x + i as i32 * (d + s(12)), y, d, d);
            let id = 40 + i as u32;
            let resp = cx.ui.interact(id, c);
            let accent = if i < ACCENTS.len() { Some(i) } else { None };
            let col = match accent {
                Some(a) => 0xff00_0000 | ACCENTS[a].1,
                None => t.neutral().accent,
            };
            let sel = cfg.accent == accent;
            let grow = cx.ui.anim(id, 1, if resp.hovered { 255 } else { 0 }, 120);
            let rr = c.inset(-(s(2) * grow / 255));
            cx.c.fill_circle(rr.center_x(), rr.center_y(), rr.w / 2, col);
            if accent.is_none() {
                // "No accent": the theme's own neutral colour, crossed.
                cx.c.line(
                    c.x + s(7),
                    c.bottom() - s(7),
                    c.right() - s(7),
                    c.y + s(7),
                    FX * 2,
                    t.window,
                );
            }
            if sel {
                cx.c.stroke_circle_fx(
                    c.center_x() as i64 * FX,
                    c.center_y() as i64 * FX,
                    (d / 2 + s(4)) as i64 * FX,
                    s(2) as i64 * FX,
                    col,
                );
            }
            if resp.activated && !sel {
                cfg.accent = accent;
                changed = true;
            }
        }
        y += d + s(34);
        let rows: [(&str, &str, u8); 3] = [
            (
                tr(ru, "Прозрачность", "Transparency"),
                tr(
                    ru,
                    "Панель и меню пропускают обои",
                    "The panel and menus let the wallpaper show",
                ),
                0,
            ),
            (
                tr(ru, "Скруглённые углы", "Rounded corners"),
                tr(
                    ru,
                    "Окна, панель и кнопки",
                    "Windows, the panel and buttons",
                ),
                1,
            ),
            (
                tr(ru, "Анимации", "Animations"),
                tr(
                    ru,
                    "Плавное появление окон и переходы",
                    "Windows open and change smoothly",
                ),
                2,
            ),
        ];
        for (title, sub, k) in rows {
            let row = Rect::new(r.x, y, r.w, s(50));
            cx.c.fill_rrect(row, if cfg.rounded_corners { s(12) } else { s(3) }, t.field);
            cx.f.draw(
                cx.c,
                Weight::Medium,
                cx.ui.px(13),
                row.x + s(18),
                row.y + s(22),
                title,
                t.text,
            );
            cx.f.draw(
                cx.c,
                Weight::Regular,
                cx.ui.px(11),
                row.x + s(18),
                row.y + s(39),
                sub,
                t.text_faint,
            );
            let tg = Rect::new(row.right() - s(62), row.center_y() - s(12), s(44), s(24));
            let mut v = match k {
                0 => cfg.transparency,
                1 => cfg.rounded_corners,
                _ => cfg.animations,
            };
            let mut ctx = Ctx {
                ui: cx.ui,
                c: cx.c,
                f: cx.f,
            };
            if ctx.toggle(60 + k as u32, tg, &mut v) {
                match k {
                    0 => cfg.transparency = v,
                    1 => cfg.rounded_corners = v,
                    _ => cfg.animations = v,
                }
                changed = true;
            }
            y += s(58);
        }
        changed
    }

    fn wallpaper(&mut self, cx: &mut Cx, r: Rect, cfg: &mut Config) -> bool {
        let t = cx.theme;
        let sc = cx.ui.scale;
        let s = move |d: i32| ((d as i64 * sc + 128) >> 8) as i32;
        let gap = s(16);
        let cols = 3;
        let tw = (r.w - gap * (cols - 1)) / cols;
        let th = tw * 9 / 16;
        self.thumbs(cx.host, tw as usize, th as usize);
        let rows_n = (self.walls.len() as i32 + cols - 1) / cols;
        let row_h = th + s(40);
        let max_scroll = (rows_n * row_h - r.h).max(0);
        if r.contains(cx.ui.mouse.0, cx.ui.mouse.1) {
            for ev in cx.events {
                if let Event::Wheel(d) = *ev {
                    self.scroll = (self.scroll + d * s(40)).clamp(0, max_scroll);
                }
            }
        }
        let old = cx.c.set_clip(r.inset(-s(6)));
        let mut changed = false;
        let rad = if cfg.rounded_corners { s(12) } else { s(3) };
        for (i, (id, pic)) in self.walls.iter().enumerate() {
            let (col, row) = (i as i32 % cols, i as i32 / cols);
            let cell = Rect::new(
                r.x + col * (tw + gap),
                r.y + row * row_h - self.scroll + s(4),
                tw,
                th,
            );
            let uid = 100 + i as u32;
            let resp = cx.ui.interact(uid, cell);
            if let Some(p) = pic {
                let corner = crate::gfx::Corner::new(rad);
                crate::gfx::blit(cx.c, p, cell.x, cell.y, &corner, 255);
            }
            let sel = cfg.wallpaper == *id;
            let hov = cx.ui.anim(uid, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
            if sel {
                cx.c.stroke_rrect_fx(cell.inset(-s(3)), rad + s(3), s(2) as i64 * FX, t.accent);
            } else if hov > 0 {
                cx.c.stroke_rrect_fx(
                    cell.inset(-s(3)),
                    rad + s(3),
                    s(2) as i64 * FX,
                    fade(t.text_faint, hov),
                );
            }
            let name = match id.as_str() {
                "waves" => String::from(tr(cx.ru, "Волны", "Waves")),
                "graphite" => String::from(tr(cx.ru, "Графит", "Graphite")),
                "aurora" => String::from(tr(cx.ru, "Сияние", "Aurora")),
                "ember" => String::from(tr(cx.ru, "Угли", "Ember")),
                other => String::from(other.rsplit('/').next().unwrap_or(other)),
            };
            cx.f.draw(
                cx.c,
                Weight::Medium,
                cx.ui.px(12),
                cell.x,
                cell.bottom() + s(22),
                &name,
                if sel { t.text } else { t.text_dim },
            );
            if resp.activated && !sel {
                cfg.wallpaper = id.clone();
                changed = true;
            }
        }
        cx.c.set_clip(old);
        let note = if cx.ru {
            format!(
                "Свои картинки PNG: {} или {}/Pictures",
                WALLPAPER_DIR, self.home
            )
        } else {
            format!(
                "Your own PNG pictures: {} or {}/Pictures",
                WALLPAPER_DIR, self.home
            )
        };
        let _ = note;
        changed
    }

    fn pointer(&mut self, cx: &mut Cx, r: Rect, cfg: &mut Config) -> bool {
        let t = cx.theme;
        let ru = cx.ru;
        let sc = cx.ui.scale;
        let s = move |d: i32| ((d as i64 * sc + 128) >> 8) as i32;
        let mut changed = false;
        Self::section(cx, r.x, r.y + s(12), tr(ru, "Стиль", "Style"));
        let names = [
            tr(ru, "Классический", "Classic"),
            tr(ru, "Клинок", "Blade"),
            tr(ru, "Капля", "Drop"),
        ];
        let card_w = s(150);
        for (i, name) in names.iter().enumerate() {
            let card = Rect::new(
                r.x + i as i32 * (card_w + s(16)),
                r.y + s(26),
                card_w,
                s(110),
            );
            let id = 80 + i as u32;
            let resp = cx.ui.interact(id, card);
            let sel = cfg.pointer == i;
            let hov = cx.ui.anim(id, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
            let rad = if cfg.rounded_corners { s(12) } else { s(3) };
            cx.c.fill_rrect(card, rad, lerp(t.field, t.field_hover, hov));
            if sel {
                cx.c.stroke_rrect_fx(card.inset(-s(3)), rad + s(3), s(2) as i64 * FX, t.accent);
            }
            let size = s(if cfg.large_pointer { 48 } else { 34 }) as usize;
            let (img, _, _) = cx.assets.pointers[i].at(size);
            let (w, h) = (img.width as i32, img.height as i32);
            cx.c.draw_image(
                img,
                card.center_x() - w / 2,
                card.y + s(14) + (s(56) - h) / 2,
                255,
            );
            let tw = cx.f.measure(Weight::Medium, cx.ui.px(12), name);
            cx.f.draw(
                cx.c,
                Weight::Medium,
                cx.ui.px(12),
                card.center_x() - tw / 2,
                card.bottom() - s(14),
                name,
                if sel { t.text } else { t.text_dim },
            );
            if resp.activated && !sel {
                cfg.pointer = i;
                changed = true;
            }
        }
        let row = Rect::new(r.x, r.y + s(160), r.w, s(50));
        cx.c.fill_rrect(row, if cfg.rounded_corners { s(12) } else { s(3) }, t.field);
        cx.f.draw(
            cx.c,
            Weight::Medium,
            cx.ui.px(13),
            row.x + s(18),
            row.y + s(22),
            tr(ru, "Крупный указатель", "Large pointer"),
            t.text,
        );
        cx.f.draw(
            cx.c,
            Weight::Regular,
            cx.ui.px(11),
            row.x + s(18),
            row.y + s(39),
            tr(ru, "В полтора раза больше", "One and a half times the size"),
            t.text_faint,
        );
        let tg = Rect::new(row.right() - s(62), row.center_y() - s(12), s(44), s(24));
        let mut v = cfg.large_pointer;
        let mut ctx = Ctx {
            ui: cx.ui,
            c: cx.c,
            f: cx.f,
        };
        if ctx.toggle(90, tg, &mut v) {
            cfg.large_pointer = v;
            changed = true;
        }
        changed
    }

    fn time(&mut self, cx: &mut Cx, r: Rect, cfg: &mut Config) -> bool {
        let t = cx.theme;
        let ru = cx.ru;
        let sc = cx.ui.scale;
        let s = move |d: i32| ((d as i64 * sc + 128) >> 8) as i32;
        let mut changed = false;
        if let Some(utc) = cx.host.utc() {
            let l = tz::local(&cfg.region, utc);
            let big = l.hhmm();
            cx.f.draw(
                cx.c,
                Weight::SemiBold,
                cx.ui.px(36),
                r.x,
                r.y + s(36),
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
                r.x,
                r.y + s(60),
                &sub,
                t.text_dim,
            );
        }
        Self::section(cx, r.x, r.y + s(98), tr(ru, "Регион", "Region"));
        let cols = 3;
        let cw = (r.w - s(12) * (cols - 1)) / cols;
        for (i, (id, rn, en)) in tz::REGIONS.iter().enumerate() {
            let (c, rw) = (i as i32 % cols, i as i32 / cols);
            let cell = Rect::new(r.x + c * (cw + s(12)), r.y + s(112) + rw * s(44), cw, s(36));
            let uid = 120 + i as u32;
            let resp = cx.ui.interact(uid, cell);
            let sel = cfg.region == *id;
            let hov = cx.ui.anim(uid, 1, if resp.hovered { 255 } else { 0 }, 120) as u8;
            let rad = if cfg.rounded_corners { s(9) } else { s(3) };
            let fill = if sel {
                lerp(t.field, t.accent, 60)
            } else {
                lerp(t.field, t.field_hover, hov)
            };
            cx.c.fill_rrect(cell, rad, fill);
            let name = if ru { *rn } else { *en };
            cx.f.draw_centered_y(
                cx.c,
                Weight::Medium,
                cx.ui.px(12),
                cell.x + s(14),
                cell.center_y(),
                name,
                if sel { t.text } else { t.text_dim },
            );
            if resp.activated && !sel {
                cfg.region = String::from(*id);
                changed = true;
            }
        }
        changed
    }
}

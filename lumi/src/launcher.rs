//! The app menu: over the whole screen, the wallpaper blurred and
//! darkened, a search line at the top and the apps in a grid. Typing
//! searches at once, Enter starts the first app found (or the selected
//! one), Escape or a click outside closes it. It opens by growing and
//! fading in.

use crate::apps::{self, AppId, MENU, tr};
use crate::desktop::Desktop;
use crate::host::Host;
use alloc::string::String;
use alloc::vec::Vec;
use velora_ui::anim;
use velora_ui::canvas::{Canvas, FX, fade, lerp};
use velora_ui::icons::{self, Icon};
use velora_ui::text::{Fonts, Weight};
use velora_ui::ui::Key;
use velora_ui::{Event, Rect};

const HOVER: u64 = 0x800;

pub(crate) struct Launcher {
    pub(crate) t0: u64,
    pub(crate) closing: bool,
    pub(crate) query: String,
    pub(crate) sel: usize,
    pub(crate) hover: Option<usize>,
    /// A press started on this tile (it starts on release there).
    pub(crate) press: Option<usize>,
}

impl Desktop {
    pub(crate) fn toggle_launcher(&mut self) {
        match self.launcher.as_mut() {
            Some(l) if !l.closing => {
                l.closing = true;
                l.t0 = self.now;
            }
            _ => {
                self.close_popover();
                self.cancel_switcher();
                self.launcher = Some(Launcher {
                    t0: self.now,
                    closing: false,
                    query: String::new(),
                    sel: 0,
                    hover: None,
                    press: None,
                });
                self.host_log_pending = Some("lumi: app menu");
            }
        }
        self.damage_all();
    }

    /// 0 (closed) to 1000 (open).
    pub(crate) fn launcher_progress(&self) -> u32 {
        let Some(l) = self.launcher.as_ref() else {
            return 0;
        };
        let d = if l.closing {
            self.dur(170)
        } else {
            self.dur(260)
        } as u64;
        let el = self.now.saturating_sub(l.t0);
        let p = if d == 0 || el >= d {
            1000
        } else if l.closing {
            anim::ease_in_out((el * 1000 / d) as u32)
        } else {
            anim::ease_out((el * 1000 / d) as u32)
        };
        if l.closing { 1000 - p } else { p }
    }

    pub(crate) fn launcher_animating(&self) -> bool {
        self.launcher.as_ref().is_some_and(|l| {
            let d = if l.closing {
                self.dur(170)
            } else {
                self.dur(260)
            } as u64;
            self.now.saturating_sub(l.t0) <= d
        })
    }

    pub(crate) fn launcher_wake(&self) -> Option<u64> {
        None
    }

    pub(crate) fn tick_launcher(&mut self, _now: u64) {
        if self.launcher_animating() {
            self.damage_all();
        } else if self.launcher.as_ref().is_some_and(|l| l.closing) {
            self.launcher = None;
            self.damage_all();
        }
        if self.anim_prev && self.launcher.is_some() {
            self.damage_all();
        }
    }

    fn found(&self) -> Vec<AppId> {
        let q = self.launcher.as_ref().map_or("", |l| l.query.as_str());
        MENU.iter().copied().filter(|a| a.matches(q)).collect()
    }

    fn search_rect(&self) -> Rect {
        let w = self.s(460).min(self.w - self.s(40));
        Rect::new((self.w - w) / 2, self.h * 11 / 100, w, self.s(48))
    }

    fn tile_cells(&self) -> Vec<(AppId, Rect)> {
        let apps = self.found();
        let cols = apps.len().clamp(1, 6) as i32;
        let (cw, ch) = (self.s(140), self.s(140));
        let x0 = (self.w - cols * cw) / 2;
        let y0 = self.search_rect().bottom() + self.s(70);
        apps.into_iter()
            .enumerate()
            .map(|(i, a)| {
                let (c, r) = (i as i32 % cols, i as i32 / cols);
                (a, Rect::new(x0 + c * cw, y0 + r * ch, cw, ch))
            })
            .collect()
    }

    fn launch_from_menu(&mut self, app: AppId, host: &mut dyn Host, fonts: &mut Fonts) {
        if let Some(l) = self.launcher.as_mut() {
            l.closing = true;
            l.t0 = self.now;
        }
        self.damage_all();
        self.launch(app, host, fonts);
    }

    pub(crate) fn launcher_input(&mut self, ev: &Event, host: &mut dyn Host, fonts: &mut Fonts) {
        let cells = self.tile_cells();
        let n = cells.len();
        let closing = self.launcher.as_ref().is_some_and(|l| l.closing);
        if closing {
            return;
        }
        match *ev {
            Event::Key { key, .. } => {
                let l = self.launcher.as_mut().unwrap();
                match key {
                    Key::Escape => {
                        self.toggle_launcher();
                        return;
                    }
                    Key::Char(c) => {
                        l.query.push(c);
                        l.sel = 0;
                    }
                    Key::Space => {
                        l.query.push(' ');
                        l.sel = 0;
                    }
                    Key::Backspace => {
                        l.query.pop();
                        l.sel = 0;
                    }
                    Key::Right | Key::Tab => l.sel = (l.sel + 1).min(n.saturating_sub(1)),
                    Key::Left => l.sel = l.sel.saturating_sub(1),
                    Key::Down => l.sel = (l.sel + 6).min(n.saturating_sub(1)),
                    Key::Up => l.sel = l.sel.saturating_sub(6),
                    Key::Enter => {
                        let sel = l.sel;
                        if let Some(&(a, _)) = cells.get(sel).or(cells.first()) {
                            self.launch_from_menu(a, host, fonts);
                        }
                        return;
                    }
                    _ => {}
                }
                self.damage_all();
            }
            Event::MouseMove { x, y } => {
                self.mouse = (x, y);
                let h = cells.iter().position(|(_, r)| r.contains(x, y));
                let l = self.launcher.as_mut().unwrap();
                if h != l.hover {
                    l.hover = h;
                    if let Some(i) = h {
                        l.sel = i;
                    }
                    self.damage_all();
                }
            }
            Event::MouseButton { down } => {
                let (x, y) = self.mouse;
                let h = cells.iter().position(|(_, r)| r.contains(x, y));
                let l = self.launcher.as_mut().unwrap();
                if down {
                    l.press = h;
                    if h.is_none() && !self.search_rect().contains(x, y) {
                        self.toggle_launcher();
                    }
                } else if let Some(i) = l.press.take()
                    && h == Some(i)
                {
                    let a = cells[i].0;
                    self.launch_from_menu(a, host, fonts);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn draw_launcher(&mut self, c: &mut Canvas, fonts: &mut Fonts) {
        if self.launcher.is_none() {
            return;
        }
        let p = self.launcher_progress();
        if p == 0 {
            return;
        }
        let area = c.frame();
        let t = self.theme;
        let a = p * 255 / 1000;
        self.frost.mix_to(c, area, a);
        let dim = if t.dark { 0x30000000u32 } else { 0x20000000 };
        c.fill_rect(area, fade(dim, (((dim >> 24) * a) / 255) as u8));
        let ru = self.ru();
        let al = |col: u32| fade(col, (((col >> 24) * a) / 255) as u8);
        // Everything grows from 94% around the middle of the screen.
        let k = 940 + 60 * p / 1000;
        let (mx, my) = (self.w / 2, self.h * 45 / 100);
        let sc = |r: Rect| {
            let x = mx + ((r.x - mx) as i64 * k as i64 / 1000) as i32;
            let y = my + ((r.y - my) as i64 * k as i64 / 1000) as i32;
            Rect::new(
                x,
                y,
                (r.w as i64 * k as i64 / 1000) as i32,
                (r.h as i64 * k as i64 / 1000) as i32,
            )
        };
        // The search line.
        let (query, sel, hover) = {
            let l = self.launcher.as_ref().unwrap();
            (l.query.clone(), l.sel, l.hover)
        };
        let sr = sc(self.search_rect());
        let rad = if self.cfg.rounded_corners {
            sr.h / 2
        } else {
            self.s(6)
        };
        c.fill_rrect(sr, rad, al(if t.dark { 0x30ffffff } else { 0xb0ffffff }));
        c.stroke_rrect_fx(
            sr,
            rad,
            FX,
            al(if t.dark { 0x38ffffff } else { 0x30000000 }),
        );
        let isz = self.s(20);
        icons::draw(
            c,
            Icon::Search,
            sr.x + self.s(18),
            sr.center_y() - isz / 2,
            isz,
            al(t.text_dim),
            0,
        );
        let px = self.px(16);
        let tx = sr.x + self.s(50);
        if query.is_empty() {
            let ph = tr(ru, "Поиск приложений", "Search apps");
            fonts.draw_centered_y(
                c,
                Weight::Regular,
                px,
                tx,
                sr.center_y(),
                ph,
                al(t.text_faint),
            );
        } else {
            let w =
                fonts.draw_centered_y(c, Weight::Medium, px, tx, sr.center_y(), &query, al(t.text));
            let caret = Rect::new(
                tx + w + self.s(2),
                sr.center_y() - self.s(11),
                self.s(2),
                self.s(22),
            );
            c.fill_rect(caret, al(t.detail));
        }
        if query.is_empty() {
            let caret = Rect::new(
                tx - self.s(4),
                sr.center_y() - self.s(11),
                self.s(2),
                self.s(22),
            );
            c.fill_rect(caret, al(fade(t.detail, 200)));
        }
        // The apps.
        let cells = self.tile_cells();
        if cells.is_empty() {
            let msg = tr(ru, "Ничего не найдено", "Nothing found");
            let w = fonts.measure(Weight::Medium, self.px(15), msg);
            fonts.draw(
                c,
                Weight::Medium,
                self.px(15),
                (self.w - w) / 2,
                sr.bottom() + self.s(110),
                msg,
                al(t.text_dim),
            );
        }
        for (i, (app, cell)) in cells.iter().enumerate() {
            let cell = sc(*cell);
            let on = sel == i || hover == Some(i);
            let h = self
                .tw
                .value(HOVER + i as u64, if on { 255 } else { 0 }, anim::FAST)
                as u32;
            let box_ = cell.inset(self.s(6));
            if h > 0 {
                let r = if self.cfg.rounded_corners {
                    self.s(20)
                } else {
                    self.s(4)
                };
                c.fill_rrect(
                    box_,
                    r,
                    fade(
                        if t.dark { 0x26ffffff } else { 0x60ffffff },
                        (h * a / 255 * ((if t.dark { 0x26 } else { 0x60 }) as u32) / 255) as u8,
                    ),
                );
            }
            let tile = (self.s(76) as i64 * k as i64 / 1000) as i32;
            let lift = (h as i32) * self.s(3) / 255;
            let tr_ = Rect::new(
                cell.center_x() - tile / 2,
                cell.y + self.s(16) - lift,
                tile,
                tile,
            );
            apps::draw_tile(c, *app, tr_, self.cfg.rounded_corners, a);
            let name = app.name(ru);
            let npx = self.px(14);
            let w = fonts.measure(Weight::Medium, npx, name);
            let col = lerp(t.text_dim, t.text, h as u8);
            fonts.draw(
                c,
                Weight::Medium,
                npx,
                cell.center_x() - w / 2,
                tr_.bottom() + self.s(28),
                name,
                al(col),
            );
        }
    }
}

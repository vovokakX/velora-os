//! The panel: a floating rounded bar at the bottom middle with the app
//! menu button, the pinned and running apps, the volume, the network and
//! the clock; their tooltips and small windows; the Alt+Tab switcher.

use crate::apps::{self, AppId, PINNED, tr};
use crate::desktop::{Desktop, Exit, PopKind, Popover, Switcher};
use crate::gfx::Corner;
use crate::host::{Host, Power};
use crate::tz;
use alloc::string::String;
use alloc::vec::Vec;
use velora_ui::Rect;
use velora_ui::anim;
use velora_ui::canvas::{Canvas, FX, fade, lerp};
use velora_ui::icons::{self, Icon};
use velora_ui::text::{Fonts, Weight};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    Menu,
    Sep,
    App(AppId),
    Volume,
    Network,
    Clock,
}

/// Tween keys.
const HOVER: u64 = 0x100;
const TIP: u64 = 0x400;
const POP: u64 = 0x500;
const POP_HOVER: u64 = 0x600;
const SWITCH: u64 = 0x700;

/// Actions in the clock's small window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopAction {
    Settings,
    Logout,
    Restart,
    PowerOff,
}

impl Desktop {
    pub(crate) fn panel_items(&self) -> Vec<Item> {
        let mut v = alloc::vec![Item::Menu, Item::Sep];
        for a in PINNED {
            v.push(Item::App(a));
        }
        for w in &self.windows {
            let id = w.app.id();
            if !w.closing && !v.contains(&Item::App(id)) {
                v.push(Item::App(id));
            }
        }
        v.push(Item::Sep);
        v.push(Item::Volume);
        v.push(Item::Network);
        v.push(Item::Clock);
        v
    }

    fn item_w(&self, it: Item) -> i32 {
        match it {
            Item::Sep => self.s(17),
            Item::Clock => self.s(62),
            _ => self.s(42),
        }
    }

    pub fn panel_rect(&self) -> Rect {
        let items = self.panel_items();
        let pad = self.s(7);
        let w: i32 = items.iter().map(|&i| self.item_w(i)).sum::<i32>() + 2 * pad;
        let h = self.s(54);
        Rect::new((self.w - w) / 2, self.h - h - self.s(10), w, h)
    }

    fn panel_layout(&self) -> Vec<(Item, Rect)> {
        let pr = self.panel_rect();
        let mut x = pr.x + self.s(7);
        let size = self.s(42);
        let y = pr.y + (pr.h - size) / 2;
        self.panel_items()
            .into_iter()
            .map(|it| {
                let w = self.item_w(it);
                let r = Rect::new(x, y, w, size);
                x += w;
                (it, r)
            })
            .collect()
    }

    pub(crate) fn panel_item_at(&self, x: i32, y: i32) -> Option<usize> {
        self.panel_layout()
            .iter()
            .position(|(it, r)| *it != Item::Sep && r.contains(x, y))
    }

    /// The panel changed (windows opened or closed, focus).
    pub(crate) fn panel_changed(&mut self) {
        // The panel can change its width: the old and new places.
        let r = self.panel_rect();
        let wide = Rect::new(
            self.s(0),
            r.y - self.s(70),
            self.w,
            self.h - r.y + self.s(70),
        );
        self.add_damage(wide);
    }

    pub(crate) fn panel_icon_rect_for(&self, win: u32) -> Option<Rect> {
        let app = self.windows.iter().find(|w| w.id == win)?.app.id();
        self.panel_layout()
            .into_iter()
            .find(|(it, _)| *it == Item::App(app))
            .map(|(_, r)| r)
    }

    pub(crate) fn panel_hover_at(&mut self, x: i32, y: i32) {
        let h = if self.panel_rect().contains(x, y) && self.drag_none() {
            self.panel_item_at(x, y)
        } else {
            None
        };
        if h != self.panel_hover {
            if h.is_some() && self.panel_hover.is_none() {
                self.panel_hover_t = self.now;
            } else if h.is_some() {
                // Moving along the panel keeps the tooltip.
                if self.now.saturating_sub(self.panel_hover_t) < 450 {
                    self.panel_hover_t = self.now;
                }
            }
            if h.is_none() {
                self.tw.set(TIP, 0);
            }
            self.panel_hover = h;
            let pr = self.panel_rect();
            self.add_damage(Rect::new(
                pr.x - self.s(120),
                pr.y - self.s(70),
                pr.w + self.s(240),
                pr.h + self.s(80),
            ));
        }
    }

    fn drag_none(&self) -> bool {
        self.pointer == crate::desktop::Pointer::Arrow
    }

    pub(crate) fn panel_click(&mut self, i: usize, host: &mut dyn Host, fonts: &mut Fonts) {
        let Some(&(it, _)) = self.panel_layout().get(i) else {
            return;
        };
        self.tw.set(TIP, 0);
        self.panel_hover_t = self.now + 100_000;
        match it {
            Item::Menu => self.toggle_launcher(),
            Item::App(a) => {
                let wins: Vec<(u32, bool)> = self
                    .windows
                    .iter()
                    .filter(|w| w.app.id() == a && !w.closing)
                    .map(|w| (w.id, w.minimized))
                    .collect();
                match wins.last() {
                    None => self.launch(a, host, fonts),
                    Some(&(id, minimized)) => {
                        if minimized {
                            self.unminimize(id);
                        } else if self.focus == Some(id) {
                            self.minimize(id);
                        } else {
                            self.raise(id);
                        }
                    }
                }
            }
            Item::Clock => self.toggle_popover(PopKind::Clock),
            Item::Network => self.toggle_popover(PopKind::Network),
            Item::Volume => self.toggle_popover(PopKind::Volume),
            Item::Sep => {}
        }
    }

    /// The panel's appearance at the start of a session: 0..=1000.
    fn panel_appear(&self) -> u32 {
        let d = self.dur(520) as u64;
        let el = self.now.saturating_sub(self.session_t0 + 120);
        if d == 0 || el >= d {
            1000
        } else {
            anim::ease_out((el * 1000 / d) as u32)
        }
    }

    pub(crate) fn draw_panel(&mut self, c: &mut Canvas, fonts: &mut Fonts) {
        let pr = self.panel_rect();
        let appear = self.panel_appear();
        let dy = (self.s(70) as i64 * (1000 - appear) as i64 / 1000) as i32;
        let alpha = appear * 255 / 1000;
        let r = pr.offset(0, dy);
        if r.inset(-self.s(30)).intersect(c.frame()).is_empty() {
            return;
        }
        let t = self.theme;
        let rad = if self.cfg.rounded_corners {
            self.s(18)
        } else {
            self.s(4)
        };
        self.shadow_small.draw(
            c,
            r.offset(0, self.s(5)),
            rad.min(self.s(16)),
            (0x60 * alpha / 255) << 24,
            r,
        );
        let corner = Corner::new(rad);
        if self.cfg.transparency {
            self.frost.rounded_to(c, r, &corner, alpha);
            c.fill_rrect(r, rad, fade(t.window, (150 * alpha / 255) as u8));
        } else {
            c.fill_rrect(r, rad, fade(t.window, alpha as u8));
        }
        c.stroke_rrect_fx(r, rad, FX, fade(t.window_border, (255 * alpha / 255) as u8));
        let focus_app = self
            .focus
            .and_then(|f| self.windows.iter().find(|w| w.id == f && !w.minimized))
            .map(|w| w.app.id());
        let layout = self.panel_layout();
        for (i, (it, ir)) in layout.iter().enumerate() {
            let ir = ir.offset(0, dy);
            let hov = self.tw.value(
                HOVER + i as u64,
                if self.panel_hover == Some(i) { 255 } else { 0 },
                anim::FAST,
            ) as u32;
            let pressed = self.panel_press == Some(i);
            let br = ir.inset(self.s(3));
            let brad = if self.cfg.rounded_corners {
                self.s(11)
            } else {
                self.s(3)
            };
            let active = matches!(it, Item::App(a) if Some(*a) == focus_app);
            if active {
                c.fill_rrect(
                    br,
                    brad,
                    fade(lerp(t.field_hover, t.accent, 40), (alpha * 235 / 255) as u8),
                );
            }
            if hov > 0 && *it != Item::Sep {
                let base = if pressed {
                    t.field_pressed
                } else {
                    t.field_hover
                };
                c.fill_rrect(br, brad, fade(base, (hov * alpha / 255 * 200 / 255) as u8));
            }
            let col = fade(t.text, alpha as u8);
            let isz = self.s(21);
            let ix = ir.center_x() - isz / 2;
            let iy = ir.center_y() - isz / 2;
            match *it {
                Item::Sep => {
                    let h = self.s(20);
                    c.fill_rect(
                        Rect::new(ir.center_x(), ir.center_y() - h / 2, 1, h),
                        fade(t.text_faint, (alpha * 160 / 255) as u8),
                    );
                }
                Item::Menu => icons::draw(c, Icon::Apps, ix, iy, isz, col, 0),
                Item::App(a) => {
                    icons::draw(c, a.icon(), ix, iy, isz, col, 0);
                    let running = self
                        .windows
                        .iter()
                        .filter(|w| w.app.id() == a && !w.closing)
                        .count();
                    if running > 0 {
                        let dw = if active { self.s(12) } else { self.s(4) };
                        let dot = Rect::new(
                            ir.center_x() - dw / 2,
                            ir.bottom() - self.s(5),
                            dw,
                            self.s(3),
                        );
                        c.fill_rrect(
                            dot,
                            dot.h / 2,
                            fade(if active { t.accent } else { t.text_dim }, alpha as u8),
                        );
                    }
                }
                Item::Volume => icons::draw(
                    c,
                    Icon::Speaker,
                    ix + self.s(1),
                    iy + self.s(1),
                    isz - self.s(2),
                    fade(t.text_dim, alpha as u8),
                    0,
                ),
                Item::Network => {
                    let on = self.net.ip.is_some();
                    let ncol = if on { t.text_dim } else { t.text_faint };
                    icons::draw(
                        c,
                        Icon::Network,
                        ix + self.s(1),
                        iy + self.s(1),
                        isz - self.s(2),
                        fade(ncol, alpha as u8),
                        0,
                    );
                    if !on {
                        c.line(
                            ix + self.s(4),
                            iy + self.s(4),
                            ix + isz - self.s(4),
                            iy + isz - self.s(4),
                            FX * 3 / 2,
                            fade(t.text_dim, alpha as u8),
                        );
                    }
                }
                Item::Clock => {
                    let txt = self.clock_text();
                    let px = self.px(14);
                    let w = fonts.measure(Weight::Medium, px, &txt);
                    fonts.draw_centered_y(
                        c,
                        Weight::Medium,
                        px,
                        ir.center_x() - w / 2,
                        ir.center_y(),
                        &txt,
                        col,
                    );
                }
            }
        }
    }

    pub(crate) fn clock_text(&self) -> String {
        if self.minute < 0 {
            return String::from("--:--");
        }
        let m = self.minute.rem_euclid(24 * 60);
        alloc::format!("{:02}:{:02}", m / 60, m % 60)
    }

    fn local_now(&self) -> Option<tz::Local> {
        if self.minute < 0 {
            return None;
        }
        // `minute` is already local; rebuild the date from it.
        let t = self.minute * 60;
        Some(tz::local("UTC", t))
    }

    fn tooltip(&self) -> Option<(String, Rect)> {
        let i = self.panel_hover?;
        if self.popover.is_some() || self.launcher.is_some() {
            return None;
        }
        let (it, r) = *self.panel_layout().get(i)?;
        let ru = self.ru();
        let text = match it {
            Item::Menu => String::from(tr(ru, "Приложения", "Applications")),
            Item::App(a) => String::from(a.name(ru)),
            Item::Volume => String::from(tr(ru, "Звук", "Sound")),
            Item::Network => String::from(match self.net.ip {
                Some(_) => tr(ru, "Сеть: подключено", "Network: connected"),
                None => tr(ru, "Сеть: нет подключения", "Network: not connected"),
            }),
            Item::Clock => self.local_now().map_or(String::new(), |l| l.date(ru)),
            Item::Sep => return None,
        };
        let w = (text.chars().count() as i32 * self.s(8)).max(self.s(40)) + self.s(24);
        let h = self.s(32);
        let tip = Rect::new(r.center_x() - w / 2, r.y - self.s(16) - h, w, h);
        Some((text, tip))
    }

    pub(crate) fn draw_tooltip(&mut self, c: &mut Canvas, fonts: &mut Fonts) {
        let due = self.panel_hover.is_some() && self.now >= self.panel_hover_t + 450;
        let a = self.tw.value(TIP, if due { 255 } else { 0 }, anim::FAST) as u32;
        let Some((text, r)) = self.tooltip() else {
            return;
        };
        if a == 0 {
            return;
        }
        let t = self.theme;
        let px = self.px(13);
        let tw = fonts.measure(Weight::Medium, px, &text);
        let r = Rect::new(
            r.center_x() - (tw + self.s(24)) / 2,
            r.y + (255 - a as i32) * self.s(4) / 255,
            tw + self.s(24),
            r.h,
        );
        let rad = if self.cfg.rounded_corners {
            r.h / 2
        } else {
            self.s(4)
        };
        c.shadow(
            r.offset(0, self.s(3)),
            rad,
            self.s(8),
            fade(0xff000000, (a * 70 / 255) as u8),
        );
        c.fill_rrect(
            r,
            rad,
            fade(if t.dark { 0xff2c2c31 } else { 0xfffafafa }, a as u8),
        );
        c.stroke_rrect_fx(r, rad, FX, fade(t.window_border, a as u8));
        fonts.draw_centered_y(
            c,
            Weight::Medium,
            px,
            r.x + self.s(12),
            r.center_y(),
            &text,
            fade(t.text, a as u8),
        );
    }

    // --- small windows ----------------------------------------------------

    fn toggle_popover(&mut self, k: PopKind) {
        if self
            .popover
            .as_ref()
            .is_some_and(|p| p.kind == k && !p.closing)
        {
            self.close_popover();
            return;
        }
        if let Some(p) = self.popover.as_ref() {
            let r = self.popover_rect();
            let _ = p;
            self.add_damage(r.inset(-self.s(30)));
        }
        self.popover = Some(Popover {
            kind: k,
            t0: self.now,
            closing: false,
        });
        self.tw.set(POP, 0);
        let r = self.popover_rect();
        self.add_damage(r.inset(-self.s(30)));
    }

    pub(crate) fn close_popover(&mut self) {
        if let Some(p) = self.popover.as_mut()
            && !p.closing
        {
            p.closing = true;
            p.t0 = self.now;
        }
        let r = self.popover_rect();
        self.add_damage(r.inset(-self.s(30)));
    }

    pub(crate) fn popover_rect(&self) -> Rect {
        let Some(p) = self.popover.as_ref() else {
            return Rect::default();
        };
        let pr = self.panel_rect();
        let (w, h) = match p.kind {
            PopKind::Clock => (self.s(300), self.s(330)),
            _ => (self.s(300), self.s(132)),
        };
        let anchor = self
            .panel_layout()
            .into_iter()
            .find(|(it, _)| {
                matches!(
                    (p.kind, it),
                    (PopKind::Clock, Item::Clock)
                        | (PopKind::Network, Item::Network)
                        | (PopKind::Volume, Item::Volume)
                )
            })
            .map_or(pr, |x| x.1);
        let x = (anchor.center_x() - w / 2).clamp(self.s(8), self.w - w - self.s(8));
        let x = x.min(pr.right() - w).max(self.s(8));
        Rect::new(x, pr.y - self.s(12) - h, w, h)
    }

    fn pop_items(&self) -> Vec<(PopAction, Rect)> {
        let Some(p) = self.popover.as_ref() else {
            return Vec::new();
        };
        if p.kind != PopKind::Clock {
            return Vec::new();
        }
        let r = self.popover_rect();
        let mut v = Vec::new();
        let mut y = r.y + self.s(150);
        for a in [
            PopAction::Settings,
            PopAction::Logout,
            PopAction::Restart,
            PopAction::PowerOff,
        ] {
            v.push((
                a,
                Rect::new(r.x + self.s(10), y, r.w - self.s(20), self.s(40)),
            ));
            y += self.s(42);
        }
        v
    }

    pub(crate) fn popover_hover_at(&mut self, x: i32, y: i32) {
        let h = self.pop_items().iter().position(|(_, r)| r.contains(x, y));
        if h != self.pop_hover {
            self.pop_hover = h;
            let r = self.popover_rect();
            self.add_damage(r);
        }
    }

    /// A click on the small window; true if it was inside.
    pub(crate) fn popover_click(
        &mut self,
        x: i32,
        y: i32,
        host: &mut dyn Host,
        fonts: &mut Fonts,
    ) -> bool {
        if !self.popover_rect().contains(x, y) {
            return false;
        }
        let hit = self.pop_items().into_iter().find(|(_, r)| r.contains(x, y));
        if let Some((a, _)) = hit {
            self.close_popover();
            match a {
                PopAction::Settings => self.launch(AppId::Settings, host, fonts),
                PopAction::Logout => {
                    host.log("lumi: logout");
                    self.exit = Some(Exit::Logout);
                }
                PopAction::Restart => self.exit = Some(Exit::Power(Power::Restart)),
                PopAction::PowerOff => self.exit = Some(Exit::Power(Power::Off)),
            }
        }
        true
    }

    pub(crate) fn draw_popover(&mut self, c: &mut Canvas, fonts: &mut Fonts) {
        let Some((kind, t0, closing)) = self.popover.as_ref().map(|p| (p.kind, p.t0, p.closing))
        else {
            return;
        };
        let d = self.dur(160).max(1) as u64;
        let el = self.now.saturating_sub(t0).min(d);
        let p = anim::ease_out((el * 1000 / d) as u32);
        let a = if closing { 1000 - p } else { p } * 255 / 1000;
        let base = self.popover_rect();
        let r = base.offset(0, (255 - a as i32) * self.s(10) / 255);
        if r.inset(-self.s(30)).intersect(c.frame()).is_empty() || a == 0 {
            return;
        }
        let t = self.theme;
        let ru = self.ru();
        let rad = if self.cfg.rounded_corners {
            self.s(18)
        } else {
            self.s(4)
        };
        self.shadow_small.draw(
            c,
            r.offset(0, self.s(6)),
            rad.min(self.s(16)),
            (0x70 * a / 255) << 24,
            r,
        );
        let corner = Corner::new(rad);
        if self.cfg.transparency {
            self.frost.rounded_to(c, r, &corner, a);
            c.fill_rrect(r, rad, fade(t.window, (190 * a / 255) as u8));
        } else {
            c.fill_rrect(r, rad, fade(t.window, a as u8));
        }
        c.stroke_rrect_fx(r, rad, FX, fade(t.window_border, a as u8));
        let al = |col: u32| fade(col, ((col >> 24) * a / 255) as u8);
        let x = r.x + self.s(22);
        match kind {
            PopKind::Clock => {
                let time = self.clock_text();
                fonts.draw(
                    c,
                    Weight::SemiBold,
                    self.px(34),
                    x,
                    r.y + self.s(52),
                    &time,
                    al(t.text),
                );
                let date = self.local_now().map_or(String::new(), |l| l.date(ru));
                fonts.draw(
                    c,
                    Weight::Regular,
                    self.px(13),
                    x,
                    r.y + self.s(76),
                    &date,
                    al(t.text_dim),
                );
                // The user.
                let uy = r.y + self.s(112);
                let d = self.s(32);
                c.fill_circle(x + d / 2, uy, d / 2, al(t.accent));
                let name = self.user.as_ref().map_or(String::new(), |u| u.name.clone());
                let initial: String = name
                    .chars()
                    .next()
                    .map(|ch| ch.to_uppercase().collect())
                    .unwrap_or_default();
                let iw = fonts.measure(Weight::SemiBold, self.px(15), &initial);
                fonts.draw_centered_y(
                    c,
                    Weight::SemiBold,
                    self.px(15),
                    x + d / 2 - iw / 2,
                    uy,
                    &initial,
                    al(t.on_accent),
                );
                fonts.draw_centered_y(
                    c,
                    Weight::Medium,
                    self.px(14),
                    x + d + self.s(12),
                    uy,
                    &name,
                    al(t.text),
                );
                c.fill_rect(
                    Rect::new(r.x + self.s(16), r.y + self.s(140), r.w - self.s(32), 1),
                    al(fade(t.window_border, 255)),
                );
                let items = self.pop_items();
                for (i, (act, ir)) in items.iter().enumerate() {
                    let hov = self.tw.value(
                        POP_HOVER + i as u64,
                        if self.pop_hover == Some(i) { 255 } else { 0 },
                        anim::FAST,
                    ) as u32;
                    if hov > 0 {
                        c.fill_rrect(*ir, self.s(10), fade(t.field_hover, (hov * a / 255) as u8));
                    }
                    let (icon, label) = match act {
                        PopAction::Settings => (Icon::Gear, tr(ru, "Настройки", "Settings")),
                        PopAction::Logout => (Icon::LogOut, tr(ru, "Выйти", "Log out")),
                        PopAction::Restart => (Icon::Refresh, tr(ru, "Перезагрузить", "Restart")),
                        PopAction::PowerOff => (Icon::Power, tr(ru, "Выключить", "Power off")),
                    };
                    let isz = self.s(18);
                    icons::draw(
                        c,
                        icon,
                        ir.x + self.s(12),
                        ir.center_y() - isz / 2,
                        isz,
                        al(t.text_dim),
                        0,
                    );
                    fonts.draw_centered_y(
                        c,
                        Weight::Medium,
                        self.px(14),
                        ir.x + self.s(44),
                        ir.center_y(),
                        label,
                        al(t.text),
                    );
                }
            }
            PopKind::Network => {
                fonts.draw(
                    c,
                    Weight::SemiBold,
                    self.px(16),
                    x,
                    r.y + self.s(40),
                    tr(ru, "Сеть", "Network"),
                    al(t.text),
                );
                let (line1, line2) = match (self.net.present, self.net.ip) {
                    (_, Some(ip)) => (
                        String::from(tr(ru, "Проводное подключение", "Wired connection")),
                        alloc::format!(
                            "{} {}.{}.{}.{}",
                            tr(ru, "Адрес", "Address"),
                            ip[0],
                            ip[1],
                            ip[2],
                            ip[3]
                        ),
                    ),
                    (true, None) => (
                        String::from(tr(ru, "Нет подключения", "Not connected")),
                        String::from(tr(ru, "Адрес не получен (DHCP)", "No address from DHCP")),
                    ),
                    (false, None) => (
                        String::from(tr(ru, "Сетевая карта не найдена", "No network card found")),
                        String::from(tr(
                            ru,
                            "Поддерживаются virtio-net и e1000",
                            "virtio-net and e1000 are supported",
                        )),
                    ),
                };
                fonts.draw(
                    c,
                    Weight::Medium,
                    self.px(14),
                    x,
                    r.y + self.s(74),
                    &line1,
                    al(t.text),
                );
                fonts.draw(
                    c,
                    Weight::Regular,
                    self.px(12),
                    x,
                    r.y + self.s(98),
                    &line2,
                    al(t.text_dim),
                );
            }
            PopKind::Volume => {
                fonts.draw(
                    c,
                    Weight::SemiBold,
                    self.px(16),
                    x,
                    r.y + self.s(40),
                    tr(ru, "Звук", "Sound"),
                    al(t.text),
                );
                fonts.draw(
                    c,
                    Weight::Medium,
                    self.px(14),
                    x,
                    r.y + self.s(74),
                    tr(ru, "Звуковое устройство не найдено", "No sound device"),
                    al(t.text),
                );
                fonts.draw(
                    c,
                    Weight::Regular,
                    self.px(12),
                    x,
                    r.y + self.s(98),
                    tr(
                        ru,
                        "Драйвера звука в Velora пока нет",
                        "Velora has no sound driver yet",
                    ),
                    al(t.text_dim),
                );
            }
        }
        let _ = base;
    }

    // --- the window switcher ----------------------------------------------

    pub(crate) fn switch_step(&mut self, back: bool) {
        match self.switcher.as_mut() {
            Some(s) => {
                let n = s.order.len();
                if n > 0 {
                    s.sel = if back {
                        (s.sel + n - 1) % n
                    } else {
                        (s.sel + 1) % n
                    };
                }
            }
            None => {
                let order: Vec<u32> = self
                    .windows
                    .iter()
                    .rev()
                    .filter(|w| !w.closing)
                    .map(|w| w.id)
                    .collect();
                if order.is_empty() {
                    return;
                }
                let sel = if order.len() > 1 { 1 } else { 0 };
                self.tw.set(SWITCH, 0);
                self.switcher = Some(Switcher { order, sel });
                if self.launcher.is_some() {
                    self.launcher = None;
                    self.damage_all();
                }
            }
        }
        let r = self.switcher_rect();
        self.add_damage(r.inset(-self.s(40)));
    }

    pub(crate) fn finish_switcher(&mut self) {
        if let Some(s) = self.switcher.take() {
            let r = self.switcher_rect_for(s.order.len());
            self.add_damage(r.inset(-self.s(40)));
            if let Some(&id) = s.order.get(s.sel) {
                self.unminimize(id);
            }
        }
    }

    pub(crate) fn cancel_switcher(&mut self) {
        if let Some(s) = self.switcher.take() {
            let r = self.switcher_rect_for(s.order.len());
            self.add_damage(r.inset(-self.s(40)));
        }
    }

    fn switcher_rect_for(&self, n: usize) -> Rect {
        let cell = self.s(132);
        let w = n as i32 * cell + self.s(32);
        let h = self.s(176);
        Rect::new((self.w - w) / 2, (self.h - h) / 2, w, h)
    }

    fn switcher_rect(&self) -> Rect {
        self.switcher_rect_for(self.switcher.as_ref().map_or(0, |s| s.order.len()))
    }

    pub(crate) fn draw_switcher(&mut self, c: &mut Canvas, fonts: &mut Fonts) {
        let Some(s) = self.switcher.as_ref() else {
            return;
        };
        let order = s.order.clone();
        let sel = s.sel;
        let a = self.tw.value(SWITCH, 255, self.dur(120)) as u32;
        let r = self.switcher_rect();
        if r.inset(-self.s(40)).intersect(c.frame()).is_empty() {
            return;
        }
        let t = self.theme;
        let rad = if self.cfg.rounded_corners {
            self.s(22)
        } else {
            self.s(4)
        };
        self.shadow_small.draw(
            c,
            r.offset(0, self.s(8)),
            rad.min(self.s(16)),
            (0x80 * a / 255) << 24,
            r,
        );
        let corner = Corner::new(rad);
        if self.cfg.transparency {
            self.frost.rounded_to(c, r, &corner, a);
            c.fill_rrect(r, rad, fade(t.window, (200 * a / 255) as u8));
        } else {
            c.fill_rrect(r, rad, fade(t.window, a as u8));
        }
        c.stroke_rrect_fx(r, rad, FX, fade(t.window_border, a as u8));
        let cell = self.s(132);
        for (i, id) in order.iter().enumerate() {
            let Some(w) = self.windows.iter().find(|w| w.id == *id) else {
                continue;
            };
            let cr = Rect::new(
                r.x + self.s(16) + i as i32 * cell,
                r.y + self.s(16),
                cell,
                r.h - self.s(32),
            );
            if i == sel {
                c.fill_rrect(
                    cr,
                    self.s(14),
                    fade(lerp(t.field_hover, t.accent, 50), a as u8),
                );
            }
            let tile = self.s(68);
            let tr_ = Rect::new(cr.center_x() - tile / 2, cr.y + self.s(16), tile, tile);
            apps::draw_tile(c, w.app.id(), tr_, self.cfg.rounded_corners, a);
            let px = self.px(12);
            let mut title = w.title.clone();
            while title.chars().count() > 3
                && fonts.measure(Weight::Medium, px, &title) > cell - self.s(16)
            {
                title.pop();
                title = String::from(title.trim_end_matches('…'));
                title.push('…');
            }
            let tw = fonts.measure(Weight::Medium, px, &title);
            let col = if i == sel { t.text } else { t.text_dim };
            fonts.draw(
                c,
                Weight::Medium,
                px,
                cr.center_x() - tw / 2,
                cr.bottom() - self.s(20),
                &title,
                fade(col, a as u8),
            );
        }
    }

    // --- per-frame upkeep ---------------------------------------------------

    pub(crate) fn tick_overlays(&mut self, now: u64) {
        let pr = self.panel_rect();
        if self.panel_appear() < 1000 || self.now.saturating_sub(self.session_t0) < 700 {
            self.add_damage(Rect::new(
                0,
                pr.y - self.s(20),
                self.w,
                self.h - pr.y + self.s(20),
            ));
        }
        if self.anim_prev {
            self.add_damage(Rect::new(
                pr.x - self.s(140),
                pr.y - self.s(70),
                pr.w + self.s(280),
                pr.h + self.s(80),
            ));
            if self.popover.is_some() {
                let r = self.popover_rect();
                self.add_damage(r.inset(-self.s(30)));
            }
            if self.switcher.is_some() {
                let r = self.switcher_rect();
                self.add_damage(r.inset(-self.s(40)));
            }
        }
        // Tooltip due: draw it.
        if self.panel_hover.is_some()
            && now >= self.panel_hover_t + 450
            && now < self.panel_hover_t + 600
        {
            self.add_damage(Rect::new(
                pr.x - self.s(140),
                pr.y - self.s(70),
                pr.w + self.s(280),
                self.s(70),
            ));
        }
        if let Some(p) = self.popover.as_ref() {
            let d = self.dur(160) as u64;
            let el = now.saturating_sub(p.t0);
            if el <= d + 20 {
                let r = self.popover_rect();
                self.add_damage(r.inset(-self.s(30)));
            } else if p.closing {
                let r = self.popover_rect();
                self.popover = None;
                self.add_damage(r.inset(-self.s(30)));
            }
        }
        self.tick_launcher(now);
    }

    pub(crate) fn overlays_animating(&self) -> bool {
        self.mode == crate::desktop::Mode::Session
            && (self.panel_appear() < 1000
                || self
                    .popover
                    .is_some_and_anim(self.now, self.dur(160) as u64)
                || self.launcher_animating())
    }

    pub(crate) fn overlay_wake(&self) -> Option<u64> {
        let mut t = None;
        if self.panel_hover.is_some() && self.now < self.panel_hover_t + 450 {
            t = Some(self.panel_hover_t + 450);
        }
        if let Some(at) = self.login_wake() {
            t = Some(t.map_or(at, |x: u64| x.min(at)));
        }
        if let Some(at) = self.launcher_wake() {
            t = Some(t.map_or(at, |x: u64| x.min(at)));
        }
        t
    }
}

trait PopAnim {
    fn is_some_and_anim(&self, now: u64, d: u64) -> bool;
}

impl PopAnim for Option<Popover> {
    fn is_some_and_anim(&self, now: u64, d: u64) -> bool {
        self.as_ref()
            .is_some_and(|p| now.saturating_sub(p.t0) <= d + 20)
    }
}

//! The panel: a floating rounded bar at the bottom middle with the app
//! menu button, the pinned and running apps, the volume, the network and
//! the clock; their tooltips and small windows; the Alt+Tab switcher.

use crate::apps::{self, AppId, tr};
use crate::config::{PANEL_ALWAYS, PANEL_ITEM, PANEL_SMART};
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

/// The panel hiding (autohide and smart modes): where it goes and since
/// when.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Hide {
    /// The panel is out of sight or on its way.
    pub(crate) hidden: bool,
    /// The change started at `t0` from `from` (hidden, 0..=1000).
    pub(crate) t0: u64,
    pub(crate) from: u32,
    /// The pointer left: the panel hides at this time.
    pub(crate) at: Option<u64>,
}

/// The panel hides this long after the pointer leaves it.
const HIDE_DELAY_MS: u64 = 500;

/// Actions in the clock's small window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopAction {
    Settings,
    Logout,
    Restart,
    PowerOff,
}

impl Desktop {
    /// The pinned apps, in the order of the settings.
    pub(crate) fn pinned(&self) -> Vec<AppId> {
        let mut v = Vec::new();
        for k in &self.cfg.pinned {
            if let Some(a) = AppId::from_key(k)
                && !v.contains(&a)
            {
                v.push(a);
            }
        }
        v
    }

    pub(crate) fn panel_items(&self) -> Vec<Item> {
        let mut v = alloc::vec![Item::Menu, Item::Sep];
        for a in self.pinned() {
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

    /// The size of a panel item in design units.
    pub(crate) fn item_du(&self) -> i32 {
        PANEL_ITEM[self.cfg.panel_size.min(2)]
    }

    fn item_w(&self, it: Item) -> i32 {
        match it {
            Item::Sep => self.s(17),
            Item::Clock => {
                let time = if self.cfg.clock_24h { 62 } else { 84 };
                self.s(time + if self.cfg.show_date { 64 } else { 0 })
            }
            _ => self.s(self.item_du()),
        }
    }

    /// Where the panel is when shown.
    pub fn panel_rect(&self) -> Rect {
        let items = self.panel_items();
        let pad = self.s(7);
        let w: i32 = items.iter().map(|&i| self.item_w(i)).sum::<i32>() + 2 * pad;
        let h = self.s(self.item_du() + 12);
        Rect::new((self.w - w) / 2, self.h - h - self.s(10), w, h)
    }

    /// How far the panel is hidden now: 0 (shown) to 1000.
    pub(crate) fn panel_hidden(&self) -> u32 {
        let h = &self.panel_hide;
        let to = if h.hidden { 1000 } else { 0 };
        let d = self.dur(if h.hidden { 260 } else { 200 }) as u64;
        let el = self.now.saturating_sub(h.t0);
        if d == 0 || el >= d {
            return to;
        }
        let p = anim::ease_out((el * 1000 / d) as u32) as i64;
        (h.from as i64 + (to as i64 - h.from as i64) * p / 1000) as u32
    }

    /// The panel's offset down from its place (hiding and appearing).
    fn panel_dy(&self) -> i32 {
        let appear = self.panel_appear();
        let pr = self.panel_rect();
        let away = self.h - pr.y + self.s(12);
        let a = (self.s(70) as i64 * (1000 - appear) as i64 / 1000) as i32;
        let b = (away as i64 * self.panel_hidden() as i64 / 1000) as i32;
        a.max(b)
    }

    /// The panel is fully in sight (for tests and logs).
    pub fn panel_visible(&self) -> bool {
        self.panel_hidden() == 0
    }

    /// The apps on the panel, in order (for tests).
    pub fn panel_apps(&self) -> Vec<AppId> {
        self.panel_items()
            .into_iter()
            .filter_map(|i| match i {
                Item::App(a) => Some(a),
                _ => None,
            })
            .collect()
    }

    /// Where the panel is now, if it can be clicked.
    pub(crate) fn panel_hit_rect(&self) -> Rect {
        if self.panel_hidden() > 300 {
            Rect::default()
        } else {
            self.panel_rect()
        }
    }

    fn set_panel_hidden(&mut self, hidden: bool) {
        if self.panel_hide.hidden == hidden {
            return;
        }
        let from = self.panel_hidden();
        self.panel_hide = Hide {
            hidden,
            t0: self.now,
            from,
            at: None,
        };
        if hidden {
            self.panel_hover = None;
            self.panel_press = None;
            self.tw.set(TIP, 0);
        }
        self.host_log_pending = Some(if hidden {
            "lumi: panel hidden"
        } else {
            "lumi: panel shown"
        });
        self.damage_panel_strip();
    }

    /// The bottom of the screen where the panel, its shadow and its
    /// tooltips are.
    fn damage_panel_strip(&mut self) {
        let pr = self.panel_rect();
        self.add_damage(Rect::new(
            0,
            pr.y - self.s(70),
            self.w,
            self.h - pr.y + self.s(70),
        ));
    }

    /// A window covers the panel's place (smart hiding).
    fn window_over_panel(&self) -> bool {
        let pr = self.panel_rect();
        self.windows
            .iter()
            .any(|w| !w.minimized && !w.closing && !w.rect.intersect(pr).is_empty())
    }

    /// The pointer is near the bottom edge under the panel, or over it.
    fn pointer_wants_panel(&self) -> bool {
        let (x, y) = self.mouse;
        let pr = self.panel_rect();
        let zone = Rect::new(
            pr.x - self.s(60),
            self.h - self.s(32),
            pr.w + self.s(120),
            self.s(32),
        );
        zone.contains(x, y) || (!self.panel_hide.hidden && pr.inset(-self.s(10)).contains(x, y))
    }

    /// The panel's mode changed: windows that fill the work area fit the
    /// new one.
    pub(crate) fn panel_mode_changed(&mut self, _host: &mut dyn Host) {
        self.refit = true;
        if self.cfg.panel_mode == PANEL_ALWAYS {
            self.set_panel_hidden(false);
        }
    }

    /// Shows or hides the panel by its mode, the pointer and what is
    /// open; once per frame.
    pub(crate) fn tick_panel_hide(&mut self) {
        let mode = self.cfg.panel_mode;
        let keep = mode == PANEL_ALWAYS
            || self.launcher.is_some()
            || self.popover.is_some()
            || self.switcher.is_some()
            || self.panel_press.is_some()
            || self.mods & velora_ui::ui::mods::SUPER != 0
            || self.pointer_wants_panel()
            || (mode == PANEL_SMART && !self.window_over_panel());
        if keep {
            self.panel_hide.at = None;
            // Out of the way of a window being dragged over it, though.
            self.set_panel_hidden(false);
        } else if !self.panel_hide.hidden {
            match self.panel_hide.at {
                None => self.panel_hide.at = Some(self.now + HIDE_DELAY_MS),
                Some(t) if self.now >= t => self.set_panel_hidden(true),
                Some(_) => {}
            }
        }
        let d = self.dur(260) as u64 + 20;
        if self.now.saturating_sub(self.panel_hide.t0) <= d {
            self.damage_panel_strip();
        }
    }

    fn panel_layout(&self) -> Vec<(Item, Rect)> {
        let pr = self.panel_rect();
        let mut x = pr.x + self.s(7);
        let size = self.s(self.item_du());
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
        let h = if self.panel_hit_rect().contains(x, y) && self.drag_none() {
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
        let dy = self.panel_dy();
        if pr.y + dy >= self.h {
            return;
        }
        let alpha = appear * 255 / 1000;
        let r = pr.offset(0, dy);
        if r.inset(-self.s(30)).intersect(c.frame()).is_empty() {
            return;
        }
        let t = self.theme;
        let rad = self.s(self.cfg.round(18));
        self.shadow_small.draw(
            c,
            r.offset(0, self.s(5)),
            rad.min(self.s(16)),
            (0x60 * alpha / 255) << 24,
            r,
        );
        let corner = Corner::new(rad);
        if self.cfg.transparency > 0 {
            self.frost.rounded_to(c, r, &corner, alpha);
            c.fill_rrect(
                r,
                rad,
                fade(t.window, (self.cfg.panel_alpha() * alpha / 255) as u8),
            );
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
            let brad = self.s(self.cfg.round(11));
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
            let isz = self.s(self.item_du() / 2);
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
                Item::Volume => {
                    let (icon, col) = match self.sound.as_ref() {
                        Some(m) if m.mute || m.volume == 0 => (Icon::SpeakerMute, t.text_dim),
                        Some(m) if !m.devices.is_empty() => (Icon::Speaker, t.text_dim),
                        _ => (Icon::Speaker, t.text_faint),
                    };
                    icons::draw(
                        c,
                        icon,
                        ix + self.s(1),
                        iy + self.s(1),
                        isz - self.s(2),
                        fade(col, alpha as u8),
                        0,
                    )
                }
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
        let (h, mm) = (m / 60, m % 60);
        let time = if self.cfg.clock_24h {
            alloc::format!("{:02}:{:02}", h, mm)
        } else {
            let h12 = if h % 12 == 0 { 12 } else { h % 12 };
            alloc::format!("{}:{:02} {}", h12, mm, if h < 12 { "AM" } else { "PM" })
        };
        match self.local_now().filter(|_| self.cfg.show_date) {
            Some(l) => alloc::format!("{}  {}", l.short_date(self.ru()), time),
            None => time,
        }
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
            Item::Volume => match self.sound.as_ref() {
                Some(m) if m.devices.is_empty() => {
                    String::from(tr(ru, "Нет устройств вывода", "No output devices"))
                }
                Some(m) if m.mute => String::from(tr(ru, "Звук выключен", "Sound off")),
                Some(m) => alloc::format!("{} {} %", tr(ru, "Громкость", "Volume"), m.volume),
                None => String::from(tr(ru, "Звуковое устройство не найдено", "No sound device")),
            },
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
        let rad = if self.cfg.radius > 0 {
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
        if k == PopKind::Volume {
            // Read again when it opens (Settings may have changed it).
            self.sound_next = 0;
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
            PopKind::Volume => match self.sound.as_ref() {
                Some(m) if !m.devices.is_empty() => (
                    self.s(320),
                    self.s(148) + self.s(40) * m.devices.len() as i32 + self.s(8),
                ),
                _ => (self.s(300), self.s(132)),
            },
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
        if self
            .popover
            .as_ref()
            .is_some_and(|p| p.kind == PopKind::Volume)
        {
            self.volume_click(x, y, host);
            return true;
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
        let rad = self.s(self.cfg.round(18));
        self.shadow_small.draw(
            c,
            r.offset(0, self.s(6)),
            rad.min(self.s(16)),
            (0x70 * a / 255) << 24,
            r,
        );
        let corner = Corner::new(rad);
        if self.cfg.transparency > 0 {
            self.frost.rounded_to(c, r, &corner, a);
            c.fill_rrect(
                r,
                rad,
                fade(
                    t.window,
                    ((self.cfg.panel_alpha() + 40).min(255) * a / 255) as u8,
                ),
            );
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
            PopKind::Volume => self.draw_volume(c, fonts, r, &al),
        }
        let _ = base;
    }

    // --- the sound window -------------------------------------------------

    /// Reads the sound card; a change (from Settings or a program) shows
    /// on the panel and is kept in the settings.
    pub(crate) fn refresh_sound(&mut self, host: &mut dyn Host) {
        let m = host.mixer();
        if m == self.sound {
            return;
        }
        if let Some(st) = m.as_ref()
            && (st.volume != self.cfg.volume || st.mute != self.cfg.mute)
        {
            self.cfg.volume = st.volume;
            self.cfg.mute = st.mute;
            self.save_at = Some(self.now + crate::desktop::SAVE_DELAY_MS);
        }
        self.sound = m;
        let r = self.panel_rect();
        self.add_damage(r);
        if self.popover.is_some() {
            let p = self.popover_rect();
            self.add_damage(p.inset(-self.s(30)));
        }
    }

    /// Sends a mixer command and shows the result.
    fn mixer_cmd(&mut self, host: &mut dyn Host, cmd: &str) {
        host.set_mixer(cmd);
        self.refresh_sound(host);
    }

    /// The mute button, the slider's track and the output rows.
    fn volume_layout(&self) -> (Rect, Rect, Vec<Rect>) {
        let r = self.popover_rect();
        let mute = Rect::new(r.x + self.s(14), r.y + self.s(58), self.s(40), self.s(40));
        let track = Rect::new(
            mute.right() + self.s(12),
            mute.center_y() - self.s(3),
            r.right() - mute.right() - self.s(12) - self.s(70),
            self.s(6),
        );
        let n = self.sound.as_ref().map_or(0, |m| m.devices.len());
        let rows = (0..n)
            .map(|i| {
                Rect::new(
                    r.x + self.s(10),
                    r.y + self.s(146) + self.s(40) * i as i32,
                    r.w - self.s(20),
                    self.s(38),
                )
            })
            .collect();
        (mute, track, rows)
    }

    /// Where a panel item is: `menu`, `volume`, `network` or `clock`
    /// (for tests).
    pub fn panel_item_rect(&self, name: &str) -> Option<Rect> {
        let want = match name {
            "menu" => Item::Menu,
            "volume" => Item::Volume,
            "network" => Item::Network,
            "clock" => Item::Clock,
            _ => return None,
        };
        self.panel_layout()
            .into_iter()
            .find(|(it, _)| *it == want)
            .map(|x| x.1)
    }

    /// The sound window's mute button, slider track and output rows while
    /// it is open (for tests).
    pub fn volume_controls(&self) -> Option<(Rect, Rect, Vec<Rect>)> {
        self.popover
            .as_ref()
            .filter(|p| p.kind == PopKind::Volume && !p.closing)
            .map(|_| self.volume_layout())
    }

    /// Sets the volume from the slider at `x`.
    pub(crate) fn volume_at(&mut self, x: i32, host: &mut dyn Host) {
        let (_, track, _) = self.volume_layout();
        let v = ((x - track.x) * 100 + track.w / 2) / track.w.max(1);
        let v = v.clamp(0, 100) as u32;
        if self.sound.as_ref().is_some_and(|m| m.volume != v || m.mute) {
            let cmd = alloc::format!("volume {}\nmute 0\n", v);
            self.mixer_cmd(host, &cmd);
        }
    }

    fn volume_click(&mut self, x: i32, y: i32, host: &mut dyn Host) {
        let Some(m) = self.sound.clone() else {
            return;
        };
        if m.devices.is_empty() {
            return;
        }
        let (mute, track, rows) = self.volume_layout();
        if mute.contains(x, y) {
            let cmd = alloc::format!("mute {}\n", !m.mute as u8);
            self.mixer_cmd(host, &cmd);
            if m.mute {
                host.play(&crate::sound::click());
            }
        } else if track.inset(-self.s(14)).contains(x, y) {
            self.vol_drag = true;
            self.volume_at(x, host);
        } else if let Some(i) = rows.iter().position(|r| r.contains(x, y))
            && i != m.output
        {
            let cmd = alloc::format!("output {}\n", i);
            self.mixer_cmd(host, &cmd);
            host.play(&crate::sound::click());
        }
    }

    fn draw_volume(&mut self, c: &mut Canvas, fonts: &mut Fonts, r: Rect, al: &dyn Fn(u32) -> u32) {
        let t = self.theme;
        let ru = self.ru();
        let x = r.x + self.s(22);
        fonts.draw(
            c,
            Weight::SemiBold,
            self.px(16),
            x,
            r.y + self.s(40),
            tr(ru, "Звук", "Sound"),
            al(t.text),
        );
        let Some(m) = self.sound.clone().filter(|m| !m.devices.is_empty()) else {
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
                    "Поддерживаются карты Intel HDA",
                    "Intel HDA cards are supported",
                ),
                al(t.text_dim),
            );
            return;
        };
        // The popover is drawn shifted while it slides in.
        let dy = r.y - self.popover_rect().y;
        let (mute, track, rows) = self.volume_layout();
        let (mute, track) = (mute.offset(0, dy), track.offset(0, dy));
        let off = m.mute || m.volume == 0;
        c.fill_rrect(
            mute,
            mute.h / 2,
            al(if off {
                t.field
            } else {
                lerp(t.field, t.accent, 60)
            }),
        );
        let isz = self.s(20);
        icons::draw(
            c,
            if off {
                Icon::SpeakerMute
            } else {
                Icon::Speaker
            },
            mute.center_x() - isz / 2,
            mute.center_y() - isz / 2,
            isz,
            al(t.text),
            0,
        );
        // The slider: track, filled part, knob.
        let v = if m.mute { 0 } else { m.volume as i32 };
        c.fill_rrect(track, track.h / 2, al(t.field));
        let fx = track.x + track.w * v / 100;
        c.fill_rrect(
            Rect::new(track.x, track.y, fx - track.x, track.h),
            track.h / 2,
            al(if m.mute { t.text_faint } else { t.accent }),
        );
        c.fill_circle(fx, track.center_y(), self.s(9), al(t.text));
        let pct = alloc::format!("{} %", m.volume);
        let px = self.px(13);
        let pw = fonts.measure(Weight::Medium, px, &pct);
        fonts.draw_centered_y(
            c,
            Weight::Medium,
            px,
            r.right() - self.s(22) - pw,
            track.center_y(),
            &pct,
            al(if m.mute { t.text_faint } else { t.text_dim }),
        );
        fonts.draw(
            c,
            Weight::Medium,
            self.px(12),
            x,
            r.y + self.s(132),
            tr(ru, "Устройство вывода", "Output device"),
            al(t.text_dim),
        );
        for (i, row) in rows.iter().enumerate() {
            let row = row.offset(0, dy);
            let on = i == m.output;
            if on {
                c.fill_rrect(row, self.s(10), al(lerp(t.selected, t.accent, 46)));
            }
            let (rn, en) = m.devices[i].name();
            let label = tr(ru, rn, en);
            let isz = self.s(16);
            icons::draw(
                c,
                Icon::Speaker,
                row.x + self.s(12),
                row.center_y() - isz / 2,
                isz,
                al(t.text_dim),
                0,
            );
            fonts.draw_centered_y(
                c,
                Weight::Medium,
                self.px(14),
                row.x + self.s(40),
                row.center_y(),
                label,
                al(t.text),
            );
            if on {
                icons::draw(
                    c,
                    Icon::Check,
                    row.right() - self.s(28),
                    row.center_y() - isz / 2,
                    isz,
                    al(t.accent),
                    0,
                );
            }
        }
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
        let rad = self.s(self.cfg.round(22));
        self.shadow_small.draw(
            c,
            r.offset(0, self.s(8)),
            rad.min(self.s(16)),
            (0x80 * a / 255) << 24,
            r,
        );
        let corner = Corner::new(rad);
        if self.cfg.transparency > 0 {
            self.frost.rounded_to(c, r, &corner, a);
            c.fill_rrect(
                r,
                rad,
                fade(
                    t.window,
                    ((self.cfg.panel_alpha() + 50).min(255) * a / 255) as u8,
                ),
            );
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
            apps::draw_tile(c, w.app.id(), tr_, self.cfg.radius > 0, a);
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
        self.tick_panel_hide();
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
        let hide = self.panel_hidden();
        self.mode == crate::desktop::Mode::Session
            && (self.panel_appear() < 1000
                || (hide != 0 && hide != 1000)
                || self
                    .popover
                    .is_some_and_anim(self.now, self.dur(160) as u64)
                || self.launcher_animating())
    }

    pub(crate) fn overlay_wake(&self) -> Option<u64> {
        let mut t = self.panel_hide.at;
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

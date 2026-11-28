//! The login screen, in the installer's style: the wallpaper blurred and
//! darkened, the clock, the user's avatar and name, the password field,
//! and buttons to restart and power off. A wrong password shakes the
//! field.

use crate::apps::tr;
use crate::desktop::{Desktop, Exit};
use crate::host::{Host, Power, UserInfo};
use crate::{gfx, tz};
use alloc::string::String;
use alloc::vec::Vec;
use velora_ui::anim;
use velora_ui::canvas::{Canvas, FX, fade, lerp, sin_cos};
use velora_ui::icons::{self, Icon};
use velora_ui::text::{Fonts, Weight};
use velora_ui::ui::Key;
use velora_ui::{Event, Rect};

const HOVER: u64 = 0x900;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Elem {
    Submit,
    Restart,
    PowerOff,
    Prev,
    Next,
    Field,
}

pub(crate) struct Login {
    pub(crate) user: usize,
    pub(crate) password: String,
    pub(crate) error: Option<&'static str>,
    shake_t0: u64,
    /// 1: a frame says "checking"; 2: check now.
    checking: u8,
    caret_on: bool,
    caret_next: u64,
    hover: Option<Elem>,
    press: Option<Elem>,
}

impl Login {
    pub(crate) fn new(_users: &[UserInfo]) -> Login {
        Login {
            user: 0,
            password: String::new(),
            error: None,
            shake_t0: 0,
            checking: 0,
            caret_on: true,
            caret_next: 0,
            hover: None,
            press: None,
        }
    }
}

impl Desktop {
    /// The users shown: ordinary ones, root only if there is nobody else.
    fn login_users(&self) -> Vec<&UserInfo> {
        let v: Vec<&UserInfo> = self.users.iter().filter(|u| u.uid != 0).collect();
        if v.is_empty() {
            self.users.iter().collect()
        } else {
            v
        }
    }

    fn avatar_center(&self) -> (i32, i32) {
        (self.w / 2, self.h * 47 / 100)
    }

    fn field_rect(&self) -> Rect {
        let (cx, cy) = self.avatar_center();
        let w = self.s(320);
        Rect::new(cx - w / 2, cy + self.s(112), w, self.s(48))
    }

    fn login_elems(&self) -> Vec<(Elem, Rect)> {
        let f = self.field_rect();
        let d = self.s(36);
        let mut v = alloc::vec![
            (Elem::Field, f),
            (
                Elem::Submit,
                Rect::new(f.right() - d - self.s(6), f.center_y() - d / 2, d, d)
            ),
        ];
        let b = self.s(52);
        let y = self.h - b - self.s(36);
        v.push((
            Elem::Restart,
            Rect::new(self.w - 2 * b - self.s(52), y, b, b),
        ));
        v.push((Elem::PowerOff, Rect::new(self.w - b - self.s(36), y, b, b)));
        if self.login_users().len() > 1 {
            let (cx, cy) = self.avatar_center();
            let a = self.s(40);
            v.push((
                Elem::Prev,
                Rect::new(cx - self.s(150) - a / 2, cy - a / 2, a, a),
            ));
            v.push((
                Elem::Next,
                Rect::new(cx + self.s(150) - a / 2, cy - a / 2, a, a),
            ));
        }
        v
    }

    fn submit(&mut self) {
        if self.login.checking == 0 && !self.login.password.is_empty() {
            self.login.checking = 1;
            self.login.error = None;
            self.damage_all();
        }
    }

    fn switch_user(&mut self, step: i32) {
        let n = self.login_users().len() as i32;
        if n > 1 {
            self.login.user = ((self.login.user as i32 + step).rem_euclid(n)) as usize;
            self.login.password.clear();
            self.login.error = None;
            self.damage_all();
        }
    }

    pub(crate) fn login_input(
        &mut self,
        events: &[Event],
        _host: &mut dyn Host,
        _fonts: &mut Fonts,
    ) {
        if self.login.checking != 0 {
            return;
        }
        for ev in events {
            match *ev {
                Event::Key { key, .. } => {
                    match key {
                        Key::Char(c) => {
                            if self.login.password.chars().count() < 128 {
                                self.login.password.push(c);
                            }
                        }
                        Key::Space => self.login.password.push(' '),
                        Key::Backspace => {
                            self.login.password.pop();
                        }
                        Key::Escape => self.login.password.clear(),
                        Key::Enter => self.submit(),
                        Key::Left | Key::Up => self.switch_user(-1),
                        Key::Right | Key::Down | Key::Tab => self.switch_user(1),
                        _ => {}
                    }
                    self.login.caret_on = true;
                    self.login.caret_next = self.now + 530;
                    let f = self.field_rect();
                    self.add_damage(f.inset(-self.s(60)));
                }
                Event::MouseMove { x, y } => {
                    self.mouse = (x, y);
                    let h = self
                        .login_elems()
                        .into_iter()
                        .find(|(e, r)| *e != Elem::Field && r.contains(x, y))
                        .map(|(e, _)| e);
                    if h != self.login.hover {
                        self.login.hover = h;
                        self.damage_all();
                    }
                }
                Event::MouseButton { down } => {
                    let (x, y) = self.mouse;
                    let h = self
                        .login_elems()
                        .into_iter()
                        .find(|(e, r)| *e != Elem::Field && r.contains(x, y))
                        .map(|(e, _)| e);
                    if down {
                        self.login.press = h;
                    } else if let Some(e) = self.login.press.take()
                        && h == Some(e)
                    {
                        match e {
                            Elem::Submit => self.submit(),
                            Elem::Restart => self.exit = Some(Exit::Power(Power::Restart)),
                            Elem::PowerOff => self.exit = Some(Exit::Power(Power::Off)),
                            Elem::Prev => self.switch_user(-1),
                            Elem::Next => self.switch_user(1),
                            Elem::Field => {}
                        }
                    }
                    self.damage_all();
                }
                _ => {}
            }
        }
    }

    pub(crate) fn tick_login(&mut self, now: u64, host: &mut dyn Host) {
        match self.login.checking {
            1 => self.login.checking = 2,
            2 => {
                self.login.checking = 0;
                let users = self.login_users();
                let name = users
                    .get(self.login.user)
                    .map(|u| u.name.clone())
                    .unwrap_or_default();
                let pw = core::mem::take(&mut self.login.password);
                match host.login(&name, &pw) {
                    Ok(u) => {
                        self.start_session(u, host);
                        return;
                    }
                    Err(_) => {
                        host.log("lumi: login failed");
                        self.login.error = Some(tr(self.ru(), "Неверный пароль", "Wrong password"));
                        self.login.shake_t0 = now;
                    }
                }
                self.damage_all();
            }
            _ => {}
        }
        if now >= self.login.caret_next {
            self.login.caret_on = !self.login.caret_on;
            self.login.caret_next = now + 530;
            let f = self.field_rect();
            self.add_damage(f);
        }
        if now.saturating_sub(self.login.shake_t0) < 520 || self.anim_prev {
            self.damage_all();
        }
    }

    pub(crate) fn login_wake(&self) -> Option<u64> {
        (self.mode == crate::desktop::Mode::Login).then_some(self.login.caret_next)
    }

    pub(crate) fn draw_login(&mut self, c: &mut Canvas, fonts: &mut Fonts, host: &mut dyn Host) {
        let t = self.theme;
        let ru = self.ru();
        let area = c.frame();
        gfx::copy(c, &self.frost, area);
        c.fill_rect(area, if t.dark { 0x38000000 } else { 0x30000000 });
        let white = 0xffffffff;
        let soft = 0xd0ffffff;
        // The clock and the date.
        if let Some(utc) = host.utc() {
            let l = tz::local(&self.cfg.region, utc);
            let time = l.hhmm();
            let px = self.px(96);
            let w = fonts.measure(Weight::SemiBold, px, &time);
            fonts.draw(
                c,
                Weight::SemiBold,
                px,
                (self.w - w) / 2,
                self.h * 22 / 100,
                &time,
                white,
            );
            let date = l.date(ru);
            let dpx = self.px(19);
            let dw = fonts.measure(Weight::Medium, dpx, &date);
            fonts.draw(
                c,
                Weight::Medium,
                dpx,
                (self.w - dw) / 2,
                self.h * 22 / 100 + self.s(40),
                &date,
                soft,
            );
        }
        // The avatar and the name.
        let users: Vec<UserInfo> = self.login_users().into_iter().cloned().collect();
        let user = users.get(self.login.user.min(users.len().saturating_sub(1)));
        let (cx, cy) = self.avatar_center();
        let r = self.s(54);
        c.shadow(
            Rect::new(cx - r, cy - r + self.s(8), 2 * r, 2 * r),
            r,
            self.s(20),
            0x60000000,
        );
        c.fill_circle(cx, cy, r, lerp(t.accent, 0xff000000, 40));
        c.fill_circle_inside(
            cx as i64 * FX,
            (cy - r * 2) as i64 * FX,
            (r * 2) as i64 * FX,
            (r / 2) as i64 * FX,
            Rect::new(cx - r, cy - r, 2 * r, 2 * r),
            r,
            fade(t.accent, 255),
        );
        c.stroke_circle_fx(
            cx as i64 * FX,
            cy as i64 * FX,
            r as i64 * FX,
            FX * 2,
            0x50ffffff,
        );
        let name = user.map_or(String::from("?"), |u| u.name.clone());
        let initial: String = name
            .chars()
            .next()
            .map(|ch| ch.to_uppercase().collect())
            .unwrap_or_default();
        let ipx = self.px(46);
        let iw = fonts.measure(Weight::SemiBold, ipx, &initial);
        fonts.draw_centered_y(
            c,
            Weight::SemiBold,
            ipx,
            cx - iw / 2,
            cy,
            &initial,
            t.on_accent,
        );
        let npx = self.px(22);
        let nw = fonts.measure(Weight::SemiBold, npx, &name);
        fonts.draw(
            c,
            Weight::SemiBold,
            npx,
            cx - nw / 2,
            cy + r + self.s(38),
            &name,
            white,
        );
        // The password field, shaking after a wrong password.
        let el = self.now.saturating_sub(self.login.shake_t0);
        let shake = if self.login.shake_t0 > 0 && el < 520 {
            let (s, _) = sin_cos((el * 65536 * 4 / 520) as u32);
            (s * self.s(12) as i64 / 65536 * (520 - el as i64) / 520) as i32
        } else {
            0
        };
        let elems = self.login_elems();
        let field = self.field_rect().offset(shake, 0);
        let rad = if self.cfg.rounded_corners {
            field.h / 2
        } else {
            self.s(6)
        };
        c.fill_rrect(field, rad, 0x30ffffff);
        let border = if self.login.error.is_some() {
            fade(t.error, 230)
        } else {
            0x50ffffff
        };
        c.stroke_rrect_fx(field, rad, FX * 3 / 2, border);
        let isz = self.s(18);
        icons::draw(
            c,
            Icon::Lock,
            field.x + self.s(18),
            field.center_y() - isz / 2,
            isz,
            soft,
            0,
        );
        let tx = field.x + self.s(48);
        let dots = self.login.password.chars().count();
        if self.login.checking != 0 {
            let msg = tr(ru, "Вход…", "Signing in…");
            fonts.draw_centered_y(
                c,
                Weight::Medium,
                self.px(15),
                tx,
                field.center_y(),
                msg,
                soft,
            );
        } else if dots == 0 {
            let ph = tr(ru, "Пароль", "Password");
            fonts.draw_centered_y(
                c,
                Weight::Regular,
                self.px(15),
                tx,
                field.center_y(),
                ph,
                0x99ffffff,
            );
        } else {
            let d = self.s(4);
            for i in 0..dots.min(24) {
                c.fill_circle(tx + d + i as i32 * self.s(13), field.center_y(), d, white);
            }
        }
        if self.login.caret_on && self.login.checking == 0 {
            let cx_ = if dots == 0 {
                tx - self.s(4)
            } else {
                tx + dots.min(24) as i32 * self.s(13)
            };
            c.fill_rect(
                Rect::new(cx_, field.center_y() - self.s(11), self.s(2), self.s(22)),
                t.detail,
            );
        }
        for (e, r) in &elems {
            let hov = self.tw.value(
                HOVER + *e as u64,
                if self.login.hover == Some(*e) { 255 } else { 0 },
                anim::FAST,
            ) as u32;
            match e {
                Elem::Submit => {
                    let r = r.offset(shake, 0);
                    let ready = !self.login.password.is_empty();
                    let bg = if ready {
                        lerp(t.accent, 0xffffffff, (hov / 5) as u8)
                    } else {
                        0x30ffffff
                    };
                    c.fill_circle(r.center_x(), r.center_y(), r.w / 2, bg);
                    let s = self.s(18);
                    let fg = if ready { t.on_accent } else { 0x99ffffff };
                    icons::draw(
                        c,
                        Icon::ArrowRight,
                        r.center_x() - s / 2,
                        r.center_y() - s / 2,
                        s,
                        fg,
                        0,
                    );
                }
                Elem::Restart | Elem::PowerOff => {
                    c.fill_circle(
                        r.center_x(),
                        r.center_y(),
                        r.w / 2,
                        fade(0xffffffff, (0x22 + hov * 0x30 / 255) as u8),
                    );
                    let s = self.s(22);
                    let icon = if *e == Elem::Restart {
                        Icon::Refresh
                    } else {
                        Icon::Power
                    };
                    icons::draw(
                        c,
                        icon,
                        r.center_x() - s / 2,
                        r.center_y() - s / 2,
                        s,
                        white,
                        0,
                    );
                    if hov > 0 {
                        let label = if *e == Elem::Restart {
                            tr(ru, "Перезагрузить", "Restart")
                        } else {
                            tr(ru, "Выключить", "Power off")
                        };
                        let px = self.px(12);
                        let w = fonts.measure(Weight::Medium, px, label);
                        fonts.draw(
                            c,
                            Weight::Medium,
                            px,
                            r.center_x() - w / 2,
                            r.y - self.s(12),
                            label,
                            fade(0xffffffff, hov as u8),
                        );
                    }
                }
                Elem::Prev | Elem::Next => {
                    c.fill_circle(
                        r.center_x(),
                        r.center_y(),
                        r.w / 2,
                        fade(0xffffffff, (0x18 + hov * 0x30 / 255) as u8),
                    );
                    let s = self.s(18);
                    let icon = if *e == Elem::Prev {
                        Icon::ArrowLeft
                    } else {
                        Icon::ArrowRight
                    };
                    icons::draw(
                        c,
                        icon,
                        r.center_x() - s / 2,
                        r.center_y() - s / 2,
                        s,
                        white,
                        0,
                    );
                }
                Elem::Field => {}
            }
        }
        if let Some(err) = self.login.error {
            let px = self.px(14);
            let w = fonts.measure(Weight::Medium, px, err);
            fonts.draw(
                c,
                Weight::Medium,
                px,
                cx - w / 2,
                field.bottom() + self.s(32),
                err,
                lerp(t.error, 0xffffffff, 60),
            );
        }
        // The system name in the corner.
        let px = self.px(13);
        fonts.draw(
            c,
            Weight::SemiBold,
            px,
            self.s(36),
            self.h - self.s(46),
            "Velora OS",
            soft,
        );
        let _ = Exit::Logout;
    }
}

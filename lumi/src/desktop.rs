//! The desktop: windows, the panel, the app menu, the window switcher,
//! the login screen and how they are put together on the screen.
//!
//! Each window has its own picture (title bar and content), drawn again
//! only where it changes. The screen is put together only in the areas
//! that changed: the wallpaper (drawn once), the windows with their
//! shadows, the panel and the menus on top. Moving a window copies its
//! picture; nothing inside it is drawn again. The pointer is not part of
//! the picture: the program draws it on the way to the screen.

use crate::apps::about::About;
use crate::apps::files::{self, Files, Kind};
use crate::apps::settings::Settings;
use crate::apps::terminal::TerminalApp;
use crate::apps::viewer::Viewer;
use crate::apps::{Action, App, AppId, Assets, Cx};
use crate::config::{self, Config};
use crate::gfx::{self, Corner, Shadow};
use crate::host::{DirEntry, Host, Power, UserInfo, join};
use crate::{tz, wallpaper};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use velora_ui::anim::{self, Kind as TransKind, Transition, Tweens};
use velora_ui::canvas::{Canvas, FX, fade};
use velora_ui::icons::{self, Icon};
use velora_ui::text::{Fonts, Weight};
use velora_ui::theme::Theme;
use velora_ui::ui::{Ctx, Key, Modifier, Redraw, mods};
use velora_ui::{Event, Rect, Ui};

/// Pixels per design unit times 256 for a screen height (automatic
/// interface scale): the steps of the installer.
pub fn scale_for(h: i32) -> i64 {
    config::auto_scale(h)
}

/// The settings are written this long after the last change, so a slider
/// moved through many values writes the file once.
const SAVE_DELAY_MS: u64 = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Login,
    Session,
}

/// What the program should do when the desktop is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Logout,
    Power(Power),
}

/// Pointer pictures the desktop asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    Arrow,
    /// Left-right, up-down and the two diagonals.
    ResizeH,
    ResizeV,
    ResizeNwse,
    ResizeNesw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Normal,
    Maximized,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnimKind {
    Open,
    Close,
    Minimize,
    Restore,
    Move,
}

#[derive(Clone, Copy, Debug)]
struct Anim {
    kind: AnimKind,
    from: Rect,
    to: Rect,
    a0: u32,
    a1: u32,
    t0: u64,
    dur: u32,
}

impl Anim {
    fn progress(&self, now: u64) -> u32 {
        let el = now.saturating_sub(self.t0);
        if self.dur == 0 || el >= self.dur as u64 {
            1000
        } else {
            let t = (el * 1000 / self.dur as u64) as u32;
            match self.kind {
                AnimKind::Close | AnimKind::Minimize => anim::ease_in_out(t),
                _ => anim::ease_out(t),
            }
        }
    }
}

/// Title bar buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Btn {
    Minimize,
    Maximize,
    Close,
}

pub(crate) struct Win {
    pub(crate) id: u32,
    pub(crate) app: Box<dyn App>,
    pub(crate) rect: Rect,
    restore: Rect,
    state: State,
    pub(crate) minimized: bool,
    buf: Canvas,
    ui: Ui,
    full: bool,
    hover_btn: Option<Btn>,
    press_btn: Option<Btn>,
    pub(crate) title: String,
    anim: Option<Anim>,
    pub(crate) closing: bool,
    /// Input for this frame, in window coordinates.
    events: Vec<Event>,
    /// Where the last frame showed it (with its shadow) and whether it
    /// had the focus (the shadow is darker): drawn again when it changes.
    drawn: Option<(Rect, bool)>,
}

impl Win {
    /// Minimized, closing, or on the way to either.
    fn hidden(&self) -> bool {
        self.minimized || self.closing || self.anim.is_some_and(|a| a.kind == AnimKind::Minimize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Move,
    /// Bits: 1 left, 2 right, 4 top, 8 bottom.
    Resize(u8),
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    win: u32,
    edge: Edge,
    start: (i32, i32),
    rect: Rect,
    moved: bool,
}

/// A small window above the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PopKind {
    Clock,
    Network,
    Volume,
}

pub(crate) struct Popover {
    pub(crate) kind: PopKind,
    pub(crate) t0: u64,
    pub(crate) closing: bool,
}

pub(crate) struct Switcher {
    pub(crate) order: Vec<u32>,
    pub(crate) sel: usize,
}

pub struct Desktop {
    pub w: i32,
    pub h: i32,
    pub scale: i64,
    pub cfg: Config,
    pub theme: Theme,
    pub assets: Assets,
    pub(crate) wall: Canvas,
    pub(crate) frost: wallpaper::Frost,
    /// The wallpaper, dark theme, dimming and blur it was made for.
    wall_key: (String, bool, u32, u32),
    /// The wallpaper before dimming and blur (only while they are on).
    wall_base: Option<Canvas>,
    pub mode: Mode,
    pub(crate) windows: Vec<Win>,
    next_id: u32,
    drag: Option<Drag>,
    pub(crate) snap: Option<Rect>,
    /// Window that has the keyboard (the top one, unless minimized).
    pub(crate) focus: Option<u32>,
    capture: Option<u32>,
    hover_win: Option<u32>,
    pub(crate) mouse: (i32, i32),
    damage: Vec<Rect>,
    pub(crate) tw: Tweens,
    pub(crate) clipboard: String,
    transition: Option<Transition>,
    pending_transition: bool,
    pub user: Option<UserInfo>,
    pub(crate) users: Vec<UserInfo>,
    pub exit: Option<Exit>,
    pub(crate) shadow: Shadow,
    pub(crate) shadow_small: Shadow,
    pub(crate) corner: Corner,
    pub(crate) super_alone: bool,
    pub(crate) mods: u8,
    pub(crate) session_t0: u64,
    last_title_click: (u64, u32),
    pub pointer: Pointer,
    pub(crate) now: u64,
    pub(crate) minute: i64,
    // The panel, the app menu, the switcher and small windows.
    pub(crate) panel_hover: Option<usize>,
    pub(crate) panel_hover_t: u64,
    pub(crate) panel_press: Option<usize>,
    pub(crate) launcher: Option<crate::launcher::Launcher>,
    pub(crate) switcher: Option<Switcher>,
    pub(crate) popover: Option<Popover>,
    pub(crate) pop_hover: Option<usize>,
    pub(crate) login: crate::login::Login,
    pub(crate) net: crate::host::NetState,
    net_next: u64,
    /// The last frame took this long (us), for the frame counter.
    pub frame_us: u64,
    /// A progress line to print at the next frame.
    pub(crate) host_log_pending: Option<&'static str>,
    /// Tweens moved in the last frame: draw their areas again.
    pub(crate) anim_prev: bool,
    /// Ctrl+Alt+D: every area drawn again is tinted, each frame in the
    /// next colour; areas not drawn keep theirs.
    pub debug_damage: bool,
    /// Frames drawn (the tint colour), and areas drawn since start.
    pub frames: u64,
    pub painted: u64,
    /// The settings changed: write them at this time.
    save_at: Option<u64>,
    /// The work area changed: maximized and snapped windows fit it again.
    pub(crate) refit: bool,
    /// The interface scale changed: lay the windows out again.
    relayout: bool,
    /// The panel hiding and showing.
    pub(crate) panel_hide: crate::panel::Hide,
}

impl Desktop {
    /// The desktop for a `w` x `h` screen, starting at the login screen.
    pub fn new(w: i32, h: i32, mut cfg: Config, assets: Assets, host: &mut dyn Host) -> Desktop {
        let scale = cfg.scale_for(h);
        let theme = cfg.theme();
        let s = |d: i32| ((d as i64 * scale + 128) >> 8) as i32;
        if !wallpaper::is_builtin(&cfg.wallpaper) && !cfg.wallpaper.starts_with("file:") {
            cfg.wallpaper = String::from("waves");
        }
        let users = host.users();
        let rad = s(cfg.radius as i32);
        let mut d = Desktop {
            w,
            h,
            scale,
            theme,
            assets,
            wall: Canvas::new(1, 1),
            frost: wallpaper::frost(&Canvas::new(8, 8), 0),
            wall_key: (String::new(), false, 0, 0),
            wall_base: None,
            mode: Mode::Login,
            windows: Vec::new(),
            next_id: 1,
            drag: None,
            snap: None,
            focus: None,
            capture: None,
            hover_win: None,
            mouse: (w / 2, h / 2),
            damage: vec![Rect::new(0, 0, w, h)],
            tw: Tweens::default(),
            clipboard: String::new(),
            transition: None,
            pending_transition: false,
            user: None,
            login: crate::login::Login::new(&users),
            users,
            exit: None,
            shadow: Shadow::new(rad, s(26)),
            shadow_small: Shadow::new(s(16), s(14)),
            corner: Corner::new(rad),
            super_alone: false,
            mods: 0,
            session_t0: 0,
            last_title_click: (0, 0),
            pointer: Pointer::Arrow,
            now: host.now_ms(),
            minute: -1,
            panel_hover: None,
            panel_hover_t: 0,
            panel_press: None,
            launcher: None,
            switcher: None,
            popover: None,
            pop_hover: None,
            net: host.net(),
            net_next: 0,
            frame_us: 0,
            host_log_pending: None,
            anim_prev: false,
            debug_damage: false,
            frames: 0,
            painted: 0,
            save_at: None,
            refit: false,
            relayout: false,
            panel_hide: crate::panel::Hide::default(),
            cfg,
        };
        d.tw.enabled = d.cfg.animations_on();
        d.load_wallpaper(host);
        d
    }

    pub fn s(&self, du: i32) -> i32 {
        ((du as i64 * self.scale + 128) >> 8) as i32
    }

    pub fn px(&self, du: i32) -> u32 {
        self.s(du).max(6) as u32
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }

    pub fn ru(&self) -> bool {
        self.cfg.ru()
    }

    /// Duration of an animation at the chosen speed, 0 with animations
    /// off.
    pub(crate) fn dur(&self, ms: u32) -> u32 {
        self.cfg.dur(ms)
    }

    /// The window corner radius in pixels.
    pub(crate) fn radius(&self) -> i32 {
        self.s(self.cfg.radius as i32)
    }

    pub fn add_damage(&mut self, r: Rect) {
        let r = r.intersect(self.bounds());
        if !r.is_empty() {
            self.damage.push(r);
        }
    }

    pub fn damage_all(&mut self) {
        self.damage.clear();
        self.damage.push(self.bounds());
    }

    /// The area windows use: the screen without the panel's strip when
    /// the panel is always shown; all of it when the panel hides.
    pub(crate) fn work_area(&self) -> Rect {
        let reserve = if self.mode == Mode::Session && self.cfg.panel_mode == config::PANEL_ALWAYS {
            self.panel_rect().h + 2 * self.s(10)
        } else {
            0
        };
        Rect::new(0, 0, self.w, self.h - reserve)
    }

    // --- wallpaper --------------------------------------------------------

    fn load_wallpaper(&mut self, host: &mut dyn Host) {
        let key = (
            self.cfg.wallpaper.clone(),
            self.theme.dark,
            self.cfg.wallpaper_dim,
            self.cfg.wallpaper_blur,
        );
        if key == self.wall_key {
            return;
        }
        let (w, h) = (self.w as usize, self.h as usize);
        let t0 = host.now_ms();
        if key.0 != self.wall_key.0 || self.wall.width != w {
            let mut c = Canvas::new(w, h);
            let mut ok = false;
            if let Some(path) = key.0.strip_prefix("file:")
                && let Some(img) = host
                    .read(path)
                    .and_then(|d| velora_gfx::png::decode(&d, 40 << 20).ok())
            {
                c = wallpaper::cover(&img, w, h);
                ok = true;
            }
            if !ok {
                let id = if wallpaper::is_builtin(&key.0) {
                    key.0.as_str()
                } else {
                    "waves"
                };
                wallpaper::draw_builtin(&mut c, id);
            }
            self.wall = c;
            self.wall_base = None;
        }
        // Dimming and blur work on the original picture, kept aside.
        if key.2 > 0 || key.3 > 0 {
            let base = self
                .wall_base
                .take()
                .unwrap_or_else(|| core::mem::replace(&mut self.wall, Canvas::new(1, 1)));
            self.wall = wallpaper::soften(&base, key.3, key.2);
            self.wall_base = Some(base);
        } else if let Some(base) = self.wall_base.take() {
            self.wall = base;
        }
        let tint = if self.theme.dark {
            0x58000000
        } else {
            0x38ffffff
        };
        self.frost = wallpaper::frost(&self.wall, tint);
        self.wall_key = key;
        let line = alloc::format!(
            "lumi: wallpaper {} in {} ms",
            self.cfg.wallpaper,
            host.now_ms() - t0
        );
        host.log(&line);
    }

    // --- settings ---------------------------------------------------------

    /// Applies new settings with a transition and saves them.
    pub fn apply_config(&mut self, cfg: Config, host: &mut dyn Host, save: bool) {
        let old = core::mem::replace(&mut self.cfg, cfg);
        self.theme = self.cfg.theme();
        self.tw.enabled = self.cfg.animations_on();
        let scale = self.cfg.scale_for(self.h);
        if scale != self.scale {
            self.scale = scale;
            self.relayout = true;
        }
        if old.radius != self.cfg.radius || self.relayout {
            let rad = self.radius();
            self.shadow = Shadow::new(rad, self.s(26));
            self.shadow_small = Shadow::new(self.s(16), self.s(14));
            self.corner = Corner::new(rad);
        }
        self.load_wallpaper(host);
        for w in self.windows.iter_mut() {
            w.full = true;
            w.app.settings_changed();
        }
        if old.animations_on() || self.cfg.animations_on() {
            self.pending_transition = true;
        }
        if old.panel_mode != self.cfg.panel_mode {
            self.panel_mode_changed(host);
        }
        self.damage_all();
        if save {
            self.save_at = Some(self.now + SAVE_DELAY_MS);
        }
    }

    /// Writes the settings to the user's file now if they changed.
    pub fn flush_settings(&mut self, host: &mut dyn Host) {
        if self.save_at.take().is_none() {
            return;
        }
        let Some(u) = self.user.as_ref() else {
            return;
        };
        let _ = host.mkdir(&join(&u.home, ".config"));
        let _ = host.mkdir(&join(&u.home, config::USER_DIR));
        let path = join(&u.home, config::USER_FILE);
        match host.write(&path, self.cfg.to_text().as_bytes()) {
            Ok(()) => host.log("lumi: settings saved"),
            Err(e) => host.log(&alloc::format!("lumi: cannot save settings: {}", e)),
        }
    }

    /// After a new interface scale: the windows get new widgets and lay
    /// out their content again.
    fn relayout_windows(&mut self, host: &mut dyn Host, fonts: &mut Fonts) {
        self.relayout = false;
        let work = self.work_area();
        for i in 0..self.windows.len() {
            let mut w = self.windows.remove(i);
            let mut ui = Ui::new(self.scale);
            ui.theme = self.theme;
            ui.set_animations(self.cfg.animations_on());
            w.ui = ui;
            let min = w.app.size().1;
            let (mw, mh) = (self.s(min.0).min(work.w), self.s(min.1).min(work.h));
            w.rect.w = w.rect.w.max(mw);
            w.rect.h = w.rect.h.max(mh);
            if w.state == State::Maximized {
                w.rect = work;
            }
            // A new picture, so the app lays out again.
            w.buf = Canvas::new(1, 1);
            self.resize_buffer(&mut w, host, fonts);
            self.windows.insert(i, w);
        }
        self.damage_all();
    }

    // --- windows ----------------------------------------------------------

    fn win_index(&self, id: u32) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    pub(crate) fn title_h(&self) -> i32 {
        self.s(40)
    }

    /// Where a window is drawn now and how opaque.
    fn shown(&self, w: &Win) -> Option<(Rect, u32)> {
        if let Some(a) = w.anim {
            let p = a.progress(self.now);
            let r = gfx::lerp_rect(a.from, a.to, p);
            let alpha = (a.a0 as i64 + (a.a1 as i64 - a.a0 as i64) * p as i64 / 1000) as u32;
            return Some((r, alpha));
        }
        if w.minimized {
            None
        } else {
            Some((w.rect, 255))
        }
    }

    /// The screen area a window shown at `r` covers with its shadow.
    pub(crate) fn extent(&self, r: Rect) -> Rect {
        r.union(r.offset(0, self.s(10)).inset(-self.shadow.pad))
    }

    /// Damages the old and the new place of every window that moved,
    /// changed size, opacity or focus since the last frame, and the place
    /// of windows that are gone. Every frame of an animation is erased
    /// this way, whatever changed the window.
    fn sync_windows(&mut self) {
        for i in 0..self.windows.len() {
            let w = &self.windows[i];
            let now = self
                .shown(w)
                .map(|(r, _)| (self.extent(r), self.focus == Some(w.id)));
            let (old, animating) = (w.drawn, w.anim.is_some());
            if now != old || animating {
                if let Some((r, _)) = old {
                    self.add_damage(r);
                }
                if let Some((r, _)) = now {
                    self.add_damage(r);
                }
                self.windows[i].drawn = now;
            }
        }
    }

    fn window_radius(&self, w: &Win) -> i32 {
        if w.state == State::Normal {
            self.radius()
        } else {
            0
        }
    }

    /// Opens an app in a new window.
    pub fn open(&mut self, app: Box<dyn App>, host: &mut dyn Host, fonts: &mut Fonts) -> u32 {
        let ((dw, dh), _) = app.size();
        let work = self.work_area();
        let (ww, wh) = (
            self.s(dw).min(work.w - self.s(40)),
            self.s(dh).min(work.h - self.s(40)),
        );
        let n = self.windows.iter().filter(|w| !w.closing).count() as i32;
        let off = self.s(32) * (n % 6);
        let x =
            (work.x + (work.w - ww) / 2 + off - self.s(80)).clamp(0, (work.right() - ww).max(0));
        let y =
            (work.y + (work.h - wh) / 2 + off - self.s(60)).clamp(0, (work.bottom() - wh).max(0));
        let rect = Rect::new(x, y, ww, wh);
        let id = self.next_id;
        self.next_id += 1;
        let mut ui = Ui::new(self.scale);
        ui.theme = self.theme;
        ui.set_animations(self.cfg.animations_on());
        let title = app.title(self.ru());
        let mut win = Win {
            id,
            app,
            rect,
            restore: rect,
            state: State::Normal,
            minimized: false,
            buf: Canvas::new(1, 1),
            ui,
            full: true,
            hover_btn: None,
            press_btn: None,
            title,
            anim: None,
            closing: false,
            events: Vec::new(),
            drawn: None,
        };
        self.resize_buffer(&mut win, host, fonts);
        let d = self.dur(220);
        win.anim = Some(Anim {
            kind: AnimKind::Open,
            from: gfx::scale_rect(rect, 920),
            to: rect,
            a0: 0,
            a1: 255,
            t0: self.now,
            dur: d,
        });
        let name = alloc::format!("lumi: open {}", win.title);
        host.log(&name);
        self.windows.push(win);
        self.set_focus(Some(id));
        self.add_damage(self.extent(rect));
        id
    }

    fn content_rect(&self, w: &Win) -> Rect {
        let tb = self.title_h();
        Rect::new(0, tb, w.rect.w, w.rect.h - tb)
    }

    fn resize_buffer(&self, w: &mut Win, host: &mut dyn Host, fonts: &mut Fonts) {
        if w.buf.width != w.rect.w as usize || w.buf.height != w.rect.h as usize {
            w.buf = Canvas::new(w.rect.w.max(1) as usize, w.rect.h.max(1) as usize);
            let cr = self.content_rect(w);
            w.app.resized(host, fonts, &w.ui, cr);
        }
        w.full = true;
    }

    pub(crate) fn set_focus(&mut self, id: Option<u32>) {
        if self.focus == id {
            return;
        }
        for w in self.windows.iter_mut() {
            if Some(w.id) == self.focus || Some(w.id) == id {
                w.full = true;
            }
        }
        self.focus = id;
        self.panel_changed();
    }

    /// Brings a window to the top and gives it the keyboard.
    pub(crate) fn raise(&mut self, id: u32) {
        if let Some(i) = self.win_index(id) {
            let w = self.windows.remove(i);
            let r = w.rect;
            self.windows.push(w);
            self.add_damage(self.extent(r));
        }
        self.set_focus(Some(id));
    }

    fn top_visible(&self) -> Option<u32> {
        self.windows
            .iter()
            .rev()
            .find(|w| !w.hidden())
            .map(|w| w.id)
    }

    pub(crate) fn close_window(&mut self, id: u32, host: &mut dyn Host) {
        let d = self.dur(170);
        let now = self.now;
        let Some(i) = self.win_index(id) else {
            return;
        };
        let w = &mut self.windows[i];
        if w.closing {
            return;
        }
        w.app.close(host);
        w.closing = true;
        let from = self
            .shown(&self.windows[i])
            .map_or(self.windows[i].rect, |s| s.0);
        let w = &mut self.windows[i];
        w.anim = Some(Anim {
            kind: AnimKind::Close,
            from,
            to: gfx::scale_rect(from, 940),
            a0: 255,
            a1: 0,
            t0: now,
            dur: d,
        });
        host.log(&alloc::format!("lumi: close {}", w.title));
        if self.focus == Some(id) {
            let next = self.top_visible();
            self.set_focus(next);
        }
        self.panel_changed();
    }

    pub(crate) fn minimize(&mut self, id: u32) {
        let d = self.dur(240);
        let target = self.panel_icon_rect_for(id);
        let now = self.now;
        let Some(i) = self.win_index(id) else {
            return;
        };
        let from = self.windows[i].rect;
        let to = match target {
            Some(t) => {
                let w = from.w / 8;
                let h = from.h / 8;
                Rect::new(t.center_x() - w / 2, t.center_y() - h / 2, w, h)
            }
            None => gfx::scale_rect(from, 300),
        };
        let w = &mut self.windows[i];
        w.anim = Some(Anim {
            kind: AnimKind::Minimize,
            from,
            to,
            a0: 255,
            a1: 0,
            t0: now,
            dur: d,
        });
        self.add_damage(self.extent(from));
        if self.focus == Some(id) {
            let next = self
                .windows
                .iter()
                .rev()
                .find(|w| w.id != id && !w.hidden())
                .map(|w| w.id);
            self.set_focus(next);
        }
        self.panel_changed();
    }

    pub(crate) fn unminimize(&mut self, id: u32) {
        let d = self.dur(240);
        let target = self.panel_icon_rect_for(id);
        let now = self.now;
        if let Some(i) = self.win_index(id) {
            let w = &mut self.windows[i];
            if w.minimized || w.anim.is_some_and(|a| a.kind == AnimKind::Minimize) {
                let to = w.rect;
                let from = match target {
                    Some(t) => Rect::new(
                        t.center_x() - to.w / 16,
                        t.center_y() - to.h / 16,
                        to.w / 8,
                        to.h / 8,
                    ),
                    None => gfx::scale_rect(to, 300),
                };
                w.minimized = false;
                w.anim = Some(Anim {
                    kind: AnimKind::Restore,
                    from,
                    to,
                    a0: 0,
                    a1: 255,
                    t0: now,
                    dur: d,
                });
            }
        }
        self.raise(id);
    }

    /// Moves a window to a new place and size, animated.
    fn set_geometry(
        &mut self,
        id: u32,
        to: Rect,
        state: State,
        host: &mut dyn Host,
        fonts: &mut Fonts,
    ) {
        let d = self.dur(220);
        let now = self.now;
        let Some(i) = self.win_index(id) else {
            return;
        };
        let mut w = self.windows.remove(i);
        let from = w.rect;
        if w.state == State::Normal && state != State::Normal {
            w.restore = w.rect;
        }
        w.state = state;
        w.rect = to;
        self.resize_buffer(&mut w, host, fonts);
        w.anim = Some(Anim {
            kind: AnimKind::Move,
            from,
            to,
            a0: 255,
            a1: 255,
            t0: now,
            dur: d,
        });
        self.windows.insert(i, w);
        self.add_damage(self.extent(from));
        self.add_damage(self.extent(to));
    }

    fn toggle_maximize(&mut self, id: u32, host: &mut dyn Host, fonts: &mut Fonts) {
        let Some(i) = self.win_index(id) else {
            return;
        };
        let (state, restore) = (self.windows[i].state, self.windows[i].restore);
        if state == State::Normal {
            let work = self.work_area();
            self.set_geometry(id, work, State::Maximized, host, fonts);
        } else {
            self.set_geometry(id, restore, State::Normal, host, fonts);
        }
    }

    fn snap_rect(&self, state: State) -> Rect {
        let wa = self.work_area();
        match state {
            State::Left => Rect::new(wa.x, wa.y, wa.w / 2, wa.h),
            State::Right => Rect::new(wa.x + wa.w / 2, wa.y, wa.w - wa.w / 2, wa.h),
            _ => wa,
        }
    }

    /// Which part of the windows is at (x, y): window id and what.
    fn hit(&self, x: i32, y: i32) -> Option<(u32, Edge, Option<Btn>)> {
        let grip = self.s(7);
        for w in self.windows.iter().rev() {
            if w.minimized || w.closing {
                continue;
            }
            let r = w.rect;
            if !r.inset(-grip).contains(x, y) {
                continue;
            }
            if w.state != State::Maximized {
                let mut e = 0u8;
                if x < r.x + grip / 2 {
                    e |= 1;
                }
                if x >= r.right() - grip / 2 {
                    e |= 2;
                }
                if y < r.y + grip / 2 {
                    e |= 4;
                }
                if y >= r.bottom() - grip / 2 {
                    e |= 8;
                }
                if e != 0 {
                    return Some((w.id, Edge::Resize(e), None));
                }
            }
            if !r.contains(x, y) {
                continue;
            }
            if y < r.y + self.title_h() {
                let btn = self.button_at(w, x - r.x, y - r.y);
                return Some((w.id, Edge::Move, btn));
            }
            // The content: `Resize(0)` means no edge.
            return Some((w.id, Edge::Resize(0), None));
        }
        None
    }

    /// The title bar buttons: on the right (minimize, maximize, close)
    /// or on the left (close, minimize, maximize).
    fn buttons(&self, w: &Win) -> [(Btn, Rect); 3] {
        let d = self.s(26);
        let tb = self.title_h();
        let y = (tb - d) / 2;
        let gap = self.s(8);
        if self.cfg.buttons_left {
            let x = self.s(10);
            return [
                (Btn::Close, Rect::new(x, y, d, d)),
                (Btn::Minimize, Rect::new(x + d + gap, y, d, d)),
                (Btn::Maximize, Rect::new(x + 2 * (d + gap), y, d, d)),
            ];
        }
        let right = w.rect.w - self.s(10);
        [
            (Btn::Minimize, Rect::new(right - 3 * d - 2 * gap, y, d, d)),
            (Btn::Maximize, Rect::new(right - 2 * d - gap, y, d, d)),
            (Btn::Close, Rect::new(right - d, y, d, d)),
        ]
    }

    fn button_at(&self, w: &Win, lx: i32, ly: i32) -> Option<Btn> {
        self.buttons(w)
            .iter()
            .find(|(_, r)| r.inset(-self.s(3)).contains(lx, ly))
            .map(|(b, _)| *b)
    }

    /// Draws a window's title bar into its picture.
    fn draw_chrome(&self, w: &mut Win, fonts: &mut Fonts) {
        let t = self.theme;
        let focused = self.focus == Some(w.id);
        let tb = self.title_h();
        let bar = Rect::new(0, 0, w.rect.w, tb);
        let buttons = self.buttons(w);
        let c = &mut w.buf;
        c.fill_rect(bar, t.window);
        let title_col = if focused { t.text } else { t.text_faint };
        let icon_col = if focused { t.text_dim } else { t.text_faint };
        let isz = self.s(16);
        // The app's icon on the side without the buttons.
        let left = self.cfg.buttons_left;
        let ix = if left {
            w.rect.w - self.s(16) - isz
        } else {
            self.s(16)
        };
        icons::draw(c, w.app.id().icon(), ix, (tb - isz) / 2, isz, icon_col, 0);
        let px = self.px(13);
        let tw = fonts.measure(Weight::Medium, px, &w.title);
        let (lo, hi) = if left {
            (buttons[2].1.right() + self.s(12), ix - self.s(8))
        } else {
            (self.s(40), buttons[0].1.x - self.s(12))
        };
        let tx = ((w.rect.w - tw) / 2).max(lo);
        let old = c.set_clip(Rect::new(lo, 0, (hi - lo).max(0), tb));
        fonts.draw_centered_y(c, Weight::Medium, px, tx, tb / 2, &w.title, title_col);
        c.set_clip(old);
        for (b, r) in buttons {
            let hov = w.hover_btn == Some(b);
            let pressed = w.press_btn == Some(b);
            let (bg, fg) = match (b, hov) {
                (Btn::Close, true) => (if pressed { 0xffc8403a } else { 0xffe5534b }, 0xffffffff),
                (_, true) => (
                    if pressed {
                        t.field_pressed
                    } else {
                        t.field_hover
                    },
                    t.text,
                ),
                _ => (
                    fade(t.field, if focused { 255 } else { 150 }),
                    if focused { t.text_dim } else { t.text_faint },
                ),
            };
            c.fill_circle(r.center_x(), r.center_y(), r.w / 2, bg);
            let icon = match b {
                Btn::Minimize => Icon::Minimize,
                Btn::Maximize if w.state != State::Normal => Icon::Restore,
                Btn::Maximize => Icon::Maximize,
                Btn::Close => Icon::Close,
            };
            let s = self.s(14);
            icons::draw(
                c,
                icon,
                r.center_x() - s / 2,
                r.center_y() - s / 2,
                s,
                fg,
                (self.s(2) as i64 * FX * 3 / 4).max(FX),
            );
        }
        // A thin line under the title bar.
        c.fill_rect(
            Rect::new(0, tb - 1, w.rect.w, 1),
            fade(t.window_border, 160),
        );
    }

    /// Draws the window's picture again where needed; returns the area
    /// of the picture that changed.
    fn render_window(
        &mut self,
        i: usize,
        host: &mut dyn Host,
        fonts: &mut Fonts,
        actions: &mut Vec<(u32, Action)>,
    ) -> Option<Rect> {
        let mut w = self.windows.remove(i);
        let now = self.now;
        let polled = w.app.poll(host, now);
        let title = w.app.title(self.ru());
        if title != w.title {
            w.title = title;
            w.full = true;
            self.panel_changed();
        }
        let events = core::mem::take(&mut w.events);
        w.ui.theme = self.theme;
        w.ui.set_animations(self.cfg.animations_on());
        w.ui.set_time(now);
        let all = Rect::new(0, 0, w.rect.w, w.rect.h);
        let content = self.content_rect(&w);
        let focused = self.focus == Some(w.id);
        let mut out = Vec::new();
        let area = if w.full {
            Some(all)
        } else if w.app.uses_ui() {
            if polled {
                Some(all)
            } else if events.is_empty() && !w.ui.animating() {
                None
            } else {
                match w.ui.plan(&events, now) {
                    Redraw::All => Some(all),
                    Redraw::Area(r) => Some(r),
                    Redraw::Nothing => {
                        w.ui.track_pointer(&events);
                        None
                    }
                }
            }
        } else {
            let mut cx = Cx {
                ui: &mut w.ui,
                c: &mut w.buf,
                f: fonts,
                host,
                assets: &mut self.assets,
                cfg: &self.cfg,
                theme: self.theme,
                events: &events,
                focused,
                now,
                ru: self.cfg.ru(),
                out: &mut out,
                clipboard: &self.clipboard,
            };
            w.app.input(&events, &mut cx, content);
            w.app.damage(content)
        };
        let mut changed = None;
        if let Some(area) = area.map(|a| a.intersect(all)).filter(|a| !a.is_empty()) {
            w.buf.set_frame(area);
            if area.intersect(Rect::new(0, 0, all.w, self.title_h())) != Rect::default() {
                self.draw_chrome(&mut w, fonts);
            }
            let old = w.buf.set_clip(content);
            if w.app.uses_ui() {
                w.ui.begin(&events);
            }
            let mut cx = Cx {
                ui: &mut w.ui,
                c: &mut w.buf,
                f: fonts,
                host,
                assets: &mut self.assets,
                cfg: &self.cfg,
                theme: self.theme,
                events: &events,
                focused,
                now,
                ru: self.cfg.ru(),
                out: &mut out,
                clipboard: &self.clipboard,
            };
            w.app.draw(&mut cx, content);
            if w.app.uses_ui() {
                let again = Ctx {
                    ui: &mut w.ui,
                    c: &mut w.buf,
                    f: fonts,
                }
                .end();
                let more = w.ui.end_frame();
                if again || more {
                    w.full = true;
                }
            }
            w.buf.set_clip(old);
            // The border, over the content edges.
            let rad = self.window_radius(&w);
            if area.x <= 1 || area.y <= 1 || area.right() >= all.w - 1 || area.bottom() >= all.h - 1
            {
                w.buf
                    .stroke_rrect_fx(all, rad, FX, self.theme.window_border);
            }
            w.buf.set_frame(all);
            changed = Some(area);
            if !w.full || area == all {
                w.full = w.full && area != all;
            }
        } else if w.app.uses_ui() && !events.is_empty() {
            w.ui.track_pointer(&events);
        }
        let id = w.id;
        self.windows.insert(i, w);
        for a in out {
            actions.push((id, a));
        }
        changed
    }

    // --- actions ------------------------------------------------------------

    fn home(&self) -> String {
        self.user
            .as_ref()
            .map_or(String::from("/"), |u| u.home.clone())
    }

    /// Starts an app (a second Settings or About focuses the first).
    pub fn launch(&mut self, app: AppId, host: &mut dyn Host, fonts: &mut Fonts) {
        if matches!(app, AppId::Settings | AppId::About)
            && let Some(w) = self
                .windows
                .iter()
                .find(|w| w.app.id() == app && !w.closing)
        {
            let id = w.id;
            self.unminimize(id);
            return;
        }
        let b: Box<dyn App> = match app {
            AppId::Terminal => Box::new(TerminalApp::new(
                AppId::Terminal,
                vec![String::from("/bin/vsh")],
            )),
            AppId::Plume => Box::new(TerminalApp::new(
                AppId::Plume,
                vec![String::from("/bin/plume")],
            )),
            AppId::Files => {
                let home = self.home();
                Box::new(Files::new(host, &home, &home, &self.cfg.region))
            }
            AppId::Settings => {
                let home = self.home();
                Box::new(Settings::new(host, &home))
            }
            AppId::About => Box::new(About::new(host, (self.w, self.h))),
            AppId::Viewer => return,
        };
        self.open(b, host, fonts);
    }

    /// Opens a path with the app that fits it.
    pub fn open_path(&mut self, path: &str, host: &mut dyn Host, fonts: &mut Fonts) {
        let name = String::from(path.rsplit('/').next().unwrap_or(path));
        let entry = host
            .list(&crate::host::parent(path))
            .ok()
            .and_then(|v| v.into_iter().find(|e| e.name == name))
            .unwrap_or(DirEntry {
                name,
                ..DirEntry::default()
            });
        let b: Box<dyn App> = match files::kind_of(&entry) {
            Kind::Folder => {
                let home = self.home();
                Box::new(Files::new(host, path, &home, &self.cfg.region))
            }
            Kind::Picture => Box::new(Viewer::open(host, path)),
            Kind::Program => Box::new(TerminalApp::new(AppId::Terminal, vec![String::from(path)])),
            Kind::Text | Kind::Other => Box::new(TerminalApp::new(
                AppId::Plume,
                vec![String::from("/bin/plume"), String::from(path)],
            )),
        };
        self.open(b, host, fonts);
    }

    fn run_action(&mut self, from: u32, a: Action, host: &mut dyn Host, fonts: &mut Fonts) {
        match a {
            Action::Launch(app) => self.launch(app, host, fonts),
            Action::Open(p) => self.open_path(&p, host, fonts),
            Action::Browse(p) => {
                let home = self.home();
                let f = Files::new(host, &p, &home, &self.cfg.region);
                self.open(Box::new(f), host, fonts);
            }
            Action::Run { argv, app } => {
                let t = TerminalApp::new(app, argv);
                self.open(Box::new(t), host, fonts);
            }
            Action::Config(c) => self.apply_config(*c, host, true),
            Action::Copy(s) => self.clipboard = s,
            Action::Close => self.close_window(from, host),
        }
    }

    // --- input ------------------------------------------------------------

    fn send_to_window(&mut self, id: u32, ev: Event) {
        if let Some(i) = self.win_index(id) {
            let r = self.windows[i].rect;
            let ev = match ev {
                Event::MouseMove { x, y } => Event::MouseMove {
                    x: x - r.x,
                    y: y - r.y,
                },
                e => e,
            };
            self.windows[i].events.push(ev);
        }
    }

    fn focused_window(&self) -> Option<u32> {
        self.focus.filter(|id| {
            self.windows
                .iter()
                .any(|w| w.id == *id && !w.minimized && !w.closing)
        })
    }

    /// Shortcuts that work everywhere; true if the event was one.
    fn shortcut(&mut self, ev: &Event, host: &mut dyn Host, fonts: &mut Fonts) -> bool {
        match *ev {
            Event::Modifier { key, down, mods: m } => {
                self.mods = m;
                if key == Modifier::Super {
                    if down {
                        self.super_alone = true;
                    } else if self.super_alone {
                        self.super_alone = false;
                        self.toggle_launcher();
                    }
                }
                if key == Modifier::Alt && !down && self.switcher.is_some() {
                    self.finish_switcher();
                }
                true
            }
            Event::Chord { key, mods: m } => {
                self.super_alone = false;
                let alt = m & mods::ALT != 0;
                let sup = m & mods::SUPER != 0;
                let ctrl = m & mods::CTRL != 0;
                match key {
                    Key::Tab if alt => {
                        self.switch_step(m & mods::SHIFT != 0);
                        true
                    }
                    Key::F(4) if alt => {
                        if let Some(id) = self.focused_window() {
                            self.close_window(id, host);
                        }
                        true
                    }
                    Key::Char('d') if ctrl && alt => {
                        self.debug_damage = !self.debug_damage;
                        host.log(if self.debug_damage {
                            "lumi: damage debug on"
                        } else {
                            "lumi: damage debug off"
                        });
                        self.damage_all();
                        true
                    }
                    Key::Char('t') if ctrl && alt => {
                        self.launch(AppId::Terminal, host, fonts);
                        true
                    }
                    Key::Char('e') if sup => {
                        self.launch(AppId::Files, host, fonts);
                        true
                    }
                    Key::Up if sup => {
                        if let Some(id) = self.focused_window()
                            && let Some(i) = self.win_index(id)
                            && self.windows[i].state != State::Maximized
                        {
                            let wa = self.work_area();
                            self.set_geometry(id, wa, State::Maximized, host, fonts);
                        }
                        true
                    }
                    Key::Down if sup => {
                        if let Some(id) = self.focused_window()
                            && let Some(i) = self.win_index(id)
                        {
                            if self.windows[i].state != State::Normal {
                                let r = self.windows[i].restore;
                                self.set_geometry(id, r, State::Normal, host, fonts);
                            } else {
                                self.minimize(id);
                            }
                        }
                        true
                    }
                    Key::Left | Key::Right if sup => {
                        if let Some(id) = self.focused_window() {
                            let st = if key == Key::Left {
                                State::Left
                            } else {
                                State::Right
                            };
                            let r = self.snap_rect(st);
                            self.set_geometry(id, r, st, host, fonts);
                        }
                        true
                    }
                    _ if sup => true,
                    _ => false,
                }
            }
            Event::Key { .. } => {
                self.super_alone = false;
                false
            }
            _ => false,
        }
    }

    fn update_pointer(&mut self) {
        let (x, y) = self.mouse;
        let edge = match self.drag {
            Some(d) => Some(d.edge),
            None => {
                if self.launcher.is_some() || self.mode == Mode::Login {
                    None
                } else {
                    self.hit(x, y).map(|h| h.1)
                }
            }
        };
        self.pointer = match edge {
            Some(Edge::Resize(e)) => match e {
                1 | 2 => Pointer::ResizeH,
                4 | 8 => Pointer::ResizeV,
                5 | 10 => Pointer::ResizeNwse,
                6 | 9 => Pointer::ResizeNesw,
                _ => Pointer::Arrow,
            },
            _ => Pointer::Arrow,
        };
    }

    fn mouse_move(&mut self, x: i32, y: i32, host: &mut dyn Host, fonts: &mut Fonts) {
        self.mouse = (x, y);
        if let Some(mut d) = self.drag {
            let (dx, dy) = (x - d.start.0, y - d.start.1);
            if dx.abs() + dy.abs() > self.s(3) {
                d.moved = true;
            }
            self.drag = Some(d);
            if !d.moved {
                return;
            }
            let Some(i) = self.win_index(d.win) else {
                self.drag = None;
                return;
            };
            let old = self.windows[i].rect;
            let min = self.windows[i].app.size().1;
            let (mw, mh) = (self.s(min.0), self.s(min.1));
            let mut r = d.rect;
            match d.edge {
                Edge::Move => {
                    if self.windows[i].state != State::Normal {
                        // Dragging a maximized window: back to its size,
                        // under the pointer at the same relative place.
                        let rest = self.windows[i].restore;
                        let fx =
                            (d.start.0 - d.rect.x) as i64 * rest.w as i64 / d.rect.w.max(1) as i64;
                        let nr = Rect::new(x - fx as i32, y - self.title_h() / 2, rest.w, rest.h);
                        self.windows[i].state = State::Normal;
                        self.windows[i].rect = nr;
                        let mut w = self.windows.remove(i);
                        self.resize_buffer(&mut w, host, fonts);
                        self.windows.insert(i, w);
                        self.drag = Some(Drag {
                            win: d.win,
                            edge: Edge::Move,
                            start: (x, y),
                            rect: nr,
                            moved: true,
                        });
                        self.add_damage(self.extent(old));
                        self.add_damage(self.extent(nr));
                        return;
                    }
                    r.x += dx;
                    // Keep the title bar reachable.
                    r.y = (r.y + dy).clamp(0, self.h - self.title_h());
                    let edge = self.s(2);
                    let target = if !self.cfg.snap {
                        None
                    } else if x <= edge {
                        Some(self.snap_rect(State::Left))
                    } else if x >= self.w - 1 - edge {
                        Some(self.snap_rect(State::Right))
                    } else if y <= edge {
                        Some(self.work_area())
                    } else {
                        None
                    };
                    if target != self.snap {
                        if let Some(s) = self.snap {
                            self.add_damage(s.inset(-self.s(12)));
                        }
                        if let Some(s) = target {
                            self.add_damage(s.inset(-self.s(12)));
                            self.tw.set(0x5000, 0);
                        }
                        self.snap = target;
                    }
                }
                Edge::Resize(e) => {
                    if e & 1 != 0 {
                        let nx = (r.x + dx).min(r.right() - mw);
                        r.w = r.right() - nx;
                        r.x = nx;
                    }
                    if e & 2 != 0 {
                        r.w = (r.w + dx).max(mw);
                    }
                    if e & 4 != 0 {
                        let ny = (r.y + dy).clamp(0, r.bottom() - mh);
                        r.h = r.bottom() - ny;
                        r.y = ny;
                    }
                    if e & 8 != 0 {
                        r.h = (r.h + dy).max(mh);
                    }
                }
            }
            if r != old {
                self.windows[i].rect = r;
                if r.w != old.w || r.h != old.h {
                    let mut w = self.windows.remove(i);
                    self.resize_buffer(&mut w, host, fonts);
                    self.windows.insert(i, w);
                }
                self.add_damage(self.extent(old));
                self.add_damage(self.extent(r));
            }
            return;
        }
        // Hover over title bar buttons and window content.
        let hit = self.hit(x, y);
        let over = hit.map(|h| h.0);
        for i in 0..self.windows.len() {
            let w = &self.windows[i];
            let b = if Some(w.id) == over {
                hit.and_then(|h| h.2)
            } else {
                None
            };
            if w.hover_btn != b {
                let id = w.id;
                let r = w.rect;
                let tb = self.title_h();
                self.windows[i].hover_btn = b;
                if self.windows[i].press_btn.is_some() && b.is_none() {
                    self.windows[i].press_btn = None;
                }
                self.windows[i].full = true;
                self.add_damage(Rect::new(r.x, r.y, r.w, tb));
                let _ = id;
            }
        }
        let target = self.capture.or(over.filter(|id| {
            self.win_index(*id)
                .is_some_and(|i| y >= self.windows[i].rect.y + self.title_h())
        }));
        if self.hover_win != target
            && let Some(old) = self.hover_win
        {
            // Leaving a window: its widgets lose the hover.
            self.send_to_window(
                old,
                Event::MouseMove {
                    x: -10000,
                    y: -10000,
                },
            );
        }
        self.hover_win = target;
        if let Some(id) = target {
            self.send_to_window(id, Event::MouseMove { x, y });
        }
    }

    fn mouse_down(&mut self, host: &mut dyn Host, fonts: &mut Fonts) {
        let (x, y) = self.mouse;
        let Some((id, edge, btn)) = self.hit(x, y) else {
            return;
        };
        if self.focus != Some(id) || self.windows.last().map(|w| w.id) != Some(id) {
            self.raise(id);
        }
        let Some(i) = self.win_index(id) else {
            return;
        };
        let r = self.windows[i].rect;
        if let Some(b) = btn {
            self.windows[i].press_btn = Some(b);
            self.windows[i].full = true;
            self.add_damage(Rect::new(r.x, r.y, r.w, self.title_h()));
            return;
        }
        let in_title = y < r.y + self.title_h() && edge == Edge::Move;
        match edge {
            Edge::Move if in_title => {
                let double = self.last_title_click.1 == id
                    && self.now.saturating_sub(self.last_title_click.0) < 400;
                if double {
                    self.last_title_click = (0, 0);
                    self.toggle_maximize(id, host, fonts);
                    return;
                }
                self.last_title_click = (self.now, id);
                self.drag = Some(Drag {
                    win: id,
                    edge,
                    start: (x, y),
                    rect: r,
                    moved: false,
                });
            }
            Edge::Resize(e) if e != 0 => {
                self.drag = Some(Drag {
                    win: id,
                    edge,
                    start: (x, y),
                    rect: r,
                    moved: false,
                });
            }
            _ => {
                self.capture = Some(id);
                self.send_to_window(id, Event::MouseMove { x, y });
                self.send_to_window(id, Event::MouseButton { down: true });
            }
        }
    }

    fn mouse_up(&mut self, host: &mut dyn Host, fonts: &mut Fonts) {
        if let Some(d) = self.drag.take() {
            if let Some(s) = self.snap.take() {
                self.add_damage(s.inset(-self.s(12)));
                let st = if s == self.work_area() {
                    State::Maximized
                } else if s.x == 0 {
                    State::Left
                } else {
                    State::Right
                };
                if let Some(i) = self.win_index(d.win) {
                    self.windows[i].restore = d.rect;
                    if self.windows[i].state == State::Normal {
                        self.windows[i].restore = self.windows[i].rect;
                    }
                }
                self.set_geometry(d.win, s, st, host, fonts);
            }
            return;
        }
        for i in 0..self.windows.len() {
            if let Some(b) = self.windows[i].press_btn.take() {
                let id = self.windows[i].id;
                let r = self.windows[i].rect;
                self.windows[i].full = true;
                self.add_damage(Rect::new(r.x, r.y, r.w, self.title_h()));
                if self.windows[i].hover_btn == Some(b) {
                    match b {
                        Btn::Close => self.close_window(id, host),
                        Btn::Minimize => self.minimize(id),
                        Btn::Maximize => self.toggle_maximize(id, host, fonts),
                    }
                }
                return;
            }
        }
        if let Some(id) = self.capture.take() {
            self.send_to_window(id, Event::MouseButton { down: false });
        }
    }

    /// Input for the session.
    fn session_input(&mut self, events: &[Event], host: &mut dyn Host, fonts: &mut Fonts) {
        for ev in events {
            if self.shortcut(ev, host, fonts) {
                continue;
            }
            if self.launcher.is_some() {
                self.launcher_input(ev, host, fonts);
                continue;
            }
            if self.switcher.is_some() {
                if let Event::Key {
                    key: Key::Escape, ..
                } = ev
                {
                    self.cancel_switcher();
                }
                continue;
            }
            match *ev {
                Event::MouseMove { x, y } => {
                    self.panel_hover_at(x, y);
                    self.popover_hover_at(x, y);
                    self.mouse_move(x, y, host, fonts);
                }
                Event::MouseButton { down: true } => {
                    let (x, y) = self.mouse;
                    if self.popover.is_some() {
                        if self.popover_click(x, y, host, fonts) {
                            continue;
                        }
                        self.close_popover();
                        if self.panel_hit_rect().contains(x, y) {
                            continue;
                        }
                    }
                    if self.panel_hit_rect().contains(x, y) {
                        self.panel_press = self.panel_item_at(x, y);
                        continue;
                    }
                    self.mouse_down(host, fonts);
                }
                Event::MouseButton { down: false } => {
                    if let Some(i) = self.panel_press.take() {
                        let (x, y) = self.mouse;
                        if self.panel_item_at(x, y) == Some(i) {
                            self.panel_click(i, host, fonts);
                        }
                        continue;
                    }
                    self.mouse_up(host, fonts);
                }
                Event::Wheel(_) | Event::RightButton { .. } => {
                    let (x, y) = self.mouse;
                    // Natural scrolling: the content follows the fingers.
                    let ev = match *ev {
                        Event::Wheel(d) if self.cfg.natural_scroll => Event::Wheel(-d),
                        e => e,
                    };
                    if let Some((id, Edge::Resize(0), None)) = self.hit(x, y) {
                        self.send_to_window(id, ev);
                    }
                }
                Event::Key {
                    key: Key::Escape, ..
                } if self.popover.is_some() => self.close_popover(),
                Event::Key { .. } | Event::Chord { .. } => {
                    if let Some(id) = self.focused_window() {
                        self.send_to_window(id, *ev);
                    }
                }
                _ => {}
            }
        }
        self.update_pointer();
    }

    // --- frames -----------------------------------------------------------

    /// Handles a frame's input, updates everything and draws what changed
    /// into `c` (the last frame shown). Returns the areas to show.
    pub fn frame(
        &mut self,
        events: &[Event],
        c: &mut Canvas,
        fonts: &mut Fonts,
        host: &mut dyn Host,
    ) -> Vec<Rect> {
        self.now = host.now_ms();
        self.anim_prev = self.tw.active();
        self.tw.begin(self.now);
        match self.mode {
            Mode::Login => self.login_input(events, host, fonts),
            Mode::Session => self.session_input(events, host, fonts),
        }
        self.tick(host, fonts);
        self.compose(c, fonts, host)
    }

    fn tick(&mut self, host: &mut dyn Host, fonts: &mut Fonts) {
        let now = self.now;
        if let Some(line) = self.host_log_pending.take() {
            host.log(line);
        }
        if self.relayout {
            self.relayout_windows(host, fonts);
        }
        if self.refit {
            self.refit = false;
            let fits: Vec<(u32, State)> = self
                .windows
                .iter()
                .filter(|w| w.state != State::Normal && !w.closing)
                .map(|w| (w.id, w.state))
                .collect();
            for (id, st) in fits {
                let r = self.snap_rect(st);
                self.set_geometry(id, r, st, host, fonts);
            }
        }
        if self.save_at.is_some_and(|t| now >= t) || (self.exit.is_some() && self.save_at.is_some())
        {
            self.flush_settings(host);
        }
        // The clock.
        if let Some(utc) = host.utc() {
            let m = (utc + tz::offset_minutes(&self.cfg.region, utc) as i64 * 60).div_euclid(60);
            if m != self.minute {
                self.minute = m;
                match self.mode {
                    Mode::Login => self.damage_all(),
                    Mode::Session => {
                        let r = self.panel_rect();
                        self.add_damage(r.inset(-self.s(4)));
                        if self.popover.is_some() {
                            let p = self.popover_rect();
                            self.add_damage(p.inset(-self.s(30)));
                        }
                    }
                }
            }
        }
        if self.mode == Mode::Session && now >= self.net_next {
            // Often while there is no address yet (DHCP), else now and
            // then: nothing tells Lumi when the network changes.
            self.net_next = now
                + if self.net.present && self.net.ip.is_none() {
                    3000
                } else {
                    30_000
                };
            let n = host.net();
            if n != self.net {
                self.net = n;
                let r = self.panel_rect();
                self.add_damage(r);
            }
        }
        // Windows: animations, apps, closed programs.
        let mut actions: Vec<(u32, Action)> = Vec::new();
        let mut i = 0;
        while i < self.windows.len() {
            if let Some(a) = self.windows[i].anim
                && a.progress(now) >= 1000
            {
                self.windows[i].anim = None;
                match a.kind {
                    AnimKind::Close => {
                        // Erase it from wherever the last frame showed it.
                        let w = self.windows.remove(i);
                        if let Some((r, _)) = w.drawn {
                            self.add_damage(r);
                        }
                        self.add_damage(self.extent(a.from));
                        self.panel_changed();
                        continue;
                    }
                    AnimKind::Minimize => self.windows[i].minimized = true,
                    _ => {}
                }
            }
            if !self.windows[i].closing && !self.windows[i].app.alive() {
                // The program ended or crashed: only its window goes.
                let w = &self.windows[i];
                let id = w.id;
                let line = match w.app.exit_status() {
                    Some(st) => match crate::host::crash_reason(st) {
                        Some(why) => alloc::format!(
                            "lumi: program ended in {} (status {}, crashed: {})",
                            w.title,
                            st,
                            why
                        ),
                        None => {
                            alloc::format!("lumi: program ended in {} (status {})", w.title, st)
                        }
                    },
                    None => alloc::format!("lumi: program ended in {}", w.title),
                };
                host.log(&line);
                self.close_window(id, host);
            }
            if !self.windows[i].closing
                && let Some(changed) = self.render_window(i, host, fonts, &mut actions)
            {
                let w = &self.windows[i];
                if w.anim.is_none() && !w.minimized {
                    let r = changed.offset(w.rect.x, w.rect.y);
                    self.add_damage(r);
                }
            }
            i += 1;
        }
        for (from, a) in actions {
            self.run_action(from, a, host, fonts);
        }
        self.sync_windows();
        if self.mode == Mode::Session {
            self.tick_overlays(now);
        } else {
            self.tick_login(now, host);
        }
    }

    /// True while something moves: frames should come at 60 per second.
    pub fn animating(&self) -> bool {
        self.windows
            .iter()
            .any(|w| w.anim.is_some() || w.ui.animating())
            || self.tw.active()
            || self.transition.is_some()
            || self.pending_transition
            || self.overlays_animating()
            || !self.damage.is_empty()
    }

    /// When the desktop needs a frame without input (ms since boot).
    pub fn wake_at(&self) -> u64 {
        let mut t = self.now + 60_000;
        if let Some(utc) = self.utc_hint() {
            let to_next = 60 - utc.rem_euclid(60);
            t = t.min(self.now + to_next as u64 * 1000);
        }
        for w in &self.windows {
            if let Some(at) = w.app.wake_at(self.now) {
                t = t.min(at);
            }
        }
        if let Some(at) = self.overlay_wake() {
            t = t.min(at);
        }
        if let Some(at) = self.save_at {
            t = t.min(at);
        }
        if self.mode == Mode::Session && self.net.present && self.net.ip.is_none() {
            t = t.min(self.net_next);
        }
        t.max(self.now + 1)
    }

    fn utc_hint(&self) -> Option<i64> {
        if self.minute < 0 {
            None
        } else {
            Some(self.minute * 60)
        }
    }

    /// Merges the damaged areas into a few rectangles.
    fn take_damage(&mut self) -> Vec<Rect> {
        let mut rects = core::mem::take(&mut self.damage);
        let screen = (self.w as i64) * (self.h as i64);
        let mut merged: Vec<Rect> = Vec::new();
        rects.sort_by_key(|r| r.y);
        for r in rects {
            let mut r = r;
            // Join with any area it overlaps or nearly fills a box with.
            while let Some(i) = merged.iter().position(|m| {
                let u = m.union(r);
                let ua = u.w as i64 * u.h as i64;
                let sum = m.w as i64 * m.h as i64 + r.w as i64 * r.h as i64;
                !m.intersect(r).is_empty() || ua <= sum * 5 / 4
            }) {
                r = merged.remove(i).union(r);
            }
            merged.push(r);
        }
        let total: i64 = merged.iter().map(|r| r.w as i64 * r.h as i64).sum();
        if merged.len() > 6 || total > screen * 6 / 10 {
            let mut u = merged[0];
            for r in &merged[1..] {
                u = u.union(*r);
            }
            merged = vec![u];
        }
        merged
    }

    /// Draws the damaged areas into `c`.
    fn compose(&mut self, c: &mut Canvas, fonts: &mut Fonts, host: &mut dyn Host) -> Vec<Rect> {
        if self.pending_transition {
            self.pending_transition = false;
            self.transition = Some(Transition::capture(
                c,
                c.bounds(),
                self.now,
                self.dur(320),
                TransKind::Fade,
            ));
            self.damage_all();
        }
        if self.transition.is_some() {
            self.damage_all();
        }
        let rects = if self.damage.is_empty() {
            Vec::new()
        } else {
            self.take_damage()
        };
        for &r in &rects {
            c.set_frame(r);
            match self.mode {
                Mode::Login => self.draw_login(c, fonts, host),
                Mode::Session => self.draw_session(c, fonts),
            }
            self.painted += r.w as u64 * r.h as u64;
        }
        c.set_frame(c.bounds());
        if !rects.is_empty() {
            self.frames += 1;
            if self.debug_damage {
                const TINTS: [u32; 6] = [
                    0x50ff3040, 0x5030d050, 0x503060ff, 0x50ffc020, 0x50d040e0, 0x5020d0d0,
                ];
                let tint = TINTS[(self.frames % 6) as usize];
                for &r in &rects {
                    c.fill_rect(r, tint);
                    c.stroke_rrect_fx(r, 0, FX * 2, tint | 0xff00_0000);
                }
            }
        }
        if let Some(t) = self.transition.as_mut()
            && !t.apply(c, self.now)
        {
            self.transition = None;
        }
        rects
    }

    fn draw_session(&mut self, c: &mut Canvas, fonts: &mut Fonts) {
        let area = c.frame();
        let menu_full = self.launcher_progress() >= 1000;
        if menu_full {
            self.frost.copy_to(c, area);
        } else {
            gfx::copy(c, &self.wall, area);
            self.draw_windows(c);
            self.draw_snap(c);
            self.draw_panel(c, fonts);
            self.draw_popover(c, fonts);
            self.draw_tooltip(c, fonts);
        }
        self.draw_launcher(c, fonts);
        self.draw_switcher(c, fonts);
    }

    fn draw_windows(&self, c: &mut Canvas) {
        let area = c.frame();
        for w in &self.windows {
            let Some((r, alpha)) = self.shown(w) else {
                continue;
            };
            if self.extent(r).intersect(area).is_empty() {
                continue;
            }
            let focused = self.focus == Some(w.id);
            let rad = self.window_radius(w);
            let strength = if focused { 0x78 } else { 0x48 } * alpha / 255;
            let sh_rect = r.offset(0, self.s(10));
            if r.w == w.rect.w && r.h == w.rect.h {
                if rad == self.corner.r {
                    self.shadow.draw(c, sh_rect, rad, strength << 24, r);
                    gfx::blit(c, &w.buf, r.x, r.y, &self.corner, alpha);
                } else {
                    let corner = Corner::new(rad);
                    self.shadow.draw(c, sh_rect, rad, strength << 24, r);
                    gfx::blit(c, &w.buf, r.x, r.y, &corner, alpha);
                }
            } else {
                // Animating: the picture scaled, a lighter shadow.
                let k = (r.w as i64 * 1000 / w.rect.w.max(1) as i64) as i32;
                let rr = (rad as i64 * k as i64 / 1000) as i32;
                self.shadow
                    .draw(c, sh_rect, rad.min(rr.max(0)), (strength * 3 / 4) << 24, r);
                let corner = Corner::new(rr.max(0));
                gfx::blit_scaled(c, &w.buf, r, &corner, alpha);
            }
        }
    }

    fn draw_snap(&mut self, c: &mut Canvas) {
        let Some(s) = self.snap else {
            return;
        };
        let a = self.tw.value(0x5000, 255, self.dur(160)) as u32;
        let r = s.inset(self.s(8));
        let rad = self.radius();
        c.fill_rrect(r, rad, fade(self.theme.accent, (a * 60 / 255) as u8));
        c.stroke_rrect_fx(
            r,
            rad,
            (self.s(2) as i64 * FX).max(FX),
            fade(self.theme.accent, (a * 200 / 255) as u8),
        );
        if self.tw.active() {
            self.add_damage(s.inset(-self.s(12)));
        }
    }

    /// Titles of the open windows, bottom to top (for tests and logs).
    pub fn window_titles(&self) -> Vec<String> {
        self.windows
            .iter()
            .filter(|w| !w.closing)
            .map(|w| w.title.clone())
            .collect()
    }

    /// A window is being moved or resized with the pointer.
    pub fn dragging(&self) -> bool {
        self.drag.is_some_and(|d| d.moved)
    }

    /// The title of the window that has the keyboard.
    pub fn focused_title(&self) -> Option<String> {
        let id = self.focused_window()?;
        self.windows
            .iter()
            .find(|w| w.id == id)
            .map(|w| w.title.clone())
    }

    /// Where the window with this title is.
    pub fn window_rect_of(&self, title: &str) -> Option<Rect> {
        self.windows
            .iter()
            .find(|w| w.title == title && !w.closing)
            .map(|w| w.rect)
    }

    /// Starts the session of `user` (after the login screen).
    pub fn start_session(&mut self, user: UserInfo, host: &mut dyn Host) {
        let mut cfg = self.cfg.clone();
        // The user's file, or where older versions kept it.
        let text = [config::USER_FILE, config::OLD_USER_FILE]
            .iter()
            .find_map(|f| host.read(&join(&user.home, f)))
            .and_then(|d| String::from_utf8(d).ok());
        if let Some(text) = text {
            cfg.apply(&text);
        }
        host.log(&alloc::format!("lumi: session {}", user.name));
        self.user = Some(user);
        self.mode = Mode::Session;
        self.session_t0 = self.now;
        if cfg != self.cfg {
            self.apply_config(cfg, host, false);
        }
        self.pending_transition = self.cfg.animations_on();
        self.damage_all();
        self.panel_changed();
    }
}

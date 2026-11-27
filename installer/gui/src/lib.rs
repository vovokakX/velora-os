//! The screens of the graphical installer and of the start menu, drawn
//! with velora-ui. They are plain functions of the state ([`App`],
//! [`Chooser`]): the program on Velora feeds input and shows the canvas,
//! and the host renders the same screens for comparison with the design
//! (`docs/design/installer-concept.png`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod art;
pub mod chooser;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use art::StepState;
use velora_install::settings::{self, Lang, Settings};
use velora_install::{Layout, Phase, Plan};
use velora_ui::Rect;
use velora_ui::canvas::{Canvas, fade, rgb};
use velora_ui::decor;
use velora_ui::icons::Icon;
use velora_ui::image::{Picture, Pointer};
use velora_ui::text::{Fonts, Weight};
use velora_ui::theme::{DARK, LIGHT};
use velora_ui::ui::{ButtonKind, Ctx, Key, Ui};

/// Design units of the window (the design's screens are 805 x 368).
pub const WIN_W: i32 = 805;
pub const WIN_H: i32 = 368;
const SIDEBAR_W: i32 = 193;
const CONTENT_X: i32 = 226;
const CONTENT_R: i32 = 781;

/// The window for a screen of `w` x `h` pixels and its scale (pixels per
/// design unit times 256): the design's proportions, as large as fits.
pub fn window_rect(w: i32, h: i32) -> (Rect, i64) {
    let ww = (w * 94 / 100).min(h * 94 / 100 * WIN_W / WIN_H);
    let scale = ww as i64 * 256 / WIN_W as i64;
    let wh = (WIN_H as i64 * scale / 256) as i32;
    (Rect::new((w - ww) / 2, (h - wh) / 2, ww, wh), scale)
}

/// Pictures the screens use.
pub struct Assets {
    pub logo: Picture,
    /// Classic, blade, drop (the order of `settings::POINTERS`).
    pub pointers: [Pointer; 3],
}

/// A disk offered for installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskEntry {
    pub name: String,
    pub size: u64,
    pub model: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Welcome,
    Location,
    Keyboard,
    Appearance,
    Disk,
    Partitions,
    User,
    Confirm,
    Installing,
    Done,
    Failed,
}

impl Screen {
    /// Name for logs and screenshot files.
    pub fn name(self) -> &'static str {
        match self {
            Screen::Welcome => "welcome",
            Screen::Location => "location",
            Screen::Keyboard => "keyboard",
            Screen::Appearance => "appearance",
            Screen::Disk => "disk",
            Screen::Partitions => "partitions",
            Screen::User => "user",
            Screen::Confirm => "confirm",
            Screen::Installing => "installing",
            Screen::Done => "done",
            Screen::Failed => "failed",
        }
    }

    /// The step of the sidebar this screen belongs to.
    fn step(self) -> usize {
        match self {
            Screen::Welcome => 0,
            Screen::Location => 1,
            Screen::Keyboard => 2,
            Screen::Appearance => 3,
            Screen::Disk | Screen::Partitions => 4,
            Screen::User => 5,
            Screen::Confirm | Screen::Installing | Screen::Failed => 6,
            Screen::Done => 7,
        }
    }
}

/// What the program has to do after a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Erase the disk and install (the plan is [`App::plan`]).
    Install,
    Reboot,
    /// Leave the installer (back to the start menu).
    Quit,
}

const STEPS: [(&str, &str); 8] = [
    ("Добро пожаловать", "Welcome"),
    ("Расположение", "Location"),
    ("Клавиатура", "Keyboard"),
    ("Оформление", "Appearance"),
    ("Разметка диска", "Disk"),
    ("Пользователь", "User"),
    ("Установка", "Installation"),
    ("Готово", "Done"),
];

/// EFI partition sizes offered in the manual modes (MiB).
pub const ESP_SIZES: [u64; 4] = [64, 128, 256, 512];
/// Root sizes offered in the manual modes: the whole rest of the disk or
/// a share of it (percent).
pub const ROOT_SHARES: [u64; 4] = [100, 75, 50, 25];

/// The installer's state.
pub struct App {
    pub screen: Screen,
    pub dark: bool,
    pub settings: Settings,
    pub disks: Vec<DiskEntry>,
    pub disk: usize,
    /// 0 automatic, 1 manual, 2 expert.
    pub mode: usize,
    pub esp: usize,
    pub root_share: usize,
    pub label: String,
    pub user: String,
    pub password: String,
    pub password2: String,
    pub root_password: String,
    pub root_password2: String,
    /// A problem to show on the current screen.
    pub error: Option<String>,
    pub progress: u32,
    pub phase: Option<Phase>,
    pub failure: String,
    background: decor::Cache,
}

impl App {
    pub fn new(disks: Vec<DiskEntry>) -> App {
        App {
            screen: Screen::Welcome,
            dark: true,
            settings: Settings::default(),
            disks,
            disk: 0,
            mode: 0,
            esp: 0,
            root_share: 0,
            label: String::from("velora-root"),
            user: String::new(),
            password: String::new(),
            password2: String::new(),
            root_password: String::new(),
            root_password2: String::new(),
            error: None,
            progress: 0,
            phase: None,
            failure: String::new(),
            background: decor::Cache::default(),
        }
    }

    pub fn lang(&self) -> Lang {
        self.settings.installer_language
    }

    fn l(&self, ru: &'static str, en: &'static str) -> &'static str {
        match self.lang() {
            Lang::Ru => ru,
            Lang::En => en,
        }
    }

    /// The pointer the user picked (shown live) and whether it is large.
    pub fn pointer(&self) -> (usize, bool) {
        (self.settings.pointer, self.settings.large_pointer)
    }

    pub fn selected_disk(&self) -> Option<&DiskEntry> {
        self.disks.get(self.disk)
    }

    /// The partition layout of the chosen mode.
    pub fn layout(&self) -> Layout {
        if self.mode == 0 {
            return Layout::default();
        }
        let esp = ESP_SIZES[self.esp.min(ESP_SIZES.len() - 1)];
        let share = ROOT_SHARES[self.root_share.min(ROOT_SHARES.len() - 1)];
        let root_mib = (share < 100).then(|| {
            let total = self.selected_disk().map_or(0, |d| d.size / (1024 * 1024));
            total.saturating_sub(esp + 2) * share / 100
        });
        Layout {
            esp_mib: esp,
            root_mib,
            root_label: if self.mode == 2 {
                self.label.clone()
            } else {
                String::from("velora-root")
            },
        }
    }

    /// What will be installed.
    pub fn plan(&self) -> Plan {
        Plan {
            user: self.user.clone(),
            user_password: self.password.clone(),
            root_password: self.root_password.clone(),
            layout: self.layout(),
            settings: self.settings.clone(),
        }
    }

    fn go(&mut self, s: Screen) {
        self.screen = s;
        self.error = None;
    }

    fn next(&mut self) -> Action {
        let lang = self.lang();
        match self.screen {
            Screen::Welcome => self.go(Screen::Location),
            Screen::Location => self.go(Screen::Keyboard),
            Screen::Keyboard => self.go(Screen::Appearance),
            Screen::Appearance => self.go(Screen::Disk),
            Screen::Disk => {
                let Some(d) = self.selected_disk() else {
                    self.error = Some(String::from(self.l("Диски не найдены", "No disks found")));
                    return Action::None;
                };
                if let Err(p) = velora_install::check_layout(&Layout::default(), d.size) {
                    self.error = Some(p.text(lang));
                    return Action::None;
                }
                self.go(if self.mode == 0 {
                    Screen::User
                } else {
                    Screen::Partitions
                });
            }
            Screen::Partitions => {
                let size = self.selected_disk().map_or(0, |d| d.size);
                match velora_install::check_layout(&self.layout(), size) {
                    Ok(()) => self.go(Screen::User),
                    Err(p) => self.error = Some(p.text(lang)),
                }
            }
            Screen::User => {
                let plan = self.plan();
                if let Err(p) = velora_install::check_user(&plan) {
                    self.error = Some(p.text(lang));
                } else if self.password != self.password2
                    || self.root_password != self.root_password2
                {
                    self.error = Some(String::from(
                        self.l("Пароли не совпадают", "The passwords differ"),
                    ));
                } else {
                    self.go(Screen::Confirm);
                }
            }
            Screen::Confirm => {
                self.go(Screen::Installing);
                self.progress = 0;
                self.phase = None;
                return Action::Install;
            }
            Screen::Done => return Action::Reboot,
            Screen::Installing | Screen::Failed => {}
        }
        Action::None
    }

    fn back(&mut self) -> Action {
        match self.screen {
            Screen::Welcome => return Action::Quit,
            Screen::Location => self.go(Screen::Welcome),
            Screen::Keyboard => self.go(Screen::Location),
            Screen::Appearance => self.go(Screen::Keyboard),
            Screen::Disk => self.go(Screen::Appearance),
            Screen::Partitions => self.go(Screen::Disk),
            Screen::User => self.go(if self.mode == 0 {
                Screen::Disk
            } else {
                Screen::Partitions
            }),
            Screen::Confirm => self.go(Screen::User),
            Screen::Failed => self.go(Screen::Disk),
            Screen::Installing | Screen::Done => {}
        }
        Action::None
    }

    /// The installation reports progress.
    pub fn set_progress(&mut self, p: velora_install::Progress) {
        self.progress = p.permille;
        self.phase = Some(p.phase);
    }

    pub fn finish(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.progress = 1000;
                self.go(Screen::Done);
            }
            Err(e) => {
                self.failure = e;
                self.go(Screen::Failed);
            }
        }
    }

    /// Draws the current screen and handles its input. Returns what the
    /// program must do and whether the frame should be drawn again.
    pub fn draw(
        &mut self,
        ui: &mut Ui,
        c: &mut Canvas,
        f: &mut Fonts,
        a: &mut Assets,
    ) -> (Action, bool) {
        let start = self.screen;
        let (win, scale) = window_rect(c.width as i32, c.height as i32);
        ui.scale = scale;
        ui.theme = if self.dark { DARK } else { LIGHT };
        let t = ui.theme;
        let radius = ui.s(16);
        let kind = match self.screen {
            Screen::Done => 2,
            _ => 1,
        };
        let side = Rect::new(win.x, win.y, ui.s(SIDEBAR_W), win.h);
        self.background
            .paint(c, (self.dark as u64) << 8 | kind, |c| {
                decor::backdrop(c, &t);
                decor::window(c, win, radius, &t, scale);
                if kind == 2 {
                    decor::waves_side(c, win, radius, &t, true);
                } else {
                    decor::sidebar(c, side, radius, &t);
                }
            });
        let mut x = Ctx { ui, c, f };
        let mut action = Action::None;
        // Sidebar.
        match self.screen {
            Screen::Done => {}
            Screen::Installing => self.sidebar(&mut x, win, a, false),
            _ => self.sidebar(&mut x, win, a, true),
        }
        match self.screen {
            Screen::Welcome => self.welcome(&mut x, win, a),
            Screen::Location => self.location(&mut x, win),
            Screen::Keyboard => self.keyboard(&mut x, win, a),
            Screen::Appearance => self.appearance(&mut x, win),
            Screen::Disk => self.disk_screen(&mut x, win),
            Screen::Partitions => self.partitions(&mut x, win),
            Screen::User => self.user_screen(&mut x, win),
            Screen::Confirm => self.confirm(&mut x, win),
            Screen::Installing => self.installing(&mut x, win),
            Screen::Done => {
                if self.done(&mut x, win) {
                    action = Action::Reboot;
                }
            }
            Screen::Failed => self.failed(&mut x, win),
        }
        // Buttons at the bottom right.
        let busy = self.screen == Screen::Installing;
        if !matches!(self.screen, Screen::Done | Screen::Installing) {
            let (go_label, kind) = match self.screen {
                Screen::Confirm => (self.l("Установить", "Install"), ButtonKind::Primary),
                Screen::Failed => (self.l("Перезагрузить", "Restart"), ButtonKind::Secondary),
                _ => (self.l("Продолжить", "Continue"), ButtonKind::Primary),
            };
            let bh = x.ui.s(32);
            let gw = x.ui.s(if self.screen == Screen::Confirm {
                128
            } else {
                124
            });
            let go = Rect::new(win.x + x.ui.s(CONTENT_R) - gw, win.y + x.ui.s(324), gw, bh);
            if self.screen != Screen::Welcome {
                let bw = x.ui.s(86);
                let back = Rect::new(go.x - x.ui.s(8) - bw, go.y, bw, bh);
                if x.button(
                    900,
                    back,
                    self.l("Назад", "Back"),
                    ButtonKind::Secondary,
                    Some(Icon::ArrowLeft),
                    None,
                ) {
                    action = self.back();
                }
            }
            let icon = if self.screen == Screen::Failed {
                None
            } else {
                Some(Icon::ArrowRight)
            };
            if x.button(901, go, go_label, kind, None, icon) {
                action = if self.screen == Screen::Failed {
                    Action::Reboot
                } else {
                    self.next()
                };
            }
        }
        if !busy && action == Action::None && !x.ui.popup_open() {
            if x.ui.take_key(Key::Escape) {
                action = self.back();
            } else if x.ui.focused().is_none() && x.ui.take_key(Key::Enter) {
                action = match self.screen {
                    Screen::Failed => Action::None,
                    _ => self.next(),
                };
            }
        }
        // A new screen is drawn at once, not at the next input.
        let again = x.end() || action != Action::None || self.screen != start;
        (action, again)
    }

    fn sidebar(&mut self, x: &mut Ctx, win: Rect, a: &mut Assets, steps: bool) {
        let t = x.ui.theme;
        let side = Rect::new(win.x, win.y, x.ui.s(SIDEBAR_W), win.h);
        let lh = x.ui.s(30);
        let logo_y = if steps {
            win.y + x.ui.s(26)
        } else {
            win.y + x.ui.s(140)
        };
        let img = a.logo.fit((lh * 2) as usize, lh as usize);
        let (iw, ih) = (img.width as i32, img.height as i32);
        x.c.draw_image_tinted(
            img,
            side.center_x() - iw / 2,
            logo_y + (lh - ih) / 2,
            t.text,
        );
        let px = x.ui.px(13);
        let name = "Velora OS";
        let nw = x.f.measure(Weight::Medium, px, name);
        x.label(
            side.center_x() - nw / 2,
            logo_y + x.ui.s(38),
            Weight::Medium,
            13,
            name,
            t.text,
        );
        if !steps {
            return;
        }
        let cur = self.screen.step();
        let x0 = side.x + x.ui.s(34);
        let d = x.ui.s(17);
        for (i, (ru, en)) in STEPS.iter().enumerate() {
            let cy = win.y + x.ui.s(124 + 27 * i as i32);
            let state = if i < cur {
                StepState::Done
            } else if i == cur {
                StepState::Current
            } else {
                StepState::Ahead
            };
            art::step_mark(x.c, x0, cy, d, state, &t);
            let (w, color) = match state {
                StepState::Current => (Weight::SemiBold, t.text),
                StepState::Done => (Weight::Regular, t.text_dim),
                StepState::Ahead => (Weight::Regular, t.text_faint),
            };
            let label = if self.lang() == Lang::Ru { *ru } else { *en };
            x.label_mid(
                Rect::new(x0 + x.ui.s(22), cy - d / 2, x.ui.s(150), d),
                w,
                11,
                label,
                color,
            );
        }
        // Light/dark switch.
        let r = Rect::new(
            side.x + x.ui.s(22),
            win.y + x.ui.s(334),
            x.ui.s(48),
            x.ui.s(24),
        );
        let (resp, step) = x.selectable(800, r, r.h / 2);
        art::theme_switch(x.c, r, self.dark, resp.hovered, &t);
        if resp.activated || step != 0 {
            self.dark = !self.dark;
            x.ui.request_redraw();
        }
    }

    fn title(&self, x: &mut Ctx, win: Rect, title: &str, sub: &str) {
        let t = x.ui.theme;
        x.label(
            win.x + x.ui.s(CONTENT_X),
            win.y + x.ui.s(26),
            Weight::SemiBold,
            18,
            title,
            t.text,
        );
        if !sub.is_empty() {
            x.label(
                win.x + x.ui.s(CONTENT_X),
                win.y + x.ui.s(56),
                Weight::Regular,
                11,
                sub,
                t.text_dim,
            );
        }
    }

    fn error_line(&self, x: &mut Ctx, win: Rect, v: i32) {
        if let Some(e) = &self.error {
            let color = if x.ui.theme.dark {
                rgb(255, 138, 128)
            } else {
                rgb(196, 48, 40)
            };
            x.icon(
                Icon::Warning,
                win.x + x.ui.s(CONTENT_X),
                win.y + x.ui.s(v),
                x.ui.s(14),
                color,
            );
            x.label(
                win.x + x.ui.s(CONTENT_X + 20),
                win.y + x.ui.s(v - 1),
                Weight::Medium,
                11,
                e,
                color,
            );
        }
    }

    fn welcome(&mut self, x: &mut Ctx, win: Rect, a: &mut Assets) {
        let t = x.ui.theme;
        let cx = win.x + x.ui.s(CONTENT_X);
        let (l1, l2) = match self.lang() {
            Lang::Ru => ("Добро пожаловать", "в Velora OS!"),
            Lang::En => ("Welcome", "to Velora OS!"),
        };
        x.label(cx, win.y + x.ui.s(82), Weight::SemiBold, 22, l1, t.text);
        x.label(cx, win.y + x.ui.s(112), Weight::SemiBold, 22, l2, t.text);
        let desc = self.l(
            "Современная, быстрая и удобная система\nдля работы, игр и творчества.",
            "A modern, fast and friendly system\nfor work, games and creativity.",
        );
        for (i, line) in desc.split('\n').enumerate() {
            x.label(
                cx,
                win.y + x.ui.s(152 + 20 * i as i32),
                Weight::Regular,
                13,
                line,
                t.text_dim,
            );
        }
        // The big mark at the right.
        let lw = x.ui.s(150);
        let lh = x.ui.s(122);
        let img = a.logo.fit(lw as usize, lh as usize);
        let (iw, ih) = (img.width as i32, img.height as i32);
        x.c.draw_image_tinted(
            img,
            win.x + x.ui.s(676) - iw / 2,
            win.y + x.ui.s(196) - ih / 2,
            fade(t.text, 225),
        );
        let names: Vec<&str> = settings::INSTALLER_LANGUAGES.iter().map(|c| c.ru).collect();
        let mut sel = self.lang() as usize;
        let r = Rect::new(cx, win.y + x.ui.s(282), x.ui.s(208), x.ui.s(31));
        if x.dropdown(10, r, r.h / 2, &names, &mut sel, Some(Icon::Globe), 11) {
            self.settings.installer_language = if sel == 1 { Lang::En } else { Lang::Ru };
            x.ui.request_redraw();
        }
    }

    fn location(&mut self, x: &mut Ctx, win: Rect) {
        let t = x.ui.theme;
        let lang = self.lang();
        self.title(
            x,
            win,
            self.l("Выберите язык и регион", "Choose the language and region"),
            "",
        );
        let cols = [
            (
                CONTENT_X,
                Icon::Globe,
                self.l("Язык системы", "System language"),
            ),
            (528, Icon::Clock, self.l("Регион", "Region")),
        ];
        for (u, icon, label) in cols {
            let ix = win.x + x.ui.s(u + 4);
            let iy = win.y + x.ui.s(78);
            x.icon(icon, ix, iy, x.ui.s(17), t.text);
            x.label_mid(
                Rect::new(ix + x.ui.s(26), iy, x.ui.s(200), x.ui.s(17)),
                Weight::Medium,
                11,
                label,
                t.text,
            );
        }
        let langs: Vec<&str> = settings::LANGUAGES.iter().map(|c| c.name(lang)).collect();
        let regions: Vec<&str> = settings::REGIONS.iter().map(|c| c.name(lang)).collect();
        let h = x.ui.s(198);
        let r1 = Rect::new(
            win.x + x.ui.s(CONTENT_X),
            win.y + x.ui.s(106),
            x.ui.s(256),
            h,
        );
        let r2 = Rect::new(win.x + x.ui.s(526), win.y + x.ui.s(106), x.ui.s(248), h);
        x.select_list(20, r1, &langs, &mut self.settings.language);
        x.select_list(21, r2, &regions, &mut self.settings.region);
    }

    fn keyboard(&mut self, x: &mut Ctx, win: Rect, a: &mut Assets) {
        let t = x.ui.theme;
        let lang = self.lang();
        self.title(
            x,
            win,
            self.l("Настройка клавиатуры", "Keyboard"),
            self.l(
                "Выберите раскладку клавиатуры и курсор мыши.",
                "Choose the keyboard layout and the mouse pointer.",
            ),
        );
        let cx = win.x + x.ui.s(CONTENT_X);
        x.icon(Icon::Keyboard, cx, win.y + x.ui.s(82), x.ui.s(22), t.text);
        x.label(
            cx,
            win.y + x.ui.s(114),
            Weight::Medium,
            11,
            self.l("Раскладка клавиатуры", "Keyboard layout"),
            t.text,
        );
        let layouts: Vec<&str> = settings::LAYOUTS.iter().map(|c| c.name(lang)).collect();
        let dd = Rect::new(cx, win.y + x.ui.s(136), x.ui.s(256), x.ui.s(32));
        x.dropdown(
            30,
            dd,
            x.ui.s(12),
            &layouts,
            &mut self.settings.layout,
            None,
            11,
        );
        let hk = self.l(
            "Использовать системные\nгорячие клавиши",
            "Use the system\nkeyboard shortcuts",
        );
        for (i, line) in hk.split('\n').enumerate() {
            x.label(
                cx,
                win.y + x.ui.s(188 + 15 * i as i32),
                Weight::Regular,
                10,
                line,
                t.text_dim,
            );
        }
        let tg = Rect::new(
            dd.right() - x.ui.s(34),
            win.y + x.ui.s(192),
            x.ui.s(34),
            x.ui.s(18),
        );
        x.toggle(31, tg, &mut self.settings.system_hotkeys);
        // Divider.
        let dx = win.x + x.ui.s(512);
        x.c.fill_rect(
            Rect::new(dx, win.y + x.ui.s(80), x.ui.s(1).max(1), x.ui.s(220)),
            t.field_border,
        );
        // Pointer styles.
        let rx = win.x + x.ui.s(532);
        x.icon(Icon::Pointer, rx, win.y + x.ui.s(82), x.ui.s(16), t.text);
        x.label_mid(
            Rect::new(rx + x.ui.s(24), win.y + x.ui.s(82), x.ui.s(200), x.ui.s(16)),
            Weight::Medium,
            11,
            self.l("Стиль курсора", "Pointer style"),
            t.text,
        );
        let bs = x.ui.s(52);
        #[allow(clippy::needless_range_loop)]
        for i in 0..3 {
            let r = Rect::new(
                rx + i as i32 * (bs + x.ui.s(14)),
                win.y + x.ui.s(112),
                bs,
                bs,
            );
            let selected = self.settings.pointer == i;
            let (resp, step) = x.selectable(40 + i as u32, r, x.ui.s(10));
            if resp.activated {
                self.settings.pointer = i;
                x.ui.request_redraw();
            }
            if step != 0 {
                let n = (i as i32 + step).clamp(0, 2) as usize;
                self.settings.pointer = n;
                x.ui.set_focus(Some(40 + n as u32));
                x.ui.request_redraw();
            }
            let bg = if resp.hovered { t.field_hover } else { t.field };
            x.c.fill_rrect(r, x.ui.s(10), bg);
            let border = if selected {
                fade(t.text, 210)
            } else {
                t.field_border
            };
            let bw = if selected {
                x.ui.sfx(400)
            } else {
                x.ui.sfx(256)
            };
            x.c.stroke_rrect_fx(r, x.ui.s(10), bw.max(256), border);
            let ps = (bs * 6 / 10) as usize;
            let (img, hx, hy) = a.pointers[i].at(ps);
            let _ = (hx, hy);
            let (iw, ih) = (img.width as i32, img.height as i32);
            x.c.draw_image(img, r.center_x() - iw / 3, r.center_y() - ih / 2, 255);
        }
        let tg = Rect::new(rx, win.y + x.ui.s(190), x.ui.s(30), x.ui.s(16));
        x.toggle(45, tg, &mut self.settings.large_pointer);
        x.label_mid(
            Rect::new(tg.right() + x.ui.s(12), tg.y, x.ui.s(180), tg.h),
            Weight::Regular,
            10,
            self.l("Увеличенный курсор", "Large pointer"),
            t.text_dim,
        );
    }

    fn appearance(&mut self, x: &mut Ctx, win: Rect) {
        let t = x.ui.theme;
        let lang = self.lang();
        self.title(
            x,
            win,
            self.l("Внешний вид системы", "Appearance"),
            self.l(
                "Выберите стандартную тему, иконки и обои.",
                "Choose the theme, icons and wallpaper.",
            ),
        );
        let cx = win.x + x.ui.s(CONTENT_X);
        let top = win.y + x.ui.s(84);
        let (cw, ch) = (x.ui.s(72), x.ui.s(50));
        for i in 0..3 {
            let r = Rect::new(cx + i as i32 * (cw + x.ui.s(8)), top, cw, ch);
            let (resp, step) = x.selectable(50 + i as u32, r, x.ui.s(8));
            if resp.activated {
                self.settings.theme = i;
                x.ui.request_redraw();
            }
            if step != 0 {
                let n = (i as i32 + step).clamp(0, 2) as usize;
                self.settings.theme = n;
                x.ui.set_focus(Some(50 + n as u32));
                x.ui.request_redraw();
            }
            art::preview(x.c, r, x.ui.s(8), art::theme_colors(i));
            if self.settings.theme == i {
                x.c.stroke_rrect_fx(
                    r.inset(-x.ui.s(2)),
                    x.ui.s(10),
                    x.ui.sfx(400).max(256),
                    fade(t.text, 220),
                );
            }
        }
        x.label(
            cx,
            win.y + x.ui.s(148),
            Weight::Medium,
            11,
            self.l("Тема оформления", "Theme"),
            t.text,
        );
        let themes: Vec<&str> = settings::THEMES.iter().map(|c| c.name(lang)).collect();
        x.dropdown(
            53,
            Rect::new(cx, win.y + x.ui.s(168), x.ui.s(232), x.ui.s(30)),
            x.ui.s(12),
            &themes,
            &mut self.settings.theme,
            None,
            11,
        );
        let div = |x: &mut Ctx, u: i32| {
            x.c.fill_rect(
                Rect::new(
                    win.x + x.ui.s(u),
                    win.y + x.ui.s(80),
                    x.ui.s(1).max(1),
                    x.ui.s(220),
                ),
                t.field_border,
            );
        };
        div(x, 474);
        let mx = win.x + x.ui.s(490);
        x.label(
            mx,
            win.y + x.ui.s(82),
            Weight::Medium,
            11,
            self.l("Иконки", "Icons"),
            t.text,
        );
        let icons: Vec<&str> = settings::ICONS.iter().map(|c| c.name(lang)).collect();
        x.dropdown(
            54,
            Rect::new(mx, win.y + x.ui.s(102), x.ui.s(136), x.ui.s(30)),
            x.ui.s(12),
            &icons,
            &mut self.settings.icons,
            None,
            10,
        );
        x.label(
            mx,
            win.y + x.ui.s(148),
            Weight::Medium,
            11,
            self.l("Обои", "Wallpaper"),
            t.text,
        );
        let wp = Rect::new(mx, win.y + x.ui.s(168), x.ui.s(50), x.ui.s(30));
        art::preview(
            x.c,
            wp,
            x.ui.s(6),
            art::wallpaper_colors(self.settings.wallpaper),
        );
        x.c.stroke_rrect_fx(wp, x.ui.s(6), x.ui.sfx(300).max(256), fade(t.text, 160));
        let walls: Vec<&str> = settings::WALLPAPERS.iter().map(|c| c.name(lang)).collect();
        x.menu_button(
            55,
            Rect::new(wp.right() + x.ui.s(8), wp.y, x.ui.s(78), wp.h),
            self.l("Выбрать…", "Choose…"),
            &walls,
            &mut self.settings.wallpaper,
        );
        div(x, 640);
        let ox = win.x + x.ui.s(654);
        x.label(
            ox,
            win.y + x.ui.s(82),
            Weight::SemiBold,
            10,
            self.l("Дополнительные опции:", "More options:"),
            t.text,
        );
        let rows: [(&str, &str); 4] = [
            ("Прозрачность окон", "Window transparency"),
            ("Закруглённые углы", "Rounded corners"),
            ("Эффекты анимации", "Animations"),
            ("Акцентный цвет", "Accent colour"),
        ];
        for (i, (ru, en)) in rows.iter().enumerate() {
            let y = win.y + x.ui.s(108 + 26 * i as i32);
            let label = if lang == Lang::Ru { *ru } else { *en };
            x.label_mid(
                Rect::new(ox, y, x.ui.s(90), x.ui.s(14)),
                Weight::Regular,
                9,
                label,
                t.text_dim,
            );
            let r = Rect::new(win.x + x.ui.s(CONTENT_R - 26), y, x.ui.s(26), x.ui.s(14));
            let v = match i {
                0 => &mut self.settings.transparency,
                1 => &mut self.settings.rounded_corners,
                2 => &mut self.settings.animations,
                _ => &mut self.settings.use_accent,
            };
            if x.toggle(60 + i as u32, r, v) {
                x.ui.request_redraw();
            }
        }
        let d = x.ui.s(11);
        for (i, (_, color)) in settings::ACCENTS.iter().enumerate() {
            let cx = ox + d / 2 + i as i32 * x.ui.s(19);
            let cy = win.y + x.ui.s(222);
            let r = Rect::new(cx - d / 2, cy - d / 2, d, d);
            let (resp, step) = x.selectable(70 + i as u32, r, d / 2);
            if resp.activated {
                self.settings.accent = i;
                self.settings.use_accent = true;
                x.ui.request_redraw();
            }
            if step != 0 {
                let n = (i as i32 + step).clamp(0, settings::ACCENTS.len() as i32 - 1) as usize;
                self.settings.accent = n;
                x.ui.set_focus(Some(70 + n as u32));
                x.ui.request_redraw();
            }
            let col = 0xff00_0000 | color;
            let col = if self.settings.use_accent {
                col
            } else {
                fade(col, 90)
            };
            if self.settings.accent == i && self.settings.use_accent {
                x.c.stroke_circle_fx(
                    cx as i64 * 256,
                    cy as i64 * 256,
                    (d as i64 * 256) * 75 / 100,
                    x.ui.sfx(300).max(256),
                    fade(t.text, 200),
                );
            }
            x.c.fill_circle_fx(cx as i64 * 256, cy as i64 * 256, d as i64 * 128, col);
        }
    }

    fn disk_screen(&mut self, x: &mut Ctx, win: Rect) {
        let t = x.ui.theme;
        self.title(
            x,
            win,
            self.l("Разметка диска", "Disk"),
            self.l("Выберите способ установки.", "Choose how to install."),
        );
        let items = match self.lang() {
            Lang::Ru => [
                (
                    "Автоматическая установка",
                    "Использовать весь диск (рекомендуется)",
                ),
                ("Ручная разметка", "Для опытных пользователей"),
                ("Режим «Эксперт»", "Больше контроля над разделами"),
            ],
            Lang::En => [
                ("Automatic", "Use the whole disk (recommended)"),
                ("Manual partitioning", "For experienced users"),
                ("Expert mode", "More control over the partitions"),
            ],
        };
        let rows: Vec<Rect> = (0..3)
            .map(|i| {
                Rect::new(
                    win.x + x.ui.s(CONTENT_X),
                    win.y + x.ui.s(78 + 48 * i),
                    x.ui.s(320),
                    x.ui.s(40),
                )
            })
            .collect();
        if x.radio_group(80, &rows, &items, &mut self.mode) {
            x.ui.request_redraw();
        }
        // The disk card.
        let card = Rect::new(
            win.x + x.ui.s(600),
            win.y + x.ui.s(70),
            x.ui.s(172),
            x.ui.s(208),
        );
        x.c.fill_rrect(card, x.ui.s(14), fade(t.field, 200));
        x.c.stroke_rrect_fx(card, x.ui.s(14), x.ui.sfx(256).max(256), t.field_border);
        art::drive(
            x.c,
            Rect::new(
                card.center_x() - x.ui.s(48),
                card.y + x.ui.s(20),
                x.ui.s(96),
                x.ui.s(64),
            ),
            &t,
        );
        match self.disks.get(self.disk).cloned() {
            Some(d) => {
                x.label_center(
                    Rect::new(card.x, card.y + x.ui.s(100), card.w, x.ui.s(16)),
                    Weight::Regular,
                    11,
                    self.l("Диск:", "Disk:"),
                    t.text_dim,
                );
                x.label_center(
                    Rect::new(card.x, card.y + x.ui.s(118), card.w, x.ui.s(24)),
                    Weight::SemiBold,
                    18,
                    &size_text(d.size),
                    t.text,
                );
                let model = if d.model.is_empty() {
                    d.name.clone()
                } else {
                    d.model.clone()
                };
                x.label_center(
                    Rect::new(card.x, card.y + x.ui.s(146), card.w, x.ui.s(14)),
                    Weight::Regular,
                    9,
                    &model,
                    t.text_faint,
                );
                if self.disks.len() > 1 {
                    let names: Vec<String> = self
                        .disks
                        .iter()
                        .map(|d| format!("{} · {}", d.name, size_text(d.size)))
                        .collect();
                    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                    let r = Rect::new(
                        card.x + x.ui.s(12),
                        card.bottom() - x.ui.s(40),
                        card.w - x.ui.s(24),
                        x.ui.s(28),
                    );
                    x.dropdown(90, r, x.ui.s(10), &refs, &mut self.disk, None, 9);
                } else {
                    x.label_center(
                        Rect::new(card.x, card.y + x.ui.s(164), card.w, x.ui.s(14)),
                        Weight::Regular,
                        9,
                        &format!("/dev/{}", d.name),
                        t.text_faint,
                    );
                }
            }
            None => {
                x.label_center(
                    Rect::new(card.x, card.y + x.ui.s(120), card.w, x.ui.s(20)),
                    Weight::Medium,
                    11,
                    self.l("Диски не найдены", "No disks found"),
                    t.text_dim,
                );
            }
        }
        self.error_line(x, win, 300);
    }

    fn partitions(&mut self, x: &mut Ctx, win: Rect) {
        let t = x.ui.theme;
        let expert = self.mode == 2;
        let (title, sub) = if expert {
            (
                self.l("Режим «Эксперт»", "Expert mode"),
                self.l(
                    "Размеры, метка и точная таблица разделов.",
                    "Sizes, label and the exact partition table.",
                ),
            )
        } else {
            (
                self.l("Ручная разметка", "Manual partitioning"),
                self.l("Выберите размеры разделов.", "Choose the partition sizes."),
            )
        };
        self.title(x, win, title, sub);
        let cx = win.x + x.ui.s(CONTENT_X);
        let esp_items: Vec<String> = ESP_SIZES
            .iter()
            .map(|m| format!("{} {}", m, self.l("МиБ", "MiB")))
            .collect();
        let esp_refs: Vec<&str> = esp_items.iter().map(|s| s.as_str()).collect();
        let total = self.selected_disk().map_or(0, |d| d.size);
        let root_items: Vec<String> = ROOT_SHARES
            .iter()
            .map(|&s| {
                if s == 100 {
                    String::from(self.l("Всё остальное место", "All the rest"))
                } else {
                    let esp = ESP_SIZES[self.esp];
                    let mib = (total / (1024 * 1024)).saturating_sub(esp + 2) * s / 100;
                    format!("{}% · {}", s, mib_text(mib))
                }
            })
            .collect();
        let root_refs: Vec<&str> = root_items.iter().map(|s| s.as_str()).collect();
        x.label(
            cx,
            win.y + x.ui.s(82),
            Weight::Medium,
            11,
            self.l("Раздел EFI (FAT32)", "EFI partition (FAT32)"),
            t.text,
        );
        x.dropdown(
            100,
            Rect::new(cx, win.y + x.ui.s(100), x.ui.s(200), x.ui.s(30)),
            x.ui.s(12),
            &esp_refs,
            &mut self.esp,
            None,
            11,
        );
        x.label(
            cx,
            win.y + x.ui.s(142),
            Weight::Medium,
            11,
            self.l("Системный раздел (VeloraFS)", "System partition (VeloraFS)"),
            t.text,
        );
        x.dropdown(
            101,
            Rect::new(cx, win.y + x.ui.s(160), x.ui.s(200), x.ui.s(30)),
            x.ui.s(12),
            &root_refs,
            &mut self.root_share,
            None,
            11,
        );
        if expert {
            x.label(
                cx,
                win.y + x.ui.s(202),
                Weight::Medium,
                11,
                self.l("Метка файловой системы", "File system label"),
                t.text,
            );
            x.text_field(
                102,
                Rect::new(cx, win.y + x.ui.s(220), x.ui.s(200), x.ui.s(30)),
                &mut self.label,
                "velora-root",
                false,
                16,
            );
        }
        // The resulting table.
        let tx = win.x + x.ui.s(452);
        let card = Rect::new(
            tx,
            win.y + x.ui.s(82),
            x.ui.s(CONTENT_R - 452),
            x.ui.s(if expert { 168 } else { 112 }),
        );
        x.c.fill_rrect(card, x.ui.s(12), fade(t.field, 200));
        x.c.stroke_rrect_fx(card, x.ui.s(12), x.ui.sfx(256).max(256), t.field_border);
        let l = self.layout();
        let disk_mib = total / (1024 * 1024);
        let root_mib = l.root_mib.unwrap_or(disk_mib.saturating_sub(l.esp_mib + 2));
        let rows = [
            (
                String::from("1  EFI  FAT32"),
                mib_text(l.esp_mib),
                String::from("LBA 2048"),
            ),
            (
                format!("2  {}  VeloraFS", l.root_label),
                mib_text(root_mib),
                format!("LBA {}", 2048 + l.esp_mib * 2048),
            ),
        ];
        let mut y = card.y + x.ui.s(14);
        x.label(
            card.x + x.ui.s(14),
            y,
            Weight::SemiBold,
            10,
            self.l("Таблица разделов GPT", "GPT partition table"),
            t.text,
        );
        y += x.ui.s(22);
        for (name, size, start) in rows.iter() {
            x.label(card.x + x.ui.s(14), y, Weight::Medium, 10, name, t.text);
            let sw = x.f.measure(Weight::Regular, x.ui.px(10), size);
            x.label(
                card.right() - x.ui.s(14) - sw,
                y,
                Weight::Regular,
                10,
                size,
                t.text_dim,
            );
            if expert {
                x.label(
                    card.x + x.ui.s(30),
                    y + x.ui.s(15),
                    Weight::Regular,
                    9,
                    start,
                    t.text_faint,
                );
                y += x.ui.s(15);
            }
            y += x.ui.s(22);
        }
        if expert {
            let types = [
                ("EFI: C12A7328-F81F-11D2-BA4B-00A0C93EC93B", ""),
                ("Velora: 5E1F7A30-9C42-4D8B-A1E7-56454C4F5241", ""),
            ];
            for (s, _) in types {
                x.label(card.x + x.ui.s(14), y, Weight::Regular, 8, s, t.text_faint);
                y += x.ui.s(14);
            }
        }
        self.error_line(x, win, 300);
    }

    fn user_screen(&mut self, x: &mut Ctx, win: Rect) {
        let t = x.ui.theme;
        self.title(
            x,
            win,
            self.l("Учётная запись", "Account"),
            self.l(
                "Имя пользователя и пароли для входа в систему.",
                "User name and passwords to log in.",
            ),
        );
        let cx = win.x + x.ui.s(CONTENT_X);
        let rx = win.x + x.ui.s(516);
        let w = x.ui.s(250);
        let h = x.ui.s(30);
        let lang = self.lang();
        let tr = |ru: &'static str, en: &'static str| if lang == Lang::Ru { ru } else { en };
        let lab = |x: &mut Ctx, px: i32, v: i32, icon: Icon, s: &str| {
            x.icon(icon, px, win.y + x.ui.s(v), x.ui.s(13), t.text_dim);
            x.label(
                px + x.ui.s(19),
                win.y + x.ui.s(v - 1),
                Weight::Medium,
                10,
                s,
                t.text,
            );
        };
        lab(
            x,
            cx,
            82,
            Icon::User,
            self.l("Имя пользователя", "User name"),
        );
        x.text_field(
            110,
            Rect::new(cx, win.y + x.ui.s(100), w, h),
            &mut self.user,
            tr("например, anna", "for example, anna"),
            false,
            32,
        );
        lab(x, cx, 146, Icon::Lock, self.l("Пароль", "Password"));
        x.text_field(
            111,
            Rect::new(cx, win.y + x.ui.s(164), w, h),
            &mut self.password,
            tr("Пароль", "Password"),
            true,
            64,
        );
        x.text_field(
            112,
            Rect::new(cx, win.y + x.ui.s(202), w, h),
            &mut self.password2,
            tr("Повторите пароль", "Repeat the password"),
            true,
            64,
        );
        lab(
            x,
            rx,
            146,
            Icon::Lock,
            self.l(
                "Пароль администратора (root)",
                "Administrator (root) password",
            ),
        );
        x.text_field(
            113,
            Rect::new(rx, win.y + x.ui.s(164), w, h),
            &mut self.root_password,
            tr("Пароль root", "root password"),
            true,
            64,
        );
        x.text_field(
            114,
            Rect::new(rx, win.y + x.ui.s(202), w, h),
            &mut self.root_password2,
            tr("Повторите пароль", "Repeat the password"),
            true,
            64,
        );
        x.label(
            rx,
            win.y + x.ui.s(100),
            Weight::Regular,
            9,
            self.l(
                "Строчные латинские буквы, цифры,",
                "Lowercase letters, digits,",
            ),
            t.text_faint,
        );
        x.label(
            rx,
            win.y + x.ui.s(114),
            Weight::Regular,
            9,
            self.l(
                "«_» и «-». Имя не может быть root.",
                "'_' and '-'. The name cannot be root.",
            ),
            t.text_faint,
        );
        self.error_line(x, win, 252);
    }

    fn confirm(&mut self, x: &mut Ctx, win: Rect) {
        let t = x.ui.theme;
        self.title(
            x,
            win,
            self.l("Подтвердите установку", "Confirm the installation"),
            "",
        );
        let card = Rect::new(
            win.x + x.ui.s(CONTENT_X),
            win.y + x.ui.s(70),
            x.ui.s(CONTENT_R - CONTENT_X),
            x.ui.s(150),
        );
        x.c.fill_rrect(card, x.ui.s(14), fade(t.field, 200));
        x.c.stroke_rrect_fx(card, x.ui.s(14), x.ui.sfx(256).max(256), t.field_border);
        art::drive(
            x.c,
            Rect::new(
                card.x + x.ui.s(22),
                card.y + x.ui.s(34),
                x.ui.s(92),
                x.ui.s(62),
            ),
            &t,
        );
        let tx = card.x + x.ui.s(140);
        let d = self.selected_disk().cloned();
        let (model, size, dev) = match &d {
            Some(d) => (
                if d.model.is_empty() {
                    String::from(self.l("Без названия", "Unnamed"))
                } else {
                    d.model.clone()
                },
                size_text(d.size),
                format!("/dev/{}", d.name),
            ),
            None => (String::new(), String::new(), String::new()),
        };
        x.label(
            tx,
            card.y + x.ui.s(22),
            Weight::SemiBold,
            15,
            &model,
            t.text,
        );
        x.label(
            tx,
            card.y + x.ui.s(48),
            Weight::Regular,
            11,
            &format!("{}: {}   ·   {}", self.l("Размер", "Size"), size, dev),
            t.text_dim,
        );
        let mode = [
            self.l("Автоматическая разметка", "Automatic partitioning"),
            self.l("Ручная разметка", "Manual partitioning"),
            self.l("Режим «Эксперт»", "Expert mode"),
        ][self.mode.min(2)];
        x.label(
            tx,
            card.y + x.ui.s(68),
            Weight::Regular,
            11,
            &format!(
                "{} · {}: {}",
                mode,
                self.l("пользователь", "user"),
                self.user
            ),
            t.text_dim,
        );
        let warn = if t.dark {
            rgb(255, 176, 92)
        } else {
            rgb(178, 92, 0)
        };
        x.icon(Icon::Warning, tx, card.y + x.ui.s(100), x.ui.s(18), warn);
        x.label(
            tx + x.ui.s(26),
            card.y + x.ui.s(98),
            Weight::SemiBold,
            12,
            self.l(
                "Все данные на этом диске будут удалены.",
                "All data on this disk will be erased.",
            ),
            warn,
        );
        x.label(
            tx + x.ui.s(26),
            card.y + x.ui.s(118),
            Weight::Regular,
            10,
            self.l("Это действие нельзя отменить.", "This cannot be undone."),
            t.text_dim,
        );
    }

    fn installing(&mut self, x: &mut Ctx, win: Rect) {
        let t = x.ui.theme;
        let (cx, cy) = (win.x + x.ui.s(352), win.y + x.ui.s(176));
        x.progress_ring(cx, cy, x.ui.s(76), x.ui.s(7), self.progress);
        let pct = format!("{}%", self.progress / 10);
        x.label_center(
            Rect::new(cx - x.ui.s(60), cy - x.ui.s(16), x.ui.s(120), x.ui.s(32)),
            Weight::SemiBold,
            26,
            &pct,
            t.text,
        );
        let tx = win.x + x.ui.s(478);
        x.label(
            tx,
            win.y + x.ui.s(72),
            Weight::SemiBold,
            17,
            self.l("Установка системы...", "Installing the system..."),
            t.text,
        );
        x.label(
            tx,
            win.y + x.ui.s(104),
            Weight::Regular,
            11,
            self.l(
                "Это может занять несколько минут.",
                "This may take a few minutes.",
            ),
            t.text_dim,
        );
        x.label(
            tx,
            win.y + x.ui.s(122),
            Weight::Regular,
            11,
            self.l(
                "Пожалуйста, не выключайте компьютер.",
                "Please do not turn the computer off.",
            ),
            t.text_dim,
        );
        x.progress_bar(
            Rect::new(
                tx,
                win.y + x.ui.s(154),
                x.ui.s(CONTENT_R - 478 - 8),
                x.ui.s(4),
            ),
            self.progress,
        );
        for (i, ph) in Phase::ALL.iter().enumerate() {
            let y = win.y + x.ui.s(180 + 26 * i as i32);
            let s = x.ui.s(14);
            let state = match self.phase {
                Some(p) if p > *ph => StepState::Done,
                Some(p) if p == *ph => StepState::Current,
                _ => StepState::Ahead,
            };
            match state {
                StepState::Done => x.icon(Icon::Check, tx, y, s, t.text),
                StepState::Current => {
                    let w = x.ui.sfx(450).max(256);
                    x.c.arc_fx(
                        (tx + s / 2) as i64 * 256,
                        (y + s / 2) as i64 * 256,
                        (s as i64 * 256) * 4 / 10,
                        w,
                        10000,
                        50000,
                        true,
                        t.text,
                    );
                }
                StepState::Ahead => {
                    x.c.fill_circle(tx + s / 2, y + s / 2, s * 4 / 10, fade(t.text_faint, 80))
                }
            }
            let color = match state {
                StepState::Ahead => t.text_faint,
                _ => t.text_dim,
            };
            let w = if state == StepState::Current {
                Weight::Medium
            } else {
                Weight::Regular
            };
            x.label_mid(
                Rect::new(tx + x.ui.s(24), y, x.ui.s(240), s),
                w,
                10,
                ph.name(self.lang()),
                color,
            );
        }
    }

    /// The last screen; true when Restart is pressed.
    fn done(&mut self, x: &mut Ctx, win: Rect) -> bool {
        let t = x.ui.theme;
        let cx = win.center_x();
        let cy = win.y + x.ui.s(78);
        x.c.fill_circle(cx, cy, x.ui.s(34), t.selected);
        let s = x.ui.s(32);
        x.icon(Icon::Check, cx - s / 2, cy - s / 2, s, t.text);
        let title = self.l("Установка завершена!", "Installation complete!");
        x.label_center(
            Rect::new(win.x, win.y + x.ui.s(134), win.w, x.ui.s(28)),
            Weight::SemiBold,
            20,
            title,
            t.text,
        );
        let desc = self.l(
            "Теперь вы можете перезагрузить компьютер\nи начать использовать Velora OS.",
            "You can now restart the computer\nand start using Velora OS.",
        );
        for (i, line) in desc.split('\n').enumerate() {
            x.label_center(
                Rect::new(
                    win.x,
                    win.y + x.ui.s(172 + 20 * i as i32),
                    win.w,
                    x.ui.s(18),
                ),
                Weight::Regular,
                12,
                line,
                t.text_dim,
            );
        }
        let bw = x.ui.s(196);
        let r = Rect::new(cx - bw / 2, win.y + x.ui.s(258), bw, x.ui.s(38));
        x.button(
            902,
            r,
            self.l("Перезагрузить", "Restart"),
            ButtonKind::Tonal,
            Some(Icon::Refresh),
            None,
        )
    }

    fn failed(&mut self, x: &mut Ctx, win: Rect) {
        let t = x.ui.theme;
        let warn = if t.dark {
            rgb(255, 138, 128)
        } else {
            rgb(196, 48, 40)
        };
        self.title(
            x,
            win,
            self.l("Установка не удалась", "The installation failed"),
            "",
        );
        x.icon(
            Icon::Warning,
            win.x + x.ui.s(CONTENT_X),
            win.y + x.ui.s(74),
            x.ui.s(22),
            warn,
        );
        let lines =
            x.f.wrap(Weight::Regular, x.ui.px(11), &self.failure, x.ui.s(500));
        for (i, l) in lines.iter().take(6).enumerate() {
            x.label(
                win.x + x.ui.s(CONTENT_X + 32),
                win.y + x.ui.s(76 + 18 * i as i32),
                Weight::Regular,
                11,
                l,
                t.text_dim,
            );
        }
        x.label(
            win.x + x.ui.s(CONTENT_X + 32),
            win.y + x.ui.s(200),
            Weight::Regular,
            11,
            self.l(
                "Диск мог быть записан частично. Можно вернуться и попробовать снова.",
                "The disk may be partly written. You can go back and try again.",
            ),
            t.text_faint,
        );
    }
}

/// "64 MiB", "476.4 GiB": binary units, for partitions.
pub fn mib_text(mib: u64) -> String {
    if mib < 1024 {
        format!("{} MiB", mib)
    } else {
        let tenths = (mib * 10 + 512) / 1024;
        format!("{}.{} GiB", tenths / 10, tenths % 10)
    }
}

/// "512 GB", "2 GB", "128 MB": decimal units, as disks are sold.
pub fn size_text(bytes: u64) -> String {
    const G: u64 = 1_000_000_000;
    const M: u64 = 1_000_000;
    if bytes >= 100 * G {
        format!("{} GB", (bytes + G / 2) / G)
    } else if bytes >= G {
        let tenths = (bytes * 10 + G / 2) / G;
        if tenths.is_multiple_of(10) {
            format!("{} GB", tenths / 10)
        } else {
            format!("{}.{} GB", tenths / 10, tenths % 10)
        }
    } else {
        format!("{} MB", (bytes + M / 2) / M)
    }
}

#[cfg(test)]
mod tests;

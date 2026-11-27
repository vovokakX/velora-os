//! A screen with every widget, for trying the toolkit (the `uidemo`
//! program) and for its tests.

use crate::canvas::Canvas;
use crate::decor;
use crate::geom::Rect;
use crate::icons::Icon;
use crate::image::Picture;
use crate::text::{Fonts, Weight};
use crate::theme::{DARK, LIGHT};
use crate::ui::{ButtonKind, Ctx, Ui};
use alloc::string::String;

pub struct Gallery {
    pub dark: bool,
    pub language: usize,
    pub region: usize,
    pub option: usize,
    pub toggles: [bool; 3],
    pub name: String,
    pub password: String,
    pub progress: u32,
    pub clicks: u32,
    /// The program should end (Escape or the Quit button).
    pub quit: bool,
}

impl Default for Gallery {
    fn default() -> Self {
        Gallery {
            dark: true,
            language: 0,
            region: 0,
            option: 0,
            toggles: [true, false, true],
            name: String::new(),
            password: String::new(),
            progress: 720,
            clicks: 0,
            quit: false,
        }
    }
}

const LANGUAGES: [&str; 8] = [
    "Русский",
    "English",
    "Українська",
    "Deutsch",
    "Français",
    "Español",
    "Polski",
    "Italiano",
];
const REGIONS: [&str; 6] = [
    "Россия",
    "Беларусь",
    "Казахстан",
    "Германия",
    "США",
    "Польша",
];

/// The window rectangle for a screen of `w` x `h`: the design's
/// proportions (805 x 368 units), as large as fits with a margin.
pub fn window_rect(w: i32, h: i32) -> (Rect, i64) {
    let ww = (w * 94 / 100).min(h * 94 / 100 * 805 / 368);
    let scale = ww as i64 * 256 / 805;
    let wh = (368 * scale / 256) as i32;
    (Rect::new((w - ww) / 2, (h - wh) / 2, ww, wh), scale)
}

impl Gallery {
    /// Draws the whole screen. Returns true if it should be drawn again.
    pub fn draw(&mut self, ui: &mut Ui, c: &mut Canvas, f: &mut Fonts, logo: &mut Picture) -> bool {
        let (win, scale) = window_rect(c.width as i32, c.height as i32);
        ui.scale = scale;
        ui.theme = if self.dark { DARK } else { LIGHT };
        let t = ui.theme;
        decor::backdrop(c, &t);
        let radius = ui.s(16);
        decor::window(c, win, radius, &t, scale);
        let side = Rect::new(win.x, win.y, ui.s(193), win.h);
        decor::sidebar(c, side, radius, &t);
        let mut x = Ctx { ui, c, f };
        // Logo and name.
        let lw = x.ui.s(40);
        let img = logo.fit(lw as usize, lw as usize);
        let (iw, ih) = (img.width as i32, img.height as i32);
        x.c.draw_image_tinted(
            img,
            side.center_x() - iw / 2,
            win.y + x.ui.s(28) + (lw - ih) / 2,
            t.text,
        );
        let name = "Velora OS";
        let px = x.ui.px(13);
        let nw = x.f.measure(Weight::Medium, px, name);
        x.label(
            side.center_x() - nw / 2,
            win.y + x.ui.s(72),
            Weight::Medium,
            13,
            name,
            t.text,
        );
        // Theme switch at the bottom of the sidebar.
        let tr = Rect::new(
            side.x + x.ui.s(24),
            win.bottom() - x.ui.s(36),
            x.ui.s(40),
            x.ui.s(20),
        );
        let mut light = !self.dark;
        if x.toggle(1, tr, &mut light) {
            self.dark = !light;
            x.ui.request_redraw();
        }
        // Content.
        let cx = side.right() + x.ui.s(32);
        x.label(
            cx,
            win.y + x.ui.s(28),
            Weight::SemiBold,
            20,
            "Элементы интерфейса",
            t.text,
        );
        x.label(
            cx,
            win.y + x.ui.s(58),
            Weight::Regular,
            11,
            "Мышь и клавиатура: Tab, стрелки, Enter, Esc.",
            t.text_dim,
        );
        let top = win.y + x.ui.s(86);
        let row_h = x.ui.s(30);
        x.dropdown(
            2,
            Rect::new(cx, top, x.ui.s(180), row_h),
            row_h / 2,
            &LANGUAGES,
            &mut self.language,
            Some(Icon::Globe),
            11,
        );
        x.text_field(
            3,
            Rect::new(cx, top + x.ui.s(40), x.ui.s(180), row_h),
            &mut self.name,
            "Имя пользователя",
            false,
            32,
        );
        x.text_field(
            4,
            Rect::new(cx, top + x.ui.s(80), x.ui.s(180), row_h),
            &mut self.password,
            "Пароль",
            true,
            64,
        );
        let rows = [
            Rect::new(cx, top + x.ui.s(124), x.ui.s(180), x.ui.s(34)),
            Rect::new(cx, top + x.ui.s(160), x.ui.s(180), x.ui.s(34)),
        ];
        x.radio_group(
            5,
            &rows,
            &[("Автоматически", "Весь диск"), ("Вручную", "Для опытных")],
            &mut self.option,
        );
        let col2 = cx + x.ui.s(210);
        x.select_list(
            10,
            Rect::new(col2, top, x.ui.s(150), x.ui.s(150)),
            &REGIONS,
            &mut self.region,
        );
        let col3 = col2 + x.ui.s(180);
        let labels = ["Прозрачность", "Анимации", "Скругления"];
        for (i, l) in labels.iter().enumerate() {
            let y = top + x.ui.s(4) + x.ui.s(28) * i as i32;
            x.label_mid(
                Rect::new(col3, y, x.ui.s(100), x.ui.s(16)),
                Weight::Regular,
                10,
                l,
                t.text_dim,
            );
            let r = Rect::new(col3 + x.ui.s(110), y, x.ui.s(28), x.ui.s(16));
            x.toggle(20 + i as u32, r, &mut self.toggles[i]);
        }
        let ring_y = top + x.ui.s(130);
        x.progress_ring(
            col3 + x.ui.s(40),
            ring_y,
            x.ui.s(32),
            x.ui.s(4),
            self.progress,
        );
        let pct = alloc::format!("{}%", self.progress / 10);
        x.label_center(
            Rect::new(
                col3 + x.ui.s(8),
                ring_y - x.ui.s(10),
                x.ui.s(64),
                x.ui.s(20),
            ),
            Weight::SemiBold,
            14,
            &pct,
            t.text,
        );
        x.progress_bar(
            Rect::new(col2, top + x.ui.s(166), x.ui.s(150), x.ui.s(4)),
            self.progress,
        );
        // Icons.
        let icons = [
            Icon::Globe,
            Icon::Clock,
            Icon::Keyboard,
            Icon::Pointer,
            Icon::Check,
            Icon::Moon,
            Icon::Sun,
            Icon::Refresh,
            Icon::Picture,
        ];
        for (i, ic) in icons.iter().enumerate() {
            let s = x.ui.s(16);
            x.icon(
                *ic,
                col2 + i as i32 * x.ui.s(22),
                top + x.ui.s(186),
                s,
                t.text_dim,
            );
        }
        // Buttons.
        let by = win.bottom() - x.ui.s(48);
        let bh = x.ui.s(32);
        let quit = Rect::new(
            win.right() - x.ui.s(24) - x.ui.s(124) - x.ui.s(100),
            by,
            x.ui.s(92),
            bh,
        );
        if x.button(
            30,
            quit,
            "Выйти",
            ButtonKind::Secondary,
            Some(Icon::ArrowLeft),
            None,
        ) {
            self.quit = true;
        }
        let go = Rect::new(win.right() - x.ui.s(24) - x.ui.s(124), by, x.ui.s(124), bh);
        if x.button(
            31,
            go,
            "Продолжить",
            ButtonKind::Primary,
            None,
            Some(Icon::ArrowRight),
        ) {
            self.clicks += 1;
            self.progress = (self.progress + 100) % 1100;
            x.ui.request_redraw();
        }
        if x.ui.take_key(crate::ui::Key::Escape) {
            self.quit = true;
        }
        x.end()
    }
}

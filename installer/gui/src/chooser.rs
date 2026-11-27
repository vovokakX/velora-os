//! The start menu of the Live system: graphical installation (the
//! default, started after a countdown), installation in the terminal, or
//! the Live system without installing. Labels are in Russian with English
//! below, since no language has been chosen yet.

use crate::{Assets, window_rect};
use alloc::format;
use velora_ui::Rect;
use velora_ui::canvas::{Canvas, fade};
use velora_ui::decor;
use velora_ui::icons::Icon;
use velora_ui::text::{Fonts, Weight};
use velora_ui::theme::DARK;
use velora_ui::ui::{Ctx, Key, Ui};

/// The three ways to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Graphical,
    Terminal,
    Live,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Graphical, Mode::Terminal, Mode::Live];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Graphical => "graphical",
            Mode::Terminal => "terminal",
            Mode::Live => "live",
        }
    }

    fn texts(self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            Mode::Graphical => (
                "Графическая установка",
                "Graphical installation",
                "Пошаговая установка с выбором\nязыка, оформления и диска.",
                "Step by step, with language,\nappearance and disk.",
            ),
            Mode::Terminal => (
                "Установка в терминале",
                "Installation in the terminal",
                "Программа velora-setup:\nвопросы и ответы в консоли.",
                "velora-setup: questions\nand answers in the console.",
            ),
            Mode::Live => (
                "Live-режим",
                "Live system",
                "Терминал vsh без установки.\nПароль для входа: velora.",
                "The vsh terminal, no install.\nLogin password: velora.",
            ),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Mode::Graphical => Icon::Pointer,
            Mode::Terminal => Icon::Keyboard,
            Mode::Live => Icon::Globe,
        }
    }
}

pub struct Chooser {
    pub selected: usize,
    /// Seconds left before the default starts; `None` once the user has
    /// touched anything.
    pub countdown: Option<u32>,
    background: decor::Cache,
}

impl Default for Chooser {
    fn default() -> Self {
        Chooser {
            selected: 0,
            countdown: Some(10),
            background: decor::Cache::default(),
        }
    }
}

impl Chooser {
    /// Draws the menu. Returns the chosen mode (when the user confirms or
    /// the countdown ends) and whether the frame should be drawn again.
    pub fn draw(
        &mut self,
        ui: &mut Ui,
        c: &mut Canvas,
        f: &mut Fonts,
        a: &mut Assets,
    ) -> (Option<Mode>, bool) {
        let (win, scale) = window_rect(c.width as i32, c.height as i32);
        ui.scale = scale;
        ui.theme = DARK;
        let t = ui.theme;
        let radius = ui.s(16);
        self.background.paint(c, 1, |c| {
            decor::backdrop(c, &t);
            decor::window(c, win, radius, &t, scale);
            decor::waves_side(c, win, radius, &t, true);
        });
        let mut x = Ctx { ui, c, f };
        let mut chosen = None;
        // Logo and title.
        let lh = x.ui.s(34);
        let img = a.logo.fit((lh * 2) as usize, lh as usize);
        let (iw, ih) = (img.width as i32, img.height as i32);
        x.c.draw_image_tinted(
            img,
            win.center_x() - iw / 2,
            win.y + x.ui.s(24) + (lh - ih) / 2,
            t.text,
        );
        x.label_center(
            Rect::new(win.x, win.y + x.ui.s(66), win.w, x.ui.s(24)),
            Weight::SemiBold,
            18,
            "Как запустить Velora OS?",
            t.text,
        );
        x.label_center(
            Rect::new(win.x, win.y + x.ui.s(90), win.w, x.ui.s(16)),
            Weight::Regular,
            10,
            "How do you want to start Velora OS?",
            t.text_faint,
        );
        // Cards.
        let cw = x.ui.s(220);
        let ch = x.ui.s(150);
        let gap = x.ui.s(18);
        let total = cw * 3 + gap * 2;
        let x0 = win.center_x() - total / 2;
        let y0 = win.y + x.ui.s(124);
        for (i, mode) in Mode::ALL.iter().enumerate() {
            let r = Rect::new(x0 + i as i32 * (cw + gap), y0, cw, ch);
            let (resp, step) = x.selectable(1 + i as u32, r, x.ui.s(14));
            if resp.activated {
                self.selected = i;
                chosen = Some(*mode);
            }
            if step != 0 {
                self.selected = (i as i32 + step).clamp(0, 2) as usize;
                x.ui.set_focus(Some(1 + self.selected as u32));
                self.countdown = None;
                x.ui.request_redraw();
            }
            let sel = self.selected == i;
            if sel {
                x.c.shadow(
                    r.offset(0, x.ui.s(6)),
                    x.ui.s(14),
                    x.ui.s(14),
                    fade(t.shadow, 200),
                );
            }
            let bg = if sel {
                t.selected
            } else if resp.hovered {
                t.field_hover
            } else {
                t.field
            };
            x.c.fill_rrect(r, x.ui.s(14), bg);
            x.c.stroke_rrect_fx(
                r,
                x.ui.s(14),
                x.ui.sfx(if sel { 400 } else { 256 }).max(256),
                if sel {
                    fade(t.text, 180)
                } else {
                    t.field_border
                },
            );
            let (ru, en, dru, _den) = mode.texts();
            let is = x.ui.s(26);
            // A number badge and the icon.
            let bx = r.x + x.ui.s(18);
            let by = r.y + x.ui.s(18);
            x.c.fill_circle(
                bx + is / 2,
                by + is / 2,
                is / 2 + x.ui.s(6),
                fade(t.text, if sel { 40 } else { 18 }),
            );
            x.icon(mode.icon(), bx, by, is, t.text);
            let num = format!("{}", i + 1);
            x.label(
                r.right() - x.ui.s(28),
                r.y + x.ui.s(16),
                Weight::SemiBold,
                12,
                &num,
                t.text_faint,
            );
            x.label(
                r.x + x.ui.s(18),
                r.y + x.ui.s(64),
                Weight::SemiBold,
                13,
                ru,
                t.text,
            );
            x.label(
                r.x + x.ui.s(18),
                r.y + x.ui.s(83),
                Weight::Regular,
                9,
                en,
                t.text_faint,
            );
            for (k, line) in dru.split('\n').enumerate() {
                x.label(
                    r.x + x.ui.s(18),
                    r.y + x.ui.s(104 + 15 * k as i32),
                    Weight::Regular,
                    9,
                    line,
                    t.text_dim,
                );
            }
        }
        // Countdown or hint.
        let hint = match self.countdown {
            Some(s) if self.selected == 0 => format!(
                "Графическая установка начнётся через {} с  ·  Graphical installation starts in {} s",
                s, s
            ),
            _ => alloc::string::String::from(
                "Стрелки и Enter, мышь или 1–3  ·  Arrows and Enter, the mouse or 1–3",
            ),
        };
        x.label_center(
            Rect::new(win.x, win.y + x.ui.s(300), win.w, x.ui.s(16)),
            Weight::Regular,
            10,
            &hint,
            t.text_faint,
        );
        // Keys: digits choose at once, arrows move, Enter starts.
        for (i, k) in ['1', '2', '3'].iter().enumerate() {
            if x.ui.take_key(Key::Char(*k)) {
                self.selected = i;
                chosen = Some(Mode::ALL[i]);
            }
        }
        if x.ui.focused().is_none() {
            if x.ui.take_key(Key::Right) || x.ui.take_key(Key::Down) {
                self.selected = (self.selected + 1).min(2);
                self.countdown = None;
                x.ui.request_redraw();
            }
            if x.ui.take_key(Key::Left) || x.ui.take_key(Key::Up) {
                self.selected = self.selected.saturating_sub(1);
                self.countdown = None;
                x.ui.request_redraw();
            }
            if x.ui.take_key(Key::Enter) {
                chosen = Some(Mode::ALL[self.selected]);
            }
        }
        if x.ui.focused().is_some() {
            self.countdown = None;
        }
        let again = x.end();
        (chosen, again)
    }

    /// One second passed. Returns the default mode when the countdown ends.
    pub fn tick(&mut self) -> Option<Mode> {
        match self.countdown {
            Some(0) | Some(1) => {
                self.countdown = None;
                Some(Mode::ALL[self.selected])
            }
            Some(n) => {
                self.countdown = Some(n - 1);
                None
            }
            None => None,
        }
    }
}

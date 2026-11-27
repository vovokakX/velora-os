//! Immediate-mode widgets with mouse and keyboard control.
//!
//! Every frame the program calls [`Ui::begin`] with the input events,
//! draws its widgets through a [`Ctx`] (each call draws the widget and
//! reports what the user did with it) and calls [`Ctx::end`]. Widgets that
//! take focus are visited with Tab and Shift+Tab in the order they were
//! drawn; Enter or Space activates the focused one; arrows move inside
//! lists, radio groups and open drop-down lists; Escape closes a list.
//! Keys that no widget used are left for the program ([`Ui::key`]).

use crate::canvas::{Canvas, Color, FX, fade};
use crate::geom::Rect;
use crate::icons::{self, Icon};
use crate::text::{Fonts, Weight};
use crate::theme::{DARK, Theme};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

pub type Id = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Tab,
    Enter,
    Escape,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key {
        key: Key,
        shift: bool,
    },
    MouseMove {
        x: i32,
        y: i32,
    },
    /// Left button pressed or released at the current position.
    MouseButton {
        down: bool,
    },
    /// Wheel steps, positive towards the user (scrolls down a list).
    Wheel(i32),
}

/// Splits input into frames with at most one key each: widgets take keys
/// while they draw, so keys of one frame could be taken out of order.
pub fn one_key_per_frame(events: &[Event]) -> Vec<Vec<Event>> {
    let mut out: Vec<Vec<Event>> = Vec::new();
    let mut cur = Vec::new();
    let mut has_key = false;
    for ev in events {
        let is_key = matches!(ev, Event::Key { .. } | Event::MouseButton { .. });
        if is_key && has_key {
            out.push(core::mem::take(&mut cur));
            has_key = false;
        }
        has_key |= is_key;
        cur.push(*ev);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// What happened to a widget this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Response {
    pub hovered: bool,
    pub focused: bool,
    /// Clicked or activated with Enter/Space.
    pub activated: bool,
}

struct Popup {
    id: Id,
    rect: Rect,
    row_h: i32,
    items: Vec<String>,
    hover: usize,
    radius: i32,
}

pub struct Ui {
    pub theme: Theme,
    /// Pixels per design unit, times 256.
    pub scale: i64,
    pub mouse: (i32, i32),
    mouse_down: bool,
    press_at: Option<(i32, i32)>,
    click: Option<(i32, i32)>,
    wheel: i32,
    keys: Vec<(Key, bool)>,
    focus: Option<Id>,
    /// Focus rings show only after keyboard use.
    pub focus_visible: bool,
    order: Vec<Id>,
    /// The focus order of the previous frame (Tab moves along it).
    prev_order: Vec<Id>,
    popup: Option<Popup>,
    picked: Option<(Id, usize)>,
    scroll: BTreeMap<Id, i32>,
    /// Set when the frame should be drawn again (focus moved).
    again: bool,
}

impl Ui {
    pub fn new(scale: i64) -> Self {
        Ui {
            theme: DARK,
            scale,
            mouse: (-1, -1),
            mouse_down: false,
            press_at: None,
            click: None,
            wheel: 0,
            keys: Vec::new(),
            focus: None,
            focus_visible: false,
            order: Vec::new(),
            prev_order: Vec::new(),
            popup: None,
            picked: None,
            scroll: BTreeMap::new(),
            again: false,
        }
    }

    /// Design units to pixels.
    pub fn s(&self, du: i32) -> i32 {
        ((du as i64 * self.scale + 128) >> 8) as i32
    }

    /// Fractional design units (in 1/256) to pixels.
    pub fn sf(&self, du_fx: i64) -> i32 {
        ((du_fx * self.scale + 32768) >> 16) as i32
    }

    /// Design units to 1/256 pixel (line widths).
    pub fn sfx(&self, du_fx: i64) -> i64 {
        du_fx * self.scale / 256
    }

    /// Font size in pixels for a size in design units.
    pub fn px(&self, du: i32) -> u32 {
        self.s(du).max(6) as u32
    }

    pub fn focused(&self) -> Option<Id> {
        self.focus
    }

    pub fn set_focus(&mut self, id: Option<Id>) {
        self.focus = id;
    }

    /// Starts a frame with the events since the last one.
    pub fn begin(&mut self, events: &[Event]) {
        self.click = None;
        self.wheel = 0;
        self.keys.clear();
        if !self.order.is_empty() {
            self.prev_order = core::mem::take(&mut self.order);
        }
        self.again = false;
        if !self.mouse_down {
            self.press_at = None;
        }
        for ev in events {
            match *ev {
                Event::MouseMove { x, y } => {
                    self.mouse = (x, y);
                    if let Some(p) = self.popup.as_mut()
                        && p.rect.contains(x, y)
                    {
                        p.hover = ((y - p.rect.y) / p.row_h.max(1)) as usize;
                        p.hover = p.hover.min(p.items.len().saturating_sub(1));
                    }
                }
                Event::MouseButton { down: true } => {
                    self.focus_visible = false;
                    self.mouse_down = true;
                    let (x, y) = self.mouse;
                    if let Some(p) = self.popup.take() {
                        if p.rect.contains(x, y) {
                            let i = ((y - p.rect.y) / p.row_h.max(1)) as usize;
                            if i < p.items.len() {
                                self.picked = Some((p.id, i));
                            }
                        }
                        // The press closes the list and goes nowhere else.
                        self.press_at = None;
                        continue;
                    }
                    self.press_at = Some((x, y));
                }
                Event::MouseButton { down: false } => {
                    self.mouse_down = false;
                    if self.press_at.is_some() {
                        self.click = Some(self.mouse);
                    }
                }
                Event::Wheel(d) => self.wheel += d,
                Event::Key { key, shift } => {
                    if let Some(p) = self.popup.as_mut() {
                        match key {
                            Key::Up => p.hover = p.hover.saturating_sub(1),
                            Key::Down => p.hover = (p.hover + 1).min(p.items.len() - 1),
                            Key::Home => p.hover = 0,
                            Key::End => p.hover = p.items.len() - 1,
                            Key::Enter | Key::Space => {
                                self.picked = Some((p.id, p.hover));
                                self.popup = None;
                            }
                            Key::Escape | Key::Tab => self.popup = None,
                            _ => {}
                        }
                        self.focus_visible = true;
                        continue;
                    }
                    if matches!(key, Key::Tab | Key::Up | Key::Down | Key::Left | Key::Right) {
                        self.focus_visible = true;
                    }
                    if key == Key::Tab {
                        self.move_focus(shift);
                        continue;
                    }
                    self.keys.push((key, shift));
                }
            }
        }
    }

    /// A key no widget has used (it stays for the program).
    pub fn key(&self, k: Key) -> bool {
        self.keys.iter().any(|&(key, _)| key == k)
    }

    /// Takes a key press if it is in this frame's input.
    pub fn take_key(&mut self, k: Key) -> bool {
        if let Some(i) = self.keys.iter().position(|&(key, _)| key == k) {
            self.keys.remove(i);
            true
        } else {
            false
        }
    }

    /// Typed characters of this frame (taken).
    pub fn take_chars(&mut self) -> Vec<char> {
        let mut out = Vec::new();
        self.keys.retain(|&(k, _)| match k {
            Key::Char(c) => {
                out.push(c);
                false
            }
            Key::Space => {
                out.push(' ');
                false
            }
            _ => true,
        });
        out
    }

    pub fn popup_open(&self) -> bool {
        self.popup.is_some()
    }

    /// Mouse position over `r`, unless a list is open on top.
    pub fn hover(&self, r: Rect) -> bool {
        self.popup.is_none() && r.contains(self.mouse.0, self.mouse.1)
    }

    /// Registers a focusable widget and reports what happened to it.
    pub fn interact(&mut self, id: Id, r: Rect) -> Response {
        self.order.push(id);
        let hovered = self.hover(r);
        let mut clicked = false;
        if let (Some(c), Some(p)) = (self.click, self.press_at)
            && r.contains(c.0, c.1)
            && r.contains(p.0, p.1)
        {
            clicked = true;
            self.focus = Some(id);
            self.click = None;
        }
        let focused = self.focus == Some(id);
        let mut activated = clicked;
        if focused && !clicked && (self.take_key(Key::Enter) || self.take_key(Key::Space)) {
            activated = true;
        }
        Response {
            hovered,
            focused,
            activated,
        }
    }

    /// Tab: the next (or previous) widget of the last frame's order.
    fn move_focus(&mut self, back: bool) {
        let order = &self.prev_order;
        if order.is_empty() {
            return;
        }
        let n = order.len();
        let cur = self.focus.and_then(|f| order.iter().position(|&x| x == f));
        let next = match (cur, back) {
            (None, false) => 0,
            (None, true) => n - 1,
            (Some(c), false) => (c + 1) % n,
            (Some(c), true) => (c + n - 1) % n,
        };
        self.focus = Some(order[next]);
    }

    /// Ends the frame. Returns true if the frame should be drawn again
    /// because something changed after drawing.
    pub fn end_frame(&mut self) -> bool {
        if let Some(f) = self.focus
            && !self.order.is_empty()
            && !self.order.contains(&f)
        {
            // The focused widget is gone (another screen).
            self.focus = None;
        }
        self.again
    }

    pub fn request_redraw(&mut self) {
        self.again = true;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// The main action: a light pill on the dark theme.
    Primary,
    /// A dark pill with a thin border.
    Secondary,
    /// A lighter grey pill without a border (a calm main action).
    Tonal,
}

/// The frame's drawing context: the widget state, the canvas and fonts.
pub struct Ctx<'a> {
    pub ui: &'a mut Ui,
    pub c: &'a mut Canvas,
    pub f: &'a mut Fonts,
}

impl Ctx<'_> {
    pub fn theme(&self) -> Theme {
        self.ui.theme
    }

    fn focus_ring(&mut self, r: Rect, radius: i32, focused: bool) {
        if focused && self.ui.focus_visible {
            let g = self.ui.s(3);
            let w = self.ui.sfx(384).max(FX);
            let t = self.ui.theme;
            self.c.stroke_rrect_fx(r.inset(-g), radius + g, w, t.focus);
        }
    }

    /// Text in the given weight and size (design units) with its top-left
    /// line box at (x, y); returns the width.
    pub fn label(&mut self, x: i32, y: i32, w: Weight, du: i32, text: &str, color: Color) -> i32 {
        let px = self.ui.px(du);
        let asc = self.f.ascent(w, px);
        self.f.draw(self.c, w, px, x, y + asc, text, color)
    }

    /// Text vertically centred in `r`, at its left edge.
    pub fn label_mid(&mut self, r: Rect, w: Weight, du: i32, text: &str, color: Color) -> i32 {
        let px = self.ui.px(du);
        self.f
            .draw_centered_y(self.c, w, px, r.x, r.center_y(), text, color)
    }

    /// Text centred in `r` both ways.
    pub fn label_center(&mut self, r: Rect, w: Weight, du: i32, text: &str, color: Color) {
        let px = self.ui.px(du);
        let tw = self.f.measure(w, px, text);
        self.f.draw_centered_y(
            self.c,
            w,
            px,
            r.center_x() - tw / 2,
            r.center_y(),
            text,
            color,
        );
    }

    pub fn icon(&mut self, icon: Icon, x: i32, y: i32, size: i32, color: Color) {
        let stroke = self.ui.sfx(400).max(FX);
        icons::draw(self.c, icon, x, y, size, color, stroke);
    }

    /// A pill button with an optional icon before or after the label.
    pub fn button(
        &mut self,
        id: Id,
        r: Rect,
        label: &str,
        kind: ButtonKind,
        before: Option<Icon>,
        after: Option<Icon>,
    ) -> bool {
        let resp = self.ui.interact(id, r);
        let t = self.ui.theme;
        let radius = r.h / 2;
        let (bg, fg) = match kind {
            ButtonKind::Primary => {
                let shadow = fade(t.shadow, 120);
                self.c
                    .shadow(r.offset(0, self.ui.s(2)), radius, self.ui.s(4), shadow);
                (
                    if resp.hovered {
                        t.primary_hover
                    } else {
                        t.primary
                    },
                    t.on_primary,
                )
            }
            ButtonKind::Secondary => (if resp.hovered { t.field_hover } else { t.field }, t.text),
            ButtonKind::Tonal => (
                if resp.hovered {
                    crate::canvas::lerp(t.selected, t.text, 30)
                } else {
                    t.selected
                },
                t.text,
            ),
        };
        self.c.fill_rrect(r, radius, bg);
        if kind == ButtonKind::Secondary {
            self.c
                .stroke_rrect_fx(r, radius, self.ui.sfx(256).max(FX), t.field_border);
        }
        let px = self.ui.px(12);
        let weight = Weight::SemiBold;
        let tw = self.f.measure(weight, px, label);
        let isz = self.ui.s(13);
        let gap = self.ui.s(6);
        let total = tw
            + if before.is_some() { isz + gap } else { 0 }
            + if after.is_some() { isz + gap } else { 0 };
        let mut x = r.center_x() - total / 2;
        let iy = r.center_y() - isz / 2;
        if let Some(i) = before {
            self.icon(i, x, iy, isz, fg);
            x += isz + gap;
        }
        self.f
            .draw_centered_y(self.c, weight, px, x, r.center_y(), label, fg);
        x += tw + gap;
        if let Some(i) = after {
            self.icon(i, x, iy, isz, fg);
        }
        self.focus_ring(r, radius, resp.focused);
        resp.activated
    }

    /// An on/off switch drawn in `r` (the track). Returns true if changed.
    pub fn toggle(&mut self, id: Id, r: Rect, on: &mut bool) -> bool {
        let resp = self.ui.interact(id, r);
        let mut changed = false;
        if resp.activated {
            *on = !*on;
            changed = true;
        }
        if resp.focused {
            if self.ui.take_key(Key::Left) && *on {
                *on = false;
                changed = true;
            }
            if self.ui.take_key(Key::Right) && !*on {
                *on = true;
                changed = true;
            }
        }
        let t = self.ui.theme;
        let radius = r.h / 2;
        self.c
            .fill_rrect(r, radius, if *on { t.track_on } else { t.track_off });
        let pad = (r.h / 8).max(1);
        let d = r.h - 2 * pad;
        let kx = if *on { r.right() - pad - d } else { r.x + pad };
        let knob = Rect::new(kx, r.y + pad, d, d);
        self.c.shadow(
            knob.offset(0, pad / 2),
            d / 2,
            pad.max(1),
            fade(t.shadow, 90),
        );
        self.c
            .fill_rrect(knob, d / 2, if *on { t.knob } else { t.knob_off });
        self.focus_ring(r, radius, resp.focused);
        changed
    }

    /// A radio button row: the circle at the left of `r`, a title and an
    /// optional second line. Returns true when chosen.
    pub fn radio(&mut self, id: Id, r: Rect, selected: bool, title: &str, sub: &str) -> bool {
        let resp = self.ui.interact(id, r);
        let t = self.ui.theme;
        let d = self.ui.s(18);
        let cx = r.x + d / 2;
        let cy = r.center_y();
        let (fcx, fcy) = (cx as i64 * FX, cy as i64 * FX);
        let rr = d as i64 * FX / 2;
        if selected {
            self.c
                .fill_circle_fx(fcx, fcy, rr, if t.dark { t.text } else { t.primary });
            self.c
                .fill_circle_fx(fcx, fcy, rr - self.ui.sfx(512), t.window);
            self.c.fill_circle_fx(
                fcx,
                fcy,
                rr - self.ui.sfx(1152),
                if t.dark { t.text } else { t.primary },
            );
        } else {
            self.c.fill_circle_fx(
                fcx,
                fcy,
                rr,
                if resp.hovered { t.field_hover } else { t.field },
            );
            self.c
                .stroke_circle_fx(fcx, fcy, rr - FX / 2, FX, t.field_border);
        }
        let tx = r.x + d + self.ui.s(12);
        if sub.is_empty() {
            self.label_mid(
                Rect::new(tx, r.y, r.w, r.h),
                Weight::Medium,
                11,
                title,
                t.text,
            );
        } else {
            let half = r.h / 2;
            self.label_mid(
                Rect::new(tx, r.y + half / 8, r.w, half),
                Weight::Medium,
                11,
                title,
                t.text,
            );
            self.label_mid(
                Rect::new(tx, r.y + half - half / 8, r.w, half),
                Weight::Regular,
                9,
                sub,
                t.text_faint,
            );
        }
        self.focus_ring(Rect::new(r.x, cy - d / 2, d, d), d / 2, resp.focused);
        resp.activated
    }

    /// Radio buttons for `items` (title, second line) in `rows`; the arrows
    /// move the choice. Returns true if it changed.
    pub fn radio_group(
        &mut self,
        base: Id,
        rows: &[Rect],
        items: &[(&str, &str)],
        sel: &mut usize,
    ) -> bool {
        let before = *sel;
        for (i, (r, (title, sub))) in rows.iter().zip(items).enumerate() {
            let id = base + i as Id;
            if self.radio(id, *r, *sel == i, title, sub) {
                *sel = i;
            }
            if self.ui.focused() == Some(id) {
                if self.ui.take_key(Key::Down) && i + 1 < items.len() {
                    *sel = i + 1;
                    self.ui.set_focus(Some(base + i as Id + 1));
                    self.ui.request_redraw();
                } else if self.ui.take_key(Key::Up) && i > 0 {
                    *sel = i - 1;
                    self.ui.set_focus(Some(base + i as Id - 1));
                    self.ui.request_redraw();
                }
            }
        }
        *sel != before
    }

    /// A closed drop-down list: the chosen item, an optional icon at the
    /// left and a chevron. Clicking (or Enter) opens the list below it.
    #[allow(clippy::too_many_arguments)]
    pub fn dropdown(
        &mut self,
        id: Id,
        r: Rect,
        radius: i32,
        items: &[&str],
        sel: &mut usize,
        icon: Option<Icon>,
        text_du: i32,
    ) -> bool {
        let mut changed = false;
        if let Some((pid, i)) = self.ui.picked
            && pid == id
        {
            self.ui.picked = None;
            if i != *sel {
                *sel = i;
                changed = true;
            }
        }
        let resp = self.ui.interact(id, r);
        if resp.focused {
            if self.ui.take_key(Key::Down) && *sel + 1 < items.len() {
                *sel += 1;
                changed = true;
            }
            if self.ui.take_key(Key::Up) && *sel > 0 {
                *sel -= 1;
                changed = true;
            }
        }
        let t = self.ui.theme;
        let bg = if resp.hovered { t.field_hover } else { t.field };
        self.c.fill_rrect(r, radius, bg);
        self.c
            .stroke_rrect_fx(r, radius, self.ui.sfx(256).max(FX), t.field_border);
        let pad = self.ui.s(12);
        let mut x = r.x + pad;
        if let Some(i) = icon {
            let isz = self.ui.s(17);
            self.icon(i, x, r.center_y() - isz / 2, isz, t.text);
            x += isz + self.ui.s(10);
        }
        let text = items.get(*sel).copied().unwrap_or("");
        self.label_mid(
            Rect::new(x, r.y, r.w, r.h),
            Weight::Medium,
            text_du,
            text,
            t.text,
        );
        let csz = self.ui.s(14);
        self.icon(
            Icon::ChevronDown,
            r.right() - pad - csz,
            r.center_y() - csz / 2,
            csz,
            t.text_dim,
        );
        self.focus_ring(r, radius, resp.focused);
        if resp.activated && !items.is_empty() {
            self.open_popup(id, r, items, *sel, radius, r.w);
        }
        changed
    }

    fn open_popup(&mut self, id: Id, r: Rect, items: &[&str], sel: usize, radius: i32, w: i32) {
        let row_h = self.ui.s(26);
        let n = items.len() as i32;
        let h = row_h * n.min(8);
        let below = r.bottom() + self.ui.s(4);
        let y = if below + h <= self.c.height as i32 {
            below
        } else {
            r.y - self.ui.s(4) - h
        };
        self.ui.popup = Some(Popup {
            id,
            rect: Rect::new(r.x, y, w, h),
            row_h,
            items: items.iter().map(|s| String::from(*s)).collect(),
            hover: sel,
            radius: radius.min(self.ui.s(10)),
        });
    }

    /// A button that opens a list of `items` ("Choose..."). Returns true
    /// if the choice changed.
    pub fn menu_button(
        &mut self,
        id: Id,
        r: Rect,
        label: &str,
        items: &[&str],
        sel: &mut usize,
    ) -> bool {
        let mut changed = false;
        if let Some((pid, i)) = self.ui.picked
            && pid == id
        {
            self.ui.picked = None;
            if i != *sel {
                *sel = i;
                changed = true;
            }
        }
        let resp = self.ui.interact(id, r);
        let t = self.ui.theme;
        let radius = self.ui.s(10);
        self.c.fill_rrect(
            r,
            radius,
            if resp.hovered { t.field_hover } else { t.field },
        );
        self.c
            .stroke_rrect_fx(r, radius, self.ui.sfx(256).max(FX), t.field_border);
        self.label_center(r, Weight::Medium, 11, label, t.text);
        self.focus_ring(r, radius, resp.focused);
        if resp.activated && !items.is_empty() {
            let px = self.ui.px(11);
            let widest = items
                .iter()
                .map(|s| self.f.measure(Weight::Regular, px, s))
                .max()
                .unwrap_or(0);
            let w = r.w.max(widest + self.ui.s(36));
            self.open_popup(id, r, items, *sel, radius, w);
        }
        changed
    }

    /// Any focusable area drawn by the program itself (a card, a swatch).
    /// The arrows Left/Right report as -1/+1 when it has the focus.
    pub fn selectable(&mut self, id: Id, r: Rect, radius: i32) -> (Response, i32) {
        let resp = self.ui.interact(id, r);
        let mut step = 0;
        if resp.focused {
            if self.ui.take_key(Key::Left) || self.ui.take_key(Key::Up) {
                step = -1;
            } else if self.ui.take_key(Key::Right) || self.ui.take_key(Key::Down) {
                step = 1;
            }
        }
        self.focus_ring(r, radius, resp.focused);
        (resp, step)
    }

    /// A list in the concept's style: the chosen item on top (highlighted,
    /// with a chevron), the other items under it, scrolling with the
    /// wheel and the arrows. Returns true if the choice changed.
    pub fn select_list(&mut self, id: Id, r: Rect, items: &[&str], sel: &mut usize) -> bool {
        let t = self.ui.theme;
        let before = *sel;
        let radius = self.ui.s(8);
        self.c.fill_rrect(r, radius, t.field);
        self.c
            .stroke_rrect_fx(r, radius, self.ui.sfx(256).max(FX), t.field_border);
        let pad = self.ui.s(4);
        let row_h = self.ui.s(26);
        let head = Rect::new(
            r.x + pad,
            r.y + pad,
            r.w - 2 * pad - self.ui.s(10),
            self.ui.s(30),
        );
        let resp = self.ui.interact(id, r);
        if resp.focused {
            if self.ui.take_key(Key::Down) && *sel + 1 < items.len() {
                *sel += 1;
            }
            if self.ui.take_key(Key::Up) && *sel > 0 {
                *sel -= 1;
            }
            if self.ui.take_key(Key::Home) {
                *sel = 0;
            }
            if self.ui.take_key(Key::End) {
                *sel = items.len().saturating_sub(1);
            }
        }
        // The other items, in order, below the header.
        let others: Vec<usize> = (0..items.len()).filter(|&i| i != *sel).collect();
        let area = Rect::new(
            r.x,
            head.bottom() + pad,
            r.w,
            r.bottom() - head.bottom() - 2 * pad,
        );
        let visible = (area.h / row_h.max(1)).max(1) as usize;
        let mut scroll = *self.ui.scroll.get(&id).unwrap_or(&0);
        if self.ui.hover(r) && self.ui.wheel != 0 {
            scroll += self.ui.wheel;
            self.ui.wheel = 0;
        }
        let max_scroll = others.len().saturating_sub(visible) as i32;
        scroll = scroll.clamp(0, max_scroll);
        self.ui.scroll.insert(id, scroll);
        // Header.
        self.c.fill_rrect(head, self.ui.s(6), t.selected);
        let tx = head.x + self.ui.s(10);
        self.label_mid(
            Rect::new(tx, head.y, head.w, head.h),
            Weight::SemiBold,
            11,
            items.get(*sel).copied().unwrap_or(""),
            t.text,
        );
        let csz = self.ui.s(13);
        self.icon(
            Icon::ChevronDown,
            head.right() - self.ui.s(10) - csz,
            head.center_y() - csz / 2,
            csz,
            t.text_dim,
        );
        let old_clip = self.c.set_clip(area);
        for (row, &i) in others.iter().enumerate().skip(scroll as usize) {
            let y = area.y + (row as i32 - scroll) * row_h;
            if y >= area.bottom() {
                break;
            }
            let rr = Rect::new(head.x, y, head.w, row_h);
            if self.ui.hover(rr) {
                self.c.fill_rrect(rr, self.ui.s(6), t.field_hover);
            }
            if let (Some(c), Some(p)) = (self.ui.click, self.ui.press_at)
                && rr.contains(c.0, c.1)
                && rr.contains(p.0, p.1)
            {
                *sel = i;
            }
            self.label_mid(
                Rect::new(tx, y, rr.w, row_h),
                Weight::Regular,
                11,
                items[i],
                t.text_dim,
            );
        }
        self.c.set_clip(old_clip);
        // Scroll bar.
        if max_scroll > 0 {
            let track = Rect::new(
                r.right() - pad - self.ui.s(4),
                r.y + pad * 2,
                self.ui.s(3),
                r.h - pad * 4,
            );
            let th = (track.h * visible as i32 / others.len() as i32).max(self.ui.s(16));
            let ty = track.y + (track.h - th) * scroll / max_scroll;
            self.c.fill_rrect(
                Rect::new(track.x, ty, track.w, th),
                track.w / 2,
                t.scrollbar,
            );
        }
        self.focus_ring(r, radius, resp.focused);
        if *sel != before {
            // Keep the list at the top after a choice.
            self.ui.scroll.insert(id, 0);
            self.ui.request_redraw();
        }
        *sel != before
    }

    /// A one-line text field. `secret` shows dots. Returns true if the
    /// text changed.
    pub fn text_field(
        &mut self,
        id: Id,
        r: Rect,
        text: &mut String,
        placeholder: &str,
        secret: bool,
        max_len: usize,
    ) -> bool {
        let resp = self.ui.interact(id, r);
        let mut changed = false;
        if resp.focused {
            for ch in self.ui.take_chars() {
                if !ch.is_control() && text.chars().count() < max_len {
                    text.push(ch);
                    changed = true;
                }
            }
            if self.ui.take_key(Key::Backspace) && text.pop().is_some() {
                changed = true;
            }
        }
        let t = self.ui.theme;
        let radius = self.ui.s(8);
        self.c.fill_rrect(
            r,
            radius,
            if resp.hovered { t.field_hover } else { t.field },
        );
        let border = if resp.focused {
            t.focus
        } else {
            t.field_border
        };
        self.c
            .stroke_rrect_fx(r, radius, self.ui.sfx(256).max(FX), border);
        let x = r.x + self.ui.s(12);
        let inner = Rect::new(x, r.y, r.w - self.ui.s(24), r.h);
        let old = self.c.set_clip(inner);
        let shown: String = if secret {
            text.chars().map(|_| '\u{2022}').collect()
        } else {
            text.clone()
        };
        let w = if shown.is_empty() {
            self.label_mid(inner, Weight::Regular, 11, placeholder, t.text_faint);
            0
        } else {
            self.label_mid(inner, Weight::Regular, 11, &shown, t.text)
        };
        if resp.focused {
            let ch = self.ui.s(14);
            let cx = if shown.is_empty() {
                x
            } else {
                x + w + self.ui.s(1)
            };
            self.c.fill_rect(
                Rect::new(cx, r.center_y() - ch / 2, self.ui.s(1).max(1), ch),
                t.text,
            );
        }
        self.c.set_clip(old);
        changed
    }

    /// A horizontal progress bar; `permille` from 0 to 1000.
    pub fn progress_bar(&mut self, r: Rect, permille: u32) {
        let t = self.ui.theme;
        self.c.fill_rrect(r, r.h / 2, t.progress_track);
        let w = (r.w as i64 * permille.min(1000) as i64 / 1000) as i32;
        if w > 0 {
            self.c
                .fill_rrect(Rect::new(r.x, r.y, w.max(r.h), r.h), r.h / 2, t.progress);
        }
    }

    /// A circular progress indicator with round ends, starting at the top.
    pub fn progress_ring(&mut self, cx: i32, cy: i32, radius: i32, width: i32, permille: u32) {
        let t = self.ui.theme;
        let (fx, fy) = (cx as i64 * FX, cy as i64 * FX);
        let r = radius as i64 * FX;
        let w = width as i64 * FX;
        self.c.stroke_circle_fx(fx, fy, r, w, t.progress_track);
        let len = (permille.min(1000) as u64 * 65536 / 1000) as u32;
        if len > 0 {
            self.c.arc_fx(fx, fy, r, w, 0, len, true, t.progress);
            // A dot marks the start.
            let (sx, sy) = crate::canvas::point_on_circle(fx, fy, r, 0);
            self.c.fill_circle_fx(sx, sy, w * 7 / 10, t.text);
        }
    }

    /// Draws the open drop-down list (if any) over everything else and
    /// ends the frame. Returns true if the frame should be drawn again.
    pub fn end(self) -> bool {
        if let Some(p) = self.ui.popup.as_ref() {
            let t = self.ui.theme;
            let r = p.rect;
            self.c
                .shadow(r.offset(0, self.ui.s(4)), p.radius, self.ui.s(10), t.shadow);
            self.c.fill_rrect(r, p.radius, t.field);
            self.c
                .stroke_rrect_fx(r, p.radius, self.ui.sfx(256).max(FX), t.field_border);
            let px = self.ui.px(11);
            for (i, item) in p.items.iter().enumerate() {
                let row = Rect::new(
                    r.x + self.ui.s(4),
                    r.y + p.row_h * i as i32,
                    r.w - self.ui.s(8),
                    p.row_h,
                );
                if row.bottom() > r.bottom() + 1 {
                    break;
                }
                if i == p.hover {
                    self.c
                        .fill_rrect(row.inset(self.ui.s(2)), self.ui.s(6), t.selected);
                }
                self.f.draw_centered_y(
                    self.c,
                    Weight::Regular,
                    px,
                    row.x + self.ui.s(10),
                    row.center_y(),
                    item,
                    t.text,
                );
            }
        }
        self.ui.end_frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_moves_focus_in_drawing_order() {
        let mut ui = Ui::new(256);
        let rects = [
            Rect::new(0, 0, 10, 10),
            Rect::new(20, 0, 10, 10),
            Rect::new(40, 0, 10, 10),
        ];
        let frame = |ui: &mut Ui, ev: &[Event]| {
            ui.begin(ev);
            let r: Vec<Response> = rects
                .iter()
                .enumerate()
                .map(|(i, r)| ui.interact(i as Id + 1, *r))
                .collect();
            ui.end_frame();
            r
        };
        frame(&mut ui, &[]);
        assert_eq!(ui.focused(), None);
        let tab = Event::Key {
            key: Key::Tab,
            shift: false,
        };
        frame(&mut ui, &[tab]);
        assert_eq!(ui.focused(), Some(1));
        frame(&mut ui, &[tab, tab]);
        assert_eq!(ui.focused(), Some(3));
        frame(&mut ui, &[tab]);
        assert_eq!(ui.focused(), Some(1));
        frame(&mut ui, &[tab]);
        assert_eq!(ui.focused(), Some(2));
        frame(
            &mut ui,
            &[Event::Key {
                key: Key::Tab,
                shift: true,
            }],
        );
        assert_eq!(ui.focused(), Some(1));
        // Enter activates the focused widget.
        let r = frame(
            &mut ui,
            &[Event::Key {
                key: Key::Enter,
                shift: false,
            }],
        );
        assert!(r[0].activated && !r[1].activated);
        assert!(ui.focus_visible);
    }

    #[test]
    fn click_needs_press_and_release_inside() {
        let mut ui = Ui::new(256);
        let r = Rect::new(10, 10, 20, 20);
        let mut run = |ev: &[Event]| {
            ui.begin(ev);
            let resp = ui.interact(7, r);
            ui.end_frame();
            resp
        };
        let mv = |x, y| Event::MouseMove { x, y };
        let down = Event::MouseButton { down: true };
        let up = Event::MouseButton { down: false };
        assert!(run(&[mv(15, 15), down, up]).activated);
        assert!(!run(&[mv(5, 5), down, mv(15, 15), up]).activated);
        assert!(run(&[mv(15, 15), down]).hovered);
        assert!(run(&[up]).activated);
        assert!(!ui.focus_visible);
        assert_eq!(ui.focused(), Some(7));
    }

    #[test]
    fn scale_converts_design_units() {
        let ui = Ui::new(384); // 1.5 pixels per unit
        assert_eq!(ui.s(10), 15);
        assert_eq!(ui.s(-10), -15);
        assert_eq!(ui.px(12), 18);
        assert_eq!(ui.sf(256 * 3), 5); // 4.5 rounds up
    }
}

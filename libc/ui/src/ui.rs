//! Immediate-mode widgets with mouse and keyboard control.
//!
//! Every frame the program sets the time ([`Ui::set_time`]), calls
//! [`Ui::begin`] with the input events, draws its widgets through a [`Ctx`]
//! (each call draws the widget and reports what the user did with it) and
//! calls [`Ctx::end`]. Widgets that take focus are visited with Tab and
//! Shift+Tab in the order they were drawn; Enter or Space activates the
//! focused one; arrows move inside lists, radio groups and open drop-down
//! lists; Escape closes a list. Keys that no widget used are left for the
//! program ([`Ui::key`]).
//!
//! Widgets animate their states (hover, press, a switch's knob, the
//! focus ring, a list opening) with [`Tweens`]; while something moves,
//! [`Ui::animating`] asks the program for more frames.

use crate::anim::{self, Tweens};
use crate::canvas::{Canvas, Color, FX, fade, lerp};
use crate::geom::Rect;
use crate::icons::{self, Icon};
use crate::text::{Fonts, Weight};
use crate::theme::{DARK, Theme};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

pub type Id = u32;

/// Rows of an open drop-down list are reported with these ids (plus the
/// row index) in [`Ui::areas`].
pub const POPUP_ROW: Id = 0x7000_0000;

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
    /// A function key, F1 is 1.
    F(u8),
    Insert,
}

/// Modifier bits of [`Event::Chord`] and [`Event::Modifier`].
pub mod mods {
    pub const SHIFT: u8 = 1;
    pub const CTRL: u8 = 2;
    pub const ALT: u8 = 4;
    pub const SUPER: u8 = 8;
}

/// A modifier key itself (see [`Event::Modifier`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Shift,
    Ctrl,
    Alt,
    Super,
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
    /// A key pressed with Ctrl, Alt or Super held (`mods`: bits of
    /// [`mods`]); a letter comes as its lower case character. Widgets
    /// ignore these: they are shortcuts for the program.
    Chord {
        key: Key,
        mods: u8,
    },
    /// A modifier key pressed or released; `mods` is the state after it.
    /// Only reported when the program asks for it
    /// (`Screen::extended_input`).
    Modifier {
        key: Modifier,
        down: bool,
        mods: u8,
    },
    /// The right button pressed or released (only with
    /// `Screen::extended_input`).
    RightButton {
        down: bool,
    },
}

/// What the next frame has to draw again (see [`Ui::plan`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redraw {
    /// Nothing visible changes: the frame can be skipped (only the
    /// pointer moved).
    Nothing,
    /// Only this part of the screen.
    Area(Rect),
    /// The whole screen.
    All,
}

/// Splits input into frames with at most one key each: widgets take keys
/// while they draw, so keys of one frame could be taken out of order.
pub fn one_key_per_frame(events: &[Event]) -> Vec<Vec<Event>> {
    let mut out: Vec<Vec<Event>> = Vec::new();
    let mut cur = Vec::new();
    let mut has_key = false;
    for ev in events {
        let is_key = matches!(
            ev,
            Event::Key { .. }
                | Event::MouseButton { .. }
                | Event::Wheel(_)
                | Event::Chord { .. }
                | Event::RightButton { .. }
        );
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
    /// The button is held down on it.
    pub pressed: bool,
    /// Clicked or activated with Enter/Space.
    pub activated: bool,
    pub disabled: bool,
}

struct Popup {
    id: Id,
    /// Where the open list is (all visible rows).
    rect: Rect,
    row_h: i32,
    items: Vec<String>,
    hover: usize,
    scroll: usize,
    visible: usize,
    radius: i32,
}

impl Popup {
    fn row_at(&self, y: i32) -> Option<usize> {
        if y < self.rect.y || y >= self.rect.bottom() {
            return None;
        }
        let i = self.scroll + ((y - self.rect.y) / self.row_h.max(1)) as usize;
        (i < self.items.len()).then_some(i)
    }

    fn max_scroll(&self) -> usize {
        self.items.len().saturating_sub(self.visible)
    }

    fn keep_visible(&mut self) {
        if self.hover < self.scroll {
            self.scroll = self.hover;
        } else if self.hover >= self.scroll + self.visible {
            self.scroll = self.hover + 1 - self.visible;
        }
    }
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
    /// Widgets drawn while this is false are shown dimmed and ignore
    /// input.
    enabled: bool,
    /// Animated values; their clock is set with [`Ui::set_time`].
    pub tw: Tweens,
    /// Interactive areas of this frame: (id, area). Rows of lists use
    /// their own ids. Programs print them for automatic tests.
    pub areas: Vec<(Id, Rect)>,
    /// The widget that last took a click or a key (for tests).
    pub last_hit: Option<Id>,
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
            enabled: true,
            tw: Tweens::default(),
            areas: Vec::new(),
            last_hit: None,
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

    /// The clock for animations, in milliseconds.
    pub fn set_time(&mut self, now_ms: u64) {
        self.tw.now = now_ms;
    }

    /// Turns animations on or off.
    pub fn set_animations(&mut self, on: bool) {
        self.tw.enabled = on;
    }

    /// Something is still moving: draw another frame soon.
    pub fn animating(&self) -> bool {
        self.tw.active()
    }

    /// What the next frame must draw for `events`, judged from the last
    /// frame: the widgets the pointer enters or leaves and the widgets
    /// whose animated values moved since they were drawn (each with a
    /// margin for focus rings and shadows). Keys, clicks, the wheel and
    /// animations that belong to no widget need the whole screen.
    pub fn plan(&self, events: &[Event], now: u64) -> Redraw {
        if self.again || self.tw.marked() {
            return Redraw::All;
        }
        let margin = self.s(8).max(2);
        let mut area = Rect::default();
        let mut add = |r: Rect| {
            let r = r.inset(-margin);
            area = if area.is_empty() { r } else { area.union(r) };
        };
        let mut at = self.mouse;
        for ev in events {
            match *ev {
                Event::MouseMove { x, y } if !self.mouse_down => {
                    for (_, r) in &self.areas {
                        if r.contains(at.0, at.1) != r.contains(x, y) {
                            add(*r);
                        }
                    }
                    at = (x, y);
                }
                _ => return Redraw::All,
            }
        }
        for key in self.tw.changed(now) {
            let id = (key >> 8) as Id;
            match self.areas.iter().find(|(i, _)| *i == id) {
                Some((_, r)) => add(*r),
                None => return Redraw::All,
            }
            if let Some(p) = self.popup.as_ref()
                && p.id == id
            {
                // With its frame, the shadow and the soft edge of the
                // opening (see `Ctx::end`).
                add(p.rect.inset(-self.s(28)));
            }
        }
        if area.is_empty() {
            Redraw::Nothing
        } else {
            Redraw::Area(area)
        }
    }

    /// Takes the pointer position from events whose frame is skipped
    /// ([`Redraw::Nothing`]).
    pub fn track_pointer(&mut self, events: &[Event]) {
        for ev in events {
            if let Event::MouseMove { x, y } = *ev {
                self.mouse = (x, y);
            }
        }
    }

    /// The open drop-down list's id, if any.
    pub fn open_popup_id(&self) -> Option<Id> {
        self.popup.as_ref().map(|p| p.id)
    }

    /// Scroll position of a list (for tests and state reports).
    pub fn scroll_of(&self, id: Id) -> i32 {
        if let Some(p) = self.popup.as_ref()
            && p.id == id
        {
            return p.scroll as i32;
        }
        *self.scroll.get(&id).unwrap_or(&0)
    }

    /// Starts a frame with the events since the last one.
    pub fn begin(&mut self, events: &[Event]) {
        let now = self.tw.now;
        self.tw.begin(now);
        self.click = None;
        self.wheel = 0;
        self.keys.clear();
        self.areas.clear();
        self.enabled = true;
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
                        && let Some(i) = p.row_at(y)
                    {
                        p.hover = i;
                    }
                }
                Event::MouseButton { down: true } => {
                    self.focus_visible = false;
                    self.mouse_down = true;
                    let (x, y) = self.mouse;
                    if let Some(p) = self.popup.as_ref() {
                        if p.rect.contains(x, y)
                            && let Some(i) = p.row_at(y)
                        {
                            self.picked = Some((p.id, i));
                            self.last_hit = Some(POPUP_ROW + i as Id);
                        }
                        // The press closes the list and goes nowhere else.
                        self.popup = None;
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
                Event::Wheel(d) => {
                    if let Some(p) = self.popup.as_mut() {
                        let s = (p.scroll as i32 + d).clamp(0, p.max_scroll() as i32);
                        p.scroll = s as usize;
                        let (x, y) = self.mouse;
                        if p.rect.contains(x, y)
                            && let Some(i) = p.row_at(y)
                        {
                            p.hover = i;
                        }
                    } else {
                        self.wheel += d;
                    }
                }
                Event::Key { key, shift } => {
                    if let Some(p) = self.popup.as_mut() {
                        let last = p.items.len().saturating_sub(1);
                        let page = p.visible.max(1);
                        match key {
                            Key::Up => p.hover = p.hover.saturating_sub(1),
                            Key::Down => p.hover = (p.hover + 1).min(last),
                            Key::PageUp => p.hover = p.hover.saturating_sub(page),
                            Key::PageDown => p.hover = (p.hover + page).min(last),
                            Key::Home => p.hover = 0,
                            Key::End => p.hover = last,
                            Key::Enter | Key::Space => {
                                self.picked = Some((p.id, p.hover));
                                self.last_hit = Some(POPUP_ROW + p.hover as Id);
                                self.popup = None;
                            }
                            Key::Escape | Key::Tab => self.popup = None,
                            _ => {}
                        }
                        if let Some(p) = self.popup.as_mut() {
                            p.keep_visible();
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
                Event::Chord { .. } | Event::Modifier { .. } | Event::RightButton { .. } => {}
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

    /// Widgets drawn after `set_enabled(false)` are dimmed and do not take
    /// input or focus, until `set_enabled(true)` (or the next frame).
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Mouse position over `r`, unless a list is open on top.
    pub fn hover(&self, r: Rect) -> bool {
        self.popup.is_none() && r.contains(self.mouse.0, self.mouse.1)
    }

    /// A click (press and release) inside `r` this frame; it is taken.
    pub fn take_click(&mut self, r: Rect) -> bool {
        if let (Some(c), Some(p)) = (self.click, self.press_at)
            && r.contains(c.0, c.1)
            && r.contains(p.0, p.1)
        {
            self.click = None;
            true
        } else {
            false
        }
    }

    /// Registers an area for the tests without making it focusable.
    pub fn area(&mut self, id: Id, r: Rect) {
        self.areas.push((id, r));
    }

    /// Registers a focusable widget and reports what happened to it.
    pub fn interact(&mut self, id: Id, r: Rect) -> Response {
        if !self.enabled {
            if self.focus == Some(id) {
                self.focus = None;
            }
            return Response {
                disabled: true,
                ..Response::default()
            };
        }
        self.order.push(id);
        self.areas.push((id, r));
        let hovered = self.hover(r);
        let pressed = self.mouse_down
            && self.popup.is_none()
            && self.press_at.is_some_and(|p| r.contains(p.0, p.1));
        let clicked = self.take_click(r);
        if clicked {
            self.focus = Some(id);
        }
        let focused = self.focus == Some(id);
        let mut activated = clicked;
        if focused && !clicked && (self.take_key(Key::Enter) || self.take_key(Key::Space)) {
            activated = true;
        }
        if activated {
            self.last_hit = Some(id);
        }
        Response {
            hovered,
            focused,
            pressed,
            activated,
            disabled: false,
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

    /// An animated value for widget `id` (`slot` tells its values apart).
    pub fn anim(&mut self, id: Id, slot: u8, target: i32, dur: u32) -> i32 {
        self.tw.value((id as u64) << 8 | slot as u64, target, dur)
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

/// Animation slots of a widget.
const HOVER: u8 = 1;
const PRESS: u8 = 2;
const FOCUS: u8 = 3;
const VALUE: u8 = 4;
const OPEN: u8 = 5;

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

    /// `c` as drawn by a widget: dimmed when disabled.
    fn tone(&self, c: Color, resp: &Response) -> Color {
        if resp.disabled { fade(c, 100) } else { c }
    }

    /// Hover and press amounts (0..=255) of a widget, animated.
    fn states(&mut self, id: Id, resp: &Response) -> (u8, u8) {
        let h = self
            .ui
            .anim(id, HOVER, if resp.hovered { 255 } else { 0 }, anim::FAST);
        let p = self
            .ui
            .anim(id, PRESS, if resp.pressed { 255 } else { 0 }, 70);
        (h.clamp(0, 255) as u8, p.clamp(0, 255) as u8)
    }

    /// The keyboard focus ring around `r`, in the accent colour; it fades
    /// in and out.
    pub fn focus_ring(&mut self, id: Id, r: Rect, radius: i32, focused: bool) {
        let on = focused && self.ui.focus_visible;
        let a = self
            .ui
            .anim(id, FOCUS, if on { 255 } else { 0 }, anim::FAST);
        if a <= 0 {
            return;
        }
        let g = self.ui.s(3);
        let w = self.ui.sfx(448).max(FX * 3 / 2);
        let color = fade(self.ui.theme.focus(), a as u8);
        self.c.stroke_rrect_fx(r.inset(-g), radius + g, w, color);
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
        let (h, p) = self.states(id, &resp);
        let t = self.ui.theme;
        // Pressed buttons sink a little.
        let sink = self.ui.sf(160) * p as i32 / 255;
        let body = Rect::new(r.x + sink, r.y + sink, r.w - 2 * sink, r.h - 2 * sink);
        let radius = body.h / 2;
        let (bg, fg) = match kind {
            ButtonKind::Primary => {
                let shadow = fade(t.shadow, (120 - 60 * p as u32 / 255) as u8);
                if !resp.disabled {
                    self.c
                        .shadow(body.offset(0, self.ui.s(2)), radius, self.ui.s(4), shadow);
                }
                let bg = lerp(lerp(t.primary, t.primary_hover, h), t.primary_pressed, p);
                (bg, t.on_primary)
            }
            ButtonKind::Secondary => (
                lerp(lerp(t.field, t.field_hover, h), t.field_pressed, p),
                t.text,
            ),
            ButtonKind::Tonal => (
                lerp(
                    lerp(t.selected, lerp(t.selected, t.text, 30), h),
                    t.field_pressed,
                    p,
                ),
                t.text,
            ),
        };
        let bg = self.tone(bg, &resp);
        let fg = self.tone(fg, &resp);
        self.c.fill_rrect(body, radius, bg);
        if kind == ButtonKind::Secondary {
            let border = self.tone(t.field_border, &resp);
            self.c
                .stroke_rrect_fx(body, radius, self.ui.sfx(256).max(FX), border);
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
            // The arrow leans forward while hovered.
            let nudge = self.ui.sf(400) * h as i32 / 255;
            self.icon(i, x + nudge, iy, isz, fg);
        }
        self.focus_ring(id, r, r.h / 2, resp.focused);
        resp.activated
    }

    /// An on/off switch drawn in `r` (the track): the accent colour when
    /// on, neutral when off; the knob slides. Returns true if changed.
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
        if changed {
            self.ui.last_hit = Some(id);
        }
        let (h, p) = self.states(id, &resp);
        let v = self
            .ui
            .anim(id, VALUE, if *on { 1000 } else { 0 }, anim::NORMAL);
        let t = self.ui.theme;
        let radius = r.h / 2;
        let off = lerp(t.track_off, t.text_faint, h / 3);
        let track = lerp(off, t.accent, (v * 255 / 1000) as u8);
        self.c.fill_rrect(r, radius, self.tone(track, &resp));
        let pad = (r.h / 8).max(1);
        let d = r.h - 2 * pad;
        // The knob stretches a little while pressed.
        let stretch = d * p as i32 / (255 * 5);
        let travel = r.w - 2 * pad - d - stretch;
        let kx = r.x + pad + travel * v / 1000;
        let knob = Rect::new(kx, r.y + pad, d + stretch, d);
        if !resp.disabled {
            self.c.shadow(
                knob.offset(0, pad / 2),
                d / 2,
                pad.max(1),
                fade(t.shadow, 90),
            );
        }
        let kc = lerp(t.knob_off, t.knob, (v * 255 / 1000) as u8);
        self.c.fill_rrect(knob, d / 2, self.tone(kc, &resp));
        self.focus_ring(id, r, radius, resp.focused);
        changed
    }

    /// A radio button row: the circle at the left of `r`, a title and an
    /// optional second line. Returns true when chosen.
    pub fn radio(&mut self, id: Id, r: Rect, selected: bool, title: &str, sub: &str) -> bool {
        let resp = self.ui.interact(id, r);
        let (h, _) = self.states(id, &resp);
        let v = self
            .ui
            .anim(id, VALUE, if selected { 1000 } else { 0 }, anim::NORMAL);
        let t = self.ui.theme;
        let d = self.ui.s(18);
        let cx = r.x + d / 2;
        let cy = r.center_y();
        let (fcx, fcy) = (cx as i64 * FX, cy as i64 * FX);
        let rr = d as i64 * FX / 2;
        // Hover: a soft halo behind the circle.
        if h > 0 && !resp.disabled {
            self.c
                .fill_circle_fx(fcx, fcy, rr * 17 / 10, fade(t.field_hover, h));
        }
        let ring = lerp(t.field, t.accent, (v * 255 / 1000) as u8);
        self.c.fill_circle_fx(fcx, fcy, rr, self.tone(ring, &resp));
        let inner = rr - self.ui.sfx(512) * v as i64 / 1000 - FX / 2 * (1000 - v) as i64 / 1000;
        self.c
            .fill_circle_fx(fcx, fcy, inner, self.tone(t.window, &resp));
        if v > 0 {
            let dot = (rr - self.ui.sfx(1152)) * v as i64 / 1000;
            self.c
                .fill_circle_fx(fcx, fcy, dot, self.tone(t.accent, &resp));
        }
        if v < 1000 {
            let border = fade(t.field_border, (255 - v * 255 / 1000) as u8);
            self.c.stroke_circle_fx(fcx, fcy, rr - FX / 2, FX, border);
        }
        let tx = r.x + d + self.ui.s(12);
        let title_c = self.tone(t.text, &resp);
        let sub_c = self.tone(t.text_faint, &resp);
        if sub.is_empty() {
            self.label_mid(
                Rect::new(tx, r.y, r.w, r.h),
                Weight::Medium,
                11,
                title,
                title_c,
            );
        } else {
            let half = r.h / 2;
            self.label_mid(
                Rect::new(tx, r.y + half / 8, r.w, half),
                Weight::Medium,
                11,
                title,
                title_c,
            );
            self.label_mid(
                Rect::new(tx, r.y + half - half / 8, r.w, half),
                Weight::Regular,
                9,
                sub,
                sub_c,
            );
        }
        self.focus_ring(id, Rect::new(r.x, cy - d / 2, d, d), d / 2, resp.focused);
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

    /// Takes the choice made in this widget's open list, if any.
    fn take_pick(&mut self, id: Id, sel: &mut usize) -> bool {
        if let Some((pid, i)) = self.ui.picked
            && pid == id
        {
            self.ui.picked = None;
            if i != *sel {
                *sel = i;
                return true;
            }
        }
        false
    }

    /// A closed drop-down list: the chosen item, an optional icon at the
    /// left and a chevron. Clicking (or Enter) opens the list below it;
    /// Up and Down change the choice without opening it.
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
        let mut changed = self.take_pick(id, sel);
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
        let (h, p) = self.states(id, &resp);
        let open = self.ui.open_popup_id() == Some(id);
        let t = self.ui.theme;
        let bg = lerp(lerp(t.field, t.field_hover, h), t.field_pressed, p);
        self.c.fill_rrect(r, radius, self.tone(bg, &resp));
        let border = if open {
            fade(t.accent, 200)
        } else {
            t.field_border
        };
        self.c.stroke_rrect_fx(
            r,
            radius,
            self.ui.sfx(256).max(FX),
            self.tone(border, &resp),
        );
        let pad = self.ui.s(12);
        let mut x = r.x + pad;
        let fg = self.tone(t.text, &resp);
        if let Some(i) = icon {
            let isz = self.ui.s(17);
            self.icon(i, x, r.center_y() - isz / 2, isz, fg);
            x += isz + self.ui.s(10);
        }
        let text = items.get(*sel).copied().unwrap_or("");
        let old = self
            .c
            .set_clip(Rect::new(r.x, r.y, r.w - pad - self.ui.s(18), r.h));
        self.label_mid(
            Rect::new(x, r.y, r.w, r.h),
            Weight::Medium,
            text_du,
            text,
            fg,
        );
        self.c.set_clip(old);
        let csz = self.ui.s(14);
        let icon = if open {
            Icon::ChevronUp
        } else {
            Icon::ChevronDown
        };
        let cc = self.tone(t.text_dim, &resp);
        self.icon(icon, r.right() - pad - csz, r.center_y() - csz / 2, csz, cc);
        self.focus_ring(id, r, radius, resp.focused);
        if resp.activated && !items.is_empty() {
            self.open_popup(id, r, items, *sel, radius, r.w);
        }
        if changed {
            self.ui.last_hit = Some(id);
            self.ui.request_redraw();
        }
        changed
    }

    fn open_popup(&mut self, id: Id, r: Rect, items: &[&str], sel: usize, radius: i32, w: i32) {
        let row_h = self.ui.s(26);
        let pad = self.ui.s(4);
        let below = r.bottom() + self.ui.s(4);
        let space_below = self.c.height as i32 - below - self.ui.s(8);
        let space_above = r.y - self.ui.s(12);
        let fit = |space: i32| (space / row_h.max(1)).max(1) as usize;
        let want = items.len().min(8);
        let (visible, y) = if fit(space_below) >= want || space_below >= space_above {
            let v = want.min(fit(space_below));
            (v, below + pad)
        } else {
            let v = want.min(fit(space_above));
            (v, r.y - self.ui.s(4) - pad - row_h * v as i32)
        };
        let mut p = Popup {
            id,
            rect: Rect::new(r.x, y, w, row_h * visible as i32),
            row_h,
            items: items.iter().map(|s| String::from(*s)).collect(),
            hover: sel,
            scroll: 0,
            visible,
            radius: radius.min(self.ui.s(10)),
        };
        p.keep_visible();
        self.ui.popup = Some(p);
        self.ui.tw.set((id as u64) << 8 | OPEN as u64, 0);
        self.ui.request_redraw();
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
        let changed = self.take_pick(id, sel);
        let resp = self.ui.interact(id, r);
        let (h, p) = self.states(id, &resp);
        let t = self.ui.theme;
        let radius = self.ui.s(10);
        let bg = lerp(lerp(t.field, t.field_hover, h), t.field_pressed, p);
        self.c.fill_rrect(r, radius, self.tone(bg, &resp));
        self.c.stroke_rrect_fx(
            r,
            radius,
            self.ui.sfx(256).max(FX),
            self.tone(t.field_border, &resp),
        );
        let fg = self.tone(t.text, &resp);
        self.label_center(r, Weight::Medium, 11, label, fg);
        self.focus_ring(id, r, radius, resp.focused);
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
        if changed {
            self.ui.last_hit = Some(id);
            self.ui.request_redraw();
        }
        changed
    }

    /// Any focusable area drawn by the program itself (a card, a swatch).
    /// The arrows Left/Right report as -1/+1 when it has the focus. Also
    /// gives the animated hover and press amounts (0..=255).
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
        if step != 0 {
            self.ui.last_hit = Some(id);
        }
        self.focus_ring(id, r, radius, resp.focused);
        (resp, step)
    }

    /// Animated hover and press amounts (0..=255) of a widget drawn with
    /// [`Ctx::selectable`].
    pub fn hover_press(&mut self, id: Id, resp: &Response) -> (u8, u8) {
        self.states(id, resp)
    }

    /// A list in the concept's style: the chosen item on top (highlighted,
    /// with a chevron), the other items under it, scrolling with the
    /// wheel and the arrows. Click a row to choose it. Returns true if the
    /// choice changed. Rows are reported in [`Ui::areas`] as `id * 1000 +
    /// item + 1`.
    pub fn select_list(&mut self, id: Id, r: Rect, items: &[&str], sel: &mut usize) -> bool {
        let t = self.ui.theme;
        let before = *sel;
        let radius = self.ui.s(8);
        let pad = self.ui.s(4);
        let row_h = self.ui.s(26);
        let head = Rect::new(
            r.x + pad,
            r.y + pad,
            r.w - 2 * pad - self.ui.s(10),
            self.ui.s(30),
        );
        let area = Rect::new(
            r.x,
            head.bottom() + pad,
            r.w,
            r.bottom() - head.bottom() - 2 * pad,
        );
        let others: Vec<usize> = (0..items.len()).filter(|&i| i != *sel).collect();
        let visible = (area.h / row_h.max(1)).max(1) as usize;
        let max_scroll = others.len().saturating_sub(visible) as i32;
        let mut scroll = *self.ui.scroll.get(&id).unwrap_or(&0);
        if self.ui.hover(r) && self.ui.wheel != 0 {
            scroll += self.ui.wheel;
            self.ui.wheel = 0;
            self.ui.last_hit = Some(id);
        }
        scroll = scroll.clamp(0, max_scroll);
        self.ui.scroll.insert(id, scroll);
        // Clicks on the rows, before the list as a whole takes the click.
        let mut clicked_row = None;
        for (row, &i) in others.iter().enumerate().skip(scroll as usize) {
            let y = area.y + (row as i32 - scroll) * row_h;
            if y + row_h > area.bottom() + row_h / 2 {
                break;
            }
            let rr = Rect::new(head.x, y, head.w, row_h).intersect(area);
            if self.ui.is_enabled() {
                self.ui.area(id * 1000 + i as Id + 1, rr);
            }
            if self.ui.is_enabled() && self.ui.take_click(rr) {
                clicked_row = Some(i);
            }
        }
        let resp = self.ui.interact(id, r);
        if let Some(i) = clicked_row {
            *sel = i;
            self.ui.set_focus(Some(id));
            self.ui.last_hit = Some(id * 1000 + i as Id + 1);
        }
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
        self.c.fill_rrect(r, radius, self.tone(t.field, &resp));
        self.c.stroke_rrect_fx(
            r,
            radius,
            self.ui.sfx(256).max(FX),
            self.tone(t.field_border, &resp),
        );
        // Header: the choice, with an accent mark at its left.
        self.c
            .fill_rrect(head, self.ui.s(6), self.tone(t.tint(t.selected, 34), &resp));
        let bar = Rect::new(
            head.x + self.ui.s(4),
            head.y + self.ui.s(8),
            self.ui.s(3).max(2),
            head.h - self.ui.s(16),
        );
        self.c
            .fill_rrect(bar, bar.w / 2, self.tone(t.accent, &resp));
        let tx = head.x + self.ui.s(13);
        let fg = self.tone(t.text, &resp);
        self.label_mid(
            Rect::new(tx, head.y, head.w, head.h),
            Weight::SemiBold,
            11,
            items.get(*sel).copied().unwrap_or(""),
            fg,
        );
        let csz = self.ui.s(13);
        let cc = self.tone(t.text_dim, &resp);
        self.icon(
            Icon::ChevronDown,
            head.right() - self.ui.s(10) - csz,
            head.center_y() - csz / 2,
            csz,
            cc,
        );
        // The other items, in order, below the header (as they are now).
        let others: Vec<usize> = (0..items.len()).filter(|&i| i != *sel).collect();
        let max_scroll = others.len().saturating_sub(visible) as i32;
        let scroll = scroll.clamp(0, max_scroll);
        let old_clip = self.c.set_clip(area);
        let dim = self.tone(t.text_dim, &resp);
        for (row, &i) in others.iter().enumerate().skip(scroll as usize) {
            let y = area.y + (row as i32 - scroll) * row_h;
            if y >= area.bottom() {
                break;
            }
            let rr = Rect::new(head.x, y, head.w, row_h);
            let hk = id.wrapping_mul(64) ^ (i as Id + 1) << 20;
            let h = self.ui.anim(
                hk,
                HOVER,
                if self.ui.hover(rr) { 255 } else { 0 },
                anim::FAST,
            );
            if h > 0 {
                self.c
                    .fill_rrect(rr, self.ui.s(6), fade(t.field_hover, h as u8));
            }
            self.label_mid(
                Rect::new(tx, y, rr.w, row_h),
                Weight::Regular,
                11,
                items[i],
                dim,
            );
        }
        self.c.set_clip(old_clip);
        // Scroll bar.
        if max_scroll > 0 {
            let track = Rect::new(
                r.right() - pad - self.ui.s(4),
                r.y + pad * 2,
                self.ui.s(3).max(2),
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
        self.focus_ring(id, r, radius, resp.focused);
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
        if changed {
            self.ui.last_hit = Some(id);
        }
        let (h, _) = self.states(id, &resp);
        let f = self
            .ui
            .anim(id, VALUE, if resp.focused { 255 } else { 0 }, anim::FAST);
        let t = self.ui.theme;
        let radius = self.ui.s(8);
        let bg = lerp(t.field, t.field_hover, h);
        self.c.fill_rrect(r, radius, self.tone(bg, &resp));
        let border = lerp(t.field_border, t.accent, f as u8);
        let bw = self.ui.sfx(256 + 128 * f as i64 / 255).max(FX);
        self.c
            .stroke_rrect_fx(r, radius, bw, self.tone(border, &resp));
        let x = r.x + self.ui.s(12);
        let inner = Rect::new(x, r.y, r.w - self.ui.s(24), r.h);
        let old = self.c.set_clip(inner);
        let shown: String = if secret {
            text.chars().map(|_| '\u{2022}').collect()
        } else {
            text.clone()
        };
        let w = if shown.is_empty() {
            let pc = self.tone(t.text_faint, &resp);
            self.label_mid(inner, Weight::Regular, 11, placeholder, pc);
            0
        } else {
            let tc = self.tone(t.text, &resp);
            self.label_mid(inner, Weight::Regular, 11, &shown, tc)
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
                t.detail,
            );
        }
        self.c.set_clip(old);
        changed
    }

    /// A value that eases to `target` (for progress shown smoothly).
    pub fn smooth(&mut self, id: Id, target: i32, dur: u32) -> i32 {
        self.ui.anim(id, VALUE, target, dur)
    }

    /// A horizontal progress bar; `permille` from 0 to 1000.
    pub fn progress_bar(&mut self, r: Rect, permille: u32) {
        let t = self.ui.theme;
        self.c.fill_rrect(r, r.h / 2, t.progress_track);
        let w = (r.w as i64 * permille.min(1000) as i64 / 1000) as i32;
        if w > 0 {
            self.c
                .fill_rrect(Rect::new(r.x, r.y, w.max(r.h), r.h), r.h / 2, t.accent);
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
            self.c.arc_fx(fx, fy, r, w, 0, len, true, t.accent);
            // A bright dot marks the moving end.
            let (sx, sy) = crate::canvas::point_on_circle(fx, fy, r, len);
            self.c
                .fill_circle_fx(sx, sy, w * 3 / 10, lerp(t.accent, 0xffffffff, 150));
        }
    }

    /// Draws the open drop-down list (if any) over everything else and
    /// ends the frame. Returns true if the frame should be drawn again.
    pub fn end(self) -> bool {
        if let Some(p) = self.ui.popup.as_ref() {
            let (id, r, radius, row_h) = (p.id, p.rect, p.radius, p.row_h);
            let open = self
                .ui
                .tw
                .value((id as u64) << 8 | OPEN as u64, 1000, anim::FAST);
            let p = self.ui.popup.as_ref().unwrap();
            let t = self.ui.theme;
            let pad = self.ui.s(4);
            let frame = Rect::new(r.x, r.y - pad, r.w, r.h + 2 * pad);
            // Opening: the list unrolls from its top and fades in.
            let shown_h = frame.h * (600 + 400 * open) / 1000;
            let alpha = (open * 255 / 1000) as u8;
            let reveal = Rect::new(
                frame.x - self.ui.s(16),
                frame.y,
                frame.w + self.ui.s(32),
                shown_h + self.ui.s(24),
            );
            let old = self.c.set_clip(reveal);
            self.c.shadow(
                frame.offset(0, self.ui.s(4)),
                radius,
                self.ui.s(10),
                fade(t.shadow, alpha),
            );
            self.c.fill_rrect(frame, radius, fade(t.field, alpha));
            self.c.stroke_rrect_fx(
                frame,
                radius,
                self.ui.sfx(256).max(FX),
                fade(t.field_border, alpha),
            );
            let inner = Rect::new(r.x, r.y, r.w, r.h).intersect(reveal);
            self.c.set_clip(inner);
            let px = self.ui.px(11);
            let scrolls = p.items.len() > p.visible;
            let text_w = r.w - if scrolls { self.ui.s(12) } else { 0 };
            for (row, item) in p.items.iter().enumerate().skip(p.scroll).take(p.visible) {
                let y = r.y + row_h * (row - p.scroll) as i32;
                let rr = Rect::new(r.x + self.ui.s(4), y, text_w - self.ui.s(8), row_h);
                self.ui.areas.push((POPUP_ROW + row as Id, rr));
                if row == p.hover {
                    self.c.fill_rrect(
                        rr.inset(self.ui.s(2)),
                        self.ui.s(6),
                        fade(t.tint(t.selected, 40), alpha),
                    );
                }
                let mut tx = rr.x + self.ui.s(10);
                if self.ui.picked.is_none() && row == p.hover {
                    tx += self.ui.s(2);
                }
                self.f.draw_centered_y(
                    self.c,
                    Weight::Regular,
                    px,
                    tx,
                    rr.center_y(),
                    item,
                    fade(t.text, alpha),
                );
            }
            if scrolls {
                let track = Rect::new(
                    r.right() - self.ui.s(8),
                    r.y + self.ui.s(2),
                    self.ui.s(3).max(2),
                    r.h - self.ui.s(4),
                );
                let n = p.items.len() as i32;
                let th = (track.h * p.visible as i32 / n).max(self.ui.s(14));
                let ty = track.y + (track.h - th) * p.scroll as i32 / p.max_scroll().max(1) as i32;
                self.c.fill_rrect(
                    Rect::new(track.x, ty, track.w, th),
                    track.w / 2,
                    fade(t.scrollbar, alpha),
                );
            }
            self.c.set_clip(old);
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
        assert_eq!(ui.last_hit, Some(1));
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
        let held = run(&[mv(15, 15), down]);
        assert!(held.hovered && held.pressed);
        assert!(run(&[up]).activated);
        assert!(!ui.focus_visible);
        assert_eq!(ui.focused(), Some(7));
    }

    #[test]
    fn disabled_widgets_ignore_input() {
        let mut ui = Ui::new(256);
        let r = Rect::new(0, 0, 20, 20);
        let mv = Event::MouseMove { x: 5, y: 5 };
        let down = Event::MouseButton { down: true };
        let up = Event::MouseButton { down: false };
        ui.begin(&[mv, down, up]);
        ui.set_enabled(false);
        let resp = ui.interact(1, r);
        ui.end_frame();
        assert!(resp.disabled && !resp.activated && !resp.hovered);
        assert!(ui.areas.is_empty());
        // Tab skips it.
        ui.begin(&[Event::Key {
            key: Key::Tab,
            shift: false,
        }]);
        ui.set_enabled(false);
        ui.interact(1, r);
        ui.set_enabled(true);
        ui.interact(2, Rect::new(30, 0, 10, 10));
        ui.end_frame();
        ui.begin(&[Event::Key {
            key: Key::Tab,
            shift: false,
        }]);
        assert_eq!(ui.focused(), Some(2));
    }

    #[test]
    fn scale_converts_design_units() {
        let ui = Ui::new(384); // 1.5 pixels per unit
        assert_eq!(ui.s(10), 15);
        assert_eq!(ui.s(-10), -15);
        assert_eq!(ui.px(12), 18);
        assert_eq!(ui.sf(256 * 3), 5); // 4.5 rounds up
    }

    fn fonts() -> Fonts {
        crate::text::tests::fonts()
    }

    const ITEMS: [&str; 12] = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];

    /// Draws one frame with a select list and a drop-down list.
    fn frame(ui: &mut Ui, f: &mut Fonts, ev: &[Event], list: &mut usize, dd: &mut usize) {
        let mut c = Canvas::new(400, 500);
        ui.begin(ev);
        let mut x = Ctx { ui, c: &mut c, f };
        x.select_list(5, Rect::new(10, 10, 150, 150), &ITEMS, list);
        x.dropdown(6, Rect::new(200, 10, 150, 30), 15, &ITEMS, dd, None, 11);
        x.end();
    }

    fn click(x: i32, y: i32) -> [Event; 3] {
        [
            Event::MouseMove { x, y },
            Event::MouseButton { down: true },
            Event::MouseButton { down: false },
        ]
    }

    #[test]
    fn list_rows_and_popup_rows_take_clicks() {
        let mut ui = Ui::new(256);
        let mut f = fonts();
        let (mut list, mut dd) = (0, 0);
        frame(&mut ui, &mut f, &[], &mut list, &mut dd);
        // The rows are reported; clicking the second row chooses item 2.
        let row = ui
            .areas
            .iter()
            .find(|(id, _)| *id == 5 * 1000 + 3)
            .map(|a| a.1)
            .expect("row of item 2");
        frame(
            &mut ui,
            &mut f,
            &click(row.center_x(), row.center_y()),
            &mut list,
            &mut dd,
        );
        assert_eq!(list, 2);
        assert_eq!(ui.last_hit, Some(5003));
        // The wheel scrolls the list.
        frame(
            &mut ui,
            &mut f,
            &[Event::MouseMove { x: 50, y: 100 }, Event::Wheel(2)],
            &mut list,
            &mut dd,
        );
        assert_eq!(ui.scroll_of(5), 2);
        // The drop-down opens; it shows 8 of 12 rows and scrolls.
        frame(&mut ui, &mut f, &click(250, 20), &mut list, &mut dd);
        assert_eq!(ui.open_popup_id(), Some(6));
        frame(
            &mut ui,
            &mut f,
            &[Event::MouseMove { x: 250, y: 80 }, Event::Wheel(10)],
            &mut list,
            &mut dd,
        );
        assert_eq!(ui.scroll_of(6), 4);
        let last = ui
            .areas
            .iter()
            .find(|(id, _)| *id == POPUP_ROW + 11)
            .map(|a| a.1)
            .expect("last row shown");
        frame(
            &mut ui,
            &mut f,
            &click(last.center_x(), last.center_y()),
            &mut list,
            &mut dd,
        );
        frame(&mut ui, &mut f, &[], &mut list, &mut dd);
        assert_eq!(dd, 11);
        assert!(!ui.popup_open());
        // Keyboard: End in the open list scrolls to the last row.
        frame(&mut ui, &mut f, &click(250, 20), &mut list, &mut dd);
        let key = |k| Event::Key {
            key: k,
            shift: false,
        };
        frame(&mut ui, &mut f, &[key(Key::Home)], &mut list, &mut dd);
        assert_eq!(ui.scroll_of(6), 0);
        frame(&mut ui, &mut f, &[key(Key::End)], &mut list, &mut dd);
        assert_eq!(ui.scroll_of(6), 4);
        frame(
            &mut ui,
            &mut f,
            &[key(Key::Up), key(Key::Enter)],
            &mut list,
            &mut dd,
        );
        frame(&mut ui, &mut f, &[], &mut list, &mut dd);
        assert_eq!(dd, 10);
    }

    #[test]
    fn switches_animate_and_use_the_accent() {
        let mut ui = Ui::new(256);
        let mut f = fonts();
        let mut on = false;
        let r = Rect::new(10, 10, 40, 20);
        let mut draw = |ui: &mut Ui, now: u64, ev: &[Event], on: &mut bool| {
            let mut c = Canvas::new(80, 40);
            ui.set_time(now);
            ui.begin(ev);
            let mut x = Ctx {
                ui,
                c: &mut c,
                f: &mut f,
            };
            x.toggle(1, r, on);
            x.end();
            c
        };
        draw(&mut ui, 0, &[], &mut on);
        draw(&mut ui, 10, &click(20, 20), &mut on);
        assert!(on);
        let mid = draw(&mut ui, 100, &[], &mut on);
        assert!(ui.animating());
        let end = draw(&mut ui, 400, &[], &mut on);
        assert!(!ui.animating());
        // At the end the track left of the knob is the accent colour.
        assert_eq!(end.get(14, 20), crate::theme::AMBER & 0xffffff);
        assert_ne!(mid.get(14, 20), end.get(14, 20));
    }

    #[test]
    fn plan_redraws_only_what_changes() {
        let mut f = fonts();
        let mut ui = Ui::new(256);
        ui.set_animations(true);
        let mut c = Canvas::new(400, 200);
        let draw = |ui: &mut Ui, c: &mut Canvas, f: &mut Fonts, ev: &[Event], now: u64| {
            ui.set_time(now);
            ui.begin(ev);
            let mut x = Ctx { ui, c, f };
            x.button(
                1,
                Rect::new(10, 10, 80, 30),
                "One",
                ButtonKind::Secondary,
                None,
                None,
            );
            x.button(
                2,
                Rect::new(200, 10, 80, 30),
                "Two",
                ButtonKind::Secondary,
                None,
                None,
            );
            x.end();
        };
        draw(&mut ui, &mut c, &mut f, &[], 0);
        // The pointer moves over empty space: nothing to draw.
        let away = [Event::MouseMove { x: 150, y: 150 }];
        assert_eq!(ui.plan(&away, 10), Redraw::Nothing);
        ui.track_pointer(&away);
        // It enters the first button: that button and a margin.
        let over = [Event::MouseMove { x: 20, y: 20 }];
        let Redraw::Area(r) = ui.plan(&over, 20) else {
            panic!("expected an area");
        };
        assert!(r.contains(10, 10) && r.contains(89, 39));
        assert!(!r.contains(200, 10));
        draw(&mut ui, &mut c, &mut f, &over, 20);
        // While its hover fades in, frames redraw just that button.
        assert!(ui.animating());
        let Redraw::Area(r) = ui.plan(&[], 60) else {
            panic!("expected an area");
        };
        assert!(r.contains(50, 25) && !r.contains(240, 25));
        // Once the fade is drawn to its end, nothing is left.
        draw(&mut ui, &mut c, &mut f, &[], 400);
        assert_eq!(ui.plan(&[], 410), Redraw::Nothing);
        // Keys and clicks redraw everything.
        assert_eq!(
            ui.plan(&[Event::MouseButton { down: true }], 420),
            Redraw::All
        );
        let tab = [Event::Key {
            key: Key::Tab,
            shift: false,
        }];
        assert_eq!(ui.plan(&tab, 420), Redraw::All);
    }
}

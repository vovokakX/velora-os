//! The terminal: a program (vsh, or Plume for its own window) in a
//! pseudo-terminal, drawn with JetBrains Mono. Only the rows that change
//! are drawn again. The mouse selects text; Ctrl+Shift+C or Ctrl+Insert
//! copies it, Ctrl+Shift+V or Shift+Insert pastes, the right button
//! copies a selection or pastes. The wheel and Shift+PageUp/PageDown
//! scroll back through the last 2000 lines.

use super::{Action, App, AppId, Cx};
use crate::host::{Host, Term, TermRead};
use crate::vt::{self, Cell, Terminal, attr};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use velora_ui::canvas::{Color, lerp};
use velora_ui::text::{Fonts, Weight};
use velora_ui::theme::Theme;
use velora_ui::ui::{Key, mods};
use velora_ui::{Event, Rect, Ui};

/// Cursor blink half-period.
const BLINK_MS: u64 = 530;

pub struct TerminalApp {
    app: AppId,
    argv: Vec<String>,
    pub term: Terminal,
    proc: Option<Box<dyn Term>>,
    ended: bool,
    error: Option<String>,
    /// Font size, cell size and baseline in pixels.
    px: u32,
    cw: i32,
    ch: i32,
    base: i32,
    /// The text area inside the content (after the padding).
    view: Rect,
    /// Lines scrolled back from the bottom.
    scroll: usize,
    /// Selection in history coordinates (row, column), anchor and end.
    sel: Option<((usize, usize), (usize, usize))>,
    selecting: bool,
    press: Option<(usize, usize)>,
    damage: Option<Rect>,
    blink_on: bool,
    blink_next: u64,
    was_focused: bool,
    mouse: (i32, i32),
}

impl TerminalApp {
    pub fn new(app: AppId, argv: Vec<String>) -> TerminalApp {
        TerminalApp {
            app,
            argv,
            term: Terminal::new(80, 24),
            proc: None,
            ended: false,
            error: None,
            px: 14,
            cw: 8,
            ch: 18,
            base: 14,
            view: Rect::default(),
            scroll: 0,
            sel: None,
            selecting: false,
            press: None,
            damage: None,
            blink_on: true,
            blink_next: 0,
            was_focused: false,
            mouse: (0, 0),
        }
    }

    fn add_damage(&mut self, r: Rect) {
        let r = r.intersect(self.view.inset(-self.cw));
        if r.is_empty() {
            return;
        }
        self.damage = Some(match self.damage {
            Some(d) => d.union(r),
            None => r,
        });
    }

    fn damage_all(&mut self) {
        self.add_damage(self.view.inset(-self.cw));
    }

    fn row_rect(&self, row: usize) -> Rect {
        Rect::new(
            self.view.x - self.cw,
            self.view.y + row as i32 * self.ch,
            self.view.w + 2 * self.cw,
            self.ch,
        )
    }

    fn send(&mut self, data: &[u8]) {
        if let Some(p) = self.proc.as_mut() {
            p.write(data);
        }
        if self.scroll != 0 {
            self.scroll = 0;
            self.damage_all();
        }
        if self.sel.take().is_some() {
            self.damage_all();
        }
        self.blink_on = true;
    }

    /// The history row and column under a point (clamped to the view).
    fn cell_at(&self, x: i32, y: i32) -> (usize, usize) {
        let col =
            ((x - self.view.x).max(0) / self.cw.max(1)).min(self.term.cols as i32 - 1) as usize;
        let row =
            ((y - self.view.y).max(0) / self.ch.max(1)).min(self.term.rows as i32 - 1) as usize;
        (self.first_row() + row, col)
    }

    /// History index of the top row of the view.
    fn first_row(&self) -> usize {
        self.term.scrollback.len().saturating_sub(self.scroll)
    }

    fn selected(&self, hr: usize, col: usize) -> bool {
        let Some((a, b)) = self.sel else {
            return false;
        };
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        (hr, col) >= s && (hr, col) <= e
    }

    fn copy(&mut self, out: &mut Vec<Action>) {
        if let Some((a, b)) = self.sel {
            let text = self.term.text_between(a, b);
            if !text.is_empty() {
                out.push(Action::Copy(text));
            }
        }
    }

    fn paste(&mut self, text: &str) {
        if !text.is_empty() {
            let data = String::from(text);
            self.send(data.as_bytes());
        }
    }

    fn key_bytes(key: Key, shift: bool) -> Option<Vec<u8>> {
        let s: &[u8] = match key {
            Key::Char(c) => {
                let mut b = [0u8; 4];
                return Some(Vec::from(c.encode_utf8(&mut b).as_bytes()));
            }
            Key::Space => b" ",
            Key::Enter => b"\n",
            Key::Backspace => b"\x7f",
            Key::Tab if shift => b"\x1b[Z",
            Key::Tab => b"\t",
            Key::Escape => b"\x1b",
            Key::Up => b"\x1b[A",
            Key::Down => b"\x1b[B",
            Key::Right => b"\x1b[C",
            Key::Left => b"\x1b[D",
            Key::Home => b"\x1b[H",
            Key::End => b"\x1b[F",
            Key::Delete => b"\x1b[3~",
            Key::Insert => b"\x1b[2~",
            Key::PageUp => b"\x1b[5~",
            Key::PageDown => b"\x1b[6~",
            Key::F(1) => b"\x1bOP",
            Key::F(2) => b"\x1bOQ",
            Key::F(3) => b"\x1bOR",
            Key::F(4) => b"\x1bOS",
            Key::F(5) => b"\x1b[15~",
            Key::F(6) => b"\x1b[17~",
            Key::F(7) => b"\x1b[18~",
            Key::F(8) => b"\x1b[19~",
            Key::F(9) => b"\x1b[20~",
            Key::F(10) => b"\x1b[21~",
            Key::F(11) => b"\x1b[23~",
            Key::F(_) => return None,
        };
        Some(Vec::from(s))
    }

    fn scroll_by(&mut self, lines: i32) {
        let max = self.term.scrollback.len() as i32;
        let s = (self.scroll as i32 + lines).clamp(0, max) as usize;
        if s != self.scroll {
            self.scroll = s;
            self.damage_all();
        }
    }

    fn colors(t: &Theme) -> ([Color; 16], Color, Color) {
        if t.dark {
            // The window colour, darker: graphite, or violet for Gamers.
            let bg = lerp(t.window, 0xff08080a, 120);
            (
                [
                    0xff2a2a2e, 0xffe0605c, 0xff7cc47f, 0xffe6c068, 0xff6c9ef0, 0xffc68ae8,
                    0xff5fc8c8, 0xffd8d8dc, 0xff5c5c64, 0xffff8a85, 0xff9fe0a0, 0xfff5d58a,
                    0xff94b8ff, 0xffdcaaf5, 0xff86e0e0, 0xffffffff,
                ],
                0xffe4e4e8,
                bg,
            )
        } else {
            (
                [
                    0xff2b2b30, 0xffc0392b, 0xff2e8b45, 0xffa0700a, 0xff2f62c8, 0xff8a3fb0,
                    0xff1b8a8a, 0xff6a6a72, 0xff55555c, 0xffe0503e, 0xff3aa65a, 0xffc08a10,
                    0xff4a7fe0, 0xffa85ad0, 0xff27a5a5, 0xff202024,
                ],
                0xff26262b,
                0xfffbfbfc,
            )
        }
    }

    fn ink(ink: vt::Ink, pal: &[Color; 16], default: Color) -> Color {
        match ink & 0xff00_0000 {
            vt::PALETTE => {
                let n = ink & 0xff;
                if n < 16 {
                    pal[n as usize]
                } else {
                    0xff00_0000 | vt::xterm_color(n)
                }
            }
            vt::RGB => 0xff00_0000 | (ink & 0x00ff_ffff),
            _ => default,
        }
    }

    fn draw_row(&self, cx: &mut Cx, row: usize, pal: &[Color; 16], fg0: Color, bg0: Color) {
        let hr = self.first_row() + row;
        let rr = self.row_rect(row);
        cx.c.fill_rect(rr, bg0);
        let Some(line) = self.term.history_line(hr) else {
            return;
        };
        let y = self.view.y + row as i32 * self.ch;
        let cursor_here = self.scroll == 0 && row == self.term.row && self.term.cursor_visible;
        let sel_bg = lerp(bg0, cx.theme.accent, if cx.theme.dark { 110 } else { 80 });
        for (col, cell) in line.iter().enumerate() {
            let x = self.view.x + col as i32 * self.cw;
            let Cell {
                ch,
                fg,
                bg,
                attr: a,
            } = *cell;
            let mut f = Self::ink(fg, pal, fg0);
            let mut b = Self::ink(bg, pal, bg0);
            if a & attr::BOLD != 0 && fg & 0xff00_0000 == vt::PALETTE && (fg & 0xff) < 8 {
                f = pal[(fg & 0xff) as usize + 8];
            }
            if a & attr::REVERSE != 0 {
                core::mem::swap(&mut f, &mut b);
            }
            if a & attr::DIM != 0 {
                f = lerp(f, b, 100);
            }
            if self.selected(hr, col) {
                b = sel_bg;
            }
            let is_cursor = cursor_here && col == self.term.col;
            if is_cursor && cx.focused && self.blink_on {
                b = cx.theme.detail;
                f = bg0;
            }
            if b != bg0 {
                cx.c.fill_rect(Rect::new(x, y, self.cw, self.ch), b);
            }
            if is_cursor && !cx.focused {
                cx.c.stroke_rrect(Rect::new(x, y, self.cw, self.ch), 0, 1, cx.theme.detail);
            }
            if ch != ' ' {
                cx.f.draw_char(cx.c, Weight::Mono, self.px, x, y + self.base, ch, f);
            }
            if a & attr::UNDERLINE != 0 {
                cx.c.fill_rect(Rect::new(x, y + self.base + 2, self.cw, 1), f);
            }
        }
    }
}

impl App for TerminalApp {
    fn id(&self) -> AppId {
        self.app
    }

    fn title(&self, ru: bool) -> String {
        if let Some(t) = self.term.title.as_ref() {
            return t.clone();
        }
        match self.app {
            AppId::Plume => match self.argv.get(1) {
                Some(f) => alloc::format!("Plume — {}", f.rsplit('/').next().unwrap_or(f)),
                None => String::from("Plume"),
            },
            a => String::from(a.name(ru)),
        }
    }

    fn size(&self) -> ((i32, i32), (i32, i32)) {
        ((760, 460), (320, 180))
    }

    fn uses_ui(&self) -> bool {
        false
    }

    fn resized(&mut self, host: &mut dyn Host, f: &mut Fonts, ui: &Ui, r: Rect) {
        self.px = ui.px(13).max(8);
        self.cw = (f.advance(Weight::Mono, self.px, 'M') + 0.5) as i32;
        self.ch = f.line_height(Weight::Mono, self.px).max(self.px as i32 + 2);
        self.base =
            f.ascent(Weight::Mono, self.px) + (self.ch - f.line_height(Weight::Mono, self.px)) / 2;
        let pad = ui.s(10);
        let cols = ((r.w - 2 * pad) / self.cw.max(1)).max(2);
        let rows = ((r.h - 2 * pad) / self.ch.max(1)).max(2);
        self.view = Rect::new(r.x + pad, r.y + pad, cols * self.cw, rows * self.ch);
        self.term.resize(cols as usize, rows as usize);
        self.scroll = self.scroll.min(self.term.scrollback.len());
        match self.proc.as_mut() {
            Some(p) => p.resize(cols as u32, rows as u32),
            None if !self.ended && self.error.is_none() => {
                let argv: Vec<&str> = self.argv.iter().map(String::as_str).collect();
                match host.spawn_term(&argv, cols as u32, rows as u32) {
                    Ok(p) => self.proc = Some(p),
                    Err(e) => {
                        let msg = alloc::format!("lumi: cannot start {}: {}\n", self.argv[0], e);
                        self.term.feed(msg.as_bytes());
                        self.error = Some(e);
                    }
                }
            }
            None => {}
        }
        self.damage = None;
    }

    fn poll(&mut self, _host: &mut dyn Host, now: u64) -> bool {
        let mut changed = false;
        if let Some(p) = self.proc.as_mut() {
            let mut buf = [0u8; 4096];
            // Read what is there, but not forever: drawing must go on.
            for _ in 0..64 {
                match p.read(&mut buf) {
                    TermRead::Data(n) => {
                        self.term.feed(&buf[..n]);
                        changed = true;
                    }
                    TermRead::Empty => break,
                    TermRead::Closed => {
                        self.ended = true;
                        break;
                    }
                }
            }
            if !self.term.replies.is_empty() {
                let r = core::mem::take(&mut self.term.replies);
                p.write(&r);
            }
        }
        if self.ended {
            self.proc = None;
        }
        if changed {
            let scrolled = self.term.scrolled;
            if self.scroll > 0 {
                self.scroll = (self.scroll + scrolled).min(self.term.scrollback.len());
            }
            let dirty = self.term.take_dirty();
            if self.scroll > 0 && scrolled > 0 {
                self.damage_all();
            } else {
                for (row, d) in dirty.iter().enumerate() {
                    if *d {
                        let r = self.row_rect(row);
                        self.add_damage(r);
                    }
                }
            }
            self.blink_on = true;
            self.blink_next = now + BLINK_MS;
        }
        if now >= self.blink_next && self.proc.is_some() {
            self.blink_on = !self.blink_on;
            self.blink_next = now + BLINK_MS;
            if self.scroll == 0 {
                let r = self.row_rect(self.term.row);
                self.add_damage(r);
            }
        }
        self.damage.is_some()
    }

    fn wake_at(&self, _now: u64) -> Option<u64> {
        self.proc.as_ref().map(|_| self.blink_next)
    }

    fn alive(&self) -> bool {
        !self.ended
    }

    fn close(&mut self, _host: &mut dyn Host) {
        if let Some(mut p) = self.proc.take() {
            p.kill();
        }
    }

    fn input(&mut self, events: &[Event], cx: &mut Cx, _r: Rect) {
        if cx.focused != self.was_focused {
            self.was_focused = cx.focused;
            let r = self.row_rect(self.term.row);
            self.add_damage(r);
        }
        for ev in events {
            match *ev {
                Event::Key { key, shift } => {
                    if shift && matches!(key, Key::PageUp | Key::PageDown) {
                        let page = self.term.rows as i32 - 1;
                        self.scroll_by(if key == Key::PageUp { page } else { -page });
                        continue;
                    }
                    if shift && key == Key::Insert {
                        let clip = String::from(cx.clipboard);
                        self.paste(&clip);
                        continue;
                    }
                    if let Some(b) = Self::key_bytes(key, shift) {
                        self.send(&b);
                    }
                }
                Event::Chord { key, mods: m } => {
                    let ctrl = m & mods::CTRL != 0;
                    let shift = m & mods::SHIFT != 0;
                    let alt = m & mods::ALT != 0;
                    if m & mods::SUPER != 0 {
                        continue;
                    }
                    match key {
                        Key::Char('c') if ctrl && shift => self.copy(cx.out),
                        Key::Char('v') if ctrl && shift => {
                            let clip = String::from(cx.clipboard);
                            self.paste(&clip);
                        }
                        Key::Insert if ctrl => self.copy(cx.out),
                        Key::Char(c) if ctrl && c.is_ascii_lowercase() => {
                            self.send(&[c as u8 - b'a' + 1]);
                        }
                        Key::Char(c) if alt => {
                            let mut b = [0u8; 5];
                            b[0] = 0x1b;
                            let n = c.encode_utf8(&mut b[1..]).len();
                            self.send(&b[..1 + n]);
                        }
                        Key::Space if ctrl => self.send(&[0]),
                        _ => {}
                    }
                }
                Event::Wheel(d) => self.scroll_by(-d * 3),
                Event::MouseMove { x, y } => {
                    self.mouse = (x, y);
                    if let Some(p) = self.press {
                        let c = self.cell_at(x, y);
                        if c != p || self.selecting {
                            self.selecting = true;
                            self.sel = Some((p, c));
                            self.damage_all();
                        }
                    }
                }
                Event::MouseButton { down: true } => {
                    let (x, y) = self.mouse;
                    if self.view.inset(-self.cw).contains(x, y) {
                        self.press = Some(self.cell_at(x, y));
                        self.selecting = false;
                        if self.sel.take().is_some() {
                            self.damage_all();
                        }
                    }
                }
                Event::MouseButton { down: false } => {
                    self.press = None;
                    self.selecting = false;
                }
                Event::RightButton { down: true } => {
                    if self.sel.is_some() {
                        self.copy(cx.out);
                        self.sel = None;
                        self.damage_all();
                    } else {
                        let clip = String::from(cx.clipboard);
                        self.paste(&clip);
                    }
                }
                _ => {}
            }
        }
    }

    fn damage(&mut self, _r: Rect) -> Option<Rect> {
        self.damage.take()
    }

    fn draw(&mut self, cx: &mut Cx, r: Rect) {
        let (pal, fg0, bg0) = Self::colors(&cx.theme);
        let clip = cx.c.clip();
        let full = clip.intersect(r) == r;
        if full {
            cx.c.fill_rect(r, bg0);
        }
        for row in 0..self.term.rows {
            if !self.row_rect(row).intersect(clip).is_empty() {
                self.draw_row(cx, row, &pal, fg0, bg0);
            }
        }
        // The scroll position while looking back.
        if self.scroll > 0 {
            let total = (self.term.scrollback.len() + self.term.rows) as i32;
            let h = (self.view.h * self.term.rows as i32 / total).max(cx.ui.s(16));
            let top = self.view.y
                + (self.view.h - h) * (self.term.scrollback.len() - self.scroll) as i32
                    / self.term.scrollback.len().max(1) as i32;
            let bar = Rect::new(r.right() - cx.ui.s(7), top, cx.ui.s(4), h);
            cx.c.fill_rrect(bar, bar.w / 2, cx.theme.scrollbar);
        }
        if self.ended && self.error.is_none() {
            // Nothing: the window closes.
        }
    }
}

//! The terminal model: a grid of character cells with a scrollback,
//! driven by the bytes a program writes (UTF-8 text and the escape
//! sequences Velora programs use, a subset of xterm's).
//!
//! Supported: printing with wrapping, CR, LF (also returns to the first
//! column, as on the Velora console), BS, TAB; CSI cursor movement
//! (`A B C D E F G H f d`), erasing (`J K X`), inserting and deleting
//! (`@ P L M`), scroll region (`r`), scrolling (`S T`), colours and
//! attributes (`m`: bold, dim, underline, reverse; 8, 16, 256 colours and
//! RGB), save and restore (`s u`, `ESC 7`, `ESC 8`), the cursor on and off
//! (`?25`), the alternate screen (`?1049`, `?47`), and the queries for
//! device attributes, the cursor position and the size (answers are
//! queued in [`Terminal::replies`]). OSC 0 and 2 set the title. Kitty
//! graphics commands (APC) are skipped: Lumi's terminal shows text only.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// A cell's colour: [`DEFAULT`], a palette index (`PALETTE | n`) or RGB
/// (`RGB | 0xRRGGBB`).
pub type Ink = u32;
pub const DEFAULT: Ink = 0;
pub const PALETTE: Ink = 0x0100_0000;
pub const RGB: Ink = 0x0200_0000;

pub mod attr {
    pub const BOLD: u8 = 1;
    pub const DIM: u8 = 2;
    pub const UNDERLINE: u8 = 4;
    pub const REVERSE: u8 = 8;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Ink,
    pub bg: Ink,
    pub attr: u8,
}

impl Cell {
    pub const BLANK: Cell = Cell {
        ch: ' ',
        fg: DEFAULT,
        bg: DEFAULT,
        attr: 0,
    };
}

/// Lines kept above the screen.
pub const SCROLLBACK: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
    /// APC, DCS and similar strings: skipped up to ST.
    Skip,
    SkipEscape,
}

#[derive(Clone, Copy, Default)]
struct Saved {
    row: usize,
    col: usize,
    fg: Ink,
    bg: Ink,
    attr: u8,
}

pub struct Terminal {
    pub cols: usize,
    pub rows: usize,
    lines: Vec<Vec<Cell>>,
    pub scrollback: VecDeque<Vec<Cell>>,
    pub row: usize,
    pub col: usize,
    /// The last column was written: the next character starts a line.
    wrap_pending: bool,
    fg: Ink,
    bg: Ink,
    attr: u8,
    saved: Saved,
    pub cursor_visible: bool,
    top: usize,
    bottom: usize,
    /// The main screen while the alternate one is shown.
    alt: Option<(Vec<Vec<Cell>>, Saved)>,
    /// Rows changed since [`Terminal::take_dirty`].
    dirty: Vec<bool>,
    /// Lines pushed into the scrollback since [`Terminal::take_dirty`].
    pub scrolled: usize,
    state: State,
    params: Vec<u32>,
    private: bool,
    osc: String,
    utf8: [u8; 4],
    utf8_len: usize,
    utf8_need: usize,
    /// Answers to queries, for the program's input.
    pub replies: Vec<u8>,
    pub title: Option<String>,
    /// Bytes fed in total (for tests and activity marks).
    pub fed: u64,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = (cols.max(2), rows.max(2));
        Terminal {
            cols,
            rows,
            lines: vec![vec![Cell::BLANK; cols]; rows],
            scrollback: VecDeque::new(),
            row: 0,
            col: 0,
            wrap_pending: false,
            fg: DEFAULT,
            bg: DEFAULT,
            attr: 0,
            saved: Saved::default(),
            cursor_visible: true,
            top: 0,
            bottom: rows - 1,
            alt: None,
            dirty: vec![true; rows],
            scrolled: 0,
            state: State::Ground,
            params: Vec::new(),
            private: false,
            osc: String::new(),
            utf8: [0; 4],
            utf8_len: 0,
            utf8_need: 0,
            replies: Vec::new(),
            title: None,
            fed: 0,
        }
    }

    /// Row `r` of the screen.
    pub fn line(&self, r: usize) -> &[Cell] {
        &self.lines[r]
    }

    /// A row counting from the oldest scrollback line (0) to the last
    /// screen row (`scrollback.len() + rows - 1`).
    pub fn history_line(&self, i: usize) -> Option<&[Cell]> {
        let sb = self.scrollback.len();
        if i < sb {
            Some(&self.scrollback[i])
        } else {
            self.lines.get(i - sb).map(|l| l.as_slice())
        }
    }

    /// The rows changed since the last call (and resets the marks).
    pub fn take_dirty(&mut self) -> Vec<bool> {
        self.scrolled = 0;
        core::mem::replace(&mut self.dirty, vec![false; self.rows])
    }

    pub fn mark_all(&mut self) {
        self.dirty.iter_mut().for_each(|d| *d = true);
    }

    pub fn alt_screen(&self) -> bool {
        self.alt.is_some()
    }

    fn touch(&mut self, r: usize) {
        if let Some(d) = self.dirty.get_mut(r) {
            *d = true;
        }
    }

    fn blank(&self) -> Cell {
        Cell {
            ch: ' ',
            fg: self.fg,
            bg: self.bg,
            attr: 0,
        }
    }

    /// Changes the size. Lines that no longer fit above the cursor go to
    /// the scrollback; lines are cut or padded on the right.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(2));
        if cols == self.cols && rows == self.rows {
            return;
        }
        for l in self.lines.iter_mut().chain(self.scrollback.iter_mut()) {
            l.resize(cols, Cell::BLANK);
        }
        while self.lines.len() > rows {
            // Drop empty lines below the cursor first, then scroll.
            let last = self.lines.len() - 1;
            if last > self.row && self.lines[last].iter().all(|c| *c == Cell::BLANK) {
                self.lines.pop();
            } else {
                let l = self.lines.remove(0);
                if self.alt.is_none() {
                    self.push_scrollback(l);
                }
                self.row = self.row.saturating_sub(1);
            }
        }
        while self.lines.len() < rows {
            self.lines.push(vec![Cell::BLANK; cols]);
        }
        if let Some((main, _)) = self.alt.as_mut() {
            main.resize(rows, vec![Cell::BLANK; cols]);
            for l in main.iter_mut() {
                l.resize(cols, Cell::BLANK);
            }
        }
        self.cols = cols;
        self.rows = rows;
        self.row = self.row.min(rows - 1);
        self.col = self.col.min(cols - 1);
        self.top = 0;
        self.bottom = rows - 1;
        self.wrap_pending = false;
        self.dirty = vec![true; rows];
    }

    fn push_scrollback(&mut self, l: Vec<Cell>) {
        if self.scrollback.len() == SCROLLBACK {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(l);
        self.scrolled += 1;
    }

    /// Scrolls the region up by one line.
    fn scroll_up(&mut self) {
        let blank = vec![self.blank(); self.cols];
        let l = self.lines.remove(self.top);
        self.lines.insert(self.bottom, blank);
        if self.top == 0 && self.alt.is_none() {
            self.push_scrollback(l);
        }
        for r in self.top..=self.bottom {
            self.touch(r);
        }
    }

    fn scroll_down(&mut self) {
        let blank = vec![self.blank(); self.cols];
        self.lines.remove(self.bottom);
        self.lines.insert(self.top, blank);
        for r in self.top..=self.bottom {
            self.touch(r);
        }
    }

    fn newline(&mut self) {
        self.wrap_pending = false;
        if self.row == self.bottom {
            self.scroll_up();
        } else if self.row + 1 < self.rows {
            self.row += 1;
        }
    }

    fn put(&mut self, ch: char) {
        if self.wrap_pending {
            self.col = 0;
            self.newline();
        }
        let cell = Cell {
            ch,
            fg: self.fg,
            bg: self.bg,
            attr: self.attr,
        };
        let (r, c) = (self.row, self.col);
        self.lines[r][c] = cell;
        self.touch(r);
        if self.col + 1 >= self.cols {
            self.wrap_pending = true;
        } else {
            self.col += 1;
        }
    }

    /// Feeds program output.
    pub fn feed(&mut self, data: &[u8]) {
        self.fed += data.len() as u64;
        for &b in data {
            self.byte(b);
        }
    }

    fn byte(&mut self, b: u8) {
        // Strings (OSC, APC) are bytes; text is UTF-8.
        match self.state {
            State::Osc => {
                match b {
                    0x07 => self.end_osc(),
                    0x1b => self.state = State::OscEscape,
                    _ if self.osc.len() < 256 => self.osc.push(b as char),
                    _ => {}
                }
                return;
            }
            State::OscEscape => {
                self.end_osc();
                if b != b'\\' {
                    self.state = State::Escape;
                    self.byte(b);
                }
                return;
            }
            State::Skip => {
                match b {
                    0x1b => self.state = State::SkipEscape,
                    0x07 => self.state = State::Ground,
                    _ => {}
                }
                return;
            }
            State::SkipEscape => {
                self.state = if b == b'\\' {
                    State::Ground
                } else {
                    State::Skip
                };
                return;
            }
            _ => {}
        }
        if self.utf8_need > 0 {
            if b & 0xc0 == 0x80 {
                self.utf8[self.utf8_len] = b;
                self.utf8_len += 1;
                if self.utf8_len == self.utf8_need {
                    let s = core::str::from_utf8(&self.utf8[..self.utf8_len]).ok();
                    self.utf8_need = 0;
                    let c = s.and_then(|s| s.chars().next()).unwrap_or('\u{fffd}');
                    self.char(c);
                }
                return;
            }
            self.utf8_need = 0;
            self.char('\u{fffd}');
        }
        let need = match b {
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            0x80..=0xff => {
                self.char('\u{fffd}');
                return;
            }
            _ => 1,
        };
        if need > 1 {
            self.utf8[0] = b;
            self.utf8_len = 1;
            self.utf8_need = need;
            return;
        }
        self.char(b as char);
    }

    fn end_osc(&mut self) {
        self.state = State::Ground;
        let osc = core::mem::take(&mut self.osc);
        if let Some(t) = osc.strip_prefix("0;").or_else(|| osc.strip_prefix("2;")) {
            self.title = Some(String::from(t));
        }
    }

    fn char(&mut self, c: char) {
        match self.state {
            State::Ground => self.ground(c),
            State::Escape => self.escape(c),
            State::Csi => self.csi(c),
            _ => {}
        }
    }

    fn ground(&mut self, c: char) {
        match c {
            '\x1b' => self.state = State::Escape,
            '\n' | '\x0b' | '\x0c' => {
                self.col = 0;
                self.newline();
            }
            '\r' => {
                self.col = 0;
                self.wrap_pending = false;
            }
            '\x08' => {
                self.col = self.col.saturating_sub(1);
                self.wrap_pending = false;
            }
            '\t' => {
                let next = ((self.col / 8 + 1) * 8).min(self.cols - 1);
                self.col = next;
            }
            c if (c as u32) < 0x20 || c == '\x7f' => {}
            c => self.put(c),
        }
    }

    fn escape(&mut self, c: char) {
        self.state = State::Ground;
        match c {
            '[' => {
                self.state = State::Csi;
                self.params.clear();
                self.params.push(0);
                self.private = false;
            }
            ']' => {
                self.state = State::Osc;
                self.osc.clear();
            }
            '_' | 'P' | '^' | 'X' => self.state = State::Skip,
            '7' => self.save(),
            '8' => self.restore(),
            'D' => self.newline(),
            'E' => {
                self.col = 0;
                self.newline();
            }
            'M' => {
                if self.row == self.top {
                    self.scroll_down();
                } else {
                    self.row = self.row.saturating_sub(1);
                }
            }
            'c' => self.reset(),
            _ => {}
        }
    }

    fn reset(&mut self) {
        let (cols, rows) = (self.cols, self.rows);
        let sb = core::mem::take(&mut self.scrollback);
        *self = Terminal::new(cols, rows);
        self.scrollback = sb;
    }

    fn save(&mut self) {
        self.saved = Saved {
            row: self.row,
            col: self.col,
            fg: self.fg,
            bg: self.bg,
            attr: self.attr,
        };
    }

    fn restore(&mut self) {
        let s = self.saved;
        self.row = s.row.min(self.rows - 1);
        self.col = s.col.min(self.cols - 1);
        self.fg = s.fg;
        self.bg = s.bg;
        self.attr = s.attr;
        self.wrap_pending = false;
    }

    fn param(&self, i: usize, default: u32) -> u32 {
        match self.params.get(i) {
            Some(&0) | None => default,
            Some(&v) => v,
        }
    }

    fn csi(&mut self, c: char) {
        match c {
            '0'..='9' => {
                if let Some(p) = self.params.last_mut() {
                    *p = p.saturating_mul(10).saturating_add(c as u32 - '0' as u32);
                }
            }
            ';' | ':' => {
                if self.params.len() < 16 {
                    self.params.push(0);
                }
            }
            '?' | '>' | '=' => self.private = true,
            ' '..='/' => {}
            '@'..='~' => {
                self.state = State::Ground;
                self.csi_final(c);
            }
            _ => self.state = State::Ground,
        }
    }

    fn csi_final(&mut self, c: char) {
        if self.private {
            self.private_mode(c);
            return;
        }
        let n = self.param(0, 1) as usize;
        let last = self.rows - 1;
        self.wrap_pending = false;
        match c {
            'A' => {
                self.row =
                    self.row
                        .saturating_sub(n)
                        .max(if self.row >= self.top { self.top } else { 0 })
            }
            'B' | 'e' => {
                let limit = if self.row <= self.bottom {
                    self.bottom
                } else {
                    last
                };
                self.row = (self.row + n).min(limit);
            }
            'C' | 'a' => self.col = (self.col + n).min(self.cols - 1),
            'D' => self.col = self.col.saturating_sub(n),
            'E' => {
                self.row = (self.row + n).min(last);
                self.col = 0;
            }
            'F' => {
                self.row = self.row.saturating_sub(n);
                self.col = 0;
            }
            'G' | '`' => self.col = (n - 1).min(self.cols - 1),
            'd' => self.row = (n - 1).min(last),
            'H' | 'f' => {
                self.row = (self.param(0, 1) as usize - 1).min(last);
                self.col = (self.param(1, 1) as usize - 1).min(self.cols - 1);
            }
            'J' => self.erase_display(self.param(0, 0)),
            'K' => self.erase_line(self.param(0, 0)),
            'X' => {
                let b = self.blank();
                let r = self.row;
                let end = (self.col + n).min(self.cols);
                self.lines[r][self.col..end].fill(b);
                self.touch(r);
            }
            '@' => {
                let b = self.blank();
                let (r, cl) = (self.row, self.col);
                let line = &mut self.lines[r];
                for _ in 0..n.min(self.cols - cl) {
                    line.insert(cl, b);
                    line.pop();
                }
                self.touch(r);
            }
            'P' => {
                let b = self.blank();
                let (r, cl) = (self.row, self.col);
                let line = &mut self.lines[r];
                for _ in 0..n.min(self.cols - cl) {
                    line.remove(cl);
                    line.push(b);
                }
                self.touch(r);
            }
            'L' | 'M' => {
                if self.row >= self.top && self.row <= self.bottom {
                    let saved_top = self.top;
                    self.top = self.row;
                    for _ in 0..n.min(self.bottom - self.row + 1) {
                        if c == 'L' {
                            self.scroll_down();
                        } else {
                            // Deleted lines never go to the scrollback.
                            let blank = vec![self.blank(); self.cols];
                            self.lines.remove(self.top);
                            self.lines.insert(self.bottom, blank);
                            for r in self.top..=self.bottom {
                                self.touch(r);
                            }
                        }
                    }
                    self.top = saved_top;
                    self.col = 0;
                }
            }
            'S' => {
                for _ in 0..n {
                    self.scroll_up();
                }
            }
            'T' => {
                for _ in 0..n {
                    self.scroll_down();
                }
            }
            'r' => {
                let top = self.param(0, 1) as usize - 1;
                let bottom = (self.param(1, self.rows as u32) as usize - 1).min(last);
                if top < bottom {
                    self.top = top;
                    self.bottom = bottom;
                    self.row = 0;
                    self.col = 0;
                }
            }
            's' => self.save(),
            'u' => self.restore(),
            'm' => self.sgr(),
            'n' => match self.param(0, 0) {
                5 => self.replies.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let s = alloc::format!("\x1b[{};{}R", self.row + 1, self.col + 1);
                    self.replies.extend_from_slice(s.as_bytes());
                }
                _ => {}
            },
            'c' if self.param(0, 0) == 0 => self.replies.extend_from_slice(b"\x1b[?62;22c"),
            't' if self.param(0, 0) == 18 => {
                let s = alloc::format!("\x1b[8;{};{}t", self.rows, self.cols);
                self.replies.extend_from_slice(s.as_bytes());
            }
            _ => {}
        }
    }

    fn private_mode(&mut self, c: char) {
        let on = match c {
            'h' => true,
            'l' => false,
            _ => return,
        };
        for i in 0..self.params.len() {
            match self.params[i] {
                25 => {
                    self.cursor_visible = on;
                    let r = self.row;
                    self.touch(r);
                }
                47 | 1047 | 1049 => self.alternate(on),
                _ => {}
            }
        }
    }

    fn alternate(&mut self, on: bool) {
        if on && self.alt.is_none() {
            self.save();
            let blank = vec![vec![Cell::BLANK; self.cols]; self.rows];
            let main = core::mem::replace(&mut self.lines, blank);
            self.alt = Some((main, self.saved));
            self.mark_all();
        } else if !on && let Some((main, saved)) = self.alt.take() {
            self.lines = main;
            self.saved = saved;
            self.restore();
            self.mark_all();
        }
    }

    fn erase_display(&mut self, mode: u32) {
        let b = self.blank();
        let (r, c) = (self.row, self.col);
        match mode {
            0 => {
                self.lines[r][c..].fill(b);
                for l in self.lines[r + 1..].iter_mut() {
                    l.fill(b);
                }
                for i in r..self.rows {
                    self.touch(i);
                }
            }
            1 => {
                self.lines[r][..=c].fill(b);
                for l in self.lines[..r].iter_mut() {
                    l.fill(b);
                }
                for i in 0..=r {
                    self.touch(i);
                }
            }
            2 | 3 => {
                for l in self.lines.iter_mut() {
                    l.fill(b);
                }
                if mode == 3 {
                    self.scrollback.clear();
                }
                self.mark_all();
            }
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: u32) {
        let b = self.blank();
        let (r, c) = (self.row, self.col);
        match mode {
            0 => self.lines[r][c..].fill(b),
            1 => self.lines[r][..=c].fill(b),
            2 => self.lines[r].fill(b),
            _ => {}
        }
        self.touch(r);
    }

    fn sgr(&mut self) {
        let p = core::mem::take(&mut self.params);
        let mut i = 0;
        while i < p.len() {
            match p[i] {
                0 => {
                    self.fg = DEFAULT;
                    self.bg = DEFAULT;
                    self.attr = 0;
                }
                1 => self.attr |= attr::BOLD,
                2 => self.attr |= attr::DIM,
                4 => self.attr |= attr::UNDERLINE,
                7 => self.attr |= attr::REVERSE,
                22 => self.attr &= !(attr::BOLD | attr::DIM),
                24 => self.attr &= !attr::UNDERLINE,
                27 => self.attr &= !attr::REVERSE,
                v @ 30..=37 => self.fg = PALETTE | (v - 30),
                39 => self.fg = DEFAULT,
                v @ 40..=47 => self.bg = PALETTE | (v - 40),
                49 => self.bg = DEFAULT,
                v @ 90..=97 => self.fg = PALETTE | (v - 90 + 8),
                v @ 100..=107 => self.bg = PALETTE | (v - 100 + 8),
                v @ (38 | 48) => {
                    let ink = match p.get(i + 1) {
                        Some(5) => {
                            i += 2;
                            p.get(i).map(|&n| PALETTE | (n & 0xff))
                        }
                        Some(2) => {
                            let c = |k: usize| p.get(i + k).copied().unwrap_or(0).min(255);
                            let rgb = RGB | (c(2) << 16) | (c(3) << 8) | c(4);
                            i += 4;
                            Some(rgb)
                        }
                        _ => None,
                    };
                    if let Some(ink) = ink {
                        if v == 38 {
                            self.fg = ink;
                        } else {
                            self.bg = ink;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
        self.params = p;
    }

    /// The text of the cells between two positions (row, col) counted
    /// in [`Terminal::history_line`] rows, inclusive; lines end with '\n'
    /// and lose their trailing spaces.
    pub fn text_between(&self, a: (usize, usize), b: (usize, usize)) -> String {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        let mut out = String::new();
        for r in start.0..=end.0 {
            let Some(line) = self.history_line(r) else {
                break;
            };
            let c0 = if r == start.0 { start.1 } else { 0 };
            let c1 = if r == end.0 {
                end.1.min(line.len().saturating_sub(1))
            } else {
                line.len().saturating_sub(1)
            };
            let mut s: String = line
                .get(c0..=c1)
                .unwrap_or(&[])
                .iter()
                .map(|c| c.ch)
                .collect();
            if r != end.0 || c1 + 1 >= line.len() {
                let t = s.trim_end().len();
                s.truncate(t);
            }
            out.push_str(&s);
            if r != end.0 {
                out.push('\n');
            }
        }
        out
    }
}

/// The xterm 256-colour palette entry `n` beyond the 16 theme colours.
pub fn xterm_color(n: u32) -> u32 {
    match n {
        16..=231 => {
            let n = n - 16;
            let v = |k: u32| if k == 0 { 0 } else { 55 + k * 40 };
            (v(n / 36) << 16) | (v((n / 6) % 6) << 8) | v(n % 6)
        }
        232..=255 => {
            let g = 8 + (n - 232) * 10;
            (g << 16) | (g << 8) | g
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &Terminal, r: usize) -> String {
        let s: String = t.line(r).iter().map(|c| c.ch).collect();
        String::from(s.trim_end())
    }

    #[test]
    fn text_colours_and_cursor() {
        let mut t = Terminal::new(10, 3);
        t.feed("ab\x1b[1;32mвг\x1b[0m\r\nline2\nz".as_bytes());
        assert_eq!(text(&t, 0), "abвг");
        assert_eq!(t.line(0)[2].fg, PALETTE | 2);
        assert_eq!(t.line(0)[2].attr, attr::BOLD);
        assert_eq!(t.line(0)[4].fg, DEFAULT);
        assert_eq!(text(&t, 1), "line2");
        assert_eq!((t.row, t.col), (2, 1), "LF also returns the column");
        // Wrapping and scrolling into the scrollback.
        t.feed(b"\n0123456789AB");
        assert_eq!(t.scrollback.len(), 2);
        assert_eq!(text(&t, 1), "0123456789");
        assert_eq!(text(&t, 2), "AB");
        // Positioning, erasing, a split UTF-8 character and a query.
        t.feed(b"\x1b[2;3H\x1b[K\xd0");
        t.feed(b"\xbf\x1b[6n");
        assert_eq!(text(&t, 1), "01п");
        assert_eq!(t.replies, b"\x1b[2;4R");
        t.feed(b"\x1b[38;2;1;2;3mX\x1b[48;5;196mY");
        assert_eq!(t.line(1)[3].fg, RGB | 0x010203);
        assert_eq!(t.line(1)[4].bg, PALETTE | 196);
    }

    #[test]
    fn plume_screen_and_alternate_screen() {
        let mut t = Terminal::new(8, 4);
        t.feed(b"shell$ ");
        // What Plume draws: hide the cursor, clear, rows, inverse status.
        t.feed(b"\x1b[?1049h\x1b[?25l\x1b[0m\x1b[2J\x1b[1;1Hab\x1b[K\x1b[4;1H\x1b[7mst\x1b[27m\x1b[1;2H\x1b[?25h");
        assert!(t.alt_screen());
        assert_eq!(text(&t, 0), "ab");
        assert_eq!(t.line(3)[0].attr, attr::REVERSE);
        assert_eq!((t.row, t.col), (0, 1));
        t.feed(b"\x1b[?1049l");
        assert_eq!(text(&t, 0), "shell$");
        assert_eq!((t.row, t.col), (0, 7));
        // Kitty graphics strings are skipped.
        t.feed(b"\x1b_Ga=q;AAAA\x1b\\ok");
        assert_eq!(text(&t, 0), "shell$ o");
        // A smaller window keeps the cursor line.
        t.feed(b"\n\n\nlast");
        t.resize(6, 2);
        assert_eq!(text(&t, t.row), "last");
        assert_eq!(t.text_between((0, 0), (0, 3)), "shel");
    }
}

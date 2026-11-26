//! Text console on top of a framebuffer, with a subset of ANSI escapes
//! (SGR colours and reverse video, cursor movement, erase, cursor
//! visibility `?25h`/`?25l`) so that the same byte stream looks the same
//! on the screen and on the serial terminal. The cursor is an underline
//! drawn by inverting the bottom of its cell after each write.

use super::font;
use super::framebuffer::Framebuffer;

pub const DEFAULT_FG: u32 = 0xc0caf5;
pub const DEFAULT_BG: u32 = 0x0b0e14;

/// The 16 ANSI colours: 0-7 normal, 8-15 bright.
pub const PALETTE: [u32; 16] = [
    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5, //
    0x414868, 0xff7a93, 0xb9f27c, 0xff9e64, 0x7da6ff, 0xc7a9ff, 0xa4daff, 0xe6ecff,
];

const MAX_PARAMS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Normal,
    Escape,
    Csi,
}

pub struct TextConsole {
    fb: Framebuffer,
    pub cols: usize,
    pub rows: usize,
    pub col: usize,
    pub row: usize,
    pub fg: u32,
    pub bg: u32,
    bold: bool,
    reverse: bool,
    fg_index: Option<usize>,
    state: State,
    params: [u16; MAX_PARAMS],
    nparams: usize,
    /// The CSI sequence started with '?' (private mode).
    private: bool,
    cursor_visible: bool,
    /// Cell where the cursor is currently drawn.
    cursor_drawn: Option<(usize, usize)>,
}

/// Height in pixels of the underline cursor.
const CURSOR_HEIGHT: usize = 2;

impl TextConsole {
    pub fn new(fb: Framebuffer) -> Self {
        let cols = fb.width / font::WIDTH;
        let rows = fb.height / font::HEIGHT;
        let mut c = Self {
            fb,
            cols,
            rows,
            col: 0,
            row: 0,
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            bold: false,
            reverse: false,
            fg_index: None,
            state: State::Normal,
            params: [0; MAX_PARAMS],
            nparams: 0,
            private: false,
            cursor_visible: true,
            cursor_drawn: None,
        };
        c.clear();
        c
    }

    pub fn framebuffer(&self) -> &Framebuffer {
        &self.fb
    }

    pub fn framebuffer_mut(&mut self) -> &mut Framebuffer {
        &mut self.fb
    }

    pub fn clear(&mut self) {
        let (w, h) = (self.fb.width, self.fb.height);
        self.fb.fill_rect(0, 0, w, h, self.bg);
        self.col = 0;
        self.row = 0;
    }

    pub fn write_str(&mut self, s: &str) {
        self.hide_cursor();
        for c in s.chars() {
            self.write_char(c);
        }
        self.show_cursor();
    }

    fn toggle_cursor_at(&mut self, (row, col): (usize, usize)) {
        self.fb.invert_rect(
            col * font::WIDTH,
            row * font::HEIGHT + font::HEIGHT - CURSOR_HEIGHT,
            font::WIDTH,
            CURSOR_HEIGHT,
        );
    }

    fn hide_cursor(&mut self) {
        if let Some(at) = self.cursor_drawn.take() {
            self.toggle_cursor_at(at);
        }
    }

    fn show_cursor(&mut self) {
        if self.cursor_visible && self.cursor_drawn.is_none() && self.rows > 0 && self.cols > 0 {
            let at = (self.row.min(self.rows - 1), self.col.min(self.cols - 1));
            self.toggle_cursor_at(at);
            self.cursor_drawn = Some(at);
        }
    }

    pub fn write_char(&mut self, c: char) {
        match self.state {
            State::Normal => self.normal_char(c),
            State::Escape => {
                if c == '[' {
                    self.state = State::Csi;
                    self.params = [0; MAX_PARAMS];
                    self.nparams = 0;
                    self.private = false;
                } else {
                    self.state = State::Normal;
                }
            }
            State::Csi => self.csi_char(c),
        }
    }

    fn normal_char(&mut self, c: char) {
        match c {
            '\x1b' => self.state = State::Escape,
            '\n' => self.newline(),
            '\r' => self.col = 0,
            '\t' => {
                let next = (self.col / 8 + 1) * 8;
                while self.col < next.min(self.cols) {
                    self.put(' ');
                }
            }
            '\x08' => self.col = self.col.saturating_sub(1),
            c if (c as u32) < 0x20 => {}
            c => self.put(c),
        }
    }

    fn csi_char(&mut self, c: char) {
        match c {
            '0'..='9' => {
                if self.nparams == 0 {
                    self.nparams = 1;
                }
                let p = &mut self.params[self.nparams - 1];
                *p = p.saturating_mul(10).saturating_add(c as u16 - '0' as u16);
            }
            ';' => {
                if self.nparams == 0 {
                    self.nparams = 1;
                }
                if self.nparams < MAX_PARAMS {
                    self.nparams += 1;
                }
            }
            '?' => self.private = true,
            _ => {
                self.state = State::Normal;
                self.csi_final(c);
            }
        }
    }

    fn param(&self, i: usize, default: u16) -> u16 {
        if i < self.nparams && self.params[i] != 0 {
            self.params[i]
        } else {
            default
        }
    }

    fn csi_final(&mut self, c: char) {
        if self.private {
            // Only cursor visibility (DECTCEM) is supported.
            if self.param(0, 0) == 25 {
                match c {
                    'h' => self.cursor_visible = true,
                    'l' => self.cursor_visible = false,
                    _ => {}
                }
            }
            return;
        }
        match c {
            'm' => self.sgr(),
            'A' => self.row = self.row.saturating_sub(self.param(0, 1) as usize),
            'B' => self.row = (self.row + self.param(0, 1) as usize).min(self.rows - 1),
            'C' => self.col = (self.col + self.param(0, 1) as usize).min(self.cols - 1),
            'D' => self.col = self.col.saturating_sub(self.param(0, 1) as usize),
            'H' | 'f' => {
                self.row = (self.param(0, 1) as usize - 1).min(self.rows - 1);
                self.col = (self.param(1, 1) as usize - 1).min(self.cols - 1);
            }
            'J' => match self.param(0, 0) {
                0 => {
                    self.erase_line_end();
                    let y = (self.row + 1) * font::HEIGHT;
                    let (w, h) = (self.fb.width, self.fb.height.saturating_sub(y));
                    self.fb.fill_rect(0, y, w, h, self.bg);
                }
                2 => {
                    let (row, col) = (self.row, self.col);
                    self.clear();
                    self.row = row;
                    self.col = col;
                }
                _ => {}
            },
            'K' => self.erase_line_end(),
            _ => {}
        }
    }

    fn erase_line_end(&mut self) {
        let x = (self.col * font::WIDTH).min(self.fb.width);
        let y = self.row * font::HEIGHT;
        let w = self.fb.width - x;
        self.fb.fill_rect(x, y, w, font::HEIGHT, self.bg);
    }

    fn sgr(&mut self) {
        let n = self.nparams.max(1);
        for i in 0..n {
            let p = if i < self.nparams { self.params[i] } else { 0 };
            match p {
                0 => {
                    self.fg = DEFAULT_FG;
                    self.bg = DEFAULT_BG;
                    self.bold = false;
                    self.reverse = false;
                    self.fg_index = None;
                }
                7 => self.reverse = true,
                27 => self.reverse = false,
                1 => {
                    self.bold = true;
                    if let Some(idx) = self.fg_index {
                        self.fg = PALETTE[idx | 8];
                    }
                }
                22 => {
                    self.bold = false;
                    if let Some(idx) = self.fg_index {
                        self.fg = PALETTE[idx];
                    }
                }
                30..=37 => {
                    let idx = (p - 30) as usize;
                    self.fg_index = Some(idx);
                    self.fg = PALETTE[if self.bold { idx | 8 } else { idx }];
                }
                39 => {
                    self.fg = DEFAULT_FG;
                    self.fg_index = None;
                }
                40..=47 => self.bg = PALETTE[(p - 40) as usize],
                49 => self.bg = DEFAULT_BG,
                90..=97 => {
                    self.fg_index = None;
                    self.fg = PALETTE[(p - 90) as usize + 8];
                }
                100..=107 => self.bg = PALETTE[(p - 100) as usize + 8],
                _ => {}
            }
        }
    }

    fn put(&mut self, c: char) {
        if self.col >= self.cols {
            self.newline();
        }
        let glyph = font::glyph_or_replacement(c);
        let (fg, bg) = if self.reverse {
            (self.bg, self.fg)
        } else {
            (self.fg, self.bg)
        };
        self.fb.draw_glyph(
            self.col * font::WIDTH,
            self.row * font::HEIGHT,
            glyph,
            fg,
            bg,
        );
        self.col += 1;
    }

    fn newline(&mut self) {
        self.col = 0;
        if self.row + 1 >= self.rows {
            self.fb.scroll_up(font::HEIGHT, self.bg);
        } else {
            self.row += 1;
        }
    }
}

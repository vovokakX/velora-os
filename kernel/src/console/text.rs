//! Text console on top of a framebuffer, with a subset of ANSI escapes
//! (SGR colours and reverse video, cursor movement, erase, cursor
//! visibility `?25h`/`?25l`) so that the same byte stream looks the same
//! on the screen and on the serial terminal. The cursor is an underline
//! drawn by inverting the bottom of its cell after each write.
//!
//! Images arrive through the Kitty graphics protocol (see
//! `velora_gfx::kitty` for what is supported). An image is drawn into the
//! text screen at the cursor, so it scrolls with the text and text
//! written over it replaces it. Answers to queries (graphics commands,
//! device attributes `CSI c`, sizes `CSI 14/16/18 t`) are collected for
//! [`TextConsole::take_replies`]; the terminal layer gives them to the
//! reading program as input.

use super::font;
use super::framebuffer::Framebuffer;
use alloc::format;
use alloc::vec::Vec;
use velora_gfx::image::{self, Image};
use velora_gfx::kitty::{self, Event};

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
    /// Inside an APC string (`ESC _`), which ends with `ESC \`.
    Apc,
    ApcEscape,
}

/// Largest image payload accepted, in decoded bytes (about 1400x1400
/// RGBA).
const MAX_IMAGE_BYTES: usize = 8 << 20;
/// Stored images (`a=t`, or `a=T` with an id) may use this much memory;
/// the oldest are forgotten first.
const MAX_STORED_BYTES: usize = 16 << 20;

pub struct TextConsole {
    fb: Framebuffer,
    pub cols: usize,
    pub rows: usize,
    /// Cell size in pixels (the font times the scale).
    cw: usize,
    ch: usize,
    scale: usize,
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
    /// The CSI sequence started with '>', '=' or '<': not supported.
    ignored: bool,
    kitty: kitty::Parser,
    /// Stored images by id.
    images: Vec<(u32, Image)>,
    /// Answers for the program reading the terminal.
    replies: Vec<u8>,
    cursor_visible: bool,
    /// Cell where the cursor is currently drawn.
    cursor_drawn: Option<(usize, usize)>,
}

/// Height in pixels of the underline cursor.
const CURSOR_HEIGHT: usize = 2;

impl TextConsole {
    /// A console over `fb` with the font enlarged `scale` times (1..=4).
    pub fn new(fb: Framebuffer, scale: usize) -> Self {
        let scale = scale.clamp(1, 4);
        let (cw, ch) = (font::WIDTH * scale, font::HEIGHT * scale);
        let cols = fb.width / cw;
        let rows = fb.height / ch;
        let mut c = Self {
            fb,
            cols,
            rows,
            cw,
            ch,
            scale,
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
            ignored: false,
            kitty: kitty::Parser::new(MAX_IMAGE_BYTES),
            images: Vec::new(),
            replies: Vec::new(),
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

    /// Cell size in pixels.
    pub fn cell_size(&self) -> (usize, usize) {
        (self.cw, self.ch)
    }

    /// Answers produced since the last call (see the module comment).
    pub fn take_replies(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.replies)
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
            col * self.cw,
            row * self.ch + self.ch - CURSOR_HEIGHT * self.scale,
            self.cw,
            CURSOR_HEIGHT * self.scale,
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
                    self.ignored = false;
                } else if c == '_' {
                    self.state = State::Apc;
                    self.kitty.begin();
                } else {
                    self.state = State::Normal;
                }
            }
            State::Csi => self.csi_char(c),
            State::Apc => {
                if c == '\x1b' {
                    self.state = State::ApcEscape;
                } else {
                    self.kitty.push(if c.is_ascii() { c as u8 } else { 0xff });
                }
            }
            State::ApcEscape => {
                if c == '\\' {
                    self.state = State::Normal;
                    if let Some(ev) = self.kitty.end() {
                        self.graphics(ev);
                    }
                } else {
                    // Cut short: the ESC starts a new sequence.
                    self.kitty.abort();
                    self.state = State::Escape;
                    self.write_char(c);
                }
            }
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
            '>' | '=' | '<' => self.ignored = true,
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
        if self.ignored {
            return;
        }
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
                    let y = (self.row + 1) * self.ch;
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
            // Primary device attributes: a VT220 with ANSI colour.
            'c' if self.param(0, 0) == 0 => self.reply("\x1b[?62;22c"),
            't' => match self.param(0, 0) {
                14 => self.reply(&format!("\x1b[4;{};{}t", self.fb.height, self.fb.width)),
                16 => self.reply(&format!("\x1b[6;{};{}t", self.ch, self.cw)),
                18 => self.reply(&format!("\x1b[8;{};{}t", self.rows, self.cols)),
                _ => {}
            },
            _ => {}
        }
    }

    fn reply(&mut self, s: &str) {
        // Bounded, in case nobody reads the answers.
        if self.replies.len() < 4096 {
            self.replies.extend_from_slice(s.as_bytes());
        }
    }

    /// Runs a finished graphics command and queues its answer.
    fn graphics(&mut self, ev: Event) {
        let (id, quiet, result) = match ev {
            Event::Error { id, quiet, msg } => (id, quiet, Err(msg)),
            Event::Command { cmd, data } => (cmd.id, cmd.quiet, self.graphics_command(&cmd, &data)),
        };
        if let Some(a) = kitty::answer(id, quiet, result) {
            self.reply(&a);
        }
    }

    fn graphics_command(&mut self, cmd: &kitty::Command, data: &[u8]) -> Result<(), &'static str> {
        match cmd.action {
            b'q' => kitty::image(cmd, data).map(drop),
            b't' | b'T' => {
                let img = kitty::image(cmd, data)?;
                if cmd.action == b'T' {
                    self.draw_image(&img, cmd);
                }
                if cmd.id != 0 {
                    self.store_image(cmd.id, img);
                }
                Ok(())
            }
            b'p' => {
                let i = self
                    .images
                    .iter()
                    .position(|(id, _)| *id == cmd.id)
                    .ok_or("ENOENT:no image with this id")?;
                let img = self.images[i].1.clone();
                self.draw_image(&img, cmd);
                Ok(())
            }
            b'd' => {
                match cmd.delete {
                    b'i' | b'I' => self.images.retain(|(id, _)| *id != cmd.id),
                    _ => self.images.clear(),
                }
                Ok(())
            }
            _ => Err("EINVAL:unsupported action"),
        }
    }

    fn store_image(&mut self, id: u32, img: Image) {
        self.images.retain(|(i, _)| *i != id);
        if img.data.len() > MAX_STORED_BYTES {
            return;
        }
        // Keep the newest images that fit together with this one.
        let mut total = img.data.len();
        let mut keep = self.images.len();
        while keep > 0 && total + self.images[keep - 1].1.data.len() <= MAX_STORED_BYTES {
            total += self.images[keep - 1].1.data.len();
            keep -= 1;
        }
        self.images.drain(..keep);
        self.images.push((id, img));
    }

    /// Draws an image at the cursor, scrolling first so that it fits
    /// (as far as the screen allows). The cursor then moves past the
    /// image, to its last row, unless the command says `C=1`.
    fn draw_image(&mut self, img: &Image, cmd: &kitty::Command) {
        if self.rows == 0 || self.cols == 0 {
            return;
        }
        let (cols, rows, w, h) = kitty::placement(cmd, img.width, img.height, self.cw, self.ch);
        let scaled;
        let img = if (w, h) == (img.width, img.height) {
            img
        } else {
            scaled = img.resize(w, h);
            &scaled
        };
        let rows = rows.clamp(1, self.rows);
        if self.row + rows > self.rows {
            let n = self.row + rows - self.rows;
            self.fb.scroll_up(n * self.ch, self.bg);
            self.row -= n;
        }
        let (x0, y0) = (self.col.min(self.cols - 1) * self.cw, self.row * self.ch);
        let w = w.min(self.fb.width - x0);
        let h = h.min(self.fb.height - y0);
        for y in 0..h {
            for x in 0..w {
                let px = img.pixel(x, y);
                if px[3] == 0 {
                    continue;
                }
                let under = if px[3] == 255 {
                    0
                } else {
                    let n = self.fb.get_native(x0 + x, y0 + y).unwrap_or(0);
                    self.fb.rgb(n)
                };
                let out = self.fb.native(image::blend(px, under));
                self.fb.put_native(x0 + x, y0 + y, out);
            }
        }
        if !cmd.keep_cursor {
            self.row += rows - 1;
            self.col = (self.col + cols).min(self.cols);
        }
    }

    fn erase_line_end(&mut self) {
        let x = (self.col * self.cw).min(self.fb.width);
        let y = self.row * self.ch;
        let w = self.fb.width - x;
        self.fb.fill_rect(x, y, w, self.ch, self.bg);
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
            self.col * self.cw,
            self.row * self.ch,
            glyph,
            fg,
            bg,
            self.scale,
        );
        self.col += 1;
    }

    fn newline(&mut self) {
        self.col = 0;
        if self.row + 1 >= self.rows {
            self.fb.scroll_up(self.ch, self.bg);
        } else {
            self.row += 1;
        }
    }
}

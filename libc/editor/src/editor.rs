//! Editor state and key handling.

use crate::buffer::{Buffer, LoadError, Pos, char_width};
use crate::key::Key;
use crate::screen::{Cell, Frame};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// File access the editor needs; `/bin/plume` implements it with vlibc,
/// tests with a map in memory.
pub trait Io {
    /// Writes the whole file, creating or truncating it.
    fn save(&mut self, path: &str, data: &[u8]) -> Result<(), String>;
    fn exists(&self, path: &str) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PromptKind {
    Search,
    GotoLine,
    /// File name to save to; `quit` exits after a successful save.
    SaveAs {
        quit: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Prompt {
    kind: PromptKind,
    input: Vec<char>,
    cursor: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Question {
    /// "Save modified buffer?" before quitting.
    SaveBeforeQuit,
    /// The file to save to exists and is not the file being edited.
    Overwrite { path: String, quit: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Edit,
    Prompt(Prompt),
    Question(Question),
    Help,
}

pub struct Editor {
    buf: Buffer,
    cur: Pos,
    /// Column Up/Down try to keep.
    want_col: usize,
    /// First line and first column on the screen.
    top: usize,
    left: usize,
    path: Option<String>,
    modified: bool,
    read_only: bool,
    mark: Option<Pos>,
    cut: String,
    /// The previous key was a cut: another cut adds to the cut buffer.
    cutting: bool,
    last_search: String,
    message: Option<String>,
    mode: Mode,
    cols: usize,
    rows: usize,
}

/// Lines above the text (title bar).
const TOP_ROWS: usize = 1;
/// Lines below the text (status line and two lines of key hints).
const BOTTOM_ROWS: usize = 3;

const EDIT_HINTS: [(&str, &str); 12] = [
    ("^G", "Help"),
    ("^S", "Save"),
    ("^W", "Find"),
    ("^K", "Cut"),
    ("^B", "Mark"),
    ("^L", "Go to line"),
    ("^X", "Exit"),
    ("^O", "Save as"),
    ("^N", "Find next"),
    ("^U", "Paste"),
    ("^C", "Position"),
    ("^A", "Line start"),
];
const PROMPT_HINTS: [(&str, &str); 2] = [("Enter", "Confirm"), ("^C", "Cancel")];
const QUESTION_HINTS: [(&str, &str); 3] = [(" Y", "Yes"), (" N", "No"), ("^C", "Cancel")];
const HELP_HINTS: [(&str, &str); 1] = [("Any key", "Back to the text")];

const HELP: &[&str] = &[
    "Plume: a small text editor for Velora OS",
    "",
    "Type to insert text; Enter starts a new line, Tab inserts a tab.",
    "Ctrl+key commands (^X means Ctrl+X):",
    "",
    "  ^S  save the file            ^O  save under another name",
    "  ^X  exit (asks to save changes)",
    "  ^W  find text (case-insensitive), ^N  find the next match",
    "  ^K  cut the line, or the marked text; repeated ^K cuts more lines",
    "  ^U  paste what was cut       ^B  start or stop marking text",
    "  ^L  go to a line number      ^C  show the cursor position",
    "  ^A  start of line            ^E  end of line",
    "  ^Y  page up                  ^V  page down",
    "  ^D  delete the character under the cursor",
    "  Esc cancel a prompt or the mark",
    "",
    "Arrows, Home, End, Page Up, Page Down, Delete and Backspace work as",
    "usual. Alt+Shift switches the keyboard layout (EN/RU).",
    "Files are UTF-8; CRLF line ends are kept as they were.",
];

impl Editor {
    /// An empty buffer, optionally with the path it will be saved to.
    pub fn new(path: Option<&str>) -> Self {
        Self {
            buf: Buffer::new(),
            cur: Pos::default(),
            want_col: 0,
            top: 0,
            left: 0,
            path: path.map(String::from),
            modified: false,
            read_only: false,
            mark: None,
            cut: String::new(),
            cutting: false,
            last_search: String::new(),
            message: None,
            mode: Mode::Edit,
            cols: 80,
            rows: 25,
        }
    }

    /// A buffer loaded from an existing file.
    pub fn open(path: &str, bytes: &[u8]) -> Self {
        let mut e = Self::new(Some(path));
        let (buf, err) = Buffer::load(bytes);
        e.buf = buf;
        let lines = e.buf.line_count();
        e.message = Some(match err {
            Some(LoadError::NotUtf8) => {
                e.read_only = true;
                "File is not valid UTF-8: opened read-only".to_string()
            }
            None => format!(
                "Read {} line{}{}",
                lines,
                if lines == 1 { "" } else { "s" },
                if e.buf.crlf { " (CRLF)" } else { "" }
            ),
        });
        e
    }

    /// A buffer for a file that does not exist yet.
    pub fn new_file(path: &str) -> Self {
        let mut e = Self::new(Some(path));
        e.message = Some("New file".to_string());
        e
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols.max(20);
        self.rows = rows.max(TOP_ROWS + BOTTOM_ROWS + 1);
        self.scroll();
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buf
    }

    pub fn cursor(&self) -> Pos {
        self.cur
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    fn text_rows(&self) -> usize {
        self.rows - TOP_ROWS - BOTTOM_ROWS
    }

    fn say(&mut self, m: impl Into<String>) {
        self.message = Some(m.into());
    }

    // --- keys ----------------------------------------------------------------

    pub fn handle(&mut self, key: Key, io: &mut dyn Io) -> Status {
        let status = match core::mem::replace(&mut self.mode, Mode::Edit) {
            Mode::Edit => self.edit_key(key, io),
            Mode::Help => Status::Running,
            Mode::Prompt(p) => self.prompt_key(p, key, io),
            Mode::Question(q) => self.question_key(q, key, io),
        };
        self.scroll();
        status
    }

    fn set_cursor(&mut self, p: Pos) {
        self.cur = self.buf.clamp(p);
        self.want_col = self.buf.column(self.cur.line, self.cur.byte);
    }

    fn move_vertical(&mut self, lines: isize) {
        let last = self.buf.line_count() as isize - 1;
        let line = (self.cur.line as isize + lines).clamp(0, last) as usize;
        let byte = self.buf.byte_at_column(line, self.want_col);
        self.cur = Pos::new(line, byte);
    }

    fn editable(&mut self) -> bool {
        if self.read_only {
            self.say("File is not valid UTF-8: read-only");
            false
        } else {
            true
        }
    }

    fn changed(&mut self) {
        self.modified = true;
    }

    fn edit_key(&mut self, key: Key, io: &mut dyn Io) -> Status {
        let was_cutting = core::mem::replace(&mut self.cutting, false);
        if !matches!(key, Key::Ctrl('c')) {
            self.message = None;
        }
        match key {
            Key::Char(c) => self.insert_text(&c.to_string()),
            Key::Tab => self.insert_text("\t"),
            Key::Enter => self.insert_text("\n"),
            Key::Backspace => {
                if self.editable()
                    && let Some(p) = self.buf.prev(self.cur)
                {
                    self.buf.remove(p, self.cur);
                    self.set_cursor(p);
                    self.adjust_mark_after_edit();
                    self.changed();
                }
            }
            Key::Delete | Key::Ctrl('d') => {
                if self.editable()
                    && let Some(n) = self.buf.next(self.cur)
                {
                    self.buf.remove(self.cur, n);
                    self.set_cursor(self.cur);
                    self.adjust_mark_after_edit();
                    self.changed();
                }
            }
            Key::Left => {
                if let Some(p) = self.buf.prev(self.cur) {
                    self.set_cursor(p);
                }
            }
            Key::Right => {
                if let Some(p) = self.buf.next(self.cur) {
                    self.set_cursor(p);
                }
            }
            Key::Up => self.move_vertical(-1),
            Key::Down => self.move_vertical(1),
            Key::Home | Key::Ctrl('a') => self.set_cursor(Pos::new(self.cur.line, 0)),
            Key::End | Key::Ctrl('e') => {
                self.set_cursor(Pos::new(self.cur.line, self.buf.line(self.cur.line).len()))
            }
            Key::PageUp | Key::Ctrl('y') => {
                let n = self.text_rows().saturating_sub(1).max(1);
                self.top = self.top.saturating_sub(n);
                self.move_vertical(-(n as isize));
            }
            Key::PageDown | Key::Ctrl('v') => {
                let n = self.text_rows().saturating_sub(1).max(1);
                self.top = (self.top + n).min(self.buf.line_count() - 1);
                self.move_vertical(n as isize);
            }
            Key::Ctrl('k') => self.cut_text(was_cutting),
            Key::Ctrl('u') => self.paste(),
            Key::Ctrl('b') => {
                if self.mark.take().is_some() {
                    self.say("Mark unset");
                } else {
                    self.mark = Some(self.cur);
                    self.say("Mark set");
                }
            }
            Key::Ctrl('w') => self.start_prompt(PromptKind::Search, ""),
            Key::Ctrl('n') => {
                if self.last_search.is_empty() {
                    self.say("No previous search");
                } else {
                    let s = self.last_search.clone();
                    self.search(&s);
                }
            }
            Key::Ctrl('l') => self.start_prompt(PromptKind::GotoLine, ""),
            Key::Ctrl('c') => self.show_position(),
            Key::Ctrl('g') => self.mode = Mode::Help,
            Key::Ctrl('s') => {
                if self.read_only {
                    self.editable();
                } else if let Some(path) = self.path.clone() {
                    self.save_to(&path, io);
                } else {
                    self.start_prompt(PromptKind::SaveAs { quit: false }, "");
                }
            }
            Key::Ctrl('o') => {
                if self.editable() {
                    let p = self.path.clone().unwrap_or_default();
                    self.start_prompt(PromptKind::SaveAs { quit: false }, &p);
                }
            }
            Key::Ctrl('x') => {
                if self.modified {
                    self.mode = Mode::Question(Question::SaveBeforeQuit);
                } else {
                    return Status::Quit;
                }
            }
            Key::Escape => {
                if self.mark.take().is_some() {
                    self.say("Mark unset");
                }
            }
            Key::Ctrl(c) => self.say(format!("Unknown command: ^{}", c.to_ascii_uppercase())),
            Key::Unknown => {}
        }
        Status::Running
    }

    fn adjust_mark_after_edit(&mut self) {
        if let Some(m) = self.mark {
            self.mark = Some(self.buf.clamp(m));
        }
    }

    fn insert_text(&mut self, text: &str) {
        if !self.editable() {
            return;
        }
        let end = self.buf.insert(self.cur, text);
        if let Some(m) = self.mark {
            // Text inserted before the mark on its line moves it.
            if m.line == self.cur.line && m.byte > self.cur.byte {
                self.mark = Some(if end.line == m.line {
                    Pos::new(m.line, m.byte + text.len())
                } else {
                    Pos::new(end.line, end.byte + (m.byte - self.cur.byte))
                });
            } else if m.line > self.cur.line {
                self.mark = Some(Pos::new(m.line + text.matches('\n').count(), m.byte));
            }
        }
        self.set_cursor(end);
        self.changed();
    }

    fn cut_text(&mut self, append: bool) {
        if !self.editable() {
            return;
        }
        if let Some(m) = self.mark.take() {
            let start = m.min(self.cur);
            self.cut = self.buf.remove(m, self.cur);
            self.set_cursor(start);
            self.changed();
            self.say("Cut the marked text");
            return;
        }
        let line = self.cur.line;
        let last = self.buf.line_count() - 1;
        if line == last && self.buf.line(line).is_empty() {
            self.cutting = append;
            return;
        }
        let mut text = self.buf.remove_line(line);
        if line < last {
            text.push('\n');
        }
        if append {
            self.cut.push_str(&text);
        } else {
            self.cut = text;
        }
        self.cutting = true;
        self.set_cursor(Pos::new(line, 0));
        self.changed();
    }

    fn paste(&mut self) {
        if self.cut.is_empty() {
            self.say("The cut buffer is empty");
            return;
        }
        let text = self.cut.clone();
        self.insert_text(&text);
    }

    fn show_position(&mut self) {
        let lines = self.buf.line_count();
        let line = self.cur.line + 1;
        let col = self.buf.line(self.cur.line)[..self.cur.byte]
            .chars()
            .count()
            + 1;
        let len = self.buf.line(self.cur.line).chars().count() + 1;
        self.say(format!(
            "line {}/{} ({}%), column {}/{}",
            line,
            lines,
            line * 100 / lines,
            col,
            len
        ));
    }

    fn search(&mut self, needle: &str) {
        let from = self.buf.next(self.cur).unwrap_or_default();
        match self.buf.find(needle, from) {
            Some((p, wrapped)) => {
                if p == self.cur {
                    self.say("This is the only match");
                } else if wrapped || p < self.cur {
                    self.say("Search wrapped to the start");
                }
                self.set_cursor(p);
            }
            None => self.say(format!("\"{}\" not found", needle)),
        }
    }

    fn save_to(&mut self, path: &str, io: &mut dyn Io) -> bool {
        match io.save(path, &self.buf.to_bytes()) {
            Ok(()) => {
                self.path = Some(path.to_string());
                self.modified = false;
                let n = self.buf.line_count();
                self.say(format!(
                    "Wrote {} line{} to {}",
                    n,
                    if n == 1 { "" } else { "s" },
                    path
                ));
                true
            }
            Err(e) => {
                self.say(format!("Cannot write {}: {}", path, e));
                false
            }
        }
    }

    // --- prompts -------------------------------------------------------------

    fn start_prompt(&mut self, kind: PromptKind, text: &str) {
        let input: Vec<char> = text.chars().collect();
        let cursor = input.len();
        self.mode = Mode::Prompt(Prompt {
            kind,
            input,
            cursor,
        });
    }

    fn prompt_key(&mut self, mut p: Prompt, key: Key, io: &mut dyn Io) -> Status {
        match key {
            Key::Char(c) => {
                p.input.insert(p.cursor, c);
                p.cursor += 1;
            }
            Key::Backspace => {
                if p.cursor > 0 {
                    p.cursor -= 1;
                    p.input.remove(p.cursor);
                }
            }
            Key::Delete => {
                if p.cursor < p.input.len() {
                    p.input.remove(p.cursor);
                }
            }
            Key::Left => p.cursor = p.cursor.saturating_sub(1),
            Key::Right => p.cursor = (p.cursor + 1).min(p.input.len()),
            Key::Home | Key::Ctrl('a') => p.cursor = 0,
            Key::End | Key::Ctrl('e') => p.cursor = p.input.len(),
            Key::Escape | Key::Ctrl('c') => {
                self.say("Cancelled");
                return Status::Running;
            }
            Key::Enter => {
                let text: String = p.input.iter().collect();
                return self.prompt_done(p.kind, text, io);
            }
            _ => {}
        }
        self.mode = Mode::Prompt(p);
        Status::Running
    }

    fn prompt_done(&mut self, kind: PromptKind, text: String, io: &mut dyn Io) -> Status {
        match kind {
            PromptKind::Search => {
                let needle = if text.is_empty() {
                    self.last_search.clone()
                } else {
                    text
                };
                if needle.is_empty() {
                    self.say("Cancelled");
                } else {
                    self.last_search = needle.clone();
                    self.search(&needle);
                }
            }
            PromptKind::GotoLine => match text.trim().parse::<usize>() {
                Ok(n) if n >= 1 => {
                    let line = n.min(self.buf.line_count()) - 1;
                    self.set_cursor(Pos::new(line, 0));
                }
                _ => self.say("Invalid line number"),
            },
            PromptKind::SaveAs { quit } => {
                let path = text.trim().to_string();
                if path.is_empty() {
                    self.say("Cancelled");
                } else if self.path.as_deref() != Some(path.as_str()) && io.exists(&path) {
                    self.mode = Mode::Question(Question::Overwrite { path, quit });
                } else if self.save_to(&path, io) && quit {
                    return Status::Quit;
                }
            }
        }
        Status::Running
    }

    fn question_key(&mut self, q: Question, key: Key, io: &mut dyn Io) -> Status {
        let answer = match key {
            Key::Char('y' | 'Y' | 'д' | 'Д') => Some(true),
            Key::Char('n' | 'N' | 'н' | 'Н') => Some(false),
            Key::Escape | Key::Ctrl('c') => {
                self.say("Cancelled");
                return Status::Running;
            }
            _ => None,
        };
        let Some(yes) = answer else {
            self.mode = Mode::Question(q);
            return Status::Running;
        };
        match q {
            Question::SaveBeforeQuit => {
                if !yes {
                    return Status::Quit;
                }
                match self.path.clone() {
                    Some(path) => {
                        if self.save_to(&path, io) {
                            return Status::Quit;
                        }
                    }
                    None => self.start_prompt(PromptKind::SaveAs { quit: true }, ""),
                }
            }
            Question::Overwrite { path, quit } => {
                if !yes {
                    self.say("Not saved");
                } else if self.save_to(&path, io) && quit {
                    return Status::Quit;
                }
            }
        }
        Status::Running
    }

    // --- screen --------------------------------------------------------------

    /// Keeps the cursor inside the visible part of the text.
    fn scroll(&mut self) {
        let rows = self.text_rows();
        let last = self.buf.line_count() - 1;
        self.top = self.top.min(last);
        if self.cur.line < self.top {
            self.top = self.cur.line;
        } else if self.cur.line >= self.top + rows {
            self.top = self.cur.line + 1 - rows;
        }
        let col = self.buf.column(self.cur.line, self.cur.byte);
        if col < self.left {
            self.left = col;
        } else if col >= self.left + self.cols {
            self.left = col + 1 - self.cols;
        }
    }

    fn marked(&self, p: Pos) -> bool {
        match self.mark {
            Some(m) => {
                let (a, b) = if m <= self.cur {
                    (m, self.cur)
                } else {
                    (self.cur, m)
                };
                p >= a && p < b
            }
            None => false,
        }
    }

    fn text_row(&self, line: usize) -> Vec<Cell> {
        let mut cells = Vec::with_capacity(self.cols);
        let mut col = 0;
        let end = self.left + self.cols;
        let s = self.buf.line(line);
        for (i, c) in s.char_indices() {
            if col >= end {
                break;
            }
            let rev = self.marked(Pos::new(line, i));
            let w = char_width(c, col);
            let shown: [char; 2] = if c == '\t' {
                [' ', ' ']
            } else if w == 2 {
                ['^', char::from_u32((c as u32) ^ 0x40).unwrap_or('?')]
            } else {
                [c, ' ']
            };
            for k in 0..w {
                if col + k >= self.left && col + k < end {
                    let ch = if c == '\t' { ' ' } else { shown[k.min(1)] };
                    cells.push(Cell { ch, rev });
                }
            }
            col += w;
        }
        // A marked line end shows as one marked cell.
        if self.marked(Pos::new(line, s.len())) && col >= self.left && col < end {
            cells.push(Cell { ch: ' ', rev: true });
        }
        cells
    }

    fn title_row(&self) -> Vec<Cell> {
        let left = format!("  Plume {}", env!("CARGO_PKG_VERSION"));
        let name = match &self.path {
            Some(p) => p.clone(),
            None => "New buffer".to_string(),
        };
        let right = if self.read_only {
            "Read-only  "
        } else if self.modified {
            "Modified  "
        } else {
            ""
        };
        let mut row: Vec<char> = core::iter::repeat_n(' ', self.cols).collect();
        put_str(&mut row, 0, &left);
        let name_len = name.chars().count();
        let start = (self.cols.saturating_sub(name_len)) / 2;
        put_str(&mut row, start.max(left.len() + 2), &name);
        let rlen = right.chars().count();
        put_str(&mut row, self.cols.saturating_sub(rlen), right);
        row.into_iter().map(|ch| Cell { ch, rev: true }).collect()
    }

    fn hint_rows(&self, hints: &[(&str, &str)]) -> [Vec<Cell>; 2] {
        let per_row = hints.len().div_ceil(2).max(1);
        let width = self.cols / per_row.max(1);
        let mut rows = [Vec::new(), Vec::new()];
        for (r, row) in rows.iter_mut().enumerate() {
            for (k, label) in hints.iter().skip(r * per_row).take(per_row) {
                let mut used = 0;
                for ch in k.chars() {
                    row.push(Cell { ch, rev: true });
                    used += 1;
                }
                for ch in core::iter::once(' ').chain(label.chars()) {
                    if used < width.saturating_sub(1) {
                        row.push(Cell { ch, rev: false });
                        used += 1;
                    }
                }
                while used < width {
                    row.push(Cell::blank());
                    used += 1;
                }
            }
        }
        rows
    }

    /// Status line and the cursor column if a prompt is active.
    fn status_row(&self) -> (Vec<Cell>, Option<usize>) {
        let rev_row = |text: &str| -> Vec<Cell> {
            let mut row: Vec<Cell> = text.chars().map(|ch| Cell { ch, rev: true }).collect();
            row.truncate(self.cols);
            while row.len() < self.cols {
                row.push(Cell { ch: ' ', rev: true });
            }
            row
        };
        match &self.mode {
            Mode::Prompt(p) => {
                let label = match p.kind {
                    PromptKind::Search if !self.last_search.is_empty() => {
                        format!("Search [{}]: ", self.last_search)
                    }
                    PromptKind::Search => "Search: ".to_string(),
                    PromptKind::GotoLine => "Go to line: ".to_string(),
                    PromptKind::SaveAs { .. } => "File name to write: ".to_string(),
                };
                let label_len = label.chars().count();
                // Scroll the input so the cursor stays visible.
                let room = self.cols.saturating_sub(label_len + 1).max(1);
                let skip = p.cursor.saturating_sub(room);
                let input: String = p.input.iter().skip(skip).collect();
                let row = rev_row(&format!("{}{}", label, input));
                (row, Some((label_len + p.cursor - skip).min(self.cols - 1)))
            }
            Mode::Question(q) => {
                let text = match q {
                    Question::SaveBeforeQuit => {
                        "Save the changes before closing? (Y/N)".to_string()
                    }
                    Question::Overwrite { path, .. } => {
                        format!("File \"{}\" exists. Overwrite it? (Y/N)", path)
                    }
                };
                let len = text.chars().count();
                (rev_row(&text), Some(len.min(self.cols - 1)))
            }
            _ => match &self.message {
                Some(m) => {
                    let text = format!("[ {} ]", m);
                    let len = text.chars().count().min(self.cols);
                    let start = (self.cols - len) / 2;
                    let mut row: Vec<Cell> = (0..start).map(|_| Cell::blank()).collect();
                    row.extend(text.chars().take(len).map(|ch| Cell { ch, rev: true }));
                    (row, None)
                }
                None => (Vec::new(), None),
            },
        }
    }

    /// The whole screen for the current state.
    pub fn render(&self) -> Frame {
        let mut rows = Vec::with_capacity(self.rows);
        rows.push(self.title_row());
        let text_rows = self.text_rows();
        if self.mode == Mode::Help {
            for i in 0..text_rows {
                let line = HELP.get(i).copied().unwrap_or("");
                rows.push(line.chars().take(self.cols).map(Cell::plain).collect());
            }
        } else {
            for i in 0..text_rows {
                let line = self.top + i;
                if line < self.buf.line_count() {
                    rows.push(self.text_row(line));
                } else {
                    rows.push(Vec::new());
                }
            }
        }
        let (status, prompt_col) = self.status_row();
        rows.push(status);
        let hints: &[(&str, &str)] = match self.mode {
            Mode::Edit => &EDIT_HINTS,
            Mode::Prompt(_) => &PROMPT_HINTS,
            Mode::Question(_) => &QUESTION_HINTS,
            Mode::Help => &HELP_HINTS,
        };
        let [h1, h2] = self.hint_rows(hints);
        rows.push(h1);
        rows.push(h2);
        for r in rows.iter_mut() {
            r.truncate(self.cols);
        }
        let cursor = match (prompt_col, &self.mode) {
            (Some(c), _) => (self.rows - BOTTOM_ROWS, c),
            (None, Mode::Help) => (self.rows - 1, 0),
            _ => {
                let col = self.buf.column(self.cur.line, self.cur.byte) - self.left;
                (TOP_ROWS + self.cur.line - self.top, col.min(self.cols - 1))
            }
        };
        Frame {
            rows,
            cursor,
            cols: self.cols,
        }
    }
}

fn put_str(row: &mut [char], at: usize, s: &str) {
    for (i, c) in s.chars().enumerate() {
        if let Some(slot) = row.get_mut(at + i) {
            *slot = c;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MemIo {
        files: BTreeMap<String, Vec<u8>>,
        fail: bool,
    }

    impl Io for MemIo {
        fn save(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
            if self.fail {
                return Err("permission denied".into());
            }
            self.files.insert(path.into(), data.to_vec());
            Ok(())
        }
        fn exists(&self, path: &str) -> bool {
            self.files.contains_key(path)
        }
    }

    fn typed(e: &mut Editor, io: &mut MemIo, text: &str) {
        for c in text.chars() {
            let k = match c {
                '\n' => Key::Enter,
                '\t' => Key::Tab,
                c => Key::Char(c),
            };
            assert_eq!(e.handle(k, io), Status::Running);
        }
    }

    fn keys(e: &mut Editor, io: &mut MemIo, ks: &[Key]) -> Status {
        let mut s = Status::Running;
        for &k in ks {
            s = e.handle(k, io);
        }
        s
    }

    fn row_text(f: &Frame, r: usize) -> String {
        f.rows[r].iter().map(|c| c.ch).collect::<String>()
    }

    #[test]
    fn type_and_save() {
        let mut io = MemIo::default();
        let mut e = Editor::new_file("/tmp/a.txt");
        typed(&mut e, &mut io, "hello\nмир");
        assert!(e.is_modified());
        keys(&mut e, &mut io, &[Key::Ctrl('s')]);
        assert_eq!(io.files["/tmp/a.txt"], "hello\nмир".as_bytes());
        assert!(!e.is_modified());
        assert_eq!(e.message(), Some("Wrote 2 lines to /tmp/a.txt"));
    }

    #[test]
    fn editing_keys() {
        let mut io = MemIo::default();
        let mut e = Editor::open("f", b"abc\ndef");
        keys(
            &mut e,
            &mut io,
            &[Key::End, Key::Backspace, Key::Down, Key::Delete],
        );
        assert_eq!(e.buffer().text(), "ab\nde");
        // Backspace at a line start joins lines; Delete at the end too.
        keys(&mut e, &mut io, &[Key::Home, Key::Backspace]);
        assert_eq!(e.buffer().text(), "abde");
        assert_eq!(e.cursor(), Pos::new(0, 2));
        keys(
            &mut e,
            &mut io,
            &[Key::Enter, Key::Up, Key::End, Key::Delete],
        );
        assert_eq!(e.buffer().text(), "abde");
    }

    #[test]
    fn up_down_keep_the_column() {
        let mut io = MemIo::default();
        let mut e = Editor::open("f", b"long line\nab\nanother line");
        keys(&mut e, &mut io, &[Key::End, Key::Down]);
        assert_eq!(e.cursor(), Pos::new(1, 2));
        keys(&mut e, &mut io, &[Key::Down]);
        assert_eq!(e.cursor(), Pos::new(2, 9));
    }

    #[test]
    fn cut_and_paste_lines() {
        let mut io = MemIo::default();
        let mut e = Editor::open("f", b"one\ntwo\nthree\nfour\n");
        keys(
            &mut e,
            &mut io,
            &[Key::Down, Key::Ctrl('k'), Key::Ctrl('k')],
        );
        assert_eq!(e.buffer().text(), "one\nfour\n");
        keys(&mut e, &mut io, &[Key::Down, Key::Ctrl('u')]);
        assert_eq!(e.buffer().text(), "one\nfour\ntwo\nthree\n");
        // A new cut after another key replaces the cut buffer.
        keys(
            &mut e,
            &mut io,
            &[Key::Up, Key::Ctrl('k'), Key::Down, Key::Ctrl('k')],
        );
        keys(&mut e, &mut io, &[Key::Ctrl('u')]);
        assert!(
            e.buffer().text().starts_with("one\nfour\n"),
            "{}",
            e.buffer().text()
        );
    }

    #[test]
    fn cut_marked_text() {
        let mut io = MemIo::default();
        let mut e = Editor::open("f", "привет мир".as_bytes());
        keys(&mut e, &mut io, &[Key::Ctrl('b')]);
        for _ in 0..7 {
            keys(&mut e, &mut io, &[Key::Right]);
        }
        let f = e.render();
        assert!(f.rows[1][0].rev && f.rows[1][6].rev && !f.rows[1][7].rev);
        keys(&mut e, &mut io, &[Key::Ctrl('k'), Key::End, Key::Ctrl('u')]);
        assert_eq!(e.buffer().text(), "мирпривет ");
    }

    #[test]
    fn search_and_go_to_line() {
        let mut io = MemIo::default();
        let mut e = Editor::open("f", "alpha\nБета gamma\nbeta".as_bytes());
        keys(&mut e, &mut io, &[Key::Ctrl('w')]);
        typed(&mut e, &mut io, "бЕТа");
        keys(&mut e, &mut io, &[Key::Enter]);
        assert_eq!(e.cursor(), Pos::new(1, 0));
        // Ctrl+W, Enter repeats the last search; Ctrl+N too.
        keys(&mut e, &mut io, &[Key::Ctrl('w'), Key::Enter]);
        assert_eq!(e.cursor(), Pos::new(1, 0));
        assert_eq!(e.message(), Some("This is the only match"));
        keys(&mut e, &mut io, &[Key::Ctrl('w')]);
        typed(&mut e, &mut io, "a");
        keys(&mut e, &mut io, &[Key::Enter, Key::Ctrl('n')]);
        assert_eq!(e.cursor(), Pos::new(1, 13));
        keys(&mut e, &mut io, &[Key::Ctrl('n'), Key::Ctrl('n')]);
        assert_eq!(e.cursor(), Pos::new(0, 0));
        assert_eq!(e.message(), Some("Search wrapped to the start"));
        keys(&mut e, &mut io, &[Key::Ctrl('w')]);
        typed(&mut e, &mut io, "zzz");
        keys(&mut e, &mut io, &[Key::Enter]);
        assert_eq!(e.message(), Some("\"zzz\" not found"));
        keys(&mut e, &mut io, &[Key::Ctrl('l')]);
        typed(&mut e, &mut io, "3");
        keys(&mut e, &mut io, &[Key::Enter]);
        assert_eq!(e.cursor(), Pos::new(2, 0));
        keys(&mut e, &mut io, &[Key::Ctrl('l')]);
        typed(&mut e, &mut io, "x");
        keys(&mut e, &mut io, &[Key::Enter]);
        assert_eq!(e.message(), Some("Invalid line number"));
    }

    #[test]
    fn quit_without_changes() {
        let mut io = MemIo::default();
        let mut e = Editor::open("f", b"x");
        assert_eq!(e.handle(Key::Ctrl('x'), &mut io), Status::Quit);
    }

    #[test]
    fn quit_asks_to_save() {
        let mut io = MemIo::default();
        let mut e = Editor::open("f", b"x");
        typed(&mut e, &mut io, "y");
        assert_eq!(e.handle(Key::Ctrl('x'), &mut io), Status::Running);
        assert!(row_text(&e.render(), 22).contains("Save the changes"));
        // Other keys are ignored, Ctrl+C cancels.
        assert_eq!(
            keys(&mut e, &mut io, &[Key::Char('q'), Key::Ctrl('c')]),
            Status::Running
        );
        assert!(io.files.is_empty());
        assert_eq!(e.message(), Some("Cancelled"));
        // Y saves and quits (also on the Russian layout).
        assert_eq!(
            keys(&mut e, &mut io, &[Key::Ctrl('x'), Key::Char('Д')]),
            Status::Quit
        );
        assert_eq!(io.files["f"], b"yx");
    }

    #[test]
    fn quit_discarding_changes() {
        let mut io = MemIo::default();
        let mut e = Editor::open("f", b"x");
        typed(&mut e, &mut io, "z");
        assert_eq!(
            keys(&mut e, &mut io, &[Key::Ctrl('x'), Key::Char('n')]),
            Status::Quit
        );
        assert!(io.files.is_empty());
    }

    #[test]
    fn failed_save_does_not_quit() {
        let mut io = MemIo {
            fail: true,
            ..Default::default()
        };
        let mut e = Editor::open("/etc/x", b"x");
        typed(&mut e, &mut io, "z");
        assert_eq!(
            keys(&mut e, &mut io, &[Key::Ctrl('x'), Key::Char('y')]),
            Status::Running
        );
        assert!(e.is_modified());
        assert_eq!(e.message(), Some("Cannot write /etc/x: permission denied"));
    }

    #[test]
    fn save_as_and_overwrite_question() {
        let mut io = MemIo::default();
        io.files.insert("other".into(), b"old".to_vec());
        let mut e = Editor::new(None);
        typed(&mut e, &mut io, "new");
        // No name yet: Ctrl+S asks for one.
        keys(&mut e, &mut io, &[Key::Ctrl('s')]);
        typed(&mut e, &mut io, "other");
        keys(&mut e, &mut io, &[Key::Enter]);
        assert!(row_text(&e.render(), 22).contains("Overwrite"));
        keys(&mut e, &mut io, &[Key::Char('n')]);
        assert_eq!(io.files["other"], b"old");
        keys(&mut e, &mut io, &[Key::Ctrl('o')]);
        typed(&mut e, &mut io, "other");
        keys(&mut e, &mut io, &[Key::Enter, Key::Char('y')]);
        assert_eq!(io.files["other"], b"new");
        assert_eq!(e.path(), Some("other"));
        // Saving to the current name does not ask again.
        typed(&mut e, &mut io, "!");
        keys(&mut e, &mut io, &[Key::Ctrl('o'), Key::Enter]);
        assert_eq!(io.files["other"], b"new!");
    }

    #[test]
    fn quit_without_a_name_asks_for_one() {
        let mut io = MemIo::default();
        let mut e = Editor::new(None);
        typed(&mut e, &mut io, "t");
        keys(&mut e, &mut io, &[Key::Ctrl('x'), Key::Char('y')]);
        typed(&mut e, &mut io, "named");
        assert_eq!(e.handle(Key::Enter, &mut io), Status::Quit);
        assert_eq!(io.files["named"], b"t");
    }

    #[test]
    fn invalid_utf8_is_read_only() {
        let mut io = MemIo::default();
        let mut e = Editor::open("bin", b"a\xffb");
        typed(&mut e, &mut io, "x");
        assert_eq!(e.buffer().line(0), "a\u{fffd}b");
        assert!(!e.is_modified());
        assert!(row_text(&e.render(), 0).contains("Read-only"));
        assert_eq!(e.handle(Key::Ctrl('x'), &mut io), Status::Quit);
    }

    #[test]
    fn crlf_files_keep_their_line_ends() {
        let mut io = MemIo::default();
        let mut e = Editor::open("dos", b"a\r\nb\r\n");
        keys(&mut e, &mut io, &[Key::End]);
        typed(&mut e, &mut io, "\nc");
        keys(&mut e, &mut io, &[Key::Ctrl('s')]);
        assert_eq!(io.files["dos"], b"a\r\nc\r\nb\r\n");
    }

    #[test]
    fn screen_layout() {
        let mut io = MemIo::default();
        let mut e = Editor::open("/home/user/notes.txt", "строка 1\n\tx\x01".as_bytes());
        e.resize(60, 10);
        let f = e.render();
        assert_eq!(f.rows.len(), 10);
        assert!(row_text(&f, 0).contains("Plume"));
        assert!(row_text(&f, 0).contains("/home/user/notes.txt"));
        assert_eq!(row_text(&f, 1), "строка 1");
        assert_eq!(row_text(&f, 2), "        x^A");
        assert!(row_text(&f, 7).contains("[ Read 2 lines ]"));
        assert!(row_text(&f, 8).starts_with("^G Help"));
        assert!(row_text(&f, 9).contains("^X Exit"));
        assert_eq!(f.cursor, (1, 0));
        typed(&mut e, &mut io, "!");
        let f = e.render();
        assert!(row_text(&f, 0).contains("Modified"));
        assert_eq!(f.cursor, (1, 1));
        keys(&mut e, &mut io, &[Key::Ctrl('g')]);
        assert!(row_text(&e.render(), 1).contains("Plume: a small text editor"));
        keys(&mut e, &mut io, &[Key::Char('q')]);
        assert_eq!(row_text(&e.render(), 1), "!строка 1");
    }

    #[test]
    fn scrolling_follows_the_cursor() {
        let mut io = MemIo::default();
        let text: String = (1..=50).map(|i| format!("line {}\n", i)).collect();
        let mut e = Editor::open("f", text.as_bytes());
        e.resize(40, 10); // 6 text rows
        for _ in 0..10 {
            keys(&mut e, &mut io, &[Key::Down]);
        }
        let f = e.render();
        assert_eq!(row_text(&f, 6), "line 11");
        assert_eq!(f.cursor, (6, 0));
        keys(&mut e, &mut io, &[Key::PageDown]);
        assert_eq!(e.cursor().line, 15);
        // A long line scrolls sideways.
        let mut e = Editor::open("f", "x".repeat(100).as_bytes());
        e.resize(40, 10);
        keys(&mut e, &mut io, &[Key::End]);
        let f = e.render();
        assert_eq!(f.cursor, (1, 39));
        assert_eq!(row_text(&f, 1), "x".repeat(39));
    }

    #[test]
    fn prompt_editing() {
        let mut io = MemIo::default();
        let mut e = Editor::new_file("a");
        keys(&mut e, &mut io, &[Key::Ctrl('o')]);
        keys(
            &mut e,
            &mut io,
            &[Key::Home, Key::Char('/'), Key::End, Key::Backspace],
        );
        typed(&mut e, &mut io, "b");
        let f = e.render();
        assert!(row_text(&f, 22).starts_with("File name to write: /b"));
        assert_eq!(f.cursor, (22, 22));
        keys(&mut e, &mut io, &[Key::Escape]);
        assert_eq!(e.message(), Some("Cancelled"));
        assert!(io.files.is_empty());
    }
}

//! The text being edited: a list of lines (never empty) and positions in it.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// A position: line index and byte offset inside the line (always on a
/// character boundary).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub byte: usize,
}

impl Pos {
    pub fn new(line: usize, byte: usize) -> Self {
        Self { line, byte }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// The file is not valid UTF-8 (it is opened read-only).
    NotUtf8,
}

/// Screen columns a character takes when it starts at column `col`:
/// tabs reach the next multiple of 8, control characters are shown as
/// `^X` (two columns), everything else takes one column.
pub fn char_width(c: char, col: usize) -> usize {
    if c == '\t' {
        8 - col % 8
    } else if (c as u32) < 0x20 || c == '\x7f' {
        2
    } else {
        1
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buffer {
    lines: Vec<String>,
    /// Lines were separated by CRLF in the file (kept when saving).
    pub crlf: bool,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            crlf: false,
        }
    }

    /// Text with '\n' between lines. A final newline gives a last empty line.
    pub fn from_text(text: &str) -> Self {
        Self {
            lines: text.split('\n').map(String::from).collect(),
            crlf: false,
        }
    }

    /// Loads file contents. CRLF line ends are recognised when every '\n'
    /// follows a '\r'. Invalid UTF-8 gives the text with U+FFFD in place
    /// of bad bytes and [`LoadError::NotUtf8`].
    pub fn load(bytes: &[u8]) -> (Self, Option<LoadError>) {
        let (text, err) = match core::str::from_utf8(bytes) {
            Ok(s) => (String::from(s), None),
            Err(_) => (
                String::from_utf8_lossy(bytes).into_owned(),
                Some(LoadError::NotUtf8),
            ),
        };
        let newlines = text.matches('\n').count();
        let crlf = newlines > 0 && text.matches("\r\n").count() == newlines;
        let mut b = if crlf {
            Self::from_text(&text.replace("\r\n", "\n"))
        } else {
            Self::from_text(&text)
        };
        b.crlf = crlf;
        (b, err)
    }

    /// File contents to save.
    pub fn to_bytes(&self) -> Vec<u8> {
        let sep = if self.crlf { "\r\n" } else { "\n" };
        self.lines.join(sep).into_bytes()
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, i: usize) -> &str {
        self.lines.get(i).map_or("", |s| s.as_str())
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Clamps a position into the buffer and onto a character boundary.
    pub fn clamp(&self, p: Pos) -> Pos {
        let line = p.line.min(self.lines.len() - 1);
        let s = &self.lines[line];
        let mut byte = p.byte.min(s.len());
        while !s.is_char_boundary(byte) {
            byte -= 1;
        }
        Pos { line, byte }
    }

    /// Inserts text (which may contain '\n') and returns the position
    /// just after it.
    pub fn insert(&mut self, at: Pos, text: &str) -> Pos {
        let at = self.clamp(at);
        let tail = self.lines[at.line].split_off(at.byte);
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or("");
        self.lines[at.line].push_str(first);
        let mut line = at.line;
        for part in parts {
            line += 1;
            self.lines.insert(line, String::from(part));
        }
        let end = Pos::new(line, self.lines[line].len());
        self.lines[line].push_str(&tail);
        end
    }

    /// Removes the text between two positions (in any order) and returns it.
    pub fn remove(&mut self, a: Pos, b: Pos) -> String {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let a = self.clamp(a);
        let b = self.clamp(b);
        if a.line == b.line {
            return self.lines[a.line].drain(a.byte..b.byte).collect();
        }
        let mut out: String = self.lines[a.line].drain(a.byte..).collect();
        for _ in a.line + 1..b.line {
            out.push('\n');
            out.push_str(&self.lines.remove(a.line + 1));
        }
        let last = self.lines.remove(a.line + 1);
        out.push('\n');
        out.push_str(&last[..b.byte]);
        self.lines[a.line].push_str(&last[b.byte..]);
        out
    }

    /// Text between two positions (in any order).
    pub fn slice(&self, a: Pos, b: Pos) -> String {
        let mut copy = self.clone();
        copy.remove(a, b)
    }

    /// Removes line `i` and returns it. The buffer keeps at least one line.
    pub fn remove_line(&mut self, i: usize) -> String {
        if self.lines.len() == 1 {
            return core::mem::take(&mut self.lines[0]);
        }
        self.lines.remove(i)
    }

    /// Position of the character before `p` (the end of the previous line
    /// at the start of a line), or `None` at the very start.
    pub fn prev(&self, p: Pos) -> Option<Pos> {
        if p.byte > 0 {
            let s = &self.lines[p.line][..p.byte];
            let c = s.chars().next_back()?;
            Some(Pos::new(p.line, p.byte - c.len_utf8()))
        } else if p.line > 0 {
            Some(Pos::new(p.line - 1, self.lines[p.line - 1].len()))
        } else {
            None
        }
    }

    /// Position after the character at `p`, or `None` at the very end.
    pub fn next(&self, p: Pos) -> Option<Pos> {
        let s = &self.lines[p.line];
        if p.byte < s.len() {
            let c = s[p.byte..].chars().next()?;
            Some(Pos::new(p.line, p.byte + c.len_utf8()))
        } else if p.line + 1 < self.lines.len() {
            Some(Pos::new(p.line + 1, 0))
        } else {
            None
        }
    }

    /// Screen column of byte offset `byte` in line `line`.
    pub fn column(&self, line: usize, byte: usize) -> usize {
        let mut col = 0;
        for c in self.line(line)[..byte].chars() {
            col += char_width(c, col);
        }
        col
    }

    /// Byte offset of the last character starting at or before column `col`.
    pub fn byte_at_column(&self, line: usize, col: usize) -> usize {
        let mut c_col = 0;
        for (i, c) in self.line(line).char_indices() {
            let w = char_width(c, c_col);
            if c_col + w > col {
                return i;
            }
            c_col += w;
        }
        self.line(line).len()
    }

    /// Finds `needle` (case-insensitive, one line) starting at `from`
    /// and going forward, wrapping around to the start. Returns the
    /// position of the match and whether the search wrapped.
    pub fn find(&self, needle: &str, from: Pos) -> Option<(Pos, bool)> {
        if needle.is_empty() || needle.contains('\n') {
            return None;
        }
        let needle: Vec<char> = needle.chars().collect();
        let n = self.lines.len();
        for k in 0..=n {
            let line = (from.line + k) % n;
            let wrapped = from.line + k >= n;
            let s = &self.lines[line];
            for (i, _) in s.char_indices() {
                let after = k > 0 || i >= from.byte;
                let before = k < n || i < from.byte;
                if after && before && matches_at(&s[i..], &needle) {
                    return Some((Pos::new(line, i), wrapped));
                }
            }
        }
        None
    }
}

fn same_ignoring_case(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

fn matches_at(s: &str, needle: &[char]) -> bool {
    let mut it = s.chars();
    needle
        .iter()
        .all(|&n| it.next().is_some_and(|c| same_ignoring_case(c, n)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_and_save_round_trip() {
        for text in ["", "a", "a\n", "a\nb\n", "\n\n", "привет\nмир"] {
            let (b, err) = Buffer::load(text.as_bytes());
            assert_eq!(err, None);
            assert!(!b.crlf);
            assert_eq!(b.to_bytes(), text.as_bytes());
        }
        let (b, _) = Buffer::load(b"one\r\ntwo\r\n");
        assert!(b.crlf);
        assert_eq!(b.line(0), "one");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.to_bytes(), b"one\r\ntwo\r\n");
        // Mixed line ends stay as they are.
        let (b, _) = Buffer::load(b"one\r\ntwo\n");
        assert!(!b.crlf);
        assert_eq!(b.to_bytes(), b"one\r\ntwo\n");
    }

    #[test]
    fn invalid_utf8_is_reported() {
        let (b, err) = Buffer::load(b"ok\xff\n");
        assert_eq!(err, Some(LoadError::NotUtf8));
        assert_eq!(b.line(0), "ok\u{fffd}");
    }

    #[test]
    fn insert_and_remove() {
        let mut b = Buffer::from_text("hello world");
        let end = b.insert(Pos::new(0, 5), ",\nnew");
        assert_eq!(b.text(), "hello,\nnew world");
        assert_eq!(end, Pos::new(1, 3));
        let cut = b.remove(Pos::new(0, 5), Pos::new(1, 3));
        assert_eq!(cut, ",\nnew");
        assert_eq!(b.text(), "hello world");
        let mut b = Buffer::from_text("a\nb\nc\nd");
        assert_eq!(b.remove(Pos::new(2, 1), Pos::new(0, 1)), "\nb\nc");
        assert_eq!(b.text(), "a\nd");
        assert_eq!(b.slice(Pos::new(0, 0), Pos::new(1, 1)), "a\nd");
        assert_eq!(b.text(), "a\nd");
    }

    #[test]
    fn multibyte_positions() {
        let b = Buffer::from_text("мир\tx");
        assert_eq!(b.next(Pos::new(0, 0)), Some(Pos::new(0, 2)));
        assert_eq!(b.prev(Pos::new(0, 6)), Some(Pos::new(0, 4)));
        assert_eq!(b.clamp(Pos::new(0, 3)), Pos::new(0, 2));
        assert_eq!(b.column(0, 6), 3);
        assert_eq!(b.column(0, 7), 8); // after the tab
        assert_eq!(b.byte_at_column(0, 5), 6); // inside the tab
        assert_eq!(b.byte_at_column(0, 8), 7);
        assert_eq!(b.byte_at_column(0, 99), 8);
    }

    #[test]
    fn remove_line_keeps_one_line() {
        let mut b = Buffer::from_text("only");
        assert_eq!(b.remove_line(0), "only");
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line(0), "");
    }

    #[test]
    fn find_is_case_insensitive_and_wraps() {
        let b = Buffer::from_text("Alpha beta\nПРИВЕТ мир\nbeta");
        assert_eq!(
            b.find("привет", Pos::new(0, 0)),
            Some((Pos::new(1, 0), false))
        );
        assert_eq!(
            b.find("BETA", Pos::new(0, 7)),
            Some((Pos::new(2, 0), false))
        );
        assert_eq!(
            b.find("alpha", Pos::new(1, 0)),
            Some((Pos::new(0, 0), true))
        );
        // The match at the start position itself is found last.
        assert_eq!(
            b.find("beta", Pos::new(2, 0)),
            Some((Pos::new(2, 0), false))
        );
        assert_eq!(b.find("gamma", Pos::new(0, 0)), None);
        assert_eq!(b.find("", Pos::new(0, 0)), None);
    }
}

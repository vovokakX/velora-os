//! Screen frames and their ANSI output. Only rows that differ from the
//! previous frame are sent again, which keeps typing fast on the
//! framebuffer console and on the serial line.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    /// Reverse video.
    pub rev: bool,
}

impl Cell {
    pub fn blank() -> Self {
        Self {
            ch: ' ',
            rev: false,
        }
    }

    pub fn plain(ch: char) -> Self {
        Self { ch, rev: false }
    }
}

/// What the screen should show: rows of cells (shorter rows end with
/// blanks) and the cursor as (row, column), both from 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub rows: Vec<Vec<Cell>>,
    pub cursor: (usize, usize),
    pub cols: usize,
}

/// Remembers what is on the terminal and produces the output that turns
/// it into a new frame.
#[derive(Default)]
pub struct Screen {
    shown: Option<Vec<Vec<Cell>>>,
}

fn padded(row: &[Cell], cols: usize) -> Vec<Cell> {
    let mut r: Vec<Cell> = row.iter().copied().take(cols).collect();
    r.resize(cols, Cell::blank());
    r
}

impl Screen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the terminal contents: the next frame is drawn in full.
    pub fn invalidate(&mut self) {
        self.shown = None;
    }

    /// Output for a frame (hides the cursor while drawing).
    pub fn draw(&mut self, frame: &Frame) -> String {
        let mut out = String::from("\x1b[?25l");
        let rows: Vec<Vec<Cell>> = frame.rows.iter().map(|r| padded(r, frame.cols)).collect();
        let full = match &self.shown {
            Some(old) => {
                old.len() != rows.len() || old.first().map(|r| r.len()) != Some(frame.cols)
            }
            None => true,
        };
        if full {
            out.push_str("\x1b[0m\x1b[2J");
        }
        for (i, row) in rows.iter().enumerate() {
            let same = !full && self.shown.as_ref().is_some_and(|old| old[i] == *row);
            if same {
                continue;
            }
            out.push_str(&format!("\x1b[{};1H", i + 1));
            // Trailing plain blanks are erased instead of written.
            let end = row
                .iter()
                .rposition(|c| *c != Cell::blank())
                .map_or(0, |p| p + 1);
            let mut rev = false;
            for c in &row[..end] {
                if c.rev != rev {
                    out.push_str(if c.rev { "\x1b[7m" } else { "\x1b[27m" });
                    rev = c.rev;
                }
                out.push(c.ch);
            }
            if rev {
                out.push_str("\x1b[27m");
            }
            if end < row.len() {
                out.push_str("\x1b[K");
            }
        }
        out.push_str(&format!(
            "\x1b[{};{}H\x1b[?25h",
            frame.cursor.0 + 1,
            frame.cursor.1 + 1
        ));
        self.shown = Some(rows);
        out
    }

    /// Output that clears the screen when the editor exits.
    pub fn leave(&mut self) -> String {
        self.shown = None;
        String::from("\x1b[0m\x1b[2J\x1b[H\x1b[?25h")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(rows: &[&str], cursor: (usize, usize)) -> Frame {
        Frame {
            rows: rows
                .iter()
                .map(|r| r.chars().map(Cell::plain).collect())
                .collect(),
            cursor,
            cols: 10,
        }
    }

    #[test]
    fn first_frame_is_drawn_in_full() {
        let mut s = Screen::new();
        let out = s.draw(&frame(&["ab", "", "мир"], (2, 3)));
        assert_eq!(
            out,
            "\x1b[?25l\x1b[0m\x1b[2J\x1b[1;1Hab\x1b[K\x1b[2;1H\x1b[K\x1b[3;1Hмир\x1b[K\x1b[3;4H\x1b[?25h"
        );
    }

    #[test]
    fn only_changed_rows_are_sent() {
        let mut s = Screen::new();
        s.draw(&frame(&["ab", "cd", "ef"], (0, 0)));
        let out = s.draw(&frame(&["ab", "cX", "ef"], (1, 2)));
        assert_eq!(out, "\x1b[?25l\x1b[2;1HcX\x1b[K\x1b[2;3H\x1b[?25h");
        let out = s.draw(&frame(&["ab", "cX", "ef"], (0, 1)));
        assert_eq!(out, "\x1b[?25l\x1b[1;2H\x1b[?25h");
        s.invalidate();
        assert!(
            s.draw(&frame(&["ab", "cX", "ef"], (0, 1)))
                .contains("\x1b[2J")
        );
    }

    #[test]
    fn reverse_video_and_full_rows() {
        let mut s = Screen::new();
        let mut f = frame(&[""], (0, 0));
        f.rows[0] = (0..10)
            .map(|i| Cell {
                ch: 'x',
                rev: i < 3,
            })
            .collect();
        let out = s.draw(&f);
        assert!(
            out.contains("\x1b[1;1H\x1b[7mxxx\x1b[27mxxxxxxx\x1b[1;1H"),
            "{:?}",
            out
        );
        assert_eq!(s.leave(), "\x1b[0m\x1b[2J\x1b[H\x1b[?25h");
    }
}

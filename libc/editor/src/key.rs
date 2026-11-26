//! Keys from raw terminal bytes (the console's raw mode, see the kernel's
//! `tty.rs`): UTF-8 characters, control codes and ANSI escape sequences.

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    /// Ctrl + a letter, given as the lowercase letter (`Ctrl('x')`).
    /// Ctrl+H, Ctrl+I, Ctrl+J and Ctrl+M arrive as Backspace, Tab and
    /// Enter, as on any terminal.
    Ctrl(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Escape,
    Unknown,
}

/// Decodes raw terminal bytes. A sequence cut at the end of one read is
/// completed by the next, except a lone Escape, which is the Escape key
/// (the console delivers a whole escape sequence in one read).
#[derive(Default)]
pub struct KeyDecoder {
    pending: Vec<u8>,
}

fn utf8_len(lead: u8) -> usize {
    match lead {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

fn control(b: u8) -> Key {
    match b {
        b'\n' | b'\r' => Key::Enter,
        b'\t' => Key::Tab,
        0x7f | 0x08 => Key::Backspace,
        0x1b => Key::Escape,
        0x01..=0x1a => Key::Ctrl((b'a' + b - 1) as char),
        _ => Key::Unknown,
    }
}

fn csi(body: &[u8]) -> Key {
    match body {
        b"A" => Key::Up,
        b"B" => Key::Down,
        b"C" => Key::Right,
        b"D" => Key::Left,
        b"H" | b"1~" | b"7~" => Key::Home,
        b"F" | b"4~" | b"8~" => Key::End,
        b"3~" => Key::Delete,
        b"5~" => Key::PageUp,
        b"6~" => Key::PageDown,
        _ => Key::Unknown,
    }
}

impl KeyDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes one read's worth of bytes.
    pub fn decode(&mut self, bytes: &[u8]) -> Vec<Key> {
        self.pending.extend_from_slice(bytes);
        let mut keys = Vec::new();
        let mut i = 0;
        let p = &self.pending;
        while i < p.len() {
            let b = p[i];
            if b == 0x1b {
                if i + 1 >= p.len() {
                    // A lone Escape at the end of the read.
                    keys.push(Key::Escape);
                    i += 1;
                    continue;
                }
                if p[i + 1] != b'[' {
                    keys.push(Key::Escape);
                    i += 1;
                    continue;
                }
                // CSI: parameters, then a final byte 0x40..=0x7e.
                let mut j = i + 2;
                while j < p.len() && !(0x40..=0x7e).contains(&p[j]) && j - i < 16 {
                    j += 1;
                }
                if j >= p.len() {
                    break; // incomplete: wait for the rest
                }
                keys.push(csi(&p[i + 2..=j]));
                i = j + 1;
            } else if b < 0x20 || b == 0x7f {
                keys.push(control(b));
                i += 1;
            } else if b < 0x80 {
                keys.push(Key::Char(b as char));
                i += 1;
            } else {
                let n = utf8_len(b);
                if i + n > p.len() {
                    break; // incomplete character
                }
                match core::str::from_utf8(&p[i..i + n]) {
                    Ok(s) => {
                        keys.push(Key::Char(s.chars().next().unwrap_or('?')));
                        i += n;
                    }
                    Err(_) => {
                        keys.push(Key::Unknown);
                        i += 1;
                    }
                }
            }
        }
        self.pending.drain(..i);
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_control() {
        let mut d = KeyDecoder::new();
        assert_eq!(
            d.decode(b"a\x18\x0f\r\t\x7f"),
            vec![
                Key::Char('a'),
                Key::Ctrl('x'),
                Key::Ctrl('o'),
                Key::Enter,
                Key::Tab,
                Key::Backspace
            ]
        );
    }

    #[test]
    fn escape_sequences() {
        let mut d = KeyDecoder::new();
        assert_eq!(
            d.decode(b"\x1b[A\x1b[3~\x1b[5~\x1b[6~\x1b[H\x1b[F\x1b[2~"),
            vec![
                Key::Up,
                Key::Delete,
                Key::PageUp,
                Key::PageDown,
                Key::Home,
                Key::End,
                Key::Unknown
            ]
        );
    }

    #[test]
    fn lone_escape_and_split_input() {
        let mut d = KeyDecoder::new();
        assert_eq!(d.decode(b"\x1b"), vec![Key::Escape]);
        assert_eq!(d.decode(b"\x1b["), vec![]);
        assert_eq!(d.decode(b"B"), vec![Key::Down]);
        let p = "п".as_bytes();
        assert_eq!(d.decode(&p[..1]), vec![]);
        assert_eq!(d.decode(&p[1..]), vec![Key::Char('п')]);
        assert_eq!(d.decode(b"\x1bx"), vec![Key::Escape, Key::Char('x')]);
    }

    #[test]
    fn cyrillic_text() {
        let mut d = KeyDecoder::new();
        let keys = d.decode("привет".as_bytes());
        let s: String = keys
            .iter()
            .map(|k| match k {
                Key::Char(c) => *c,
                _ => '!',
            })
            .collect();
        assert_eq!(s, "привет");
    }
}

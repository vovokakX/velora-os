//! PS/2 scancode set 1 decoder (what the 8042 controller produces with
//! translation enabled, which is the default on PCs and in QEMU).

use crate::KeyCode::{self, *};

/// Make codes 0x00..=0x58 without prefix.
const BASE: [Option<KeyCode>; 0x59] = {
    let mut t: [Option<KeyCode>; 0x59] = [None; 0x59];
    t[0x01] = Some(Escape);
    t[0x02] = Some(Digit1);
    t[0x03] = Some(Digit2);
    t[0x04] = Some(Digit3);
    t[0x05] = Some(Digit4);
    t[0x06] = Some(Digit5);
    t[0x07] = Some(Digit6);
    t[0x08] = Some(Digit7);
    t[0x09] = Some(Digit8);
    t[0x0a] = Some(Digit9);
    t[0x0b] = Some(Digit0);
    t[0x0c] = Some(Minus);
    t[0x0d] = Some(Equal);
    t[0x0e] = Some(Backspace);
    t[0x0f] = Some(Tab);
    t[0x10] = Some(Q);
    t[0x11] = Some(W);
    t[0x12] = Some(E);
    t[0x13] = Some(R);
    t[0x14] = Some(T);
    t[0x15] = Some(Y);
    t[0x16] = Some(U);
    t[0x17] = Some(I);
    t[0x18] = Some(O);
    t[0x19] = Some(P);
    t[0x1a] = Some(LeftBracket);
    t[0x1b] = Some(RightBracket);
    t[0x1c] = Some(Enter);
    t[0x1d] = Some(LeftCtrl);
    t[0x1e] = Some(A);
    t[0x1f] = Some(S);
    t[0x20] = Some(D);
    t[0x21] = Some(F);
    t[0x22] = Some(G);
    t[0x23] = Some(H);
    t[0x24] = Some(J);
    t[0x25] = Some(K);
    t[0x26] = Some(L);
    t[0x27] = Some(Semicolon);
    t[0x28] = Some(Quote);
    t[0x29] = Some(Backquote);
    t[0x2a] = Some(LeftShift);
    t[0x2b] = Some(Backslash);
    t[0x2c] = Some(Z);
    t[0x2d] = Some(X);
    t[0x2e] = Some(C);
    t[0x2f] = Some(V);
    t[0x30] = Some(B);
    t[0x31] = Some(N);
    t[0x32] = Some(M);
    t[0x33] = Some(Comma);
    t[0x34] = Some(Period);
    t[0x35] = Some(Slash);
    t[0x36] = Some(RightShift);
    t[0x37] = Some(KeypadMultiply);
    t[0x38] = Some(LeftAlt);
    t[0x39] = Some(Space);
    t[0x3a] = Some(CapsLock);
    t[0x3b] = Some(F1);
    t[0x3c] = Some(F2);
    t[0x3d] = Some(F3);
    t[0x3e] = Some(F4);
    t[0x3f] = Some(F5);
    t[0x40] = Some(F6);
    t[0x41] = Some(F7);
    t[0x42] = Some(F8);
    t[0x43] = Some(F9);
    t[0x44] = Some(F10);
    t[0x45] = Some(NumLock);
    t[0x46] = Some(ScrollLock);
    t[0x47] = Some(Keypad7);
    t[0x48] = Some(Keypad8);
    t[0x49] = Some(Keypad9);
    t[0x4a] = Some(KeypadMinus);
    t[0x4b] = Some(Keypad4);
    t[0x4c] = Some(Keypad5);
    t[0x4d] = Some(Keypad6);
    t[0x4e] = Some(KeypadPlus);
    t[0x4f] = Some(Keypad1);
    t[0x50] = Some(Keypad2);
    t[0x51] = Some(Keypad3);
    t[0x52] = Some(Keypad0);
    t[0x53] = Some(KeypadPeriod);
    t[0x56] = Some(NonUsBackslash);
    t[0x57] = Some(F11);
    t[0x58] = Some(F12);
    t
};

/// Make codes after an 0xE0 prefix.
fn extended(code: u8) -> Option<KeyCode> {
    Some(match code {
        0x1c => KeypadEnter,
        0x1d => RightCtrl,
        0x35 => KeypadDivide,
        0x37 => PrintScreen,
        0x38 => RightAlt,
        0x47 => Home,
        0x48 => Up,
        0x49 => PageUp,
        0x4b => Left,
        0x4d => Right,
        0x4f => End,
        0x50 => Down,
        0x51 => PageDown,
        0x52 => Insert,
        0x53 => Delete,
        0x5b => LeftMeta,
        0x5c => RightMeta,
        0x5d => Menu,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Normal,
    Extended,
    /// Inside the 6-byte Pause sequence (E1 1D 45 E1 9D C5); counts bytes left.
    Pause(u8),
}

pub struct Decoder {
    state: State,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub const fn new() -> Self {
        Self {
            state: State::Normal,
        }
    }

    /// Returns (key, pressed) once a full scancode has arrived.
    pub fn feed(&mut self, byte: u8) -> Option<(KeyCode, bool)> {
        match self.state {
            State::Pause(left) => {
                self.state = if left > 1 {
                    State::Pause(left - 1)
                } else {
                    State::Normal
                };
                if left == 1 { Some((Pause, true)) } else { None }
            }
            State::Extended => {
                self.state = State::Normal;
                let pressed = byte & 0x80 == 0;
                let code = byte & 0x7f;
                // 0xE0 0x2A / 0xE0 0x36 are fake shifts around PrintScreen etc.
                if code == 0x2a || code == 0x36 {
                    return None;
                }
                extended(code).map(|k| (k, pressed))
            }
            State::Normal => match byte {
                0xe0 => {
                    self.state = State::Extended;
                    None
                }
                0xe1 => {
                    self.state = State::Pause(5);
                    None
                }
                // Controller/keyboard replies (ACK, resend, self-test, errors).
                0xfa | 0xfe | 0xee | 0xff => None,
                _ => {
                    let pressed = byte & 0x80 == 0;
                    let code = (byte & 0x7f) as usize;
                    BASE.get(code).copied().flatten().map(|k| (k, pressed))
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_and_break() {
        let mut d = Decoder::new();
        assert_eq!(d.feed(0x1e), Some((A, true)));
        assert_eq!(d.feed(0x9e), Some((A, false)));
        assert_eq!(d.feed(0x58), Some((F12, true)));
        assert_eq!(d.feed(0x7f), None);
    }

    #[test]
    fn replies_are_ignored() {
        let mut d = Decoder::new();
        assert_eq!(d.feed(0xfa), None);
        assert_eq!(d.feed(0xfe), None);
        // 0xAA is also left-shift release, which must still work.
        assert_eq!(d.feed(0xaa), Some((LeftShift, false)));
    }

    #[test]
    fn fake_shift_and_pause() {
        let mut d = Decoder::new();
        assert_eq!(d.feed(0xe0), None);
        assert_eq!(d.feed(0x2a), None);
        assert_eq!(d.feed(0xe0), None);
        assert_eq!(d.feed(0x37), Some((PrintScreen, true)));
        for b in [0xe1, 0x1d, 0x45, 0xe1, 0x9d] {
            assert_eq!(d.feed(b), None);
        }
        assert_eq!(d.feed(0xc5), Some((Pause, true)));
        assert_eq!(d.feed(0x1e), Some((A, true)));
    }
}

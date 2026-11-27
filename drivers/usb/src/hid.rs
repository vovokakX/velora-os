//! HID boot protocol: keyboard and mouse reports.

use alloc::vec::Vec;

pub const REQ_SET_IDLE: u8 = 0x0a;
pub const REQ_SET_PROTOCOL: u8 = 0x0b;
/// Interface subclass 1 = boot interface; protocol 1 keyboard, 2 mouse.
pub const SUBCLASS_BOOT: u8 = 1;
pub const PROTOCOL_KEYBOARD: u8 = 1;
pub const PROTOCOL_MOUSE: u8 = 2;

/// PS/2 scancode set 1 make code for a HID keyboard usage: (extended,
/// code). The break code is `code | 0x80`, after the same 0xE0 prefix.
pub fn usage_to_set1(usage: u8) -> Option<(bool, u8)> {
    const LETTERS: [u8; 26] = [
        0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
        0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
    ];
    const F_KEYS: [u8; 12] = [
        0x3b, 0x3c, 0x3d, 0x3e, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x57, 0x58,
    ];
    const KEYPAD: [u8; 9] = [0x4f, 0x50, 0x51, 0x4b, 0x4c, 0x4d, 0x47, 0x48, 0x49];
    let plain = |c| Some((false, c));
    let ext = |c| Some((true, c));
    match usage {
        0x04..=0x1d => plain(LETTERS[(usage - 0x04) as usize]),
        0x1e..=0x26 => plain(usage - 0x1e + 0x02), // 1..9
        0x27 => plain(0x0b),                       // 0
        0x28 => plain(0x1c),                       // Enter
        0x29 => plain(0x01),                       // Escape
        0x2a => plain(0x0e),                       // Backspace
        0x2b => plain(0x0f),                       // Tab
        0x2c => plain(0x39),                       // Space
        0x2d => plain(0x0c),
        0x2e => plain(0x0d),
        0x2f => plain(0x1a),
        0x30 => plain(0x1b),
        0x31 | 0x32 => plain(0x2b),
        0x33 => plain(0x27),
        0x34 => plain(0x28),
        0x35 => plain(0x29),
        0x36 => plain(0x33),
        0x37 => plain(0x34),
        0x38 => plain(0x35),
        0x39 => plain(0x3a), // Caps Lock
        0x3a..=0x45 => plain(F_KEYS[(usage - 0x3a) as usize]),
        0x46 => ext(0x37),   // Print Screen
        0x47 => plain(0x46), // Scroll Lock
        0x49 => ext(0x52),   // Insert
        0x4a => ext(0x47),   // Home
        0x4b => ext(0x49),   // Page Up
        0x4c => ext(0x53),   // Delete
        0x4d => ext(0x4f),   // End
        0x4e => ext(0x51),   // Page Down
        0x4f => ext(0x4d),   // Right
        0x50 => ext(0x4b),   // Left
        0x51 => ext(0x50),   // Down
        0x52 => ext(0x48),   // Up
        0x53 => plain(0x45), // Num Lock
        0x54 => ext(0x35),
        0x55 => plain(0x37),
        0x56 => plain(0x4a),
        0x57 => plain(0x4e),
        0x58 => ext(0x1c), // keypad Enter
        0x59..=0x61 => plain(KEYPAD[(usage - 0x59) as usize]),
        0x62 => plain(0x52),
        0x63 => plain(0x53),
        0x64 => plain(0x56),
        0x65 => ext(0x5d), // Menu
        _ => None,
    }
}

/// Modifier bits of byte 0, in order, as set 1 codes.
const MODIFIERS: [(bool, u8); 8] = [
    (false, 0x1d), // left Ctrl
    (false, 0x2a), // left Shift
    (false, 0x38), // left Alt
    (true, 0x5b),  // left GUI
    (true, 0x1d),  // right Ctrl
    (false, 0x36), // right Shift
    (true, 0x38),  // right Alt
    (true, 0x5c),  // right GUI
];

fn emit(out: &mut Vec<u8>, (extended, code): (bool, u8), pressed: bool) {
    if extended {
        out.push(0xe0);
    }
    out.push(if pressed { code } else { code | 0x80 });
}

/// Turns 8-byte boot keyboard reports into scancode set 1 bytes by
/// comparing each report with the previous one.
#[derive(Default)]
pub struct Keyboard {
    modifiers: u8,
    keys: [u8; 6],
}

impl Keyboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scancode bytes for the changes since the last report: releases
    /// first, then presses. A rollover report (too many keys) is ignored.
    pub fn report(&mut self, r: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        if r.len() < 8 || r[2..8].iter().all(|&k| k == 0x01) {
            return out;
        }
        let modifiers = r[0];
        let mut keys = [0u8; 6];
        keys.copy_from_slice(&r[2..8]);
        for (bit, &m) in MODIFIERS.iter().enumerate() {
            if self.modifiers & (1 << bit) != 0 && modifiers & (1 << bit) == 0 {
                emit(&mut out, m, false);
            }
        }
        for &k in self.keys.iter().filter(|&&k| k > 3) {
            if !keys.contains(&k)
                && let Some(c) = usage_to_set1(k)
            {
                emit(&mut out, c, false);
            }
        }
        for (bit, &m) in MODIFIERS.iter().enumerate() {
            if self.modifiers & (1 << bit) == 0 && modifiers & (1 << bit) != 0 {
                emit(&mut out, m, true);
            }
        }
        for &k in keys.iter().filter(|&&k| k > 3) {
            if !self.keys.contains(&k)
                && let Some(c) = usage_to_set1(k)
            {
                emit(&mut out, c, true);
            }
        }
        self.modifiers = modifiers;
        self.keys = keys;
        out
    }
}

/// A boot protocol mouse report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseReport {
    /// Bit 0 left, 1 right, 2 middle.
    pub buttons: u8,
    pub dx: i8,
    pub dy: i8,
    pub wheel: i8,
}

pub fn parse_mouse(r: &[u8]) -> Option<MouseReport> {
    if r.len() < 3 {
        return None;
    }
    Some(MouseReport {
        buttons: r[0] & 7,
        dx: r[1] as i8,
        dy: r[2] as i8,
        wheel: r.get(3).map_or(0, |&w| w as i8),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_extended_keys() {
        assert_eq!(usage_to_set1(0x04), Some((false, 0x1e))); // a
        assert_eq!(usage_to_set1(0x1d), Some((false, 0x2c))); // z
        assert_eq!(usage_to_set1(0x1e), Some((false, 0x02))); // 1
        assert_eq!(usage_to_set1(0x27), Some((false, 0x0b))); // 0
        assert_eq!(usage_to_set1(0x52), Some((true, 0x48))); // Up
        assert_eq!(usage_to_set1(0x45), Some((false, 0x58))); // F12
        assert_eq!(usage_to_set1(0xe0), None);
    }

    #[test]
    fn press_and_release() {
        let mut k = Keyboard::new();
        // Shift + a
        assert_eq!(k.report(&[0x02, 0, 0x04, 0, 0, 0, 0, 0]), vec![0x2a, 0x1e]);
        // a released, b pressed, shift still held
        assert_eq!(k.report(&[0x02, 0, 0x05, 0, 0, 0, 0, 0]), vec![0x9e, 0x30]);
        // everything released
        assert_eq!(k.report(&[0, 0, 0, 0, 0, 0, 0, 0]), vec![0xaa, 0xb0]);
        // An extended key and the right Ctrl.
        assert_eq!(
            k.report(&[0x10, 0, 0x50, 0, 0, 0, 0, 0]),
            vec![0xe0, 0x1d, 0xe0, 0x4b]
        );
        assert_eq!(
            k.report(&[0, 0, 0, 0, 0, 0, 0, 0]),
            vec![0xe0, 0x9d, 0xe0, 0xcb]
        );
        // Rollover reports and short reports change nothing.
        assert!(k.report(&[0, 0, 1, 1, 1, 1, 1, 1]).is_empty());
        assert!(k.report(&[0, 0, 4]).is_empty());
    }

    #[test]
    fn mouse_reports() {
        assert_eq!(
            parse_mouse(&[0x01, 0x05, 0xfb]),
            Some(MouseReport {
                buttons: 1,
                dx: 5,
                dy: -5,
                wheel: 0
            })
        );
        assert_eq!(parse_mouse(&[0x0f, 0, 0, 0xff]).unwrap().wheel, -1);
        assert_eq!(parse_mouse(&[0x0f, 0, 0, 0xff]).unwrap().buttons, 7);
        assert_eq!(parse_mouse(&[1, 2]), None);
    }
}

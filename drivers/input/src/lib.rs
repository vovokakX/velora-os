//! Keyboard input handling that does not touch hardware:
//! scancode decoding, modifier state, keyboard layouts.
//!
//! The PS/2 driver in the kernel feeds raw bytes into [`Keyboard::feed`]
//! and gets [`KeyEvent`]s back. A future USB HID driver will produce the
//! same [`KeyCode`]s, so layouts and the rest of the system stay the same.

#![cfg_attr(not(test), no_std)]

pub mod keycode;
pub mod layout;
pub mod ps2mouse;
pub mod set1;

pub use keycode::KeyCode;
pub use layout::Layout;

/// Modifier and lock state at the time of an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub scroll_lock: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: KeyCode,
    pub pressed: bool,
    /// Text produced by the key press, if any (never on release).
    pub ch: Option<char>,
    pub mods: Modifiers,
    pub layout: Layout,
}

/// What the driver has to do after feeding a byte.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// Lock state changed: update the keyboard LEDs.
    pub update_leds: bool,
    /// The active layout changed.
    pub layout_changed: bool,
}

/// Keyboard state machine: decoder + modifiers + layout.
pub struct Keyboard {
    decoder: set1::Decoder,
    left_shift: bool,
    right_shift: bool,
    left_ctrl: bool,
    right_ctrl: bool,
    left_alt: bool,
    right_alt: bool,
    left_meta: bool,
    right_meta: bool,
    caps_lock: bool,
    num_lock: bool,
    scroll_lock: bool,
    layouts: [Layout; 2],
    active: usize,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new(Layout::Us, Layout::Russian)
    }
}

impl Keyboard {
    /// `primary` is active at start; Alt+Shift switches to `secondary`.
    pub const fn new(primary: Layout, secondary: Layout) -> Self {
        Self {
            decoder: set1::Decoder::new(),
            left_shift: false,
            right_shift: false,
            left_ctrl: false,
            right_ctrl: false,
            left_alt: false,
            right_alt: false,
            left_meta: false,
            right_meta: false,
            caps_lock: false,
            num_lock: false,
            scroll_lock: false,
            layouts: [primary, secondary],
            active: 0,
        }
    }

    pub fn layout(&self) -> Layout {
        self.layouts[self.active]
    }

    pub fn modifiers(&self) -> Modifiers {
        Modifiers {
            shift: self.left_shift || self.right_shift,
            ctrl: self.left_ctrl || self.right_ctrl,
            alt: self.left_alt || self.right_alt,
            meta: self.left_meta || self.right_meta,
            caps_lock: self.caps_lock,
            num_lock: self.num_lock,
            scroll_lock: self.scroll_lock,
        }
    }

    /// LED bits for the PS/2 "set LEDs" command: scroll=1, num=2, caps=4.
    pub fn led_bits(&self) -> u8 {
        (self.scroll_lock as u8) | (self.num_lock as u8) << 1 | (self.caps_lock as u8) << 2
    }

    /// Feeds one byte from the keyboard. Returns an event when a key was
    /// pressed or released, plus side effects for the driver.
    pub fn feed(&mut self, byte: u8) -> (Option<KeyEvent>, Effects) {
        let Some((key, pressed)) = self.decoder.feed(byte) else {
            return (None, Effects::default());
        };
        let mut fx = Effects::default();
        let was = self.modifiers();
        use KeyCode::*;
        match key {
            LeftShift => self.left_shift = pressed,
            RightShift => self.right_shift = pressed,
            LeftCtrl => self.left_ctrl = pressed,
            RightCtrl => self.right_ctrl = pressed,
            LeftAlt => self.left_alt = pressed,
            RightAlt => self.right_alt = pressed,
            LeftMeta => self.left_meta = pressed,
            RightMeta => self.right_meta = pressed,
            CapsLock if pressed => {
                self.caps_lock = !self.caps_lock;
                fx.update_leds = true;
            }
            NumLock if pressed => {
                self.num_lock = !self.num_lock;
                fx.update_leds = true;
            }
            ScrollLock if pressed => {
                self.scroll_lock = !self.scroll_lock;
                fx.update_leds = true;
            }
            _ => {}
        }
        // Alt+Shift switches the layout (pressing the second of the two).
        let shift_now = matches!(key, LeftShift | RightShift) && pressed && was.alt;
        let alt_now = matches!(key, LeftAlt | RightAlt) && pressed && was.shift;
        if shift_now || alt_now {
            self.active ^= 1;
            fx.layout_changed = true;
        }
        let mods = self.modifiers();
        let ch = if pressed {
            self.text_for(key, mods)
        } else {
            None
        };
        (
            Some(KeyEvent {
                key,
                pressed,
                ch,
                mods,
                layout: self.layout(),
            }),
            fx,
        )
    }

    fn text_for(&self, key: KeyCode, mods: Modifiers) -> Option<char> {
        if let Some(c) = keycode::keypad_char(key, mods.num_lock) {
            return Some(c);
        }
        if let Some(c) = keycode::fixed_char(key) {
            return Some(c);
        }
        let (plain, shifted) = self.layout().chars(key)?;
        let upper = if plain.is_alphabetic() {
            mods.shift != mods.caps_lock
        } else {
            mods.shift
        };
        let c = if upper { shifted } else { plain };
        if mods.ctrl && !mods.alt {
            // Ctrl+letter gives the ASCII control code (Ctrl+C = 0x03),
            // based on the physical key so it works in every layout.
            return keycode::control_char(key);
        }
        Some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeyCode::*;

    /// Feeds make/break bytes and collects produced characters.
    fn type_bytes(kb: &mut Keyboard, bytes: &[u8]) -> String {
        bytes
            .iter()
            .filter_map(|&b| kb.feed(b).0)
            .filter_map(|e| e.ch)
            .collect()
    }

    #[test]
    fn plain_and_shifted_letters() {
        let mut kb = Keyboard::default();
        // h, i, shift down, h, shift up, 1, shift+1
        let s = type_bytes(
            &mut kb,
            &[
                0x23, 0xa3, 0x17, 0x97, 0x2a, 0x23, 0xa3, 0xaa, 0x02, 0x82, 0x36, 0x02, 0x82, 0xb6,
            ],
        );
        assert_eq!(s, "hiH1!");
    }

    #[test]
    fn caps_lock_inverts_letters_only_and_requests_leds() {
        let mut kb = Keyboard::default();
        let (_, fx) = kb.feed(0x3a);
        assert!(fx.update_leds);
        kb.feed(0xba);
        assert_eq!(kb.led_bits(), 4);
        assert_eq!(type_bytes(&mut kb, &[0x1e, 0x9e, 0x02, 0x82]), "A1");
        assert_eq!(type_bytes(&mut kb, &[0x2a, 0x1e, 0x9e, 0xaa]), "a");
    }

    #[test]
    fn release_produces_no_text() {
        let mut kb = Keyboard::default();
        let (e, _) = kb.feed(0x9e);
        let e = e.unwrap();
        assert_eq!((e.key, e.pressed, e.ch), (A, false, None));
    }

    #[test]
    fn extended_keys() {
        let mut kb = Keyboard::default();
        assert_eq!(kb.feed(0xe0).0, None);
        let e = kb.feed(0x48).0.unwrap();
        assert_eq!((e.key, e.pressed, e.ch), (Up, true, None));
        kb.feed(0xe0);
        let e = kb.feed(0x9c).0.unwrap();
        assert_eq!((e.key, e.pressed), (KeypadEnter, false));
        kb.feed(0xe0);
        let e = kb.feed(0x1c).0.unwrap();
        assert_eq!(e.ch, Some('\n'));
    }

    #[test]
    fn alt_shift_switches_to_russian() {
        let mut kb = Keyboard::default();
        // Alt down, Shift down -> switch; release both.
        kb.feed(0x38);
        let (_, fx) = kb.feed(0x2a);
        assert!(fx.layout_changed);
        kb.feed(0xaa);
        kb.feed(0xb8);
        assert_eq!(kb.layout(), Layout::Russian);
        // q w e [ ; ' , . /  and shift+q
        let s = type_bytes(
            &mut kb,
            &[
                0x10, 0x11, 0x12, 0x1a, 0x27, 0x28, 0x33, 0x34, 0x35, 0x29, 0x2a, 0x10, 0xaa,
            ],
        );
        assert_eq!(s, "йцухжэбю.ёЙ");
        // Shift then Alt switches back.
        kb.feed(0x2a);
        let (_, fx) = kb.feed(0x38);
        assert!(fx.layout_changed);
        kb.feed(0xb8);
        kb.feed(0xaa);
        assert_eq!(kb.layout(), Layout::Us);
        assert_eq!(type_bytes(&mut kb, &[0x10]), "q");
    }

    #[test]
    fn russian_shifted_digits() {
        let mut kb = Keyboard::new(Layout::Russian, Layout::Us);
        let s = type_bytes(&mut kb, &[0x2a, 0x03, 0x04, 0x05, 0x07, 0x08, 0x35, 0xaa]);
        assert_eq!(s, "\"№;:?,");
    }

    #[test]
    fn control_codes() {
        let mut kb = Keyboard::default();
        assert_eq!(type_bytes(&mut kb, &[0x1d, 0x2e, 0xae, 0x9d]), "\x03");
        // Same physical key in the Russian layout.
        let mut ru = Keyboard::new(Layout::Russian, Layout::Us);
        assert_eq!(type_bytes(&mut ru, &[0x1d, 0x2e, 0xae, 0x9d]), "\x03");
    }

    #[test]
    fn special_keys_text() {
        let mut kb = Keyboard::default();
        assert_eq!(
            type_bytes(&mut kb, &[0x1c, 0x0e, 0x0f, 0x39, 0x01]),
            "\n\x08\t \x1b"
        );
    }

    #[test]
    fn keypad_depends_on_num_lock() {
        let mut kb = Keyboard::default();
        assert_eq!(type_bytes(&mut kb, &[0x47, 0x37, 0x4e]), "*+");
        kb.feed(0x45);
        kb.feed(0xc5);
        assert_eq!(kb.led_bits(), 2);
        assert_eq!(type_bytes(&mut kb, &[0x47, 0x52, 0x53]), "70.");
    }

    #[test]
    fn modifiers_are_tracked() {
        let mut kb = Keyboard::default();
        kb.feed(0xe0);
        kb.feed(0x1d); // right ctrl
        assert!(kb.modifiers().ctrl);
        kb.feed(0x1d); // left ctrl
        kb.feed(0xe0);
        kb.feed(0x9d); // right ctrl up
        assert!(kb.modifiers().ctrl, "left ctrl still held");
        kb.feed(0x9d);
        assert!(!kb.modifiers().ctrl);
    }
}

//! Keyboard layouts: which characters a physical key types.

use crate::KeyCode::{self, *};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// US QWERTY.
    Us,
    /// Russian ЙЦУКЕН (the standard Windows/Linux "Russian" layout).
    Russian,
}

impl Layout {
    pub fn short_name(self) -> &'static str {
        match self {
            Layout::Us => "EN",
            Layout::Russian => "RU",
        }
    }

    /// (unshifted, shifted) characters for `key`, or `None` for keys that
    /// do not type text in this layout.
    pub fn chars(self, key: KeyCode) -> Option<(char, char)> {
        match self {
            Layout::Us => us(key),
            Layout::Russian => russian(key).or_else(|| us_symbols_shared(key)),
        }
    }
}

fn us(key: KeyCode) -> Option<(char, char)> {
    Some(match key {
        Backquote => ('`', '~'),
        Digit1 => ('1', '!'),
        Digit2 => ('2', '@'),
        Digit3 => ('3', '#'),
        Digit4 => ('4', '$'),
        Digit5 => ('5', '%'),
        Digit6 => ('6', '^'),
        Digit7 => ('7', '&'),
        Digit8 => ('8', '*'),
        Digit9 => ('9', '('),
        Digit0 => ('0', ')'),
        Minus => ('-', '_'),
        Equal => ('=', '+'),
        Q => ('q', 'Q'),
        W => ('w', 'W'),
        E => ('e', 'E'),
        R => ('r', 'R'),
        T => ('t', 'T'),
        Y => ('y', 'Y'),
        U => ('u', 'U'),
        I => ('i', 'I'),
        O => ('o', 'O'),
        P => ('p', 'P'),
        LeftBracket => ('[', '{'),
        RightBracket => (']', '}'),
        Backslash => ('\\', '|'),
        A => ('a', 'A'),
        S => ('s', 'S'),
        D => ('d', 'D'),
        F => ('f', 'F'),
        G => ('g', 'G'),
        H => ('h', 'H'),
        J => ('j', 'J'),
        K => ('k', 'K'),
        L => ('l', 'L'),
        Semicolon => (';', ':'),
        Quote => ('\'', '"'),
        NonUsBackslash => ('\\', '|'),
        Z => ('z', 'Z'),
        X => ('x', 'X'),
        C => ('c', 'C'),
        V => ('v', 'V'),
        B => ('b', 'B'),
        N => ('n', 'N'),
        M => ('m', 'M'),
        Comma => (',', '<'),
        Period => ('.', '>'),
        Slash => ('/', '?'),
        _ => return None,
    })
}

/// Keys whose characters are the same as US in the Russian layout.
fn us_symbols_shared(key: KeyCode) -> Option<(char, char)> {
    match key {
        Digit1 | Digit5 | Digit8 | Digit9 | Digit0 | Minus | Equal => us(key),
        _ => None,
    }
}

fn russian(key: KeyCode) -> Option<(char, char)> {
    Some(match key {
        Backquote => ('ё', 'Ё'),
        Digit2 => ('2', '"'),
        Digit3 => ('3', '№'),
        Digit4 => ('4', ';'),
        Digit6 => ('6', ':'),
        Digit7 => ('7', '?'),
        Q => ('й', 'Й'),
        W => ('ц', 'Ц'),
        E => ('у', 'У'),
        R => ('к', 'К'),
        T => ('е', 'Е'),
        Y => ('н', 'Н'),
        U => ('г', 'Г'),
        I => ('ш', 'Ш'),
        O => ('щ', 'Щ'),
        P => ('з', 'З'),
        LeftBracket => ('х', 'Х'),
        RightBracket => ('ъ', 'Ъ'),
        Backslash => ('\\', '/'),
        A => ('ф', 'Ф'),
        S => ('ы', 'Ы'),
        D => ('в', 'В'),
        F => ('а', 'А'),
        G => ('п', 'П'),
        H => ('р', 'Р'),
        J => ('о', 'О'),
        K => ('л', 'Л'),
        L => ('д', 'Д'),
        Semicolon => ('ж', 'Ж'),
        Quote => ('э', 'Э'),
        NonUsBackslash => ('\\', '/'),
        Z => ('я', 'Я'),
        X => ('ч', 'Ч'),
        C => ('с', 'С'),
        V => ('м', 'М'),
        B => ('и', 'И'),
        N => ('т', 'Т'),
        M => ('ь', 'Ь'),
        Comma => ('б', 'Б'),
        Period => ('ю', 'Ю'),
        Slash => ('.', ','),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_printable_us_key_has_a_russian_mapping() {
        let keys = [
            Backquote,
            Digit1,
            Digit2,
            Digit3,
            Digit4,
            Digit5,
            Digit6,
            Digit7,
            Digit8,
            Digit9,
            Digit0,
            Minus,
            Equal,
            Q,
            W,
            E,
            R,
            T,
            Y,
            U,
            I,
            O,
            P,
            LeftBracket,
            RightBracket,
            Backslash,
            A,
            S,
            D,
            F,
            G,
            H,
            J,
            K,
            L,
            Semicolon,
            Quote,
            Z,
            X,
            C,
            V,
            B,
            N,
            M,
            Comma,
            Period,
            Slash,
        ];
        for k in keys {
            assert!(Layout::Us.chars(k).is_some(), "{k:?} missing in US");
            assert!(Layout::Russian.chars(k).is_some(), "{k:?} missing in RU");
        }
    }

    #[test]
    fn russian_letters_are_cyrillic() {
        for k in [
            Q,
            A,
            Z,
            Semicolon,
            Quote,
            Comma,
            Period,
            LeftBracket,
            Backquote,
        ] {
            let (c, s) = Layout::Russian.chars(k).unwrap();
            assert!(('а'..='я').contains(&c) || c == 'ё', "{k:?} -> {c}");
            assert_eq!(c.to_uppercase().next(), Some(s));
        }
    }
}

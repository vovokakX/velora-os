//! Physical key codes, independent of layout and of the keyboard bus.
//! Names follow the US layout position of the key.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyCode {
    Escape,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
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
    Backspace,
    Tab,
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
    CapsLock,
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
    Enter,
    LeftShift,
    /// The extra key between left Shift and Z on ISO keyboards.
    NonUsBackslash,
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
    RightShift,
    LeftCtrl,
    LeftMeta,
    LeftAlt,
    Space,
    RightAlt,
    RightMeta,
    Menu,
    RightCtrl,
    PrintScreen,
    ScrollLock,
    Pause,
    Insert,
    Home,
    PageUp,
    Delete,
    End,
    PageDown,
    Up,
    Left,
    Down,
    Right,
    NumLock,
    KeypadDivide,
    KeypadMultiply,
    KeypadMinus,
    KeypadPlus,
    KeypadEnter,
    KeypadPeriod,
    Keypad0,
    Keypad1,
    Keypad2,
    Keypad3,
    Keypad4,
    Keypad5,
    Keypad6,
    Keypad7,
    Keypad8,
    Keypad9,
}

/// Text for keys that type the same thing in every layout.
pub fn fixed_char(key: KeyCode) -> Option<char> {
    use KeyCode::*;
    Some(match key {
        Enter | KeypadEnter => '\n',
        Backspace => '\x08',
        Tab => '\t',
        Space => ' ',
        Escape => '\x1b',
        _ => return None,
    })
}

/// Keypad text. Operators always type; digits only with Num Lock on
/// (otherwise the keys act as arrows/Home/End, which produce no text).
pub fn keypad_char(key: KeyCode, num_lock: bool) -> Option<char> {
    use KeyCode::*;
    let c = match key {
        KeypadDivide => return Some('/'),
        KeypadMultiply => return Some('*'),
        KeypadMinus => return Some('-'),
        KeypadPlus => return Some('+'),
        Keypad0 => '0',
        Keypad1 => '1',
        Keypad2 => '2',
        Keypad3 => '3',
        Keypad4 => '4',
        Keypad5 => '5',
        Keypad6 => '6',
        Keypad7 => '7',
        Keypad8 => '8',
        Keypad9 => '9',
        KeypadPeriod => '.',
        _ => return None,
    };
    if num_lock { Some(c) } else { None }
}

/// ASCII control character for Ctrl + a letter key (Ctrl+A = 0x01).
pub fn control_char(key: KeyCode) -> Option<char> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    ];
    LETTERS
        .iter()
        .position(|&k| k == key)
        .map(|i| char::from(i as u8 + 1))
}

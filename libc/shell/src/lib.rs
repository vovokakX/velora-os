//! The vsh command language and line editor, kept apart from the shell
//! program so they can be tested on the host (no_std + alloc).
//!
//! - [`parse`]: words, quotes, `$VAR`/`${VAR}`/`$?`, `~`, assignments,
//!   pipes `|`, redirections `<`, `>`, `>>`, `2>`, `2>>`, and the
//!   separators `;`, `&&`, `||`. Variables are expanded later, when each
//!   command runs ([`expand`]).
//! - [`KeyDecoder`] turns raw terminal bytes into keys; [`Editor`] edits a
//!   line with cursor movement and history.
//! - [`common_prefix`] for Tab completion.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// --- parsing --------------------------------------------------------------------

/// A piece of a word: literal text or a variable to expand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seg {
    Lit(String),
    Var(String),
}

pub type Word = Vec<Seg>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    /// 0 = stdin, 1 = stdout, 2 = stderr.
    pub fd: u32,
    pub target: Word,
    pub append: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    /// `NAME=value` words before the command name.
    pub assigns: Vec<(String, Word)>,
    pub args: Vec<Word>,
    pub redirects: Vec<Redirect>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>,
}

/// How a pipeline connects to the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// `;` or the end of the line: always run the next one.
    Always,
    /// `&&`: only after success.
    IfOk,
    /// `||`: only after failure.
    IfFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    UnclosedQuote,
    MissingRedirectTarget,
    /// An operator with no command on one side (`| x`, `a &&`).
    EmptyCommand,
    /// `&` (background jobs are not supported).
    Background,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            ParseError::UnclosedQuote => "unclosed quote",
            ParseError::MissingRedirectTarget => "missing file name after redirection",
            ParseError::EmptyCommand => "missing command next to an operator",
            ParseError::Background => "background jobs (&) are not supported",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Word(Word),
    Pipe,
    And,
    Or,
    Semi,
    Redir(u32, bool),
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn push_lit(w: &mut Word, c: char) {
    if let Some(Seg::Lit(s)) = w.last_mut() {
        s.push(c);
    } else {
        w.push(Seg::Lit(String::from(c)));
    }
}

/// Reads a variable reference after '$'; returns false if '$' is literal.
fn read_var(chars: &[char], i: &mut usize, w: &mut Word) -> bool {
    match chars.get(*i) {
        Some('?') => {
            *i += 1;
            w.push(Seg::Var(String::from("?")));
            true
        }
        Some('{') => {
            let start = *i + 1;
            let Some(end) = chars[start..].iter().position(|&c| c == '}') else {
                return false;
            };
            let name: String = chars[start..start + end].iter().collect();
            *i = start + end + 1;
            w.push(Seg::Var(name));
            true
        }
        Some(&c) if is_name_start(c) => {
            let start = *i;
            while chars.get(*i).is_some_and(|&c| is_name_char(c)) {
                *i += 1;
            }
            w.push(Seg::Var(chars[start..*i].iter().collect()));
            true
        }
        _ => false,
    }
}

fn lex(line: &str) -> Result<Vec<Tok>, ParseError> {
    let chars: Vec<char> = line.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' => {
                i += 1;
                continue;
            }
            '#' => break,
            '|' if chars.get(i + 1) == Some(&'|') => {
                toks.push(Tok::Or);
                i += 2;
                continue;
            }
            '|' => {
                toks.push(Tok::Pipe);
                i += 1;
                continue;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                toks.push(Tok::And);
                i += 2;
                continue;
            }
            '&' => return Err(ParseError::Background),
            ';' => {
                toks.push(Tok::Semi);
                i += 1;
                continue;
            }
            '<' => {
                toks.push(Tok::Redir(0, false));
                i += 1;
                continue;
            }
            '>' => {
                let append = chars.get(i + 1) == Some(&'>');
                toks.push(Tok::Redir(1, append));
                i += 1 + append as usize;
                continue;
            }
            '2' if chars.get(i + 1) == Some(&'>') => {
                let append = chars.get(i + 2) == Some(&'>');
                toks.push(Tok::Redir(2, append));
                i += 2 + append as usize;
                continue;
            }
            _ => {}
        }
        // A word.
        let mut w: Word = Vec::new();
        let mut quoted = false;
        if c == '~' && matches!(chars.get(i + 1), None | Some(' ' | '/' | '\t')) {
            w.push(Seg::Var(String::from("HOME")));
            i += 1;
        }
        while i < chars.len() {
            let c = chars[i];
            match c {
                ' ' | '\t' | '\n' | '|' | '&' | ';' | '<' | '>' => break,
                '\'' => {
                    quoted = true;
                    let start = i + 1;
                    let end = chars[start..]
                        .iter()
                        .position(|&c| c == '\'')
                        .ok_or(ParseError::UnclosedQuote)?;
                    for &c in &chars[start..start + end] {
                        push_lit(&mut w, c);
                    }
                    i = start + end + 1;
                }
                '"' => {
                    quoted = true;
                    i += 1;
                    loop {
                        match chars.get(i) {
                            None => return Err(ParseError::UnclosedQuote),
                            Some('"') => {
                                i += 1;
                                break;
                            }
                            Some('\\') if matches!(chars.get(i + 1), Some('"' | '\\' | '$')) => {
                                push_lit(&mut w, chars[i + 1]);
                                i += 2;
                            }
                            Some('$') => {
                                i += 1;
                                if !read_var(&chars, &mut i, &mut w) {
                                    push_lit(&mut w, '$');
                                }
                            }
                            Some(&c) => {
                                push_lit(&mut w, c);
                                i += 1;
                            }
                        }
                    }
                }
                '\\' => {
                    if let Some(&n) = chars.get(i + 1) {
                        push_lit(&mut w, n);
                    }
                    i += 2;
                }
                '$' => {
                    i += 1;
                    if !read_var(&chars, &mut i, &mut w) {
                        push_lit(&mut w, '$');
                    }
                }
                c => {
                    push_lit(&mut w, c);
                    i += 1;
                }
            }
        }
        if w.is_empty() && quoted {
            w.push(Seg::Lit(String::new()));
        }
        toks.push(Tok::Word(w));
    }
    Ok(toks)
}

/// Splits `NAME=rest` at the start of a word.
fn as_assignment(w: &Word) -> Option<(String, Word)> {
    let Some(Seg::Lit(first)) = w.first() else {
        return None;
    };
    let eq = first.find('=')?;
    let name = &first[..eq];
    let mut cs = name.chars();
    if !cs.next().is_some_and(is_name_start) || !cs.all(is_name_char) {
        return None;
    }
    let mut value: Word = Vec::new();
    let rest = &first[eq + 1..];
    if !rest.is_empty() {
        value.push(Seg::Lit(String::from(rest)));
    }
    value.extend(w[1..].iter().cloned());
    Some((String::from(name), value))
}

/// Parses one input line.
pub fn parse(line: &str) -> Result<Vec<(Pipeline, Next)>, ParseError> {
    let toks = lex(line)?;
    let mut out = Vec::new();
    let mut pipeline = Pipeline::default();
    let mut cmd = Command::default();
    let mut it = toks.into_iter().peekable();
    let finish_cmd = |cmd: &mut Command, p: &mut Pipeline| -> Result<(), ParseError> {
        if cmd.args.is_empty() && cmd.assigns.is_empty() && cmd.redirects.is_empty() {
            return Err(ParseError::EmptyCommand);
        }
        p.commands.push(core::mem::take(cmd));
        Ok(())
    };
    while let Some(t) = it.next() {
        match t {
            Tok::Word(w) => {
                if cmd.args.is_empty()
                    && let Some(a) = as_assignment(&w)
                {
                    cmd.assigns.push(a);
                } else {
                    cmd.args.push(w);
                }
            }
            Tok::Redir(fd, append) => match it.next() {
                Some(Tok::Word(target)) => cmd.redirects.push(Redirect { fd, target, append }),
                _ => return Err(ParseError::MissingRedirectTarget),
            },
            Tok::Pipe => {
                finish_cmd(&mut cmd, &mut pipeline)?;
                if it.peek().is_none() {
                    return Err(ParseError::EmptyCommand);
                }
            }
            Tok::And | Tok::Or | Tok::Semi => {
                let is_semi = t == Tok::Semi;
                if is_semi && cmd == Command::default() && pipeline.commands.is_empty() {
                    continue; // empty statement: "a;;b" or a leading ';'
                }
                finish_cmd(&mut cmd, &mut pipeline)?;
                let next = match t {
                    Tok::And => Next::IfOk,
                    Tok::Or => Next::IfFailed,
                    _ => Next::Always,
                };
                if !is_semi && it.peek().is_none() {
                    return Err(ParseError::EmptyCommand);
                }
                out.push((core::mem::take(&mut pipeline), next));
            }
        }
    }
    if cmd != Command::default() {
        finish_cmd(&mut cmd, &mut pipeline)?;
    }
    if !pipeline.commands.is_empty() {
        out.push((pipeline, Next::Always));
    }
    Ok(out)
}

/// Expands a word with `lookup` (unset variables are empty).
pub fn expand(w: &Word, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut s = String::new();
    for seg in w {
        match seg {
            Seg::Lit(l) => s.push_str(l),
            Seg::Var(v) => s.push_str(&lookup(v).unwrap_or_default()),
        }
    }
    s
}

// --- keys and line editing ------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Up,
    Down,
    Tab,
    CtrlA,
    CtrlC,
    CtrlD,
    CtrlE,
    CtrlL,
    CtrlU,
    Other,
}

/// Decodes raw terminal bytes (UTF-8 and ANSI escape sequences).
#[derive(Default)]
pub struct KeyDecoder {
    pending: Vec<u8>,
}

impl KeyDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one byte; returns a key once it is complete.
    pub fn feed(&mut self, b: u8) -> Option<Key> {
        self.pending.push(b);
        let p = &self.pending;
        let key = if p[0] == 0x1b {
            if p.len() == 1 {
                return None;
            }
            if p[1] != b'[' {
                Key::Other
            } else if p.len() == 2 {
                return None;
            } else {
                let last = *p.last().unwrap();
                if !(0x40..=0x7e).contains(&last) {
                    if p.len() > 8 {
                        self.pending.clear();
                        return Some(Key::Other);
                    }
                    return None;
                }
                match &p[2..] {
                    b"A" => Key::Up,
                    b"B" => Key::Down,
                    b"C" => Key::Right,
                    b"D" => Key::Left,
                    b"H" | b"1~" => Key::Home,
                    b"F" | b"4~" => Key::End,
                    b"3~" => Key::Delete,
                    _ => Key::Other,
                }
            }
        } else if p[0] < 0x80 {
            match p[0] {
                b'\n' | b'\r' => Key::Enter,
                0x7f | 0x08 => Key::Backspace,
                b'\t' => Key::Tab,
                0x01 => Key::CtrlA,
                0x03 => Key::CtrlC,
                0x04 => Key::CtrlD,
                0x05 => Key::CtrlE,
                0x0c => Key::CtrlL,
                0x15 => Key::CtrlU,
                c if c < 0x20 => Key::Other,
                c => Key::Char(c as char),
            }
        } else {
            let need = match p[0] {
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => 1,
            };
            if p.len() < need {
                return None;
            }
            match core::str::from_utf8(p).ok().and_then(|s| s.chars().next()) {
                Some(c) => Key::Char(c),
                None => Key::Other,
            }
        };
        self.pending.clear();
        Some(key)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// The line or cursor changed: draw it again.
    Redraw,
    Submit(String),
    /// Ctrl+D on an empty line.
    Eof,
    /// Ctrl+C: the line was dropped.
    Interrupt,
    Complete,
    ClearScreen,
}

const HISTORY_MAX: usize = 500;

#[derive(Default)]
pub struct Editor {
    pub line: Vec<char>,
    pub cursor: usize,
    history: Vec<String>,
    /// Position while browsing history (None = editing a new line).
    browse: Option<usize>,
    draft: Vec<char>,
}

impl Editor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn add_history(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || self.history.last().is_some_and(|l| l == line) {
            return;
        }
        if self.history.len() == HISTORY_MAX {
            self.history.remove(0);
        }
        self.history.push(String::from(line));
    }

    pub fn text(&self) -> String {
        self.line.iter().collect()
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.line.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    /// The word before the cursor and where it starts.
    pub fn word_before_cursor(&self) -> (usize, String) {
        let start = self.line[..self.cursor]
            .iter()
            .rposition(|&c| c == ' ' || c == '|' || c == ';' || c == '<' || c == '>')
            .map_or(0, |p| p + 1);
        (start, self.line[start..self.cursor].iter().collect())
    }

    /// True if the word at `start` is the command name (first word).
    pub fn is_command_position(&self, start: usize) -> bool {
        self.line[..start]
            .iter()
            .rev()
            .find(|&&c| c != ' ')
            .is_none_or(|&c| c == '|' || c == ';' || c == '&')
    }

    fn set_line(&mut self, l: Vec<char>) {
        self.cursor = l.len();
        self.line = l;
    }

    pub fn handle(&mut self, k: Key) -> Action {
        match k {
            Key::Char(c) => {
                self.line.insert(self.cursor, c);
                self.cursor += 1;
                Action::Redraw
            }
            Key::Enter => {
                let s = self.text();
                self.line.clear();
                self.cursor = 0;
                self.browse = None;
                Action::Submit(s)
            }
            Key::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.line.remove(self.cursor);
                Action::Redraw
            }
            Key::Delete if self.cursor < self.line.len() => {
                self.line.remove(self.cursor);
                Action::Redraw
            }
            Key::Left if self.cursor > 0 => {
                self.cursor -= 1;
                Action::Redraw
            }
            Key::Right if self.cursor < self.line.len() => {
                self.cursor += 1;
                Action::Redraw
            }
            Key::Home | Key::CtrlA => {
                self.cursor = 0;
                Action::Redraw
            }
            Key::End | Key::CtrlE => {
                self.cursor = self.line.len();
                Action::Redraw
            }
            Key::CtrlU => {
                self.line.drain(..self.cursor);
                self.cursor = 0;
                Action::Redraw
            }
            Key::Up if !self.history.is_empty() => {
                let i = match self.browse {
                    None => {
                        self.draft = self.line.clone();
                        self.history.len() - 1
                    }
                    Some(0) => return Action::None,
                    Some(i) => i - 1,
                };
                self.browse = Some(i);
                self.set_line(self.history[i].chars().collect());
                Action::Redraw
            }
            Key::Down => match self.browse {
                None => Action::None,
                Some(i) if i + 1 < self.history.len() => {
                    self.browse = Some(i + 1);
                    self.set_line(self.history[i + 1].chars().collect());
                    Action::Redraw
                }
                Some(_) => {
                    self.browse = None;
                    let d = core::mem::take(&mut self.draft);
                    self.set_line(d);
                    Action::Redraw
                }
            },
            Key::Tab => Action::Complete,
            Key::CtrlC => {
                self.line.clear();
                self.cursor = 0;
                self.browse = None;
                Action::Interrupt
            }
            Key::CtrlD if self.line.is_empty() => Action::Eof,
            Key::CtrlD => self.handle(Key::Delete),
            Key::CtrlL => Action::ClearScreen,
            _ => Action::None,
        }
    }
}

/// Longest common prefix of the candidates.
pub fn common_prefix(cands: &[String]) -> String {
    let Some(first) = cands.first() else {
        return String::new();
    };
    let mut n = first.len();
    for c in &cands[1..] {
        n = n.min(
            first
                .char_indices()
                .zip(c.chars())
                .take_while(|((_, a), b)| a == b)
                .last()
                .map_or(0, |((i, a), _)| i + a.len_utf8()),
        );
    }
    String::from(&first[..n])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        match name {
            "HOME" => Some("/home/user".into()),
            "X" => Some("a b".into()),
            "?" => Some("1".into()),
            _ => None,
        }
    }

    fn words(c: &Command) -> Vec<String> {
        c.args.iter().map(|w| expand(w, &env)).collect()
    }

    #[test]
    fn words_quotes_and_variables() {
        let s = parse(r#"echo 'a  $X' "b $X" $X \$X ${X}y ~ ~/f "" x#y # c"#).unwrap();
        assert_eq!(s.len(), 1);
        let c = &s[0].0.commands[0];
        assert_eq!(
            words(c),
            [
                "echo",
                "a  $X",
                "b a b",
                "a b",
                "$X",
                "a by",
                "/home/user",
                "/home/user/f",
                "",
                "x#y"
            ]
        );
        assert_eq!(
            words(&parse("echo $? $NONE end").unwrap()[0].0.commands[0]),
            ["echo", "1", "", "end"]
        );
    }

    #[test]
    fn pipes_redirections_and_separators() {
        let s = parse("cat < in.txt | grep x > out 2>> err; ls && echo ok || echo no").unwrap();
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].1, Next::Always);
        let p = &s[0].0;
        assert_eq!(p.commands.len(), 2);
        assert_eq!(p.commands[0].redirects[0].fd, 0);
        let r = &p.commands[1].redirects;
        assert_eq!(
            (r[0].fd, r[0].append, expand(&r[0].target, &env)),
            (1, false, "out".into())
        );
        assert_eq!((r[1].fd, r[1].append), (2, true));
        assert_eq!(
            (s[1].1, s[2].1, s[3].1),
            (Next::IfOk, Next::IfFailed, Next::Always)
        );
        // "2" that is not followed by '>' is an ordinary word.
        assert_eq!(
            words(&parse("echo 2 x2>y").unwrap()[0].0.commands[0]),
            ["echo", "2", "x2"]
        );
    }

    #[test]
    fn assignments() {
        let s = parse("A=1 B=\"$X\" cmd C=3").unwrap();
        let c = &s[0].0.commands[0];
        assert_eq!(c.assigns.len(), 2);
        assert_eq!(c.assigns[0].0, "A");
        assert_eq!(expand(&c.assigns[1].1, &env), "a b");
        assert_eq!(words(c), ["cmd", "C=3"]);
        let only = parse("PATH=/bin").unwrap();
        assert!(only[0].0.commands[0].args.is_empty());
        assert_eq!(
            words(&parse("=x 1A=2").unwrap()[0].0.commands[0]),
            ["=x", "1A=2"]
        );
    }

    #[test]
    fn errors() {
        assert_eq!(parse("echo 'x"), Err(ParseError::UnclosedQuote));
        assert_eq!(parse("echo \"x"), Err(ParseError::UnclosedQuote));
        assert_eq!(parse("ls >"), Err(ParseError::MissingRedirectTarget));
        assert_eq!(parse("| ls"), Err(ParseError::EmptyCommand));
        assert_eq!(parse("ls |"), Err(ParseError::EmptyCommand));
        assert_eq!(parse("ls &&"), Err(ParseError::EmptyCommand));
        assert_eq!(parse("sleep 1 &"), Err(ParseError::Background));
        assert_eq!(parse("  # only a comment").unwrap(), Vec::new());
        assert_eq!(parse(";;ls;").unwrap().len(), 1);
    }

    #[test]
    fn key_decoding() {
        let mut d = KeyDecoder::new();
        let mut keys = Vec::new();
        for &b in "a\x1b[A\x1b[3~\x7f\tж\n\x03".as_bytes() {
            if let Some(k) = d.feed(b) {
                keys.push(k);
            }
        }
        assert_eq!(
            keys,
            [
                Key::Char('a'),
                Key::Up,
                Key::Delete,
                Key::Backspace,
                Key::Tab,
                Key::Char('ж'),
                Key::Enter,
                Key::CtrlC
            ]
        );
    }

    #[test]
    fn editing_and_history() {
        let mut e = Editor::new();
        for c in "lsx".chars() {
            e.handle(Key::Char(c));
        }
        e.handle(Key::Backspace);
        e.handle(Key::Left);
        e.handle(Key::Char('-'));
        assert_eq!(e.text(), "l-s");
        e.handle(Key::Home);
        e.handle(Key::Delete);
        assert_eq!(e.handle(Key::Enter), Action::Submit("-s".into()));
        e.add_history("first");
        e.add_history("second");
        e.add_history("second");
        assert_eq!(e.history().len(), 2);
        e.handle(Key::Char('d'));
        e.handle(Key::Up);
        assert_eq!(e.text(), "second");
        e.handle(Key::Up);
        e.handle(Key::Up);
        assert_eq!(e.text(), "first");
        e.handle(Key::Down);
        e.handle(Key::Down);
        assert_eq!(e.text(), "d", "the draft comes back");
        assert_eq!(e.handle(Key::CtrlC), Action::Interrupt);
        assert_eq!(e.handle(Key::CtrlD), Action::Eof);
    }

    #[test]
    fn completion_helpers() {
        let mut e = Editor::new();
        e.insert_str("cat /tmp/fi");
        assert_eq!(e.word_before_cursor(), (4, "/tmp/fi".into()));
        assert!(!e.is_command_position(4));
        let mut e = Editor::new();
        e.insert_str("ls | ve");
        let (start, w) = e.word_before_cursor();
        assert_eq!(w, "ve");
        assert!(e.is_command_position(start));
        let c: Vec<String> = ["velofetch", "vecho", "vtest"].map(String::from).into();
        assert_eq!(common_prefix(&c[..2]), "ve");
        assert_eq!(common_prefix(&c), "v");
        assert_eq!(common_prefix(&["файл1".into(), "файл2".into()]), "файл");
    }
}

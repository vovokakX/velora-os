//! /bin/vsh: the Velora shell.
//!
//! Interactive on the console: line editing, history (Up/Down), Tab
//! completion, coloured prompt. Runs pipelines with redirections, `;`,
//! `&&`, `||`, variables. `vsh -c LINE` runs one line; `vsh FILE` runs a
//! script; with input that is not the console it reads lines from it.
//! The language is parsed by `velora-shell`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use velora_shell::{
    Action, Command, Editor, KeyDecoder, Next, Pipeline, common_prefix, expand, parse,
};
use vlibc::abi::errno;
use vlibc::fs::{self, File, flags};
use vlibc::{Errno, env, eprintln, io, print, println, process, term};

vlibc::main!(main);

const BUILTINS: [&str; 7] = ["cd", "pwd", "exit", "export", "unset", "history", "set"];

struct Shell {
    vars: BTreeMap<String, String>,
    exported: BTreeSet<String>,
    status: i32,
    editor: Editor,
    exit: Option<i32>,
}

fn describe(e: Errno) -> &'static str {
    match e.0 {
        errno::ENOENT => "no such file or directory",
        errno::EACCES => "permission denied",
        errno::ENOTDIR => "not a directory",
        errno::EISDIR => "is a directory",
        errno::ENOEXEC => "not an executable program",
        errno::EROFS => "read-only file system",
        _ => e.name(),
    }
}

impl Shell {
    fn new() -> Self {
        let mut s = Shell {
            vars: BTreeMap::new(),
            exported: BTreeSet::new(),
            status: 0,
            editor: Editor::new(),
            exit: None,
        };
        for v in env::vars() {
            if let Some((k, val)) = v.split_once('=') {
                s.vars.insert(k.into(), val.into());
                s.exported.insert(k.into());
            }
        }
        s.vars.entry("PATH".into()).or_insert_with(|| "/bin".into());
        s.exported.insert("PATH".into());
        s
    }

    fn lookup(&self, name: &str) -> Option<String> {
        match name {
            "?" => Some(format!("{}", self.status)),
            "PWD" => fs::getcwd().ok(),
            _ => self.vars.get(name).cloned(),
        }
    }

    fn prompt(&self) -> String {
        let (uid, _) = process::getuid();
        let user = self
            .vars
            .get("USER")
            .cloned()
            .unwrap_or_else(|| format!("{}", uid));
        let mut cwd = fs::getcwd().unwrap_or_else(|_| "?".into());
        if let Some(home) = self.vars.get("HOME")
            && home != "/"
            && (cwd == *home || cwd.starts_with(&format!("{}/", home)))
        {
            cwd = format!("~{}", &cwd[home.len()..]);
        }
        let sign = if uid == 0 { '#' } else { '$' };
        format!(
            "\x1b[1;32m{}@velora\x1b[0m:\x1b[1;34m{}\x1b[0m{} ",
            user, cwd, sign
        )
    }

    // --- running commands ---------------------------------------------------

    fn find_program(&self, name: &str) -> Option<String> {
        if name.contains('/') {
            return Some(name.into());
        }
        let path = self.vars.get("PATH").cloned().unwrap_or_default();
        path.split(':').filter(|d| !d.is_empty()).find_map(|d| {
            let p = format!("{}/{}", d.trim_end_matches('/'), name);
            fs::stat(&p).ok().filter(|st| !fs::is_dir(st)).map(|_| p)
        })
    }

    fn child_env(&self, extra: &[(String, String)]) -> Vec<String> {
        let mut m: BTreeMap<&str, &str> = BTreeMap::new();
        for k in &self.exported {
            if let Some(v) = self.vars.get(k) {
                m.insert(k, v);
            }
        }
        for (k, v) in extra {
            m.insert(k, v);
        }
        m.iter().map(|(k, v)| format!("{}={}", k, v)).collect()
    }

    /// Applies a command's redirections to descriptors 0-2 of the shell.
    fn redirect(&self, c: &Command) -> Result<(), String> {
        for r in &c.redirects {
            let target = expand(&r.target, &|n| self.lookup(n));
            let fl = match (r.fd, r.append) {
                (0, _) => flags::READ,
                (_, true) => flags::WRITE | flags::CREATE | flags::APPEND,
                (_, false) => flags::WRITE | flags::CREATE | flags::TRUNCATE,
            };
            let f = File::open_with(&target, fl, 0o644)
                .map_err(|e| format!("{}: {}", target, describe(e)))?;
            io::dup2(f.fd(), r.fd as u64).map_err(|e| format!("{}: {}", target, describe(e)))?;
        }
        Ok(())
    }

    fn builtin(&mut self, argv: &[String]) -> i32 {
        let arg = |i: usize| argv.get(i).map(String::as_str);
        match argv[0].as_str() {
            "cd" => {
                let dir = match arg(1) {
                    Some(d) => String::from(d),
                    None => self.vars.get("HOME").cloned().unwrap_or_else(|| "/".into()),
                };
                match fs::chdir(&dir) {
                    Ok(()) => 0,
                    Err(e) => {
                        eprintln!("cd: {}: {}", dir, describe(e));
                        1
                    }
                }
            }
            "pwd" => match fs::getcwd() {
                Ok(p) => {
                    println!("{}", p);
                    0
                }
                Err(e) => {
                    eprintln!("pwd: {}", describe(e));
                    1
                }
            },
            "exit" => {
                let code = arg(1).and_then(|s| s.parse().ok()).unwrap_or(self.status);
                self.exit = Some(code);
                code
            }
            "export" => {
                for a in &argv[1..] {
                    match a.split_once('=') {
                        Some((k, v)) => {
                            self.vars.insert(k.into(), v.into());
                            self.exported.insert(k.into());
                        }
                        None => {
                            self.exported.insert(a.clone());
                        }
                    }
                }
                0
            }
            "unset" => {
                for a in &argv[1..] {
                    self.vars.remove(a);
                    self.exported.remove(a);
                }
                0
            }
            "history" => {
                for (i, l) in self.editor.history().iter().enumerate() {
                    println!("{:>4}  {}", i + 1, l);
                }
                0
            }
            "set" => {
                for (k, v) in &self.vars {
                    println!("{}={}", k, v);
                }
                0
            }
            _ => 127,
        }
    }

    fn run_pipeline(&mut self, p: &Pipeline) -> i32 {
        let saved: Vec<Option<u64>> = (0..3).map(|fd| io::dup(fd).ok()).collect();
        let restore = || {
            for (fd, s) in saved.iter().enumerate() {
                if let Some(s) = s {
                    let _ = io::dup2(*s, fd as u64);
                }
            }
        };
        let n = p.commands.len();
        let mut pids = Vec::new();
        let mut prev_read: Option<u64> = None;
        let mut last_status = 0;
        for (i, c) in p.commands.iter().enumerate() {
            let look = |name: &str| self.lookup(name);
            let argv: Vec<String> = c.args.iter().map(|w| expand(w, &look)).collect();
            let assigns: Vec<(String, String)> = c
                .assigns
                .iter()
                .map(|(k, w)| (k.clone(), expand(w, &look)))
                .collect();
            if let Some(r) = prev_read.take() {
                let _ = io::dup2(r, 0);
                let _ = fs::close(r);
            }
            if i + 1 < n {
                match io::pipe() {
                    Ok((r, w)) => {
                        let _ = io::dup2(w, 1);
                        let _ = fs::close(w);
                        prev_read = Some(r);
                    }
                    Err(e) => {
                        eprintln!("vsh: pipe: {}", describe(e));
                        last_status = 1;
                        break;
                    }
                }
            }
            if let Err(msg) = self.redirect(c) {
                eprintln!("vsh: {}", msg);
                last_status = 1;
                restore();
                continue;
            }
            if argv.is_empty() {
                // Only assignments: they set shell variables.
                for (k, v) in assigns {
                    self.vars.insert(k, v);
                }
                last_status = 0;
            } else if BUILTINS.contains(&argv[0].as_str()) {
                last_status = self.builtin(&argv);
            } else {
                match self.find_program(&argv[0]) {
                    None => {
                        eprintln!("vsh: {}: command not found", argv[0]);
                        last_status = 127;
                    }
                    Some(path) => {
                        let env = self.child_env(&assigns);
                        let env: Vec<&str> = env.iter().map(String::as_str).collect();
                        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
                        match process::spawn(&path, &args, &env) {
                            Ok(pid) => pids.push(pid),
                            Err(e) => {
                                eprintln!("vsh: {}: {}", argv[0], describe(e));
                                last_status = if e.0 == errno::ENOENT { 127 } else { 126 };
                            }
                        }
                    }
                }
            }
            restore();
        }
        if let Some(r) = prev_read {
            let _ = fs::close(r);
        }
        for s in saved.into_iter().flatten() {
            let _ = fs::close(s);
        }
        if let Some(&last) = pids.last() {
            term::set_foreground(last);
            for &pid in &pids {
                let st = process::wait(pid).unwrap_or(1);
                if pid == last {
                    last_status = st as i32;
                }
            }
            term::set_foreground(0);
            if last_status == 130 {
                println!();
            }
        }
        last_status
    }

    fn run_line(&mut self, line: &str) {
        let script = match parse(line) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("vsh: {}", e);
                self.status = 2;
                return;
            }
        };
        let mut run = true;
        for (p, next) in script {
            if run {
                self.status = self.run_pipeline(&p);
                if self.exit.is_some() {
                    return;
                }
            }
            run = match next {
                Next::Always => true,
                Next::IfOk => self.status == 0,
                Next::IfFailed => self.status != 0,
            };
        }
    }

    // --- interactive input --------------------------------------------------

    fn redraw(&self, prompt: &str) {
        let back = self.editor.line.len() - self.editor.cursor;
        print!("\r{}{}\x1b[K", prompt, self.editor.text());
        if back > 0 {
            print!("\x1b[{}D", back);
        }
    }

    fn candidates(&self, word: &str, command: bool) -> Vec<String> {
        let mut out = Vec::new();
        if command && !word.contains('/') {
            for b in BUILTINS {
                if b.starts_with(word) {
                    out.push(String::from(b));
                }
            }
            let path = self.vars.get("PATH").cloned().unwrap_or_default();
            for d in path.split(':').filter(|d| !d.is_empty()) {
                if let Ok(entries) = fs::read_dir(d) {
                    for e in entries.flatten() {
                        if e.name.starts_with(word) && !out.contains(&e.name) {
                            out.push(e.name);
                        }
                    }
                }
            }
        } else {
            let (dir, prefix) = match word.rfind('/') {
                Some(i) => (&word[..=i], &word[i + 1..]),
                None => ("", word),
            };
            let listed = if dir.is_empty() { "." } else { dir };
            if let Ok(entries) = fs::read_dir(listed) {
                for e in entries.flatten() {
                    if e.name.starts_with(prefix)
                        && (!e.name.starts_with('.') || prefix.starts_with('.'))
                    {
                        out.push(format!(
                            "{}{}{}",
                            dir,
                            e.name,
                            if e.is_dir { "/" } else { "" }
                        ));
                    }
                }
            }
        }
        out.sort();
        out
    }

    fn complete(&mut self, prompt: &str) {
        let (start, word) = self.editor.word_before_cursor();
        let cands = self.candidates(&word, self.editor.is_command_position(start));
        match cands.len() {
            0 => {}
            1 => {
                let c = &cands[0];
                self.editor.insert_str(&c[word.len()..]);
                if !c.ends_with('/') {
                    self.editor.insert_str(" ");
                }
            }
            _ => {
                let common = common_prefix(&cands);
                if common.len() > word.len() {
                    self.editor.insert_str(&common[word.len()..]);
                } else {
                    println!();
                    let names: Vec<&str> = cands
                        .iter()
                        .map(|c| c.trim_end_matches('/').rsplit('/').next().unwrap_or(c))
                        .collect();
                    println!("{}", names.join("  "));
                }
            }
        }
        self.redraw(prompt);
    }

    /// Reads one line with editing; `None` on Ctrl+D or end of input.
    fn read_interactive(&mut self) -> Option<String> {
        let prompt = self.prompt();
        print!("{}", prompt);
        term::set_mode(term::RAW);
        let mut dec = KeyDecoder::new();
        let mut buf = [0u8; 64];
        let result = 'outer: loop {
            let n = match io::read(0, &mut buf) {
                Ok(0) | Err(_) => break None,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                let Some(k) = dec.feed(b) else { continue };
                match self.editor.handle(k) {
                    Action::None => {}
                    Action::Redraw => self.redraw(&prompt),
                    Action::Submit(s) => {
                        println!();
                        break 'outer Some(s);
                    }
                    Action::Eof => {
                        println!();
                        break 'outer None;
                    }
                    Action::Interrupt => {
                        println!("^C");
                        self.status = 130;
                        print!("{}", prompt);
                    }
                    Action::Complete => self.complete(&prompt),
                    Action::ClearScreen => {
                        print!("\x1b[2J\x1b[H");
                        self.redraw(&prompt);
                    }
                }
            }
        };
        term::set_mode(0);
        result
    }

    fn interactive(&mut self) -> i32 {
        println!("vsh: type 'help' for commands, Tab completes, Ctrl+D exits.");
        while self.exit.is_none() {
            let Some(line) = self.read_interactive() else {
                break;
            };
            self.editor.add_history(&line);
            self.run_line(&line);
        }
        self.exit.unwrap_or(self.status)
    }

    fn run_text(&mut self, text: &str) -> i32 {
        for line in text.lines() {
            self.run_line(line);
            if let Some(code) = self.exit {
                return code;
            }
        }
        self.status
    }
}

fn read_all(fd: u64) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = io::read(fd, &mut buf) {
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn main() -> i32 {
    let args = env::args();
    let mut sh = Shell::new();
    match (args.get(1), args.get(2)) {
        (Some("-c"), Some(line)) => {
            sh.run_line(line);
            sh.exit.unwrap_or(sh.status)
        }
        (Some(file), _) => match fs::read(file) {
            Ok(b) => sh.run_text(&String::from_utf8_lossy(&b)),
            Err(e) => {
                eprintln!("vsh: {}: {}", file, describe(e));
                127
            }
        },
        (None, _) if term::is_console(0) => sh.interactive(),
        (None, _) => {
            let text = read_all(0);
            sh.run_text(&text)
        }
    }
}

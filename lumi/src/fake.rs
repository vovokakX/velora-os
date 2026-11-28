//! A system in memory for the host tests and the screenshot example:
//! files in a map, a clock that only moves when told, users `user` and
//! `root` (password `velora`), and terminals that print a prepared
//! session.

use crate::host::{DirEntry, Host, NetState, Power, SysInfo, Term, TermRead, UserInfo, parent};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

/// What the fake terminals print first, and what they received.
#[derive(Default)]
pub struct TermLog {
    pub written: Vec<u8>,
    pub killed: bool,
    /// Set by a test: the program ends by itself with this status.
    pub exit: Option<i64>,
}

pub struct FakeHost {
    pub now: u64,
    pub utc: i64,
    pub files: BTreeMap<String, Option<Vec<u8>>>,
    pub log: Vec<String>,
    pub power: Option<Power>,
    pub term_output: Vec<u8>,
    pub terms: Vec<Rc<RefCell<TermLog>>>,
    pub logged_in: Option<String>,
    /// The password of `user` and `root`.
    pub password: String,
}

impl Default for FakeHost {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeHost {
    pub fn new() -> FakeHost {
        let mut h = FakeHost {
            now: 10_000,
            // 28 November 2026, 12:42 UTC.
            utc: 1_795_869_720,
            files: BTreeMap::new(),
            log: Vec::new(),
            power: None,
            term_output: Vec::new(),
            terms: Vec::new(),
            logged_in: None,
            password: String::from("velora"),
        };
        for d in [
            "/",
            "/home",
            "/home/user",
            "/home/user/Documents",
            "/home/user/Pictures",
            "/tmp",
            "/mnt",
            "/usr",
            "/usr/share",
            "/usr/share/wallpapers",
            "/bin",
            "/etc",
        ] {
            h.files.insert(String::from(d), None);
        }
        let file = |h: &mut FakeHost, p: &str, d: &[u8]| {
            h.files.insert(String::from(p), Some(Vec::from(d)));
        };
        file(&mut h, "/home/user/notes.txt", b"Lumi\n");
        file(&mut h, "/home/user/todo.md", b"- try the terminal\n");
        file(&mut h, "/home/user/Documents/plan.txt", b"plan\n");
        file(&mut h, "/etc/motd", b"Welcome to Velora OS\n");
        file(&mut h, "/bin/vsh", &[0x7f, b'E', b'L', b'F']);
        h
    }

    fn children(&self, dir: &str) -> Vec<DirEntry> {
        let prefix = if dir == "/" {
            String::from("/")
        } else {
            alloc::format!("{}/", dir)
        };
        self.files
            .iter()
            .filter(|(p, _)| {
                p.starts_with(&prefix) && p.len() > prefix.len() && !p[prefix.len()..].contains('/')
            })
            .map(|(p, d)| DirEntry {
                name: String::from(&p[prefix.len()..]),
                dir: d.is_none(),
                size: d.as_ref().map_or(0, |v| v.len() as u64),
                mtime: 1_795_800_000,
                exec: p.starts_with("/bin/"),
            })
            .collect()
    }
}

struct FakeTerm {
    pending: Vec<u8>,
    log: Rc<RefCell<TermLog>>,
}

impl Term for FakeTerm {
    fn read(&mut self, buf: &mut [u8]) -> TermRead {
        if self.log.borrow().killed || self.log.borrow().exit.is_some() {
            return TermRead::Closed;
        }
        if self.pending.is_empty() {
            return TermRead::Empty;
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        TermRead::Data(n)
    }

    fn write(&mut self, data: &[u8]) {
        self.log.borrow_mut().written.extend_from_slice(data);
        // Echo typed text, as the line discipline would.
        self.pending.extend_from_slice(data);
    }

    fn resize(&mut self, _cols: u32, _rows: u32) {}

    fn kill(&mut self) {
        self.log.borrow_mut().killed = true;
    }

    fn status(&self) -> Option<i64> {
        self.log.borrow().exit
    }
}

impl Host for FakeHost {
    fn now_ms(&self) -> u64 {
        self.now
    }

    fn utc(&self) -> Option<i64> {
        Some(self.utc)
    }

    fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        self.files.get(path).cloned().flatten()
    }

    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        if !self.files.contains_key(&parent(path)) {
            return Err(String::from("no such directory"));
        }
        self.files.insert(String::from(path), Some(Vec::from(data)));
        Ok(())
    }

    fn list(&mut self, path: &str) -> Result<Vec<DirEntry>, String> {
        match self.files.get(path) {
            Some(None) => Ok(self.children(path)),
            _ => Err(String::from("not a directory")),
        }
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        if self.files.contains_key(path) {
            return Err(String::from("already exists"));
        }
        self.files.insert(String::from(path), None);
        Ok(())
    }

    fn remove(&mut self, path: &str) -> Result<(), String> {
        if !self.children(path).is_empty() {
            return Err(String::from("the folder is not empty"));
        }
        self.files
            .remove(path)
            .map(|_| ())
            .ok_or_else(|| String::from("not found"))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let v = self
            .files
            .remove(from)
            .ok_or_else(|| String::from("not found"))?;
        self.files.insert(String::from(to), v);
        Ok(())
    }

    fn spawn_term(
        &mut self,
        _argv: &[&str],
        _cols: u32,
        _rows: u32,
    ) -> Result<Box<dyn Term>, String> {
        let log = Rc::new(RefCell::new(TermLog::default()));
        self.terms.push(log.clone());
        Ok(Box::new(FakeTerm {
            pending: self.term_output.clone(),
            log,
        }))
    }

    fn sysinfo(&mut self) -> SysInfo {
        SysInfo {
            version: String::from("0.16.0"),
            cpu: String::from("QEMU Virtual CPU"),
            cpus: 4,
            mem_total: 512 << 20,
            mem_used: 47 << 20,
            uptime_s: 754,
            processes: 6,
        }
    }

    fn net(&mut self) -> NetState {
        NetState {
            present: true,
            ip: Some([10, 0, 2, 15]),
        }
    }

    fn power(&mut self, p: Power) -> Result<(), String> {
        self.power = Some(p);
        Ok(())
    }

    fn users(&mut self) -> Vec<UserInfo> {
        alloc::vec![
            UserInfo {
                name: String::from("user"),
                uid: 1000,
                home: String::from("/home/user"),
            },
            UserInfo {
                name: String::from("root"),
                uid: 0,
                home: String::from("/root"),
            },
        ]
    }

    fn login(&mut self, name: &str, password: &str) -> Result<UserInfo, String> {
        if password != self.password {
            return Err(String::from("wrong password"));
        }
        let u = self
            .users()
            .into_iter()
            .find(|u| u.name == name)
            .ok_or_else(|| String::from("no user"))?;
        self.logged_in = Some(u.name.clone());
        Ok(u)
    }

    fn log(&mut self, line: &str) {
        self.log.push(String::from(line));
    }

    fn change_password(&mut self, old: &str, new: &str) -> Result<(), String> {
        if self.logged_in.is_none() {
            return Err(String::from("nobody logged in"));
        }
        if old != self.password {
            return Err(String::from("wrong password"));
        }
        self.password = String::from(new);
        Ok(())
    }

    fn display_mode(&self) -> (u32, u32, u32) {
        (1920, 1080, 32)
    }
}

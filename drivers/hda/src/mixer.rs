//! The text of `/dev/mixer`. Reading gives the state, one `key value`
//! per line:
//!
//! ```text
//! volume 70
//! mute 0
//! output 0
//! device speaker
//! device headphones
//! ```
//!
//! (`device` lines list the outputs in order; none without a sound
//! card.) Writing takes the same `volume N`, `mute 0|1` and `output N`
//! lines as commands.

use crate::codec::Jack;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    /// 0..=100.
    pub volume: u32,
    pub mute: bool,
    /// Index into `devices`.
    pub output: usize,
    pub devices: Vec<Jack>,
}

impl Default for State {
    fn default() -> Self {
        State {
            volume: 70,
            mute: false,
            output: 0,
            devices: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Volume(u32),
    Mute(bool),
    Output(usize),
}

impl Jack {
    pub fn key(self) -> &'static str {
        match self {
            Jack::Speaker => "speaker",
            Jack::Headphones => "headphones",
            Jack::LineOut => "line",
            Jack::Digital => "digital",
            Jack::Other => "other",
        }
    }

    pub fn from_key(s: &str) -> Jack {
        match s {
            "speaker" => Jack::Speaker,
            "headphones" => Jack::Headphones,
            "line" => Jack::LineOut,
            "digital" => Jack::Digital,
            _ => Jack::Other,
        }
    }
}

impl State {
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "volume {}", self.volume);
        let _ = writeln!(s, "mute {}", self.mute as u8);
        let _ = writeln!(s, "output {}", self.output);
        for d in &self.devices {
            let _ = writeln!(s, "device {}", d.key());
        }
        s
    }

    /// Reads the text of `/dev/mixer`; unknown lines are skipped.
    pub fn parse(text: &str) -> State {
        let mut st = State::default();
        for line in text.lines() {
            let Some((k, v)) = line.trim().split_once(' ') else {
                continue;
            };
            match k {
                "volume" => st.volume = v.parse().unwrap_or(st.volume).min(100),
                "mute" => st.mute = v == "1",
                "output" => st.output = v.parse().unwrap_or(0),
                "device" => st.devices.push(Jack::from_key(v)),
                _ => {}
            }
        }
        st
    }

    /// Applies a command; false if it does not fit this state.
    pub fn apply(&mut self, c: Command) -> bool {
        match c {
            Command::Volume(v) => self.volume = v.min(100),
            Command::Mute(m) => self.mute = m,
            Command::Output(o) if o < self.devices.len() => self.output = o,
            Command::Output(_) => return false,
        }
        true
    }
}

/// The commands in `text`; `None` if a line is not one.
pub fn commands(text: &str) -> Option<Vec<Command>> {
    let mut out = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (k, v) = line.split_once(' ')?;
        let v = v.trim();
        out.push(match k {
            "volume" => Command::Volume(v.parse().ok()?),
            "mute" => Command::Mute(match v {
                "1" => true,
                "0" => false,
                _ => return None,
            }),
            "output" => Command::Output(v.parse().ok()?),
            _ => return None,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_commands_apply() {
        let st = State {
            volume: 35,
            mute: true,
            output: 1,
            devices: alloc::vec![Jack::Speaker, Jack::Headphones],
        };
        let text = st.to_text();
        assert_eq!(
            text,
            "volume 35\nmute 1\noutput 1\ndevice speaker\ndevice headphones\n"
        );
        assert_eq!(State::parse(&text), st);
        let mut s2 = st.clone();
        for c in commands("volume 120\nmute 0\n\noutput 0\n").unwrap() {
            assert!(s2.apply(c));
        }
        assert_eq!((s2.volume, s2.mute, s2.output), (100, false, 0));
        assert!(!s2.apply(Command::Output(2)));
        assert_eq!(commands("volume loud"), None);
        assert_eq!(commands("bass 3"), None);
    }
}

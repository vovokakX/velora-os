//! TEMPORARY: a line editor that echoes keyboard input, so the keyboard
//! can be tried before the vsh shell exists. It is replaced by vsh in
//! stage 8.

use crate::drivers::ps2;
use crate::{input, kprint, kprintln};
use alloc::string::String;

pub fn run() -> ! {
    kprintln!();
    kprintln!("Keyboard test (temporary, until the vsh shell in stage 8).");
    kprintln!("Type a line and press Enter. Alt+Shift switches between EN and RU.");
    kprintln!("keyboard test ready");
    let mut line = String::new();
    prompt();
    loop {
        let Some(ev) = input::wait(None) else {
            continue;
        };
        if !ev.pressed {
            continue;
        }
        match ev.ch {
            Some('\n') => {
                kprintln!();
                kprintln!("echo: {}", line);
                line.clear();
                prompt();
            }
            Some('\x08') => {
                if line.pop().is_some() {
                    kprint!("\x08 \x08");
                }
            }
            Some(c) if !c.is_control() => {
                line.push(c);
                kprint!("{}", c);
            }
            _ => {}
        }
    }
}

fn prompt() {
    kprint!("\x1b[36m[{}]\x1b[0m velora> ", ps2::layout().short_name());
}

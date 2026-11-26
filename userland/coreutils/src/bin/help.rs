//! help: lists commands, shell built-ins and keys.

#![no_std]
#![no_main]

extern crate alloc;

use vlibc::println;

vlibc::main!(main);

fn main() -> i32 {
    println!("Velora OS commands (in /bin):");
    println!("  ls [-l] [-a] [PATH]   list a directory");
    println!("  cat [FILE...]         print files (or standard input)");
    println!("  cp [-r] SRC... DST    copy files");
    println!("  mv SRC... DST         move or rename");
    println!("  rm [-r] [-f] PATH...  remove files");
    println!("  mkdir [-p] DIR...     create directories");
    println!("  touch FILE...         create empty files");
    println!("  echo [-n] TEXT        print text");
    println!("  plume [FILE]          text editor (^G inside shows the keys)");
    println!("  clear                 clear the screen");
    println!("  ps                    list processes");
    println!("  kill [-s N] PID...    end processes");
    println!("  mem, uptime, date     memory, time since boot, date (UTC)");
    println!("  whoami                current user");
    println!("  dmesg                 kernel messages since boot");
    println!("  velofetch             system summary");
    println!("  reboot, shutdown      restart / power off (root)");
    println!();
    println!("Shell built-ins: cd, pwd, exit, export, unset, history, set.");
    println!("Syntax: a | b, > file, >> file, < file, 2> file, a; b, a && b, a || b,");
    println!("        $VAR, \"text $VAR\", 'literal', NAME=value.");
    println!("Keys: Tab completes, Up/Down history, Ctrl+C cancels, Ctrl+D exits,");
    println!("      Ctrl+L clears, Alt+Shift switches EN/RU.");
    0
}

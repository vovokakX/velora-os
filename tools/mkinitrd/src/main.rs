//! mkinitrd: builds the initramfs archive.
//!
//! ```text
//! mkinitrd --out initramfs.tar --file DEST=SRC[:MODE] ...
//! ```
//!
//! Parent directories of every DEST are added automatically. MODE is octal
//! (default 644; use 755 for programs).

use std::collections::BTreeSet;
use std::process::ExitCode;
use velora_tar::Builder;

fn run() -> Result<(), String> {
    let mut out = None;
    let mut files: Vec<(String, String, u32)> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(it.next().ok_or("--out needs a value")?),
            "--file" => {
                let v = it.next().ok_or("--file needs DEST=SRC[:MODE]")?;
                let (dest, rest) = v.split_once('=').ok_or("--file expects DEST=SRC[:MODE]")?;
                // Windows paths contain ':' (C:\...), so the mode is only
                // taken from a trailing ":NNN" of octal digits.
                let (src, mode) = match rest.rsplit_once(':') {
                    Some((s, m))
                        if !m.is_empty()
                            && m.len() <= 4
                            && m.bytes().all(|c| (b'0'..=b'7').contains(&c)) =>
                    {
                        (s.to_string(), u32::from_str_radix(m, 8).unwrap())
                    }
                    _ => (rest.to_string(), 0o644),
                };
                files.push((dest.trim_start_matches('/').to_string(), src, mode));
            }
            "-h" | "--help" => {
                println!("usage: mkinitrd --out FILE --file DEST=SRC[:MODE] ...");
                return Ok(());
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    let out = out.ok_or("--out is required")?;
    let mut dirs = BTreeSet::new();
    for (dest, _, _) in &files {
        let mut path = String::new();
        let parts: Vec<&str> = dest.split('/').collect();
        for p in &parts[..parts.len() - 1] {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(p);
            dirs.insert(path.clone());
        }
    }
    let mut b = Builder::new();
    for d in &dirs {
        b.add_dir(d, 0o755).map_err(|e| format!("{d}: {e:?}"))?;
    }
    for (dest, src, mode) in &files {
        let data = std::fs::read(src).map_err(|e| format!("cannot read {src}: {e}"))?;
        b.add_file(dest, *mode, &data)
            .map_err(|e| format!("{dest}: {e:?}"))?;
    }
    let tar = b.finish();
    std::fs::write(&out, &tar).map_err(|e| format!("cannot write {out}: {e}"))?;
    println!(
        "mkinitrd: wrote {} ({} KiB, {} files)",
        out,
        tar.len() / 1024,
        files.len()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mkinitrd: error: {e}");
            ExitCode::FAILURE
        }
    }
}

//! mkpasswd: writes an `/etc/shadow` file with password hashes.
//!
//! ```text
//! mkpasswd --out FILE NAME=ENV_VAR ...
//! ```
//!
//! Each password is read from the named environment variable, so it does
//! not appear on the command line. Every run uses new random salts; the
//! file holds only hashes (`$pbkdf2-sha256$...`).

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::process::ExitCode;
use velora_auth::{DEFAULT_ITERATIONS, hash_password};

/// 16 random bytes from the standard library's per-process random keys
/// mixed with the clock.
fn salt() -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, chunk) in out.chunks_mut(8).enumerate() {
        let mut h = RandomState::new().build_hasher();
        h.write_usize(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    out
}

fn run() -> Result<(), String> {
    let mut out = None;
    let mut lines = String::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == "--out" {
            out = Some(it.next().ok_or("--out needs a value")?);
            continue;
        }
        let (name, var) = a
            .split_once('=')
            .ok_or_else(|| format!("expected NAME=ENV_VAR, got '{a}'"))?;
        let password =
            std::env::var(var).map_err(|_| format!("environment variable {var} is not set"))?;
        if password.is_empty() {
            return Err(format!("empty password for {name}"));
        }
        let hash = hash_password(&password, &salt(), DEFAULT_ITERATIONS);
        lines.push_str(&format!("{name}:{hash}:0:0:99999:7:::\n"));
    }
    let out = out.ok_or("--out is required")?;
    std::fs::write(&out, lines).map_err(|e| format!("cannot write {out}: {e}"))?;
    println!("mkpasswd: wrote {out}");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mkpasswd: {e}");
            ExitCode::FAILURE
        }
    }
}

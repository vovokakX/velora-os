//! kstrip: copies an ELF64 file without the parts no program header
//! refers to (debug sections, symbol tables, section headers).
//!
//! ```text
//! kstrip INPUT OUTPUT
//! ```
//!
//! The bootloader loads the kernel by its program headers only, so the
//! result boots the same. The build keeps the full file for
//! `llvm-addr2line`; the installed system gets the small one.

use std::process::ExitCode;

fn get16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn get64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Returns the stripped image.
fn strip(elf: &[u8]) -> Result<Vec<u8>, String> {
    if elf.len() < 64 || &elf[0..4] != b"\x7fELF" || elf[4] != 2 || elf[5] != 1 {
        return Err("not a little-endian ELF64 file".into());
    }
    let phoff = get64(elf, 32) as usize;
    let phentsize = get16(elf, 54) as usize;
    let phnum = get16(elf, 56) as usize;
    if phentsize < 56 || phoff + phnum * phentsize > elf.len() {
        return Err("program headers out of range".into());
    }
    let mut end = phoff + phnum * phentsize;
    for i in 0..phnum {
        let ph = &elf[phoff + i * phentsize..];
        let off = get64(ph, 8) as usize;
        let filesz = get64(ph, 32) as usize;
        if off.checked_add(filesz).is_none_or(|e| e > elf.len()) {
            return Err(format!("segment {} out of range", i));
        }
        end = end.max(off + filesz);
    }
    let mut out = elf[..end].to_vec();
    // No section headers any more.
    out[40..48].fill(0); // e_shoff
    out[60..62].fill(0); // e_shnum
    out[62..64].fill(0); // e_shstrndx
    Ok(out)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: kstrip INPUT OUTPUT");
        return ExitCode::from(2);
    }
    let r = std::fs::read(&args[1])
        .map_err(|e| format!("cannot read {}: {}", args[1], e))
        .and_then(|elf| strip(&elf))
        .and_then(|out| {
            std::fs::write(&args[2], &out)
                .map_err(|e| format!("cannot write {}: {}", args[2], e))?;
            Ok(out.len())
        });
    match r {
        Ok(n) => {
            println!("kstrip: wrote {} ({} KiB)", args[2], n / 1024);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("kstrip: {}", e);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        // Header, one program header at 64, a 16-byte segment at 120,
        // then "debug" bytes and section headers that must go.
        let mut e = vec![0u8; 400];
        e[0..4].copy_from_slice(b"\x7fELF");
        e[4] = 2;
        e[5] = 1;
        e[32..40].copy_from_slice(&64u64.to_le_bytes());
        e[40..48].copy_from_slice(&300u64.to_le_bytes());
        e[54..56].copy_from_slice(&56u16.to_le_bytes());
        e[56..58].copy_from_slice(&1u16.to_le_bytes());
        e[60..62].copy_from_slice(&5u16.to_le_bytes());
        e[64..68].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        e[72..80].copy_from_slice(&120u64.to_le_bytes());
        e[96..104].copy_from_slice(&16u64.to_le_bytes());
        e[120..136].fill(0xaa);
        e
    }

    #[test]
    fn keeps_segments_and_drops_the_rest() {
        let out = strip(&sample()).unwrap();
        assert_eq!(out.len(), 136);
        assert_eq!(&out[120..136], &[0xaa; 16]);
        assert_eq!(get64(&out, 40), 0);
        assert_eq!(get16(&out, 60), 0);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(strip(b"not an elf").is_err());
        let mut e = sample();
        e[96..104].copy_from_slice(&1000u64.to_le_bytes());
        assert!(strip(&e).is_err());
    }
}

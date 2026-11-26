//! PE/COFF (.exe) header parsing.
//!
//! NOT IMPLEMENTED: loading and running PE programs. That needs the Win32
//! personality (a user-space layer that provides the Windows API on top of
//! Velora system calls). This module only recognises PE files and reads
//! their headers, so the kernel can give a precise "not supported yet"
//! answer instead of "unknown format".

pub const MACHINE_I386: u16 = 0x014c;
pub const MACHINE_AMD64: u16 = 0x8664;
pub const MACHINE_ARM64: u16 = 0xaa64;

pub const SUBSYSTEM_WINDOWS_GUI: u16 = 2;
pub const SUBSYSTEM_WINDOWS_CUI: u16 = 3;

#[derive(Debug, PartialEq, Eq)]
pub enum PeError {
    TooShort,
    NoMzHeader,
    NoPeSignature,
    BadOptionalHeader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeInfo {
    pub machine: u16,
    /// PE32+ (64-bit) optional header.
    pub pe32_plus: bool,
    pub sections: u16,
    pub entry_rva: u32,
    pub image_base: u64,
    pub subsystem: u16,
    /// IMAGE_FILE_DLL is set.
    pub is_dll: bool,
}

impl PeInfo {
    pub fn machine_name(&self) -> &'static str {
        match self.machine {
            MACHINE_AMD64 => "x86_64",
            MACHINE_I386 => "x86 (32-bit)",
            MACHINE_ARM64 => "arm64",
            _ => "unknown machine",
        }
    }

    pub fn subsystem_name(&self) -> &'static str {
        match self.subsystem {
            SUBSYSTEM_WINDOWS_GUI => "Windows GUI",
            SUBSYSTEM_WINDOWS_CUI => "Windows console",
            _ => "other",
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// Offset of the "PE\0\0" signature, if the file looks like a PE file.
fn pe_offset(b: &[u8]) -> Result<usize, PeError> {
    if b.len() < 64 {
        return Err(PeError::TooShort);
    }
    if &b[..2] != b"MZ" {
        return Err(PeError::NoMzHeader);
    }
    let off = u32_at(b, 0x3c) as usize;
    if off.checked_add(24).is_none_or(|end| end > b.len()) || &b[off..off + 4] != b"PE\0\0" {
        return Err(PeError::NoPeSignature);
    }
    Ok(off)
}

pub fn probe(b: &[u8]) -> bool {
    pe_offset(b).is_ok()
}

pub fn parse_headers(b: &[u8]) -> Result<PeInfo, PeError> {
    let off = pe_offset(b)?;
    let coff = off + 4;
    let machine = u16_at(b, coff);
    let sections = u16_at(b, coff + 2);
    let opt_size = u16_at(b, coff + 16) as usize;
    let characteristics = u16_at(b, coff + 18);
    let opt = coff + 20;
    if opt_size < 70 || opt + opt_size > b.len() {
        return Err(PeError::BadOptionalHeader);
    }
    let magic = u16_at(b, opt);
    let pe32_plus = match magic {
        0x20b => true,
        0x10b => false,
        _ => return Err(PeError::BadOptionalHeader),
    };
    let entry_rva = u32_at(b, opt + 16);
    let image_base = if pe32_plus {
        u64::from_le_bytes(b[opt + 24..opt + 32].try_into().unwrap())
    } else {
        u32_at(b, opt + 28) as u64
    };
    let subsystem = u16_at(b, opt + 68);
    Ok(PeInfo {
        machine,
        pe32_plus,
        sections,
        entry_rva,
        image_base,
        subsystem,
        is_dll: characteristics & 0x2000 != 0,
    })
}

/// Builds the headers of a tiny 64-bit console .exe (no sections). Used by
/// tests here and in the kernel.
pub fn sample_headers() -> [u8; 512] {
    let mut b = [0u8; 512];
    b[..2].copy_from_slice(b"MZ");
    b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    b[0x80..0x84].copy_from_slice(b"PE\0\0");
    let coff = 0x84;
    b[coff..coff + 2].copy_from_slice(&MACHINE_AMD64.to_le_bytes());
    b[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes()); // optional header size
    b[coff + 18..coff + 20].copy_from_slice(&0x0022u16.to_le_bytes()); // executable, large address aware
    let opt = coff + 20;
    b[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
    b[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes());
    b[opt + 24..opt + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
    b[opt + 68..opt + 70].copy_from_slice(&SUBSYSTEM_WINDOWS_CUI.to_le_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample_exe() {
        let b = sample_headers();
        assert!(probe(&b));
        let info = parse_headers(&b).unwrap();
        assert_eq!(info.machine_name(), "x86_64");
        assert!(info.pe32_plus);
        assert_eq!(info.entry_rva, 0x1000);
        assert_eq!(info.image_base, 0x1_4000_0000);
        assert_eq!(info.subsystem_name(), "Windows console");
        assert!(!info.is_dll);
    }

    #[test]
    fn rejects_non_pe() {
        assert_eq!(parse_headers(b"MZ"), Err(PeError::TooShort));
        assert_eq!(parse_headers(&[0u8; 128]), Err(PeError::NoMzHeader));
        let mut b = sample_headers();
        b[0x80] = b'X';
        assert_eq!(parse_headers(&b), Err(PeError::NoPeSignature));
        let mut b = sample_headers();
        b[0x3c..0x40].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert!(!probe(&b));
        assert!(!probe(
            b"\x7fELF and more bytes to reach the minimum length of sixty-four"
        ));
    }
}

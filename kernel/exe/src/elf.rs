//! ELF64 executables for x86_64.
//!
//! Supported: statically linked `ET_EXEC` files. Rejected with a clear
//! error: 32-bit files, other machines, position-independent (`ET_DYN`)
//! and dynamically linked files (`PT_INTERP`), segments outside user space,
//! overlapping segments and segments that are both writable and executable.

use alloc::vec::Vec;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_GNU_STACK: u32 = 0x6474_e551;

const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const PAGE: u64 = 4096;

#[derive(Debug, PartialEq, Eq)]
pub enum ElfError {
    TooShort,
    BadMagic,
    Not64Bit,
    NotLittleEndian,
    BadVersion,
    WrongMachine(u16),
    NotExecutable(u16),
    /// Position-independent executables need relocation (not supported yet).
    PositionIndependent,
    /// Dynamically linked: needs an interpreter / dynamic linker.
    Dynamic,
    BadProgramHeaders,
    SegmentOutsideFile,
    SegmentOutsideUserSpace,
    WritableAndExecutable,
    Overlap,
    NoLoadableSegments,
    EntryNotExecutable,
    /// The stack is requested to be executable.
    ExecutableStack,
}

impl ElfError {
    pub fn describe(&self) -> &'static str {
        match self {
            ElfError::TooShort => "file is too short for an ELF header",
            ElfError::BadMagic => "not an ELF file",
            ElfError::Not64Bit => "not a 64-bit ELF file",
            ElfError::NotLittleEndian => "not little-endian",
            ElfError::BadVersion => "unknown ELF version",
            ElfError::WrongMachine(_) => "not an x86_64 program",
            ElfError::NotExecutable(_) => "not an executable",
            ElfError::PositionIndependent => {
                "position-independent executables are not supported yet"
            }
            ElfError::Dynamic => "dynamically linked programs are not supported",
            ElfError::BadProgramHeaders => "invalid program headers",
            ElfError::SegmentOutsideFile => "a segment points outside the file",
            ElfError::SegmentOutsideUserSpace => "a segment is outside user space",
            ElfError::WritableAndExecutable => "a segment is both writable and executable (W^X)",
            ElfError::Overlap => "segments overlap",
            ElfError::NoLoadableSegments => "no loadable segments",
            ElfError::EntryNotExecutable => "entry point is not in an executable segment",
            ElfError::ExecutableStack => "executable stack requested",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u64,
    pub mem_size: u64,
    pub file_offset: u64,
    pub file_size: u64,
    pub read: bool,
    pub write: bool,
    pub exec: bool,
}

impl Segment {
    /// First page and one-past-last page covered in memory.
    pub fn page_range(&self) -> (u64, u64) {
        (
            self.vaddr & !(PAGE - 1),
            (self.vaddr + self.mem_size).next_multiple_of(PAGE),
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ElfImage {
    pub entry: u64,
    pub segments: Vec<Segment>,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// True if the data starts with the ELF magic.
pub fn probe(b: &[u8]) -> bool {
    b.len() >= 4 && &b[..4] == b"\x7fELF"
}

/// Parses and validates an executable. Segments must lie in
/// `[user_min, user_max)`.
pub fn parse(b: &[u8], user_min: u64, user_max: u64) -> Result<ElfImage, ElfError> {
    if b.len() < 64 {
        return Err(ElfError::TooShort);
    }
    if !probe(b) {
        return Err(ElfError::BadMagic);
    }
    if b[4] != 2 {
        return Err(ElfError::Not64Bit);
    }
    if b[5] != 1 {
        return Err(ElfError::NotLittleEndian);
    }
    if b[6] != 1 || u32_at(b, 20) != 1 {
        return Err(ElfError::BadVersion);
    }
    let machine = u16_at(b, 18);
    if machine != EM_X86_64 {
        return Err(ElfError::WrongMachine(machine));
    }
    match u16_at(b, 16) {
        ET_EXEC => {}
        ET_DYN => return Err(ElfError::PositionIndependent),
        t => return Err(ElfError::NotExecutable(t)),
    }
    let entry = u64_at(b, 24);
    let phoff = u64_at(b, 32) as usize;
    let phentsize = u16_at(b, 54) as usize;
    let phnum = u16_at(b, 56) as usize;
    if phentsize < 56 || phnum == 0 || phnum > 64 {
        return Err(ElfError::BadProgramHeaders);
    }
    let table_end = phoff
        .checked_add(phentsize * phnum)
        .ok_or(ElfError::BadProgramHeaders)?;
    if table_end > b.len() {
        return Err(ElfError::BadProgramHeaders);
    }

    let mut segments: Vec<Segment> = Vec::new();
    for i in 0..phnum {
        let p = &b[phoff + i * phentsize..phoff + (i + 1) * phentsize];
        let kind = u32_at(p, 0);
        let flags = u32_at(p, 4);
        match kind {
            PT_INTERP | PT_DYNAMIC => return Err(ElfError::Dynamic),
            PT_GNU_STACK if flags & PF_X != 0 => return Err(ElfError::ExecutableStack),
            PT_LOAD => {}
            _ => continue,
        }
        let s = Segment {
            file_offset: u64_at(p, 8),
            vaddr: u64_at(p, 16),
            file_size: u64_at(p, 32),
            mem_size: u64_at(p, 40),
            read: flags & PF_R != 0,
            write: flags & PF_W != 0,
            exec: flags & PF_X != 0,
        };
        if s.mem_size == 0 {
            continue;
        }
        if s.file_size > s.mem_size {
            return Err(ElfError::BadProgramHeaders);
        }
        let file_end = s
            .file_offset
            .checked_add(s.file_size)
            .ok_or(ElfError::SegmentOutsideFile)?;
        if file_end > b.len() as u64 {
            return Err(ElfError::SegmentOutsideFile);
        }
        let mem_end = s
            .vaddr
            .checked_add(s.mem_size)
            .ok_or(ElfError::SegmentOutsideUserSpace)?;
        if s.vaddr < user_min || mem_end > user_max {
            return Err(ElfError::SegmentOutsideUserSpace);
        }
        if s.write && s.exec {
            return Err(ElfError::WritableAndExecutable);
        }
        // Each page gets one set of permissions, so segments may not share
        // a page.
        let (a0, a1) = s.page_range();
        if segments.iter().any(|o| {
            let (b0, b1) = o.page_range();
            a0 < b1 && b0 < a1
        }) {
            return Err(ElfError::Overlap);
        }
        segments.push(s);
    }
    if segments.is_empty() {
        return Err(ElfError::NoLoadableSegments);
    }
    if !segments
        .iter()
        .any(|s| s.exec && entry >= s.vaddr && entry < s.vaddr + s.mem_size)
    {
        return Err(ElfError::EntryNotExecutable);
    }
    Ok(ElfImage { entry, segments })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub struct Ph {
        pub kind: u32,
        pub flags: u32,
        pub offset: u64,
        pub vaddr: u64,
        pub filesz: u64,
        pub memsz: u64,
    }

    /// Builds a minimal ELF64 file with the given program headers and
    /// `body` placed at offset 0x1000.
    pub fn build(etype: u16, machine: u16, entry: u64, phs: &[Ph], body: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 0x1000];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        b[6] = 1;
        b[16..18].copy_from_slice(&etype.to_le_bytes());
        b[18..20].copy_from_slice(&machine.to_le_bytes());
        b[20..24].copy_from_slice(&1u32.to_le_bytes());
        b[24..32].copy_from_slice(&entry.to_le_bytes());
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[52..54].copy_from_slice(&64u16.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&(phs.len() as u16).to_le_bytes());
        for (i, p) in phs.iter().enumerate() {
            let o = 64 + i * 56;
            b[o..o + 4].copy_from_slice(&p.kind.to_le_bytes());
            b[o + 4..o + 8].copy_from_slice(&p.flags.to_le_bytes());
            b[o + 8..o + 16].copy_from_slice(&p.offset.to_le_bytes());
            b[o + 16..o + 24].copy_from_slice(&p.vaddr.to_le_bytes());
            b[o + 32..o + 40].copy_from_slice(&p.filesz.to_le_bytes());
            b[o + 40..o + 48].copy_from_slice(&p.memsz.to_le_bytes());
        }
        b.extend_from_slice(body);
        b
    }

    const MIN: u64 = 0x40_0000;
    const MAX: u64 = 0x7fff_ffff_f000;

    fn code(vaddr: u64) -> Ph {
        Ph {
            kind: PT_LOAD,
            flags: PF_R | PF_X,
            offset: 0x1000,
            vaddr,
            filesz: 16,
            memsz: 16,
        }
    }

    fn data(vaddr: u64) -> Ph {
        Ph {
            kind: PT_LOAD,
            flags: PF_R | PF_W,
            offset: 0x1000,
            vaddr,
            filesz: 8,
            memsz: 0x3000,
        }
    }

    #[test]
    fn valid_static_executable() {
        let f = build(
            ET_EXEC,
            EM_X86_64,
            0x40_1000,
            &[code(0x40_1000), data(0x40_3000)],
            &[0x90; 16],
        );
        let img = parse(&f, MIN, MAX).unwrap();
        assert_eq!(img.entry, 0x40_1000);
        assert_eq!(img.segments.len(), 2);
        assert!(img.segments[0].exec && !img.segments[0].write);
        assert_eq!(img.segments[1].page_range(), (0x40_3000, 0x40_6000));
    }

    #[test]
    fn rejections() {
        let ok = [code(0x40_1000)];
        let body = [0x90u8; 16];
        let e = |f: Vec<u8>| parse(&f, MIN, MAX).unwrap_err();
        assert_eq!(
            e(build(ET_DYN, EM_X86_64, 0x40_1000, &ok, &body)),
            ElfError::PositionIndependent
        );
        assert_eq!(
            e(build(ET_EXEC, 3, 0x40_1000, &ok, &body)),
            ElfError::WrongMachine(3)
        );
        assert_eq!(
            e(build(1, EM_X86_64, 0x40_1000, &ok, &body)),
            ElfError::NotExecutable(1)
        );
        assert_eq!(
            e(build(ET_EXEC, EM_X86_64, 0x50_0000, &ok, &body)),
            ElfError::EntryNotExecutable
        );
        let kernel = [code(0xffff_ffff_8000_0000)];
        assert_eq!(
            e(build(
                ET_EXEC,
                EM_X86_64,
                0xffff_ffff_8000_0000,
                &kernel,
                &body
            )),
            ElfError::SegmentOutsideUserSpace
        );
        let low = [code(0x1000)];
        assert_eq!(
            e(build(ET_EXEC, EM_X86_64, 0x1000, &low, &body)),
            ElfError::SegmentOutsideUserSpace
        );
        let wx = [Ph {
            flags: PF_R | PF_W | PF_X,
            ..code(0x40_1000)
        }];
        assert_eq!(
            e(build(ET_EXEC, EM_X86_64, 0x40_1000, &wx, &body)),
            ElfError::WritableAndExecutable
        );
        let overlap = [code(0x40_1000), data(0x40_1800)];
        assert_eq!(
            e(build(ET_EXEC, EM_X86_64, 0x40_1000, &overlap, &body)),
            ElfError::Overlap
        );
        let interp = [
            code(0x40_1000),
            Ph {
                kind: PT_INTERP,
                flags: PF_R,
                offset: 0,
                vaddr: 0,
                filesz: 0,
                memsz: 0,
            },
        ];
        assert_eq!(
            e(build(ET_EXEC, EM_X86_64, 0x40_1000, &interp, &body)),
            ElfError::Dynamic
        );
        let past_end = [Ph {
            filesz: 1 << 20,
            memsz: 1 << 20,
            ..code(0x40_1000)
        }];
        assert_eq!(
            e(build(ET_EXEC, EM_X86_64, 0x40_1000, &past_end, &body)),
            ElfError::SegmentOutsideFile
        );
        let xstack = [
            code(0x40_1000),
            Ph {
                kind: PT_GNU_STACK,
                flags: PF_R | PF_W | PF_X,
                offset: 0,
                vaddr: 0,
                filesz: 0,
                memsz: 0,
            },
        ];
        assert_eq!(
            e(build(ET_EXEC, EM_X86_64, 0x40_1000, &xstack, &body)),
            ElfError::ExecutableStack
        );
    }

    #[test]
    fn header_checks() {
        assert_eq!(parse(b"\x7fELF", MIN, MAX), Err(ElfError::TooShort));
        let mut f = build(ET_EXEC, EM_X86_64, 0x40_1000, &[code(0x40_1000)], &[0; 16]);
        f[4] = 1;
        assert_eq!(parse(&f, MIN, MAX), Err(ElfError::Not64Bit));
        f[4] = 2;
        f[0] = 0;
        assert_eq!(parse(&f, MIN, MAX), Err(ElfError::BadMagic));
        let none = build(ET_EXEC, EM_X86_64, 0x40_1000, &[], &[]);
        assert_eq!(parse(&none, MIN, MAX), Err(ElfError::BadProgramHeaders));
    }
}

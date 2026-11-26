//! Starting programs from files.
//!
//! Executable formats implement [`ExecFormat`]. The kernel probes each
//! registered format in order:
//! - ELF64 (`velora_exe::elf`): static x86_64 executables, fully supported.
//! - PE (`velora_exe::pe`): recognised, NOT IMPLEMENTED. Running .exe files
//!   needs the Win32 personality; until then `spawn` reports a precise error.
//!
//! The initial stack follows the System V x86_64 convention: `rsp` points
//! at `argc`, followed by `argv[]`, NULL, `envp[]`, NULL and the auxiliary
//! vector, with the strings above them.

use crate::initramfs;
use crate::mm::PAGE_SIZE;
use crate::mm::paging::{AddressSpace, UserKind};
use crate::sched::{self, Personality, Pid, Priority};
use alloc::vec::Vec;
use velora_abi::{MAX_ARG_BYTES, MAX_ARGS, USER_MAX, USER_MIN, auxv};
use velora_exe::{elf, pe};

#[derive(Debug, PartialEq, Eq)]
pub enum ExecError {
    NotFound,
    /// The file is not in any executable format.
    NotExecutable,
    /// The format is known but the file is invalid or uses a feature that
    /// is not supported. The text says why.
    Invalid(&'static str),
    /// The format is recognised but running it is not implemented yet.
    Unsupported(&'static str),
    TooManyArgs,
    OutOfMemory,
}

impl ExecError {
    pub fn errno(&self) -> i64 {
        use velora_abi::errno::*;
        match self {
            ExecError::NotFound => ENOENT,
            ExecError::NotExecutable | ExecError::Invalid(_) | ExecError::Unsupported(_) => ENOEXEC,
            ExecError::TooManyArgs => E2BIG,
            ExecError::OutOfMemory => ENOMEM,
        }
    }
}

/// A loaded program image, ready to run.
pub struct Loaded {
    pub entry: u64,
    pub personality: Personality,
}

/// An executable file format.
pub trait ExecFormat: Sync {
    fn name(&self) -> &'static str;
    /// Cheap check of the file signature.
    fn probe(&self, image: &[u8]) -> bool;
    /// Maps the program into `aspace` and returns its entry point.
    fn load(&self, image: &[u8], aspace: &AddressSpace) -> Result<Loaded, ExecError>;
}

struct ElfFormat;
struct PeFormat;

static FORMATS: [&dyn ExecFormat; 2] = [&ElfFormat, &PeFormat];

impl ExecFormat for ElfFormat {
    fn name(&self) -> &'static str {
        "ELF64"
    }

    fn probe(&self, image: &[u8]) -> bool {
        elf::probe(image)
    }

    fn load(&self, image: &[u8], aspace: &AddressSpace) -> Result<Loaded, ExecError> {
        let parsed = elf::parse(image, USER_MIN, STACK_BOTTOM)
            .map_err(|e| ExecError::Invalid(e.describe()))?;
        for seg in &parsed.segments {
            let kind = if seg.exec {
                UserKind::Code
            } else if seg.write {
                UserKind::Data
            } else {
                UserKind::ReadOnly
            };
            let (start, end) = seg.page_range();
            aspace
                .map_new(start, (end - start) / PAGE_SIZE, kind)
                .map_err(|_| ExecError::OutOfMemory)?;
            let file = &image[seg.file_offset as usize..(seg.file_offset + seg.file_size) as usize];
            // Fresh frames are zeroed, so the .bss part needs no work.
            aspace
                .write_bytes(seg.vaddr, file)
                .map_err(|_| ExecError::OutOfMemory)?;
        }
        Ok(Loaded {
            entry: parsed.entry,
            personality: Personality::Native,
        })
    }
}

impl ExecFormat for PeFormat {
    fn name(&self) -> &'static str {
        "PE (Windows .exe)"
    }

    fn probe(&self, image: &[u8]) -> bool {
        pe::probe(image)
    }

    // NOT IMPLEMENTED: PE loading arrives with the Win32 personality. The
    // headers are parsed so the error names what was found.
    fn load(&self, image: &[u8], _aspace: &AddressSpace) -> Result<Loaded, ExecError> {
        match pe::parse_headers(image) {
            Ok(info) if info.machine == pe::MACHINE_AMD64 => Err(ExecError::Unsupported(
                "Windows x86_64 programs (.exe) need the Win32 personality, which is not implemented yet",
            )),
            Ok(_) => Err(ExecError::Unsupported(
                "Windows programs for other CPUs are not supported",
            )),
            Err(_) => Err(ExecError::Invalid("broken PE headers")),
        }
    }
}

/// Lowest stack address; programs must be linked below it.
const STACK_BOTTOM: u64 = sched::USER_STACK_TOP - sched::USER_STACK_PAGES * PAGE_SIZE;

/// Picks a format for `image` and loads it into `aspace`.
pub fn load_image(image: &[u8], aspace: &AddressSpace) -> Result<Loaded, ExecError> {
    let format = FORMATS
        .iter()
        .find(|f| f.probe(image))
        .ok_or(ExecError::NotExecutable)?;
    format.load(image, aspace)
}

/// Builds the initial stack and returns the user stack pointer.
fn build_stack(
    aspace: &AddressSpace,
    entry: u64,
    argv: &[&str],
    envp: &[&str],
) -> Result<u64, ExecError> {
    let top = sched::USER_STACK_TOP;
    let strings: usize = argv.iter().chain(envp).map(|s| s.len() + 1).sum();
    if argv.len() + envp.len() > MAX_ARGS || strings > MAX_ARG_BYTES {
        return Err(ExecError::TooManyArgs);
    }
    // Strings at the very top.
    let mut blob = Vec::with_capacity(strings);
    let mut addrs = Vec::with_capacity(argv.len() + envp.len());
    let str_base = (top - strings as u64) & !15;
    for s in argv.iter().chain(envp) {
        addrs.push(str_base + blob.len() as u64);
        blob.extend_from_slice(s.as_bytes());
        blob.push(0);
    }
    // Then argc, argv, NULL, envp, NULL, auxv pairs.
    let mut words: Vec<u64> = Vec::new();
    words.push(argv.len() as u64);
    words.extend(&addrs[..argv.len()]);
    words.push(0);
    words.extend(&addrs[argv.len()..]);
    words.push(0);
    words.extend([auxv::PAGESZ, PAGE_SIZE, auxv::ENTRY, entry, auxv::NULL, 0]);
    // rsp must be 16-byte aligned and point at argc.
    let rsp = (str_base - words.len() as u64 * 8) & !15;
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    aspace
        .write_bytes(rsp, &bytes)
        .map_err(|_| ExecError::OutOfMemory)?;
    aspace
        .write_bytes(str_base, &blob)
        .map_err(|_| ExecError::OutOfMemory)?;
    Ok(rsp)
}

/// Starts the program at `path` (in the initramfs) with the given
/// arguments and environment. `argv[0]` should be the program path.
pub fn spawn(path: &str, argv: &[&str], envp: &[&str]) -> Result<Pid, ExecError> {
    let image = initramfs::read(path).ok_or(ExecError::NotFound)?;
    let aspace = AddressSpace::new().map_err(|_| ExecError::OutOfMemory)?;
    let loaded = load_image(image, &aspace)?;
    aspace
        .map_new(STACK_BOTTOM, sched::USER_STACK_PAGES, UserKind::Data)
        .map_err(|_| ExecError::OutOfMemory)?;
    let rsp = build_stack(&aspace, loaded.entry, argv, envp)?;
    let name = path.rsplit('/').next().unwrap_or(path);
    sched::spawn_user(
        name,
        aspace,
        loaded.entry,
        rsp,
        loaded.personality,
        Priority::Normal,
    )
    .map_err(|_| ExecError::OutOfMemory)
}

/// Checks that an address range is in user space (for `mmap`).
pub fn user_range_ok(addr: u64, len: u64) -> bool {
    addr >= USER_MIN && addr.checked_add(len).is_some_and(|e| e <= USER_MAX)
}

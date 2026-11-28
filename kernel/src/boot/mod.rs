//! Boot information handed over by the bootloader.
//!
//! Limine's responses live in bootloader-reclaimable memory. The kernel
//! copies everything it needs into [`BootInfo`] with [`capture`], and after
//! [`release_bootloader_memory`] the raw responses must not be used again
//! (`limine::Request::response` panics if they are).

pub mod limine;

use crate::sync::RacyCell;
use core::sync::atomic::{AtomicBool, Ordering};
use limine::MemoryKind;

pub const MAX_REGIONS: usize = 512;
pub const MAX_MODULES: usize = 8;
const MAX_MODULE_PATH: usize = 64;

/// A file the bootloader loaded next to the kernel (e.g. the initramfs).
#[derive(Clone, Copy, Debug)]
pub struct Module {
    pub phys: u64,
    pub size: u64,
    path: [u8; MAX_MODULE_PATH],
    path_len: usize,
}

impl Module {
    pub fn path(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }
}
const MAX_CMDLINE: usize = 512;
const MAX_NAME: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct MemRegion {
    pub base: u64,
    pub length: u64,
    pub kind: MemoryKind,
}

impl MemRegion {
    pub fn end(&self) -> u64 {
        self.base + self.length
    }
}

pub struct BootInfo {
    pub hhdm_offset: u64,
    pub kernel_phys: u64,
    pub kernel_virt: u64,
    regions: [MemRegion; MAX_REGIONS],
    region_count: usize,
    /// Regions that did not fit into the table (reported, then ignored).
    pub regions_dropped: usize,
    cmdline: [u8; MAX_CMDLINE],
    cmdline_len: usize,
    loader: [u8; MAX_NAME],
    loader_len: usize,
    /// Virtual (HHDM) address of the ACPI RSDP, if any.
    pub rsdp: Option<u64>,
    /// UNIX time at boot, if the bootloader knew it.
    pub boot_time: Option<i64>,
    modules: [Module; MAX_MODULES],
    module_count: usize,
    /// GPT GUID of the partition the kernel was loaded from (None when it
    /// came from a CD or the network).
    pub boot_partition: Option<[u8; 16]>,
}

impl BootInfo {
    pub fn regions(&self) -> &[MemRegion] {
        &self.regions[..self.region_count]
    }

    pub fn cmdline(&self) -> &str {
        core::str::from_utf8(&self.cmdline[..self.cmdline_len]).unwrap_or("")
    }

    /// Files loaded by the bootloader (they stay in memory; their memory
    /// is "executable and modules" and is never reclaimed).
    pub fn modules(&self) -> &[Module] {
        &self.modules[..self.module_count]
    }

    /// Bootloader name and version, e.g. "Limine 12.11.1".
    pub fn bootloader(&self) -> &str {
        core::str::from_utf8(&self.loader[..self.loader_len]).unwrap_or("")
    }
}

const EMPTY_REGION: MemRegion = MemRegion {
    base: 0,
    length: 0,
    kind: MemoryKind::Reserved,
};

static INFO: RacyCell<BootInfo> = RacyCell::new(BootInfo {
    hhdm_offset: 0,
    kernel_phys: 0,
    kernel_virt: 0,
    regions: [EMPTY_REGION; MAX_REGIONS],
    region_count: 0,
    regions_dropped: 0,
    cmdline: [0; MAX_CMDLINE],
    cmdline_len: 0,
    loader: [0; MAX_NAME],
    loader_len: 0,
    rsdp: None,
    boot_time: None,
    modules: [EMPTY_MODULE; MAX_MODULES],
    module_count: 0,
    boot_partition: None,
});

const EMPTY_MODULE: Module = Module {
    phys: 0,
    size: 0,
    path: [0; MAX_MODULE_PATH],
    path_len: 0,
};
static CAPTURED: AtomicBool = AtomicBool::new(false);

fn copy_str(dst: &mut [u8], src: &str) -> usize {
    // Cut at a character boundary so the copy stays valid UTF-8.
    let mut n = src.len().min(dst.len());
    while !src.is_char_boundary(n) {
        n -= 1;
    }
    dst[..n].copy_from_slice(&src.as_bytes()[..n]);
    n
}

/// Copies the bootloader responses into kernel memory. Must run once,
/// before the memory manager starts.
pub fn capture() -> Result<(), &'static str> {
    let hhdm = limine::HHDM.response().ok_or("no HHDM response")?;
    let map = limine::MEMMAP.response().ok_or("no memory map")?;
    let exe = limine::EXECUTABLE_ADDRESS
        .response()
        .ok_or("no executable address")?;

    // SAFETY: single-threaded early boot; nothing reads INFO yet.
    let info = unsafe { &mut *INFO.get() };
    info.hhdm_offset = hhdm.offset;
    info.kernel_phys = exe.physical_base;
    info.kernel_virt = exe.virtual_base;
    for e in map.entries() {
        if info.region_count < MAX_REGIONS {
            info.regions[info.region_count] = MemRegion {
                base: e.base,
                length: e.length,
                kind: e.kind(),
            };
            info.region_count += 1;
        } else {
            info.regions_dropped += 1;
        }
    }
    if let Some(c) = limine::CMDLINE.response() {
        info.cmdline_len = copy_str(&mut info.cmdline, c.cmdline());
    }
    if let Some(b) = limine::BOOTLOADER_INFO.response() {
        let n = copy_str(&mut info.loader, b.name());
        let mut len = n;
        if n < MAX_NAME {
            info.loader[n] = b' ';
            len += 1 + copy_str(&mut info.loader[n + 1..], b.version());
        }
        info.loader_len = len;
    }
    info.rsdp = limine::RSDP.response().map(|r| r.address as u64);
    if let Some(m) = limine::MODULES.response() {
        for f in m.modules().iter().take(MAX_MODULES) {
            let mut module = EMPTY_MODULE;
            module.phys = f.address as u64 - hhdm.offset;
            module.size = f.size;
            module.path_len = copy_str(&mut module.path, f.path());
            info.modules[info.module_count] = module;
            info.module_count += 1;
        }
    }
    info.boot_time = limine::DATE_AT_BOOT.response().map(|d| d.timestamp);
    info.boot_partition = limine::EXECUTABLE_FILE
        .response()
        .and_then(|r| r.file())
        .filter(|f| f.media_type == limine::MEDIA_GENERIC && f.gpt_part_uuid != [0; 16])
        .map(|f| f.gpt_part_uuid);
    CAPTURED.store(true, Ordering::SeqCst);
    Ok(())
}

/// The captured boot information. Panics if [`capture`] has not run.
pub fn info() -> &'static BootInfo {
    assert!(
        CAPTURED.load(Ordering::SeqCst),
        "boot info used before capture"
    );
    // SAFETY: written once in `capture`, read-only afterwards.
    unsafe { &*INFO.get() }
}

/// Summary of the physical memory map.
pub struct MemorySummary {
    pub usable: u64,
    pub reclaimable: u64,
    pub regions: usize,
}

pub fn memory_summary() -> MemorySummary {
    let mut s = MemorySummary {
        usable: 0,
        reclaimable: 0,
        regions: 0,
    };
    for r in info().regions() {
        s.regions += 1;
        match r.kind {
            MemoryKind::Usable => s.usable += r.length,
            MemoryKind::BootloaderReclaimable => s.reclaimable += r.length,
            _ => {}
        }
    }
    s
}

/// Kernel command line, empty if the bootloader did not pass one.
pub fn cmdline() -> &'static str {
    info().cmdline()
}

/// Returns true if `flag` appears as a whitespace-separated word on the
/// kernel command line.
pub fn cmdline_has(flag: &str) -> bool {
    cmdline().split_ascii_whitespace().any(|w| w == flag)
}

/// Gives bootloader-reclaimable memory to the frame allocator. After this,
/// Limine responses, the boot stack and Limine's page tables are gone.
/// Returns the number of bytes reclaimed.
pub fn release_bootloader_memory() -> u64 {
    limine::invalidate_responses();
    let mut bytes = 0;
    for r in info().regions() {
        if r.kind == MemoryKind::BootloaderReclaimable {
            bytes += crate::mm::frame::release_region(r.base, r.length);
        }
    }
    bytes
}

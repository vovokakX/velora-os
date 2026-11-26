//! Boot information handed over by the bootloader.

pub mod limine;

use limine::MemoryKind;

/// Summary of the physical memory map.
pub struct MemorySummary {
    pub usable: u64,
    pub reclaimable: u64,
    pub total_reported: u64,
    pub regions: usize,
}

pub fn memory_summary() -> Option<MemorySummary> {
    let map = limine::MEMMAP.response()?;
    let mut s = MemorySummary {
        usable: 0,
        reclaimable: 0,
        total_reported: 0,
        regions: 0,
    };
    for e in map.entries() {
        s.regions += 1;
        match e.kind() {
            MemoryKind::Usable => s.usable += e.length,
            MemoryKind::BootloaderReclaimable | MemoryKind::AcpiReclaimable => {
                s.reclaimable += e.length
            }
            _ => {}
        }
        if e.kind() != MemoryKind::Reserved && e.kind() != MemoryKind::BadMemory {
            s.total_reported += e.length;
        }
    }
    Some(s)
}

/// Kernel command line, empty if the bootloader did not pass one.
pub fn cmdline() -> &'static str {
    limine::CMDLINE
        .response()
        .map(|r| r.cmdline())
        .unwrap_or("")
}

/// Returns true if `flag` appears as a whitespace-separated word on the
/// kernel command line.
pub fn cmdline_has(flag: &str) -> bool {
    cmdline().split_ascii_whitespace().any(|w| w == flag)
}

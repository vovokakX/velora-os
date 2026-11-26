//! Device drivers that live inside the kernel crate because they need
//! kernel services directly. Hardware-independent parts are in the
//! `drivers/` crates (for example `velora-input`).

pub mod ahci;
pub mod block;
pub mod dma;
pub mod nvme;
pub mod pci;
pub mod ps2;
pub mod rtc;
pub mod virtio_blk;

/// Enumerates PCI, starts the disk drivers and reads the RTC.
pub fn init() {
    let acpi = crate::acpi::get();
    let (functions, ecam) = pci::init(&acpi.mcfg);
    crate::kok!(
        "PCI: {} functions ({})",
        functions,
        if ecam { "ECAM" } else { "I/O ports" }
    );
    let disks = virtio_blk::probe_all() + ahci::probe_all() + nvme::probe_all();
    for d in block::devices() {
        crate::kok!(
            "disk {}: {}, {} MiB ({} sectors of {} bytes)",
            d.name(),
            d.description(),
            d.size_bytes() / (1024 * 1024),
            d.sector_count(),
            d.sector_size()
        );
    }
    if disks == 0 {
        crate::kinfo!("no disks found");
    }
    match rtc::read(acpi.fadt.century_register) {
        Some(t) => {
            crate::time::set_wall_clock(t.to_unix());
            crate::kok!("RTC: {}", t);
        }
        None => crate::kwarn!("RTC: no valid time"),
    }
}

//! Device drivers that live inside the kernel crate because they need
//! kernel services directly. Hardware-independent parts are in the
//! `drivers/` crates (for example `velora-input`).

pub mod ahci;
pub mod block;
pub mod dma;
pub mod e1000;
pub mod msix;
pub mod nvme;
pub mod pci;
pub mod ps2;
pub mod rtc;
pub mod virtio_blk;
pub mod virtio_net;
pub mod xhci;

/// A network card: sends and receives Ethernet frames.
pub trait NetDevice: Send {
    /// Model, for the boot log.
    fn name(&self) -> &str;
    fn mac(&self) -> [u8; 6];
    /// Queues one frame; false if it cannot be sent now (it is lost).
    fn send(&mut self, frame: &[u8]) -> bool;
    /// The next received frame, if one is waiting.
    fn receive(&mut self) -> Option<alloc::vec::Vec<u8>>;
}

/// The first network card: virtio-net, then Intel e1000 family.
pub fn probe_network() -> Option<alloc::boxed::Box<dyn NetDevice>> {
    if let Some(v) = virtio_net::probe() {
        return Some(alloc::boxed::Box::new(v));
    }
    e1000::probe().map(|n| alloc::boxed::Box::new(n) as alloc::boxed::Box<dyn NetDevice>)
}

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
    let usb = xhci::probe_all();
    if usb > 0 {
        crate::kok!("USB: {} device(s)", usb);
    }
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
    if block::scan_partitions() > 0 {
        for d in block::devices()
            .iter()
            .filter(|d| d.partition_guid().is_some())
        {
            crate::kok!(
                "partition {}: {}, {} MiB",
                d.name(),
                d.description(),
                d.size_bytes() / (1024 * 1024)
            );
        }
    }
    match rtc::read(acpi.fadt.century_register) {
        Some(t) => {
            crate::time::set_wall_clock(t.to_unix());
            crate::kok!("RTC: {}", t);
        }
        None => crate::kwarn!("RTC: no valid time"),
    }
}

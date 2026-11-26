//! Stage 6 tests: PCI, block devices (virtio-blk, AHCI, NVMe), RTC.
//!
//! The test runner attaches three 32 MiB disks with "VELORA-DISK-<KIND>"
//! at LBA 1 and checks afterwards on the host that "VELORA-WRITE-<KIND>"
//! reached LBA 2 (see tests/qemu_run.py).

use crate::drivers::block::{self, BlockDevice, BlockError};
use crate::drivers::pci;
use crate::{boot, kprint, time};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

fn disk(kind: &str) -> Arc<dyn BlockDevice> {
    block::devices()
        .into_iter()
        .find(|d| d.description().to_ascii_lowercase().contains(kind))
        .unwrap_or_else(|| panic!("no {} disk found", kind))
}

pub fn pci_finds_devices() {
    let devs = pci::devices();
    kprint!("({} functions) ", devs.len());
    assert!(
        devs.iter()
            .any(|d| d.vendor == 0x8086 && d.class == 0x06 && d.subclass == 0x00),
        "no host bridge"
    );
    assert!(
        devs.iter().any(|d| d.is(0x01, 0x06, 0x01)),
        "no AHCI controller"
    );
    assert!(
        devs.iter().any(|d| d.is(0x01, 0x08, 0x02)),
        "no NVMe controller"
    );
    assert!(devs.iter().any(|d| d.vendor == 0x1af4), "no virtio device");
    let ahci = devs.iter().find(|d| d.is(0x01, 0x06, 0x01)).unwrap();
    let (_, size) = ahci.bars[5].memory().expect("AHCI BAR5");
    assert!(size >= 0x1000, "AHCI BAR5 too small: {:#x}", size);
}

pub fn three_disks_found() {
    let names: Vec<_> = block::devices()
        .iter()
        .map(|d| alloc::string::String::from(d.name()))
        .collect();
    kprint!("({:?}) ", names);
    for kind in ["virtio", "ahci", "nvme"] {
        let d = disk(kind);
        assert_eq!(d.sector_size(), 512);
        assert_eq!(d.size_bytes(), 32 * 1024 * 1024, "{} size", kind);
    }
    assert!(
        block::find("vd0").is_some()
            && block::find("sd0").is_some()
            && block::find("nvme0n1").is_some()
    );
}

fn exercise(kind: &str) {
    let d = disk(kind);
    let ss = d.sector_size();

    // 1. The signature the test runner wrote on the host.
    let mut sector = vec![0u8; ss];
    d.read(1, &mut sector).expect("read LBA 1");
    let want = alloc::format!("VELORA-DISK-{}", kind.to_ascii_uppercase());
    assert_eq!(
        &sector[..want.len()],
        want.as_bytes(),
        "{}: signature",
        kind
    );

    // 2. A large transfer that spans several bounce-buffer chunks.
    let len = 200 * 1024;
    let pattern: Vec<u8> = (0..len).map(|i| ((i * 31 + i / 512) % 251) as u8).collect();
    d.write(1000, &pattern).expect("large write");
    let mut back = vec![0u8; len];
    d.read(1000, &mut back).expect("large read");
    assert!(back == pattern, "{}: data read back differs", kind);

    // 3. Single sectors in the middle of that range are consistent too.
    d.read(1000 + 300, &mut sector).unwrap();
    assert_eq!(&sector[..], &pattern[300 * ss..301 * ss]);

    // 4. Errors.
    let last = d.sector_count() - 1;
    assert_eq!(
        d.read(last, &mut vec![0u8; 2 * ss]),
        Err(BlockError::OutOfRange)
    );
    assert_eq!(
        d.read(0, &mut vec![0u8; ss + 1]),
        Err(BlockError::BadLength)
    );
    d.read(last, &mut sector).expect("last sector");

    // 5. The marker the host checks after QEMU exits.
    sector.fill(0);
    let mark = alloc::format!("VELORA-WRITE-{}", kind.to_ascii_uppercase());
    sector[..mark.len()].copy_from_slice(mark.as_bytes());
    d.write(2, &sector).expect("write marker");
    d.flush().expect("flush");
}

pub fn virtio_blk_read_write() {
    exercise("virtio");
}

pub fn ahci_read_write() {
    exercise("ahci");
}

pub fn nvme_read_write() {
    exercise("nvme");
}

pub fn disks_are_independent() {
    // The same LBA on different disks holds different data.
    let mut a = vec![0u8; 512];
    let mut b = vec![0u8; 512];
    disk("virtio").read(1, &mut a).unwrap();
    disk("nvme").read(1, &mut b).unwrap();
    assert_ne!(a, b);
}

pub fn rtc_matches_bootloader_time() {
    let now = time::realtime_ns().expect("wall clock not set") / 1_000_000_000;
    kprint!("({}) ", velora_rtc::DateTime::from_unix(now as i64));
    assert!(now > 1_700_000_000, "RTC time before 2023");
    if let Some(boot) = boot::info().boot_time {
        let diff = now as i64 - boot;
        assert!(
            (-5..120).contains(&diff),
            "RTC and bootloader differ by {} s",
            diff
        );
    }
    let a = time::realtime_ns().unwrap();
    time::sleep_ms(20);
    let b = time::realtime_ns().unwrap();
    assert!(b - a >= 20_000_000, "wall clock does not advance");
}

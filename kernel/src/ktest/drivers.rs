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

/// The 32 MiB signature disk of a kind (the file system tests use a
/// separate 128 MiB virtio disk).
fn disk(kind: &str) -> Arc<dyn BlockDevice> {
    block::devices()
        .into_iter()
        .find(|d| {
            d.description().to_ascii_lowercase().contains(kind)
                && d.size_bytes() == 32 * 1024 * 1024
        })
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
    // Completion is polled, but the MSI-X interrupt must arrive as well.
    time::sleep_ms(10);
    let n = crate::drivers::virtio_blk::interrupt_count();
    kprint!("({} MSI-X interrupts) ", n);
    assert!(n > 0, "no MSI-X interrupt from virtio-blk");
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

/// The test runner attaches a USB keyboard, mouse and flash drive (with
/// hello.txt on a FAT32 partition) to an xHCI controller.
pub fn usb_devices_found() {
    let (keyboards, mice) = crate::drivers::xhci::hid_devices();
    assert_eq!((keyboards, mice), (1, 1), "USB keyboard and mouse");
    let stick = block::devices()
        .into_iter()
        .find(|d| d.name() == "usb0")
        .expect("no USB drive");
    assert_eq!(stick.size_bytes(), 48 * 1024 * 1024);
    assert!(block::devices().iter().any(|d| d.name() == "usb0p1"));
}

pub fn usb_storage_files() {
    use super::fs::{read_file, write_file};
    let text = read_file("/mnt/usb0p1/hello.txt").expect("read hello.txt");
    assert_eq!(text, b"VELORA-USB-STICK\n");
    // Larger than one 64 KiB transfer, so several SCSI commands are used.
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
    write_file("/mnt/usb0p1/big.bin", &data);
    let back = read_file("/mnt/usb0p1/big.bin").expect("read big.bin");
    assert!(back == data, "big.bin differs after reading it back");
    // The host checks this file after the run.
    let boot = if read_file("/mnt/usb0p1/usb.txt").is_ok() {
        2
    } else {
        1
    };
    let msg = alloc::format!("written over USB, boot {}\n", boot);
    write_file("/mnt/usb0p1/usb.txt", msg.as_bytes());
    crate::fs::with(|v| v.sync_all()).expect("sync");
}

/// The test runner moves the mouse and clicks through the QEMU monitor.
pub fn usb_mouse_moves() {
    while crate::input::pop_mouse().is_some() {}
    // QEMU's xHCI offers MSI-X: reports arrive by interrupt (D51).
    let modes = crate::drivers::xhci::irq_modes();
    assert!(
        !modes.is_empty() && modes.iter().all(|&m| m == "MSI-X"),
        "xhci interrupts: {:?}",
        modes
    );
    let interrupts = crate::drivers::xhci::interrupt_count();
    crate::kprintln!();
    crate::kprintln!("ktest: mouse: waiting for movement");
    let start = crate::arch::cpu::rdtsc();
    let (mut dx, mut dy, mut clicked) = (0i32, 0i32, false);
    while dx < 12 || !clicked {
        if let Some(m) = crate::input::pop_mouse() {
            dx += m.dx as i32;
            dy += m.dy as i32;
            clicked |= m.buttons & 1 != 0;
            continue;
        }
        assert!(
            crate::arch::cpu::rdtsc() - start < 20 * time::tsc_hz(),
            "no mouse movement within 20 s (dx {}, clicked {})",
            dx,
            clicked
        );
        crate::sched::sleep_ms(10);
    }
    let interrupts = crate::drivers::xhci::interrupt_count() - interrupts;
    kprint!(
        "ktest: mouse: dx {} dy {} interrupts {} ",
        dx,
        dy,
        interrupts
    );
    assert_eq!((dx, dy), (12, -7));
    assert!(interrupts > 0, "the mouse reports came without interrupts");
}

pub fn rtc_matches_bootloader_time() {
    let now = time::realtime_ns().expect("wall clock not set") / 1_000_000_000;
    kprint!("({}) ", velora_rtc::DateTime::from_unix(now as i64));
    assert!(now > 1_700_000_000, "RTC time before 2023");
    if let Some(boot) = boot::info().boot_time {
        // The bootloader read the clock just before the kernel started;
        // the tests before this one may take minutes on a slow emulator.
        let uptime = (time::uptime_ms() / 1000) as i64;
        let diff = now as i64 - boot - uptime;
        assert!(
            (-5..30).contains(&diff),
            "RTC and bootloader differ by {} s (after {} s of uptime)",
            diff,
            uptime
        );
    }
    let a = time::realtime_ns().unwrap();
    time::sleep_ms(20);
    let b = time::realtime_ns().unwrap();
    assert!(b - a >= 20_000_000, "wall clock does not advance");
}

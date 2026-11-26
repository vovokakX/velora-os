//! mkusb: builds the Velora image for USB drives.
//!
//! ```text
//! mkusb --out velora-usb.img [--size-mib 64] --file DEST=SRC ...
//! ```
//!
//! The image is a whole disk: a protective MBR, a GPT and one EFI system
//! partition (FAT32) that fills the disk. Every `--file` is copied into
//! that partition at DEST (for example `EFI/BOOT/BOOTX64.EFI`). UEFI
//! firmware boots `EFI/BOOT/BOOTX64.EFI` from such a disk, and Limine
//! reads `boot():/...` paths from the same partition. GUIDs, the volume
//! serial number and file times are fixed, so the same inputs give the
//! same image.

use std::process::ExitCode;
use velora_fat::FatVolume;
use velora_gpt::{Guid, Partition};
use velora_vfs::{BlockIo, FileSystem, FsError, Kind, MemDisk, NewAttr, Slice};

const MIB: u64 = 1024 * 1024;
const SECTOR: usize = 512;
/// 2026-01-01 00:00:00 UTC, the time stamp of every file.
const TIME: u64 = 1_767_225_600;
const DISK_GUID: Guid = Guid::parse_const("56454C4F-5241-5553-4200-000000000001");
const ESP_GUID: Guid = Guid::parse_const("56454C4F-5241-5553-4200-000000000002");

struct Args {
    out: String,
    size_mib: u64,
    files: Vec<(String, String)>,
}

fn parse_args() -> Result<Args, String> {
    let mut out = None;
    let mut size_mib = 64;
    let mut files = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--out" => out = Some(value()?),
            "--size-mib" => {
                size_mib = value()?
                    .parse()
                    .map_err(|_| "--size-mib needs a number".to_string())?
            }
            "--file" => {
                let v = value()?;
                let (dest, src) = v.split_once('=').ok_or("--file expects DEST=SRC")?;
                files.push((dest.to_string(), src.to_string()));
            }
            "-h" | "--help" => {
                println!("usage: mkusb --out FILE [--size-mib N] --file DEST=SRC ...");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(Args {
        out: out.ok_or("--out is required")?,
        size_mib,
        files,
    })
}

/// Creates `path` (and its parent directories) with `data`.
fn add_file(fs: &mut dyn FileSystem, path: &str, data: &[u8]) -> Result<(), FsError> {
    let attr = NewAttr {
        mode: 0o644,
        uid: 0,
        gid: 0,
    };
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let (name, dirs) = parts.split_last().ok_or(FsError::Invalid)?;
    let mut dir = fs.root();
    for d in dirs {
        dir = match fs.lookup(dir, d) {
            Ok(ino) => ino,
            Err(FsError::NotFound) => fs.create(dir, d, Kind::Dir, attr, TIME)?,
            Err(e) => return Err(e),
        };
    }
    let ino = fs.create(dir, name, Kind::File, attr, TIME)?;
    if !data.is_empty() {
        fs.write(ino, 0, data, TIME)?;
    }
    Ok(())
}

/// Builds the image in memory.
fn build(size_mib: u64, files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let sectors = size_mib * MIB / SECTOR as u64;
    if size_mib < 8 {
        return Err("the image is too small".into());
    }
    let mut disk = MemDisk::new(SECTOR, sectors);
    let (_, last) = velora_gpt::usable_range(sectors);
    let first = 2048;
    // End the partition on a 1 MiB boundary.
    let end = (last + 1) / 2048 * 2048 - 1;
    if end <= first + 2048 {
        return Err("the image is too small".into());
    }
    let esp = Partition {
        type_guid: Guid::ESP,
        unique_guid: ESP_GUID,
        first_lba: first,
        last_lba: end,
        attributes: 0,
        name: String::from("Velora OS"),
    };
    velora_gpt::write(&mut disk, DISK_GUID, std::slice::from_ref(&esp))
        .map_err(|e| format!("GPT: {e}"))?;
    let mut part = Slice {
        dev: disk,
        start: esp.first_lba,
        count: esp.sectors(),
    };
    velora_fat::format_fat32(&mut part, "VELORA", 0x5645_4c52)
        .map_err(|e| format!("FAT32 (is the image at least 40 MiB?): {e}"))?;
    let mut vol = FatVolume::mount(part, false).map_err(|e| format!("FAT32: {e}"))?;
    for (dest, data) in files {
        add_file(&mut vol, dest, data).map_err(|e| format!("{dest}: {e}"))?;
    }
    vol.sync().map_err(|e| format!("FAT32: {e}"))?;
    let mut disk = vol.into_device().dev;
    disk.flush().map_err(|e| format!("flush: {e}"))?;
    Ok(disk.durable_image())
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let mut files = Vec::new();
    for (dest, src) in &args.files {
        let data = std::fs::read(src).map_err(|e| format!("cannot read {src}: {e}"))?;
        files.push((dest.clone(), data));
    }
    let image = build(args.size_mib, &files)?;
    std::fs::write(&args.out, &image).map_err(|e| format!("cannot write {}: {e}", args.out))?;
    println!(
        "mkusb: wrote {} ({} MiB, {} files)",
        args.out,
        image.len() as u64 / MIB,
        files.len()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mkusb: error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_has_gpt_esp_and_files() {
        let files = vec![
            (
                "EFI/BOOT/BOOTX64.EFI".to_string(),
                vec![0x4d, 0x5a, 1, 2, 3],
            ),
            ("boot/limine.conf".to_string(), b"timeout: 0\n".to_vec()),
            ("boot/velora".to_string(), vec![7u8; 300_000]),
        ];
        let img = build(64, &files).unwrap();
        assert_eq!(img.len() as u64, 64 * MIB);
        // Protective MBR signature and the GPT header.
        assert_eq!(&img[510..512], &[0x55, 0xaa]);
        assert_eq!(&img[512..520], b"EFI PART");
        let mut disk = MemDisk::from_image(SECTOR, img.clone());
        let parts = velora_gpt::read(&mut disk).unwrap().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].type_guid, Guid::ESP);
        assert_eq!(parts[0].first_lba, 2048);
        let part = Slice {
            dev: disk,
            start: parts[0].first_lba,
            count: parts[0].sectors(),
        };
        let mut vol = FatVolume::mount(part, true).unwrap();
        for (path, data) in &files {
            let mut ino = vol.root();
            for p in path.split('/') {
                ino = vol.lookup(ino, p).unwrap();
            }
            let mut buf = vec![0u8; data.len()];
            assert_eq!(vol.read(ino, 0, &mut buf).unwrap(), data.len());
            assert_eq!(&buf, data);
        }
        // Same inputs, same image.
        assert_eq!(build(64, &files).unwrap(), img);
    }

    #[test]
    fn too_small_image_is_refused() {
        assert!(build(1, &[]).is_err());
    }
}

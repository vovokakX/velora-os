//! mkiso: builds the bootable Velora ISO.
//!
//! ```text
//! mkiso --out velora.iso [--label VELORA] --efi BOOTX64.EFI --file DEST=SRC ...
//! ```
//!
//! `--efi` is the UEFI bootloader. It goes into a small FAT16 image
//! (`EFI/BOOT/BOOTX64.EFI`), which UEFI firmware boots through the El Torito
//! catalog. Every `--file` goes into the ISO 9660 tree at DEST, where Limine
//! finds its config and the kernel.

mod iso;

use std::process::ExitCode;
use velora_fat::ImageBuilder;

const MIB: usize = 1024 * 1024;

struct Args {
    out: String,
    label: String,
    efi: String,
    files: Vec<(String, String)>,
}

fn parse_args() -> Result<Args, String> {
    let mut out = None;
    let mut efi = None;
    let mut label = String::from("VELORA");
    let mut files = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--out" => out = Some(value()?),
            "--label" => label = value()?,
            "--efi" => efi = Some(value()?),
            "--file" => {
                let v = value()?;
                let (dest, src) = v.split_once('=').ok_or("--file expects DEST=SRC")?;
                files.push((dest.to_string(), src.to_string()));
            }
            "-h" | "--help" => {
                println!(
                    "usage: mkiso --out FILE [--label NAME] --efi BOOTX64.EFI --file DEST=SRC ..."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(Args {
        out: out.ok_or("--out is required")?,
        label,
        efi: efi.ok_or("--efi is required")?,
        files,
    })
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

fn run() -> Result<(), String> {
    let args = parse_args()?;

    let mut fat = ImageBuilder::new();
    fat.add_file("EFI/BOOT/BOOTX64.EFI", read(&args.efi)?)
        .map_err(|e| e.to_string())?;
    // Leave headroom for FAT metadata; FAT16 needs at least ~2 MiB.
    let size = (fat.content_bytes() * 5 / 4 + MIB).div_ceil(MIB).max(4) * MIB;
    let efi = fat
        .build_fat16(size, "VELORA_EFI", 0x5645_4c52)
        .map_err(|e| format!("building EFI image: {e}"))?;
    let efi_len = efi.len();

    let mut builder = iso::IsoBuilder::new();
    for (dest, src) in &args.files {
        builder.add_file(dest, read(src)?)?;
    }
    let image = builder.build(&args.label, efi)?;
    std::fs::write(&args.out, &image).map_err(|e| format!("cannot write {}: {e}", args.out))?;
    println!(
        "mkiso: wrote {} ({} KiB, EFI image {} KiB, {} files)",
        args.out,
        image.len() / 1024,
        efi_len / 1024,
        args.files.len()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mkiso: error: {e}");
            ExitCode::FAILURE
        }
    }
}

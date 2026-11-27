//! The running Live system: its disks and its files (feature `velora`).

use crate::{Source, SourceFile};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use velora_vfs::{BlockIo, FsError};
use vlibc::fs::{self, File, flags, mode, whence};
use vlibc::{Errno, display, process, time};

const SECTOR: u64 = 512;
/// Where the Live image keeps the kernel and Limine for installing.
const SHARE: &str = "/usr/lib/velora";
/// Programs of the Live system that are not installed.
const LIVE_ONLY: [&str; 4] = ["velora-setup", "velora-installer", "velora-start", "vtest"];

/// A disk the system can be installed on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    pub name: String,
    pub size: u64,
    /// Model name ("" if the device has none).
    pub model: String,
    /// Something from it is mounted under /mnt.
    pub in_use: bool,
}

/// True when running from the Live image (the root is the initramfs).
pub fn is_live() -> bool {
    fs::statfs("/").is_ok_and(|s| s.fs_type.starts_with(b"initramfs"))
}

/// Whole disks (not partitions), sorted by name.
pub fn disks() -> Vec<Disk> {
    let Ok(entries) = fs::read_dir("/dev") else {
        return Vec::new();
    };
    let devs: Vec<(String, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let st = fs::stat(&format!("/dev/{}", e.name)).ok()?;
            (st.mode & mode::TYPE_MASK == mode::BLOCK_DEVICE).then_some((e.name, st.size))
        })
        .collect();
    let mut out: Vec<Disk> = devs
        .iter()
        .filter(|(n, _)| {
            // "vd0p1" is a partition when "vd0" exists.
            !n.rfind('p').is_some_and(|i| {
                i > 0
                    && n[i + 1..].chars().all(|c| c.is_ascii_digit())
                    && !n[i + 1..].is_empty()
                    && devs.iter().any(|(d, _)| d == &n[..i])
            })
        })
        .map(|(name, size)| {
            let model = display::disk_info(&format!("/dev/{}", name))
                .map(|d| {
                    let end = d
                        .model
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(d.model.len());
                    String::from_utf8_lossy(&d.model[..end]).into_owned()
                })
                .unwrap_or_default();
            Disk {
                name: name.clone(),
                size: *size,
                model,
                in_use: in_use(name),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Disks with something mounted from them under /mnt.
fn in_use(disk: &str) -> bool {
    fs::read_dir("/mnt").is_ok_and(|d| {
        d.flatten().any(|e| {
            e.name == disk
                || e.name
                    .strip_prefix(disk)
                    .is_some_and(|r| r.starts_with('p'))
        })
    })
}

/// A whole disk opened through devfs.
pub struct DevDisk {
    file: File,
    sectors: u64,
}

fn io_err(_: Errno) -> FsError {
    FsError::Io
}

impl BlockIo for DevDisk {
    fn sector_size(&self) -> usize {
        SECTOR as usize
    }

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> velora_vfs::Result<()> {
        self.file
            .seek((lba * SECTOR) as i64, whence::SET)
            .map_err(io_err)?;
        let mut done = 0;
        while done < buf.len() {
            let n = self.file.read(&mut buf[done..]).map_err(io_err)?;
            if n == 0 {
                return Err(FsError::Io);
            }
            done += n;
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> velora_vfs::Result<()> {
        self.file
            .seek((lba * SECTOR) as i64, whence::SET)
            .map_err(io_err)?;
        self.file.write_all(buf).map_err(io_err)
    }

    fn flush(&mut self) -> velora_vfs::Result<()> {
        fs::sync().map_err(io_err)
    }
}

pub fn open_disk(name: &str) -> Result<DevDisk, String> {
    let path = format!("/dev/{}", name);
    let st = fs::stat(&path).map_err(|e| format!("{}: {}", path, e))?;
    let file =
        File::open_with(&path, flags::READ_WRITE, 0).map_err(|e| format!("{}: {}", path, e))?;
    Ok(DevDisk {
        file,
        sectors: st.size / SECTOR,
    })
}

/// The Live system's files.
pub struct LiveSource;

fn read(path: &str) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {}", path, e))
}

impl Source for LiveSource {
    fn files(&mut self) -> Result<Vec<SourceFile>, String> {
        let mut out = Vec::new();
        let programs = fs::read_dir("/bin").map_err(|x| format!("/bin: {}", x))?;
        for ent in programs.flatten() {
            if LIVE_ONLY.contains(&ent.name.as_str()) {
                continue;
            }
            let path = format!("/bin/{}", ent.name);
            let st = fs::stat(&path).map_err(|x| format!("{}: {}", path, x))?;
            out.push(SourceFile {
                data: read(&path)?,
                path,
                mode: st.mode & 0o7777,
            });
        }
        if let Ok(fonts) = fs::read_dir("/usr/share/fonts") {
            for ent in fonts.flatten() {
                let path = format!("/usr/share/fonts/{}", ent.name);
                out.push(SourceFile {
                    data: read(&path)?,
                    path,
                    mode: 0o644,
                });
            }
        }
        out.push(SourceFile {
            path: String::from("/etc/motd"),
            data: fs::read("/etc/motd").unwrap_or_default(),
            mode: 0o644,
        });
        Ok(out)
    }

    fn kernel(&mut self) -> Result<Vec<u8>, String> {
        read(&format!("{}/velora", SHARE))
    }

    fn bootloader(&mut self) -> Result<Vec<u8>, String> {
        read(&format!("{}/BOOTX64.EFI", SHARE))
    }

    fn random(&mut self, buf: &mut [u8]) {
        process::random(buf);
    }

    fn now(&self) -> u64 {
        time::realtime().map(|t| t.as_secs()).unwrap_or(0)
    }
}

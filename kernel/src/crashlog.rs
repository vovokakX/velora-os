//! Panic log on disk.
//!
//! At boot a FAT volume is chosen: one that holds the Velora boot files
//! (`/boot/velora`, the EFI partition of an installed system or a USB
//! drive) or one that already has a `velora-panic.log` (opt-in on any
//! FAT volume). The log file there is made 64 KiB long and synced, and the
//! sectors holding it are recorded. On a panic or a fatal CPU exception
//! the kernel can no longer trust the file system or the heap, so it
//! writes the log (a header plus the newest kernel messages, which end
//! with the report) straight to those sectors with the polling disk
//! driver, from a static buffer.
//!
//! At the next boot a log that starts with [`MAGIC`] is copied to
//! `velora-panic.prev.log`, a warning is printed and the file is cleared
//! for the next panic.

use crate::console::klog;
use crate::drivers::block::BlockDevice;
use crate::sync::{RacyCell, SpinLock};
use crate::{kok, kwarn};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;
use velora_vfs::{FileSystem, FsError, Ino, Kind, NewAttr};

pub const FILE_NAME: &str = "velora-panic.log";
pub const PREV_NAME: &str = "velora-panic.prev.log";
pub const MAGIC: &[u8] = b"VELORA PANIC LOG";
const FILE_SIZE: usize = 64 * 1024;
const MAX_RUNS: usize = 64;
/// Sectors per write request (32 KiB: below every driver's limit).
const CHUNK_SECTORS: u64 = 64;

struct Target {
    dev: Arc<dyn BlockDevice>,
    /// First sector of the file system on `dev`.
    base: u64,
    runs: Vec<(u64, u64)>,
    /// Where the log is, for messages ("/mnt/vd0p1/velora-panic.log").
    path: String,
}

static TARGET: SpinLock<Option<Target>> = SpinLock::new(None);
static BUF: RacyCell<[u8; FILE_SIZE]> = RacyCell::new([0; FILE_SIZE]);

fn find(fs: &mut dyn FileSystem, path: &str) -> Option<Ino> {
    let mut ino = fs.root();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        ino = fs.lookup(ino, part).ok()?;
    }
    Some(ino)
}

fn read_all(fs: &mut dyn FileSystem, ino: Ino, max: usize) -> Result<Vec<u8>, FsError> {
    let size = (fs.stat(ino)?.size as usize).min(max);
    let mut buf = vec![0u8; size];
    let mut off = 0;
    while off < size {
        match fs.read(ino, off as u64, &mut buf[off..])? {
            0 => break,
            n => off += n,
        }
    }
    buf.truncate(off);
    Ok(buf)
}

fn create_or_open(fs: &mut dyn FileSystem, name: &str) -> Result<Ino, FsError> {
    let root = fs.root();
    match fs.lookup(root, name) {
        Ok(ino) => Ok(ino),
        Err(FsError::NotFound) => {
            let attr = NewAttr {
                mode: 0o644,
                uid: 0,
                gid: 0,
            };
            fs.create(root, name, Kind::File, attr, now())
        }
        Err(e) => Err(e),
    }
}

fn now() -> u64 {
    crate::time::realtime_ns().map_or(0, |ns| ns / 1_000_000_000)
}

/// True if this FAT volume should hold the panic log.
pub fn wanted(fs: &mut dyn FileSystem) -> bool {
    find(fs, "/boot/velora").is_some() || find(fs, FILE_NAME).is_some()
}

/// Prepares the panic log on `fs`, which starts at sector `base` of
/// `dev` and is shown at `mount_path`. Reports a log left by the previous
/// boot.
pub fn prepare(
    fs: &mut dyn FileSystem,
    dev: Arc<dyn BlockDevice>,
    base: u64,
    mount_path: &str,
) -> Result<(), FsError> {
    if fs.read_only() {
        return Err(FsError::ReadOnly);
    }
    let ss = dev.sector_size();
    if ss == 0 || !FILE_SIZE.is_multiple_of(ss) {
        return Err(FsError::Invalid);
    }
    let path = alloc::format!("{}/{}", mount_path.trim_end_matches('/'), FILE_NAME);
    let ino = create_or_open(fs, FILE_NAME)?;
    let old = read_all(fs, ino, FILE_SIZE)?;
    if old.starts_with(MAGIC) {
        let end = old.iter().position(|&b| b == 0).unwrap_or(old.len());
        let prev = create_or_open(fs, PREV_NAME)?;
        fs.truncate(prev, 0, now())?;
        fs.write(prev, 0, &old[..end], now())?;
        kwarn!(
            "the previous boot ended with a panic; its log is in {}/{}",
            mount_path.trim_end_matches('/'),
            PREV_NAME
        );
    }
    // A cleared log: zeros (a log is recognised by its first bytes).
    fs.write(ino, 0, &vec![0u8; FILE_SIZE], now())?;
    fs.sync()?;
    let mut runs = Vec::new();
    let mut need = (FILE_SIZE / ss) as u64;
    for (start, count) in fs.data_sectors(ino)? {
        if need == 0 {
            break;
        }
        let n = count.min(need);
        runs.push((start, n));
        need -= n;
    }
    if need > 0 || runs.len() > MAX_RUNS {
        return Err(FsError::Corrupt);
    }
    kok!("panic log: {}", path);
    *TARGET.lock() = Some(Target {
        dev,
        base,
        runs,
        path,
    });
    Ok(())
}

/// Where the panic log goes, if one was prepared.
pub fn path() -> Option<String> {
    TARGET.lock().as_ref().map(|t| t.path.clone())
}

struct BufWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl core::fmt::Write for BufWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Writes the panic log. Called with interrupts off after the report was
/// printed, from the panic handler or a fatal CPU exception.
pub fn save() {
    // SAFETY: the system is going down; the lock holder will not run again.
    unsafe { TARGET.force_unlock() };
    let guard = TARGET.lock();
    let Some(t) = guard.as_ref() else {
        return;
    };
    // SAFETY: only the panicking CPU runs this, once (the callers stop
    // nested panics before they get here).
    let buf = unsafe { &mut *BUF.get() };
    buf.fill(0);
    let mut w = BufWriter { buf, len: 0 };
    let _ = w.write_fmt(format_args!(
        "{}\nVelora OS {}, {} ms after boot\n\n",
        core::str::from_utf8(MAGIC).unwrap_or(""),
        crate::VERSION,
        crate::time::uptime_ms()
    ));
    let head = w.len;
    // Keep the last byte zero so readers can find the end.
    klog::newest_plain(&mut buf[head..FILE_SIZE - 1]);
    // SAFETY: as above; the driver lock may be held by the code that
    // panicked.
    unsafe { t.dev.force_unlock() };
    let ss = t.dev.sector_size() as u64;
    let mut off = 0usize;
    let mut ok = true;
    'runs: for &(start, count) in &t.runs {
        let mut done = 0;
        while done < count {
            let k = (count - done).min(CHUNK_SECTORS);
            let bytes = (k * ss) as usize;
            if t.dev
                .write(t.base + start + done, &buf[off..off + bytes])
                .is_err()
            {
                ok = false;
                break 'runs;
            }
            off += bytes;
            done += k;
        }
    }
    if ok {
        ok = t.dev.flush().is_ok();
    }
    let path = t.path.as_str();
    crate::console::emergency(|out| {
        if ok {
            let _ = writeln!(out, "panic log saved to {}", path);
        } else {
            let _ = writeln!(out, "could not write the panic log to {}", path);
        }
    });
}

//! Kernel side of the file systems: the global VFS, the mounts made at
//! boot, open files and file descriptor tables.
//!
//! Mounts at boot:
//! - `/`     the initramfs (read-only, [`tarfs`])
//! - `/dev`  devices ([`devfs`]): null, zero, console and the disks
//! - `/tmp`  a file system in memory (`MemFs`)
//! - `/mnt/<disk>` every disk that holds VeloraFS or FAT16/FAT32
//!
//! Locking: the VFS is behind one spinlock, which also disables
//! interrupts. File system operations run to completion with it held
//! (disk drivers poll). Lock order: the scheduler lock may be held when
//! the VFS lock is taken (closing files of an exiting process), never the
//! other way round.

mod devfs;
mod disk;
pub mod pipe;
mod tarfs;

pub use devfs::DevFs;
pub use disk::DiskIo;

use crate::drivers::block;
use crate::sync::SpinLock;
use crate::{initramfs, kok, kwarn, time};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use velora_abi::{MAX_FDS, open};
use velora_vfs::{Cred, FsError, Kind, MemFs, MountId, Node, Vfs, access, may_access, path};
use velorafs::VeloraFs;

/// Most bytes /tmp may hold.
const TMP_CAPACITY: u64 = 64 * 1024 * 1024;

static VFS: SpinLock<Option<Vfs>> = SpinLock::new(None);
static DEVFS_MOUNT: AtomicU32 = AtomicU32::new(0);

/// Runs `f` with the VFS locked.
pub fn with<R>(f: impl FnOnce(&mut Vfs) -> R) -> R {
    let mut g = VFS.lock();
    f(g.as_mut().expect("VFS not initialised"))
}

/// Current time for file timestamps (0 when the clock is unknown).
pub fn now() -> u64 {
    time::realtime_ns().map_or(0, |ns| ns / 1_000_000_000)
}

/// Mounts the initramfs at `/`, devfs at `/dev` and a memory file system
/// at `/tmp`. Disks are mounted later by [`automount`].
pub fn init() {
    let mut v = Vfs::new();
    let root = tarfs::TarFs::new(initramfs::data());
    let files = root.file_count();
    v.mount("/", "initramfs", Box::new(root)).expect("mount /");
    let dev = v
        .mount("/dev", "devfs", Box::new(DevFs::new()))
        .expect("mount /dev");
    DEVFS_MOUNT.store(dev, Ordering::Relaxed);
    v.mount("/tmp", "memory", Box::new(MemFs::new(0o1777, TMP_CAPACITY)))
        .expect("mount /tmp");
    // Home directories live in memory in the Live system; login creates
    // them on first use.
    v.mount("/home", "memory", Box::new(MemFs::new(0o755, TMP_CAPACITY)))
        .expect("mount /home");
    v.mount("/root", "memory", Box::new(MemFs::new(0o700, TMP_CAPACITY)))
        .expect("mount /root");
    *VFS.lock() = Some(v);
    kok!(
        "VFS: initramfs at / ({} files, read-only), devfs at /dev, memory at /tmp, /home, /root",
        files
    );
}

#[derive(Clone, Debug)]
pub struct AutoMount {
    pub device: String,
    pub path: String,
    pub fs_type: &'static str,
    /// The journal was replayed during mount.
    pub recovered: bool,
}

static AUTOMOUNTED: SpinLock<Vec<AutoMount>> = SpinLock::new(Vec::new());

/// Mounts every disk with a known file system at `/mnt/<disk>`.
pub fn automount() {
    for d in block::devices() {
        let name = String::from(d.name());
        let at = format!("/mnt/{}", name);
        let mut io = DiskIo::new(d);
        let mounted: Result<(Box<dyn velora_vfs::FileSystem>, bool), FsError> =
            if velorafs::probe(&mut io).is_some() {
                VeloraFs::mount(io, false).map(|fs| {
                    let rec = fs.recovered();
                    (Box::new(fs) as Box<dyn velora_vfs::FileSystem>, rec)
                })
            } else if velora_fat::volume::probe(&mut io).is_some() {
                velora_fat::FatVolume::mount(io, false)
                    .map(|fs| (Box::new(fs) as Box<dyn velora_vfs::FileSystem>, false))
            } else {
                continue;
            };
        match mounted {
            Ok((fs, recovered)) => {
                let fs_type = fs.fs_type();
                match with(|v| v.mount(&at, &name, fs)) {
                    Ok(_) => {
                        kok!(
                            "{} on {} mounted at {}{}",
                            fs_type,
                            name,
                            at,
                            if recovered { " (journal replayed)" } else { "" }
                        );
                        AUTOMOUNTED.lock().push(AutoMount {
                            device: name,
                            path: at,
                            fs_type,
                            recovered,
                        });
                    }
                    Err(e) => kwarn!("cannot mount {} at {}: {}", name, at, e),
                }
            }
            Err(e) => kwarn!("{}: file system found but not mounted: {}", name, e),
        }
    }
}

pub fn automounted() -> Vec<AutoMount> {
    AUTOMOUNTED.lock().clone()
}

/// Reads a program file: the caller needs execute permission.
pub fn read_program(p: &str, cred: &Cred) -> Result<Vec<u8>, FsError> {
    with(|v| {
        let n = v.resolve(p, cred)?;
        let st = v.stat(n)?;
        if st.kind == Kind::Dir {
            return Err(FsError::IsDir);
        }
        if st.kind != Kind::File || !may_access(&st, cred, access::EXEC) {
            return Err(FsError::Access);
        }
        v.read_all(p, cred)
    })
}

// --- open files -------------------------------------------------------------

pub enum Handle {
    /// The text console: reads the keyboard, writes the screen.
    Console,
    File {
        node: Node,
        /// Normalised path it was opened by (for directory listings).
        path: String,
        kind: Kind,
    },
    /// One end of a pipe.
    Pipe { pipe: Arc<pipe::Pipe>, write: bool },
}

pub struct OpenFile {
    pub handle: Handle,
    pub offset: u64,
    pub readable: bool,
    pub writable: bool,
    pub append: bool,
}

impl Drop for OpenFile {
    fn drop(&mut self) {
        match &self.handle {
            Handle::File { node, .. } => with(|v| v.close_ref(node.mount)),
            Handle::Pipe { pipe, write } => pipe.close_end(*write),
            Handle::Console => {}
        }
    }
}

pub type FileRef = Arc<SpinLock<OpenFile>>;

fn console() -> FileRef {
    Arc::new(SpinLock::new(OpenFile {
        handle: Handle::Console,
        offset: 0,
        readable: true,
        writable: true,
        append: false,
    }))
}

/// A process's open files, indexed by descriptor number.
#[derive(Default)]
pub struct FdTable {
    slots: Vec<Option<FileRef>>,
}

impl FdTable {
    /// Descriptors 0, 1 and 2 on the console.
    pub fn standard() -> Self {
        Self {
            slots: alloc::vec![Some(console()), Some(console()), Some(console())],
        }
    }

    /// A table for a new process: descriptors 0, 1 and 2 of this one
    /// (other descriptors are not inherited, so pipe ends a shell keeps
    /// open do not leak into its children).
    pub fn inherit_std(&self) -> Self {
        let mut slots: Vec<Option<FileRef>> = (0..3).map(|fd| self.get(fd)).collect();
        for (fd, s) in slots.iter_mut().enumerate() {
            if s.is_none() && fd < 3 {
                *s = Some(console());
            }
        }
        Self { slots }
    }

    /// Puts `f` at descriptor `fd`, returning what was there.
    pub fn set(&mut self, fd: u64, f: FileRef) -> Option<Option<FileRef>> {
        let fd = fd as usize;
        if fd >= MAX_FDS {
            return None;
        }
        if self.slots.len() <= fd {
            self.slots.resize(fd + 1, None);
        }
        Some(self.slots[fd].replace(f))
    }

    pub fn get(&self, fd: u64) -> Option<FileRef> {
        self.slots.get(fd as usize).and_then(|s| s.clone())
    }

    /// Puts `f` in the lowest free slot.
    pub fn install(&mut self, f: FileRef) -> Option<u64> {
        if let Some(i) = self.slots.iter().position(Option::is_none) {
            self.slots[i] = Some(f);
            return Some(i as u64);
        }
        if self.slots.len() >= MAX_FDS {
            return None;
        }
        self.slots.push(Some(f));
        Some(self.slots.len() as u64 - 1)
    }

    pub fn remove(&mut self, fd: u64) -> Option<FileRef> {
        self.slots.get_mut(fd as usize).and_then(Option::take)
    }
}

/// Opens (and with `open::CREATE` creates) a file.
pub fn open_file(p: &str, flags: u64, mode: u32, cred: &Cred) -> Result<OpenFile, FsError> {
    let acc = flags & open::ACCESS_MASK;
    if acc > open::READ_WRITE {
        return Err(FsError::Invalid);
    }
    let readable = acc != open::WRITE;
    let writable = acc != open::READ;
    let norm = path::normalize(p)?;
    let now = now();
    with(|v| {
        let (node, created) = match v.resolve(&norm, cred) {
            Ok(n) => {
                if flags & open::CREATE != 0 && flags & open::EXCLUSIVE != 0 {
                    return Err(FsError::Exists);
                }
                (n, false)
            }
            Err(FsError::NotFound) if flags & open::CREATE != 0 => {
                (v.create(&norm, Kind::File, mode, cred, now)?, true)
            }
            Err(e) => return Err(e),
        };
        let st = v.stat(node)?;
        if flags & open::DIRECTORY != 0 && st.kind != Kind::Dir {
            return Err(FsError::NotDir);
        }
        if st.kind == Kind::Dir && writable {
            return Err(FsError::IsDir);
        }
        let mut want = 0;
        if readable {
            want |= access::READ;
        }
        if writable {
            want |= access::WRITE;
        }
        if !created && !may_access(&st, cred, want) {
            return Err(FsError::Access);
        }
        if writable && v.fs(node.mount)?.read_only() {
            return Err(FsError::ReadOnly);
        }
        if flags & open::TRUNCATE != 0 && writable && st.kind == Kind::File && st.size > 0 {
            v.truncate(node, 0, now)?;
        }
        let handle = if node.mount == DEVFS_MOUNT.load(Ordering::Relaxed)
            && node.ino == devfs::CONSOLE_INO
        {
            Handle::Console
        } else {
            v.open_ref(node.mount);
            Handle::File {
                node,
                path: norm,
                kind: st.kind,
            }
        };
        Ok(OpenFile {
            handle,
            offset: 0,
            readable,
            writable,
            append: flags & open::APPEND != 0,
        })
    })
}

/// Creates a pipe: (read end, write end).
pub fn new_pipe() -> (OpenFile, OpenFile) {
    let p = Arc::new(pipe::Pipe::new());
    let end = |write: bool| OpenFile {
        handle: Handle::Pipe {
            pipe: p.clone(),
            write,
        },
        offset: 0,
        readable: !write,
        writable: write,
        append: false,
    };
    (end(false), end(true))
}

/// Mount id of `/dev` (for tests).
pub fn devfs_mount() -> MountId {
    DEVFS_MOUNT.load(Ordering::Relaxed)
}

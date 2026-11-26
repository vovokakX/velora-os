//! The virtual file system layer, shared by the kernel and host tests.
//!
//! - [`FileSystem`]: the interface every file system implements (VeloraFS,
//!   FAT, the initramfs, devfs, [`MemFs`]). It works on inode numbers;
//!   paths are resolved by [`Vfs`].
//! - [`BlockIo`]: the sector interface disk file systems run on. The
//!   kernel adapts its block drivers to it; tests use [`MemDisk`].
//! - [`Vfs`]: the mount table, path resolution and permission checks.
//! - [`path`]: path normalisation and name rules.
//!
//! Everything here is single-threaded (`&mut self`); the kernel puts the
//! [`Vfs`] behind a lock.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod conformance;
pub mod memdisk;
pub mod memfs;
pub mod path;
mod vfs;

pub use memdisk::MemDisk;
pub use memfs::MemFs;
pub use vfs::{MountId, MountInfo, Node, Vfs};

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use velora_abi::errno;

pub type Ino = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    Exists,
    NotDir,
    IsDir,
    NotEmpty,
    NoSpace,
    ReadOnly,
    /// Bad argument (name, offset, flags).
    Invalid,
    NameTooLong,
    /// The device reported an error.
    Io,
    /// Metadata failed a consistency or checksum check.
    Corrupt,
    /// Permission denied.
    Access,
    /// The operation needs a privilege the caller does not have.
    NotPermitted,
    /// The file would exceed what the file system can store.
    TooBig,
    /// Rename across mounts.
    CrossDevice,
    /// The mount is in use.
    Busy,
    NoMemory,
    /// The operation is not supported by this file system.
    Unsupported,
}

impl FsError {
    /// The positive error number for system calls.
    pub fn errno(self) -> i64 {
        match self {
            FsError::NotFound => errno::ENOENT,
            FsError::Exists => errno::EEXIST,
            FsError::NotDir => errno::ENOTDIR,
            FsError::IsDir => errno::EISDIR,
            FsError::NotEmpty => errno::ENOTEMPTY,
            FsError::NoSpace => errno::ENOSPC,
            FsError::ReadOnly => errno::EROFS,
            FsError::Invalid => errno::EINVAL,
            FsError::NameTooLong => errno::ENAMETOOLONG,
            FsError::Io => errno::EIO,
            FsError::Corrupt => errno::EUCLEAN,
            FsError::Access => errno::EACCES,
            FsError::NotPermitted => errno::EPERM,
            FsError::TooBig => errno::EFBIG,
            FsError::CrossDevice => errno::EXDEV,
            FsError::Busy => errno::EBUSY,
            FsError::NoMemory => errno::ENOMEM,
            FsError::Unsupported => errno::ENOSYS,
        }
    }
}

impl core::fmt::Display for FsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            FsError::NotFound => "not found",
            FsError::Exists => "already exists",
            FsError::NotDir => "not a directory",
            FsError::IsDir => "is a directory",
            FsError::NotEmpty => "directory not empty",
            FsError::NoSpace => "no space left",
            FsError::ReadOnly => "read-only file system",
            FsError::Invalid => "invalid argument",
            FsError::NameTooLong => "name too long",
            FsError::Io => "I/O error",
            FsError::Corrupt => "file system is corrupt",
            FsError::Access => "permission denied",
            FsError::NotPermitted => "operation not permitted",
            FsError::TooBig => "file too large",
            FsError::CrossDevice => "cross-device rename",
            FsError::Busy => "busy",
            FsError::NoMemory => "out of memory",
            FsError::Unsupported => "not supported",
        };
        f.write_str(s)
    }
}

pub type Result<T> = core::result::Result<T, FsError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    CharDevice,
    BlockDevice,
}

impl Kind {
    /// The POSIX file type bits (`velora_abi::mode`).
    pub fn mode_bits(self) -> u32 {
        use velora_abi::mode;
        match self {
            Kind::File => mode::FILE,
            Kind::Dir => mode::DIR,
            Kind::CharDevice => mode::CHAR_DEVICE,
            Kind::BlockDevice => mode::BLOCK_DEVICE,
        }
    }
}

/// Metadata of one file. `mode` holds only the permission bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub ino: Ino,
    pub kind: Kind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub size: u64,
    /// Bytes allocated on the device.
    pub allocated: u64,
    /// Seconds since 1970-01-01 UTC.
    pub mtime: u64,
    pub ctime: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub ino: Ino,
    pub kind: Kind,
    pub name: String,
}

/// Owner and permissions of a new file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewAttr {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

/// Attribute changes; `None` leaves a field as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetAttr {
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FsStat {
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub inodes: u64,
    pub free_inodes: u64,
}

/// Who performs an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cred {
    pub uid: u32,
    pub gid: u32,
}

impl Cred {
    pub const ROOT: Cred = Cred { uid: 0, gid: 0 };
}

/// Access bits for permission checks.
pub mod access {
    pub const READ: u32 = 4;
    pub const WRITE: u32 = 2;
    pub const EXEC: u32 = 1;
}

/// POSIX permission check. Root may read and write anything and may
/// execute a file if any execute bit is set (directories: always).
pub fn may_access(st: &Stat, cred: &Cred, want: u32) -> bool {
    if cred.uid == 0 {
        return want & access::EXEC == 0 || st.kind == Kind::Dir || st.mode & 0o111 != 0;
    }
    let bits = if cred.uid == st.uid {
        st.mode >> 6
    } else if cred.gid == st.gid {
        st.mode >> 3
    } else {
        st.mode
    } & 7;
    bits & want == want
}

/// A file system. Inode numbers are stable while the file exists.
///
/// Mutating methods have read-only defaults so read-only file systems
/// only implement the lookup and read side.
pub trait FileSystem: Send {
    /// Short type name: "velorafs", "fat32", "tarfs", ...
    fn fs_type(&self) -> &'static str;
    fn root(&self) -> Ino;
    fn read_only(&self) -> bool {
        true
    }

    fn stat(&mut self, ino: Ino) -> Result<Stat>;
    fn lookup(&mut self, dir: Ino, name: &str) -> Result<Ino>;
    /// The entry at position `pos` (0 = first) and the position of the
    /// next one, or `None` past the end. "." and ".." are not listed.
    fn readdir(&mut self, dir: Ino, pos: u64) -> Result<Option<(DirEntry, u64)>>;
    fn read(&mut self, ino: Ino, offset: u64, buf: &mut [u8]) -> Result<usize>;

    fn write(&mut self, _ino: Ino, _offset: u64, _data: &[u8], _now: u64) -> Result<usize> {
        Err(FsError::ReadOnly)
    }
    fn truncate(&mut self, _ino: Ino, _size: u64, _now: u64) -> Result<()> {
        Err(FsError::ReadOnly)
    }
    fn create(
        &mut self,
        _dir: Ino,
        _name: &str,
        _kind: Kind,
        _attr: NewAttr,
        _now: u64,
    ) -> Result<Ino> {
        Err(FsError::ReadOnly)
    }
    /// Removes a file or an empty directory.
    fn unlink(&mut self, _dir: Ino, _name: &str, _now: u64) -> Result<()> {
        Err(FsError::ReadOnly)
    }
    /// Moves an entry; an existing target file (or empty directory, when
    /// the source is a directory) is replaced.
    fn rename(
        &mut self,
        _from: Ino,
        _from_name: &str,
        _to: Ino,
        _to_name: &str,
        _now: u64,
    ) -> Result<()> {
        Err(FsError::ReadOnly)
    }
    fn setattr(&mut self, _ino: Ino, _attr: SetAttr, _now: u64) -> Result<()> {
        Err(FsError::ReadOnly)
    }
    /// Makes every completed operation durable.
    fn sync(&mut self) -> Result<()> {
        Ok(())
    }
    fn statfs(&mut self) -> Result<FsStat> {
        Ok(FsStat::default())
    }
    /// Checks the whole file system and returns the problems found (empty
    /// when it is consistent).
    fn fsck(&mut self) -> Result<Vec<String>> {
        Err(FsError::Unsupported)
    }
    /// Device sectors holding a file's data, as (first sector, count)
    /// runs in file order. Used to write the panic log straight to the
    /// disk when the file system can no longer be used. Call `sync` first:
    /// the result describes what is on the disk.
    fn data_sectors(&mut self, _ino: Ino) -> Result<Vec<(u64, u64)>> {
        Err(FsError::Unsupported)
    }
}

/// A device of fixed-size sectors, addressed by sector number.
pub trait BlockIo: Send {
    fn sector_size(&self) -> usize;
    fn sector_count(&self) -> u64;
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()>;
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()>;
    /// Makes earlier writes durable.
    fn flush(&mut self) -> Result<()>;
}

impl<T: BlockIo + ?Sized> BlockIo for Box<T> {
    fn sector_size(&self) -> usize {
        (**self).sector_size()
    }
    fn sector_count(&self) -> u64 {
        (**self).sector_count()
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read(lba, buf)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()> {
        (**self).write(lba, buf)
    }
    fn flush(&mut self) -> Result<()> {
        (**self).flush()
    }
}

/// A window of another device: sectors `start..start + count`. Used for
/// partitions and for tests that put two file systems on one disk.
pub struct Slice<D: BlockIo> {
    pub dev: D,
    pub start: u64,
    pub count: u64,
}

impl<D: BlockIo> Slice<D> {
    fn check(&self, lba: u64, len: usize) -> Result<()> {
        let n = (len / self.dev.sector_size()) as u64;
        match lba.checked_add(n) {
            Some(end) if end <= self.count => Ok(()),
            _ => Err(FsError::Invalid),
        }
    }
}

impl<D: BlockIo> BlockIo for Slice<D> {
    fn sector_size(&self) -> usize {
        self.dev.sector_size()
    }
    fn sector_count(&self) -> u64 {
        self.count
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<()> {
        self.check(lba, buf.len())?;
        self.dev.read(self.start + lba, buf)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<()> {
        self.check(lba, buf.len())?;
        self.dev.write(self.start + lba, buf)
    }
    fn flush(&mut self) -> Result<()> {
        self.dev.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(kind: Kind, mode: u32, uid: u32, gid: u32) -> Stat {
        Stat {
            ino: 1,
            kind,
            mode,
            uid,
            gid,
            nlink: 1,
            size: 0,
            allocated: 0,
            mtime: 0,
            ctime: 0,
        }
    }

    #[test]
    fn permissions_follow_owner_group_other() {
        let f = st(Kind::File, 0o640, 1000, 100);
        let owner = Cred { uid: 1000, gid: 1 };
        let group = Cred {
            uid: 1001,
            gid: 100,
        };
        let other = Cred { uid: 1002, gid: 1 };
        assert!(may_access(&f, &owner, access::READ | access::WRITE));
        assert!(!may_access(&f, &owner, access::EXEC));
        assert!(may_access(&f, &group, access::READ));
        assert!(!may_access(&f, &group, access::WRITE));
        assert!(!may_access(&f, &other, access::READ));
        // The owner class applies even when it is stricter than others.
        let g = st(Kind::File, 0o066, 1000, 100);
        assert!(!may_access(&g, &owner, access::READ));
    }

    #[test]
    fn root_bypasses_read_write_but_not_exec() {
        let f = st(Kind::File, 0o000, 1000, 100);
        assert!(may_access(&f, &Cred::ROOT, access::READ | access::WRITE));
        assert!(!may_access(&f, &Cred::ROOT, access::EXEC));
        let x = st(Kind::File, 0o001, 1000, 100);
        assert!(may_access(&x, &Cred::ROOT, access::EXEC));
        let d = st(Kind::Dir, 0o000, 1000, 100);
        assert!(may_access(&d, &Cred::ROOT, access::EXEC));
    }

    #[test]
    fn errors_map_to_linux_numbers() {
        assert_eq!(FsError::NotFound.errno(), 2);
        assert_eq!(FsError::NoSpace.errno(), 28);
        assert_eq!(FsError::Corrupt.errno(), 117);
    }

    #[test]
    fn slice_offsets_and_bounds() {
        let disk = MemDisk::new(512, 100);
        let mut s = Slice {
            dev: disk,
            start: 10,
            count: 20,
        };
        s.write(0, &[7u8; 512]).unwrap();
        assert!(s.write(20, &[0u8; 512]).is_err());
        assert!(s.read(19, &mut [0u8; 1024]).is_err());
        let mut b = [0u8; 512];
        s.dev.read(10, &mut b).unwrap();
        assert_eq!(b[0], 7);
    }
}

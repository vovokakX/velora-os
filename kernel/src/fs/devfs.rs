//! /dev: null, zero, console and one node per disk.
//!
//! Disks are read and written at byte offsets that are multiples of the
//! sector size. The console node is turned into a console handle when it
//! is opened; reading it through the VFS is refused.

use crate::drivers::block::{self, BlockDevice};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use velora_vfs::{DirEntry, FileSystem, FsError, Ino, Kind, NewAttr, Result, Stat};

const ROOT: Ino = 1;
const NULL: Ino = 2;
const ZERO: Ino = 3;
pub const CONSOLE_INO: Ino = 4;
const FIRST_DISK: Ino = 16;

pub struct DevFs {
    disks: Vec<Arc<dyn BlockDevice>>,
}

impl DevFs {
    /// Takes the disks known now (drivers start before the VFS).
    pub fn new() -> Self {
        Self {
            disks: block::devices(),
        }
    }

    fn names(&self) -> Vec<(String, Ino, Kind)> {
        let mut v = alloc::vec![
            (String::from("null"), NULL, Kind::CharDevice),
            (String::from("zero"), ZERO, Kind::CharDevice),
            (String::from("console"), CONSOLE_INO, Kind::CharDevice),
        ];
        for (i, d) in self.disks.iter().enumerate() {
            v.push((
                String::from(d.name()),
                FIRST_DISK + i as Ino,
                Kind::BlockDevice,
            ));
        }
        v
    }

    fn disk(&self, ino: Ino) -> Option<&Arc<dyn BlockDevice>> {
        ino.checked_sub(FIRST_DISK)
            .and_then(|i| self.disks.get(i as usize))
    }

    /// Checks a disk transfer; returns the starting sector and the length
    /// cut to the end of the disk.
    fn disk_range(d: &dyn BlockDevice, offset: u64, len: usize) -> Result<(u64, usize)> {
        let ss = d.sector_size() as u64;
        if !offset.is_multiple_of(ss) || !(len as u64).is_multiple_of(ss) {
            return Err(FsError::Invalid);
        }
        let size = d.size_bytes();
        if offset >= size {
            return Ok((0, 0));
        }
        Ok((offset / ss, (len as u64).min(size - offset) as usize))
    }
}

impl FileSystem for DevFs {
    fn fs_type(&self) -> &'static str {
        "devfs"
    }

    fn root(&self) -> Ino {
        ROOT
    }

    fn read_only(&self) -> bool {
        false
    }

    fn stat(&mut self, ino: Ino) -> Result<Stat> {
        let (kind, mode, size) = match ino {
            ROOT => (Kind::Dir, 0o755, 0),
            NULL | ZERO | CONSOLE_INO => (Kind::CharDevice, 0o666, 0),
            _ => {
                let d = self.disk(ino).ok_or(FsError::NotFound)?;
                (Kind::BlockDevice, 0o600, d.size_bytes())
            }
        };
        Ok(Stat {
            ino,
            kind,
            mode,
            uid: 0,
            gid: 0,
            nlink: 1,
            size,
            allocated: 0,
            mtime: 0,
            ctime: 0,
        })
    }

    fn lookup(&mut self, dir: Ino, name: &str) -> Result<Ino> {
        if dir != ROOT {
            return Err(FsError::NotDir);
        }
        self.names()
            .into_iter()
            .find(|(n, ..)| n == name)
            .map(|(_, ino, _)| ino)
            .ok_or(FsError::NotFound)
    }

    fn readdir(&mut self, dir: Ino, pos: u64) -> Result<Option<(DirEntry, u64)>> {
        if dir != ROOT {
            return Err(FsError::NotDir);
        }
        Ok(self
            .names()
            .into_iter()
            .nth(pos as usize)
            .map(|(name, ino, kind)| (DirEntry { ino, kind, name }, pos + 1)))
    }

    fn read(&mut self, ino: Ino, offset: u64, buf: &mut [u8]) -> Result<usize> {
        match ino {
            ROOT => Err(FsError::IsDir),
            NULL => Ok(0),
            ZERO => {
                buf.fill(0);
                Ok(buf.len())
            }
            CONSOLE_INO => Err(FsError::Unsupported),
            _ => {
                let d = self.disk(ino).ok_or(FsError::NotFound)?;
                let (lba, len) = Self::disk_range(&**d, offset, buf.len())?;
                if len > 0 {
                    d.read(lba, &mut buf[..len]).map_err(|_| FsError::Io)?;
                }
                Ok(len)
            }
        }
    }

    fn write(&mut self, ino: Ino, offset: u64, data: &[u8], _now: u64) -> Result<usize> {
        match ino {
            ROOT => Err(FsError::IsDir),
            NULL | ZERO => Ok(data.len()),
            CONSOLE_INO => Err(FsError::Unsupported),
            _ => {
                let d = self.disk(ino).ok_or(FsError::NotFound)?;
                let (lba, len) = Self::disk_range(&**d, offset, data.len())?;
                if len == 0 {
                    return Err(FsError::NoSpace);
                }
                d.write(lba, &data[..len]).map_err(|_| FsError::Io)?;
                Ok(len)
            }
        }
    }

    fn truncate(&mut self, _ino: Ino, _size: u64, _now: u64) -> Result<()> {
        Err(FsError::Invalid)
    }

    fn create(&mut self, _: Ino, _: &str, _: Kind, _: NewAttr, _: u64) -> Result<Ino> {
        Err(FsError::NotPermitted)
    }

    fn unlink(&mut self, _: Ino, _: &str, _: u64) -> Result<()> {
        Err(FsError::NotPermitted)
    }

    fn rename(&mut self, _: Ino, _: &str, _: Ino, _: &str, _: u64) -> Result<()> {
        Err(FsError::NotPermitted)
    }

    fn sync(&mut self) -> Result<()> {
        for d in &self.disks {
            d.flush().map_err(|_| FsError::Io)?;
        }
        Ok(())
    }
}

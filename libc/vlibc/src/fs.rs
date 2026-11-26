//! Files and directories.
//!
//! ```ignore
//! let mut f = File::create("/tmp/a.txt")?;
//! f.write_all(b"hello")?;
//! let text = fs::read("/tmp/a.txt")?;
//! for e in fs::read_dir("/tmp")? { println!("{}", e?.name); }
//! ```

use crate::sys::{syscall0, syscall1, syscall2, syscall3, syscall4};
use crate::{Errno, check};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use velora_abi::{DirentHeader, errno, nr};
pub use velora_abi::{FsStat, Stat, mode, open as flags, whence};

/// An open file descriptor; closed when dropped.
pub struct File {
    fd: u64,
}

impl File {
    /// Opens with `flags` (see [`flags`]); `perm` is used when creating.
    pub fn open_with(path: &str, fl: u64, perm: u32) -> Result<File, Errno> {
        // SAFETY: the path is valid for its length.
        let fd = check(unsafe {
            syscall4(
                nr::OPEN,
                path.as_ptr() as u64,
                path.len() as u64,
                fl,
                perm as u64,
            )
        })?;
        Ok(File { fd })
    }

    /// Opens an existing file for reading.
    pub fn open(path: &str) -> Result<File, Errno> {
        Self::open_with(path, flags::READ, 0)
    }

    /// Creates a file (or empties an existing one) for writing, mode 0644.
    pub fn create(path: &str) -> Result<File, Errno> {
        Self::open_with(path, flags::WRITE | flags::CREATE | flags::TRUNCATE, 0o644)
    }

    pub fn fd(&self) -> u64 {
        self.fd
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, Errno> {
        crate::io::read(self.fd, buf)
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize, Errno> {
        crate::io::write(self.fd, buf)
    }

    pub fn write_all(&mut self, buf: &[u8]) -> Result<(), Errno> {
        crate::io::write_all(self.fd, buf)
    }

    /// Reads until the end of the file.
    pub fn read_to_end(&mut self) -> Result<Vec<u8>, Errno> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = self.read(&mut buf)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    /// Moves the position; returns the new offset.
    pub fn seek(&mut self, offset: i64, from: u64) -> Result<u64, Errno> {
        // SAFETY: no pointers.
        check(unsafe { syscall3(nr::SEEK, self.fd, offset as u64, from) })
    }

    pub fn stat(&self) -> Result<Stat, Errno> {
        let mut st = Stat::default();
        // SAFETY: `st` is writable and has the kernel's layout.
        check(unsafe { syscall2(nr::FSTAT, self.fd, &mut st as *mut Stat as u64) })?;
        Ok(st)
    }

    pub fn set_len(&mut self, size: u64) -> Result<(), Errno> {
        // SAFETY: no pointers.
        check(unsafe { syscall2(nr::FTRUNCATE, self.fd, size) }).map(|_| ())
    }

    /// Closes now and reports the error, unlike dropping.
    pub fn close(self) -> Result<(), Errno> {
        let fd = self.fd;
        core::mem::forget(self);
        close(fd)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = close(self.fd);
    }
}

pub fn close(fd: u64) -> Result<(), Errno> {
    // SAFETY: no pointers.
    check(unsafe { syscall1(nr::CLOSE, fd) }).map(|_| ())
}

/// Reads a whole file.
pub fn read(path: &str) -> Result<Vec<u8>, Errno> {
    File::open(path)?.read_to_end()
}

/// Creates or replaces a file with `data`.
pub fn write(path: &str, data: &[u8]) -> Result<(), Errno> {
    File::create(path)?.write_all(data)
}

pub fn stat(path: &str) -> Result<Stat, Errno> {
    let mut st = Stat::default();
    // SAFETY: the path is valid; `st` is writable.
    check(unsafe {
        syscall3(
            nr::STAT,
            path.as_ptr() as u64,
            path.len() as u64,
            &mut st as *mut Stat as u64,
        )
    })?;
    Ok(st)
}

pub fn statfs(path: &str) -> Result<FsStat, Errno> {
    let mut st = FsStat::default();
    // SAFETY: the path is valid; `st` is writable.
    check(unsafe {
        syscall3(
            nr::STATFS,
            path.as_ptr() as u64,
            path.len() as u64,
            &mut st as *mut FsStat as u64,
        )
    })?;
    Ok(st)
}

pub fn mkdir(path: &str, perm: u32) -> Result<(), Errno> {
    // SAFETY: the path is valid for its length.
    check(unsafe {
        syscall3(
            nr::MKDIR,
            path.as_ptr() as u64,
            path.len() as u64,
            perm as u64,
        )
    })
    .map(|_| ())
}

/// Removes a file or an empty directory.
pub fn remove(path: &str) -> Result<(), Errno> {
    // SAFETY: the path is valid for its length.
    check(unsafe { syscall2(nr::UNLINK, path.as_ptr() as u64, path.len() as u64) }).map(|_| ())
}

pub fn rename(from: &str, to: &str) -> Result<(), Errno> {
    // SAFETY: both paths are valid for their lengths.
    check(unsafe {
        syscall4(
            nr::RENAME,
            from.as_ptr() as u64,
            from.len() as u64,
            to.as_ptr() as u64,
            to.len() as u64,
        )
    })
    .map(|_| ())
}

/// Writes all cached file system data to the disks.
pub fn sync() -> Result<(), Errno> {
    // SAFETY: no arguments.
    check(unsafe { syscall0(nr::SYNC) }).map(|_| ())
}

/// True for directories in `Stat::mode`.
pub fn is_dir(st: &Stat) -> bool {
    st.mode & mode::TYPE_MASK == mode::DIR
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub ino: u64,
    pub name: String,
    pub is_dir: bool,
}

/// Directory entries, read in batches.
pub struct ReadDir {
    file: File,
    buf: Vec<u8>,
    pos: usize,
    len: usize,
    done: bool,
}

pub fn read_dir(path: &str) -> Result<ReadDir, Errno> {
    Ok(ReadDir {
        file: File::open_with(path, flags::READ | flags::DIRECTORY, 0)?,
        buf: vec![0u8; 4096],
        pos: 0,
        len: 0,
        done: false,
    })
}

impl Iterator for ReadDir {
    type Item = Result<DirEntry, Errno>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.len {
            if self.done {
                return None;
            }
            // SAFETY: the buffer is writable for its length.
            let r = check(unsafe {
                syscall3(
                    nr::READDIR,
                    self.file.fd,
                    self.buf.as_mut_ptr() as u64,
                    self.buf.len() as u64,
                )
            });
            match r {
                Ok(0) => {
                    self.done = true;
                    return None;
                }
                Ok(n) => {
                    self.len = n as usize;
                    self.pos = 0;
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
        let h = size_of::<DirentHeader>();
        if self.pos + h > self.len {
            self.done = true;
            return Some(Err(Errno(errno::EIO)));
        }
        let b = &self.buf[self.pos..self.len];
        let ino = u64::from_le_bytes(b[0..8].try_into().unwrap());
        let rec = u16::from_le_bytes([b[8], b[9]]) as usize;
        let name_len = u16::from_le_bytes([b[10], b[11]]) as usize;
        let kind = b[12] as u32;
        if rec < h || h + name_len > rec || rec > b.len() {
            self.done = true;
            return Some(Err(Errno(errno::EIO)));
        }
        let name = String::from_utf8_lossy(&b[h..h + name_len]).into_owned();
        self.pos += rec;
        Some(Ok(DirEntry {
            ino,
            name,
            is_dir: kind << 12 == mode::DIR,
        }))
    }
}

//! File system calls: descriptors, paths and directory listings.
//!
//! Relative paths start at the process's working directory (`chdir`).

use super::uaccess;
use crate::fs::{self, FileRef, Handle, OpenFile};
use crate::sched;
use crate::sync::SpinLock;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use velora_abi::{DirentHeader, FsStat, MAX_PATH, Stat, errno, whence};
use velora_vfs::{FsError, Kind, Node, path};

/// Largest single file read or write; the caller sees a short count.
pub const MAX_IO: u64 = 1024 * 1024;

fn err(e: FsError) -> i64 {
    -e.errno()
}

fn as_bytes<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: T is plain `repr(C)` data without padding holes that matter
    // (the ABI structs are fully initialised before the copy).
    unsafe { core::slice::from_raw_parts((v as *const T).cast::<u8>(), size_of::<T>()) }
}

/// Copies a path from user memory. Relative paths start at the working
/// directory.
pub fn user_path(ptr: u64, len: u64) -> Result<String, i64> {
    if len == 0 {
        return Err(-errno::ENOENT);
    }
    if len as usize > MAX_PATH {
        return Err(-errno::ENAMETOOLONG);
    }
    let bytes = uaccess::read_slice(ptr, len).ok_or(-errno::EFAULT)?;
    let s = core::str::from_utf8(bytes).map_err(|_| -errno::EINVAL)?;
    Ok(if s.starts_with('/') {
        String::from(s)
    } else {
        path::join(&sched::current_cwd(), s)
    })
}

fn file(fd: u64) -> Result<FileRef, i64> {
    sched::fd_get(fd).ok_or(-errno::EBADF)
}

fn node_of(f: &OpenFile) -> Result<(Node, Kind), i64> {
    match &f.handle {
        Handle::File { node, kind, .. } => Ok((*node, *kind)),
        Handle::Console | Handle::Pipe { .. } | Handle::Socket { .. } => Err(-errno::ESPIPE),
    }
}

/// Reads from a pipe end without holding the open file's lock while
/// waiting.
pub fn read_pipe(f: &FileRef, buf: u64, len: u64) -> i64 {
    let pipe = {
        let of = f.lock();
        match &of.handle {
            Handle::Pipe { pipe, write: false } => pipe.clone(),
            _ => return -errno::EBADF,
        }
    };
    let data = pipe.read(len.min(MAX_IO) as usize);
    if !uaccess::write(buf, &data) {
        return -errno::EFAULT;
    }
    data.len() as i64
}

pub fn write_pipe(f: &FileRef, buf: u64, len: u64) -> i64 {
    let pipe = {
        let of = f.lock();
        match &of.handle {
            Handle::Pipe { pipe, write: true } => pipe.clone(),
            _ => return -errno::EBADF,
        }
    };
    let Some(data) = uaccess::read_slice(buf, len.min(MAX_IO)) else {
        return -errno::EFAULT;
    };
    match pipe.write(data) {
        Ok(n) => n as i64,
        Err(_) => -errno::EPIPE,
    }
}

pub fn pipe(out: u64) -> i64 {
    if !uaccess::write(out, &[0u8; 16]) {
        return -errno::EFAULT;
    }
    let (r, w) = fs::new_pipe();
    let Some(rfd) = sched::fd_install(Arc::new(SpinLock::new(r))) else {
        return -errno::EMFILE;
    };
    let Some(wfd) = sched::fd_install(Arc::new(SpinLock::new(w))) else {
        drop(sched::fd_remove(rfd));
        return -errno::EMFILE;
    };
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&rfd.to_le_bytes());
    bytes[8..].copy_from_slice(&wfd.to_le_bytes());
    if uaccess::write(out, &bytes) {
        0
    } else {
        -errno::EFAULT
    }
}

pub fn dup(fd: u64) -> i64 {
    let f = match file(fd) {
        Ok(f) => f,
        Err(e) => return e,
    };
    match sched::fd_install(f) {
        Some(n) => n as i64,
        None => -errno::EMFILE,
    }
}

pub fn dup2(old: u64, new: u64) -> i64 {
    let f = match file(old) {
        Ok(f) => f,
        Err(e) => return e,
    };
    if old == new {
        return new as i64;
    }
    match sched::fd_set(new, f) {
        Some(previous) => {
            drop(previous);
            new as i64
        }
        None => -errno::EBADF,
    }
}

pub fn chdir(ptr: u64, len: u64) -> i64 {
    let p = match user_path(ptr, len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Ok(norm) = path::normalize(&p) else {
        return -errno::EINVAL;
    };
    let cred = sched::current_cred();
    let r = fs::with(|v| {
        let n = v.resolve(&norm, &cred)?;
        let st = v.stat(n)?;
        if st.kind != Kind::Dir {
            return Err(FsError::NotDir);
        }
        if !velora_vfs::may_access(&st, &cred, velora_vfs::access::EXEC) {
            return Err(FsError::Access);
        }
        Ok(())
    });
    match r {
        Ok(()) => {
            sched::set_cwd(norm);
            0
        }
        Err(e) => err(e),
    }
}

pub fn getcwd(buf: u64, len: u64) -> i64 {
    let cwd = sched::current_cwd();
    if (len as usize) < cwd.len() {
        return -errno::EINVAL;
    }
    if uaccess::write(buf, cwd.as_bytes()) {
        cwd.len() as i64
    } else {
        -errno::EFAULT
    }
}

fn set_attr(ptr: u64, len: u64, attr: velora_vfs::SetAttr) -> i64 {
    let p = match user_path(ptr, len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cred = sched::current_cred();
    let now = fs::now();
    match fs::with(|v| v.setattr(&p, attr, &cred, now)) {
        Ok(()) => 0,
        Err(e) => err(e),
    }
}

pub fn chmod(ptr: u64, len: u64, mode: u64) -> i64 {
    set_attr(
        ptr,
        len,
        velora_vfs::SetAttr {
            mode: Some(mode as u32 & 0o7777),
            ..Default::default()
        },
    )
}

pub fn chown(ptr: u64, len: u64, uid: u64, gid: u64) -> i64 {
    if sched::current_cred().uid != 0 {
        return -errno::EPERM;
    }
    set_attr(
        ptr,
        len,
        velora_vfs::SetAttr {
            uid: Some(uid as u32),
            gid: Some(gid as u32),
            mode: None,
        },
    )
}

pub fn open(ptr: u64, len: u64, flags: u64, mode: u64) -> i64 {
    let p = match user_path(ptr, len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cred = sched::current_cred();
    match fs::open_file(&p, flags, mode as u32 & 0o7777, &cred) {
        Ok(f) => match sched::fd_install(Arc::new(SpinLock::new(f))) {
            Some(fd) => fd as i64,
            None => -errno::EMFILE,
        },
        Err(e) => err(e),
    }
}

pub fn close(fd: u64) -> i64 {
    match sched::fd_remove(fd) {
        Some(f) => {
            drop(f);
            0
        }
        None => -errno::EBADF,
    }
}

/// Reads from a file at its offset. Console descriptors are handled by
/// the caller.
pub fn read_file(f: &FileRef, buf: u64, len: u64) -> i64 {
    let mut of = f.lock();
    if !of.readable {
        return -errno::EBADF;
    }
    let (node, kind) = match node_of(&of) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if kind == Kind::Dir {
        return -errno::EISDIR;
    }
    let len = len.min(MAX_IO) as usize;
    let mut tmp = vec![0u8; len];
    let n = match fs::with(|v| v.read(node, of.offset, &mut tmp)) {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    if !uaccess::write(buf, &tmp[..n]) {
        return -errno::EFAULT;
    }
    of.offset += n as u64;
    n as i64
}

pub fn write_file(f: &FileRef, buf: u64, len: u64) -> i64 {
    let mut of = f.lock();
    if !of.writable {
        return -errno::EBADF;
    }
    let (node, _) = match node_of(&of) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let len = len.min(MAX_IO);
    let Some(data) = uaccess::read_slice(buf, len) else {
        return -errno::EFAULT;
    };
    let now = fs::now();
    let append = of.append;
    let offset = of.offset;
    let r = fs::with(|v| {
        let off = if append { v.stat(node)?.size } else { offset };
        v.write(node, off, data, now).map(|n| (off, n))
    });
    match r {
        Ok((off, n)) => {
            of.offset = off + n as u64;
            n as i64
        }
        Err(e) => err(e),
    }
}

pub fn seek(fd: u64, offset: i64, how: u64) -> i64 {
    let f = match file(fd) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let mut of = f.lock();
    let (node, _) = match node_of(&of) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let base = match how {
        whence::SET => 0,
        whence::CURRENT => of.offset as i64,
        whence::END => match fs::with(|v| v.stat(node)) {
            Ok(st) => st.size as i64,
            Err(e) => return err(e),
        },
        _ => return -errno::EINVAL,
    };
    match base.checked_add(offset) {
        Some(n) if n >= 0 => {
            of.offset = n as u64;
            n
        }
        _ => -errno::EINVAL,
    }
}

fn abi_stat(node: Node, st: &velora_vfs::Stat) -> Stat {
    Stat {
        dev: node.mount as u64,
        ino: st.ino,
        mode: st.kind.mode_bits() | st.mode,
        nlink: st.nlink,
        uid: st.uid,
        gid: st.gid,
        size: st.size,
        allocated: st.allocated,
        mtime: st.mtime,
        ctime: st.ctime,
    }
}

fn console_stat() -> Stat {
    Stat {
        mode: velora_abi::mode::CHAR_DEVICE | 0o666,
        nlink: 1,
        ..Default::default()
    }
}

pub fn stat(ptr: u64, len: u64, out: u64) -> i64 {
    let p = match user_path(ptr, len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cred = sched::current_cred();
    let r = fs::with(|v| {
        let n = v.resolve(&p, &cred)?;
        Ok((n, v.stat(n)?))
    });
    match r {
        Ok((n, st)) => put(out, &abi_stat(n, &st)),
        Err(e) => err(e),
    }
}

pub fn fstat(fd: u64, out: u64) -> i64 {
    let f = match file(fd) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let of = f.lock();
    let st = match &of.handle {
        Handle::Console => console_stat(),
        Handle::Socket { .. } => Stat {
            mode: velora_abi::mode::SOCKET | 0o600,
            nlink: 1,
            ..Default::default()
        },
        Handle::Pipe { .. } => Stat {
            mode: velora_abi::mode::FIFO | 0o600,
            nlink: 1,
            ..Default::default()
        },
        Handle::File { node, .. } => match fs::with(|v| v.stat(*node)) {
            Ok(st) => abi_stat(*node, &st),
            Err(e) => return err(e),
        },
    };
    put(out, &st)
}

fn put<T: Copy>(out: u64, v: &T) -> i64 {
    if uaccess::write(out, as_bytes(v)) {
        0
    } else {
        -errno::EFAULT
    }
}

/// Fills `buf` with as many directory records as fit and advances the
/// descriptor's position. Returns 0 at the end of the directory.
pub fn readdir(fd: u64, buf: u64, len: u64) -> i64 {
    let f = match file(fd) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let mut of = f.lock();
    let (node, dir_path) = match &of.handle {
        Handle::File {
            node,
            kind: Kind::Dir,
            path,
        } => (*node, path.clone()),
        _ => return -errno::ENOTDIR,
    };
    let cap = len.min(64 * 1024) as usize;
    let mut out = vec![0u8; cap];
    let mut used = 0;
    let mut pos = of.offset;
    let mut too_small = false;
    let r: Result<(), FsError> = fs::with(|v| {
        while let Some((e, next)) = v.readdir(node, &dir_path, pos)? {
            let rec = (size_of::<DirentHeader>() + e.name.len()).next_multiple_of(8);
            if used + rec > cap {
                too_small = used == 0;
                break;
            }
            let h = DirentHeader {
                ino: e.ino,
                rec_len: rec as u16,
                name_len: e.name.len() as u16,
                kind: (e.kind.mode_bits() >> 12) as u8,
                _pad: [0; 3],
            };
            out[used..used + size_of::<DirentHeader>()].copy_from_slice(as_bytes(&h));
            let n0 = used + size_of::<DirentHeader>();
            out[n0..n0 + e.name.len()].copy_from_slice(e.name.as_bytes());
            used += rec;
            pos = next;
        }
        Ok(())
    });
    if let Err(e) = r {
        return err(e);
    }
    if too_small {
        return -errno::EINVAL; // the next record does not fit
    }
    if !uaccess::write(buf, &out[..used]) {
        return -errno::EFAULT;
    }
    of.offset = pos;
    used as i64
}

pub fn mkdir(ptr: u64, len: u64, mode: u64) -> i64 {
    let p = match user_path(ptr, len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cred = sched::current_cred();
    let now = fs::now();
    match fs::with(|v| v.create(&p, Kind::Dir, mode as u32 & 0o7777, &cred, now)) {
        Ok(_) => 0,
        Err(e) => err(e),
    }
}

pub fn unlink(ptr: u64, len: u64) -> i64 {
    let p = match user_path(ptr, len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cred = sched::current_cred();
    let now = fs::now();
    match fs::with(|v| v.unlink(&p, &cred, now)) {
        Ok(()) => 0,
        Err(e) => err(e),
    }
}

pub fn rename(old: u64, old_len: u64, new: u64, new_len: u64) -> i64 {
    let (a, b) = match (user_path(old, old_len), user_path(new, new_len)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let cred = sched::current_cred();
    let now = fs::now();
    match fs::with(|v| v.rename(&a, &b, &cred, now)) {
        Ok(()) => 0,
        Err(e) => err(e),
    }
}

pub fn ftruncate(fd: u64, size: u64) -> i64 {
    let f = match file(fd) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let of = f.lock();
    if !of.writable {
        return -errno::EBADF;
    }
    let (node, kind) = match node_of(&of) {
        Ok(n) => n,
        Err(_) => return -errno::EINVAL,
    };
    if kind == Kind::Dir {
        return -errno::EISDIR;
    }
    let now = fs::now();
    match fs::with(|v| v.truncate(node, size, now)) {
        Ok(()) => 0,
        Err(e) => err(e),
    }
}

pub fn sync() -> i64 {
    match fs::with(|v| v.sync_all()) {
        Ok(()) => 0,
        Err(e) => err(e),
    }
}

pub fn statfs(ptr: u64, len: u64, out: u64) -> i64 {
    let p = match user_path(ptr, len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cred = sched::current_cred();
    let r = fs::with(|v| {
        let n = v.resolve(&p, &cred)?;
        let st = v.statfs(n)?;
        let fsys = v.fs(n.mount)?;
        Ok((st, fsys.fs_type(), fsys.read_only()))
    });
    match r {
        Ok((st, ty, ro)) => {
            let mut fs_type = [0u8; 16];
            let k = ty.len().min(15);
            fs_type[..k].copy_from_slice(&ty.as_bytes()[..k]);
            let out_st = FsStat {
                block_size: st.block_size,
                blocks: st.blocks,
                free_blocks: st.free_blocks,
                inodes: st.inodes,
                free_inodes: st.free_inodes,
                fs_type,
                read_only: ro as u64,
            };
            put(out, &out_st)
        }
        Err(e) => err(e),
    }
}

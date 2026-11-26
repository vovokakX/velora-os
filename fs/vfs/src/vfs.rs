//! Mount table and path operations with permission checks.
//!
//! A path belongs to the mount with the longest matching mount path; the
//! rest is looked up inside that file system. Mount points do not have to
//! exist as directories in the parent file system, but `readdir` lists
//! them so they can be found.

use crate::{
    Cred, DirEntry, FileSystem, FsError, FsStat, Ino, Kind, NewAttr, Result, SetAttr, Stat, access,
    may_access, path,
};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

pub type MountId = u32;

/// A file: the mount it lives on and its inode number there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub mount: MountId,
    pub ino: Ino,
}

struct Mount {
    id: MountId,
    path: String,
    source: String,
    fs: Box<dyn FileSystem>,
    /// Open files on this mount; unmount waits for zero.
    open: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountInfo {
    pub id: MountId,
    pub path: String,
    pub source: String,
    pub fs_type: &'static str,
    pub read_only: bool,
}

/// Synthetic `readdir` positions for mount points start here.
const MOUNT_POS: u64 = 1 << 62;

#[derive(Default)]
pub struct Vfs {
    mounts: Vec<Mount>,
    next_id: MountId,
}

impl Vfs {
    pub fn new() -> Self {
        Self {
            mounts: Vec::new(),
            next_id: 1,
        }
    }

    pub fn mount(&mut self, at: &str, source: &str, fs: Box<dyn FileSystem>) -> Result<MountId> {
        let at = path::normalize(at)?;
        if self.mounts.iter().any(|m| m.path == at) {
            return Err(FsError::Busy);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.mounts.push(Mount {
            id,
            path: at,
            source: String::from(source),
            fs,
            open: 0,
        });
        Ok(id)
    }

    /// Syncs and removes the mount at `at`, returning its file system.
    pub fn unmount(&mut self, at: &str) -> Result<Box<dyn FileSystem>> {
        let at = path::normalize(at)?;
        let i = self
            .mounts
            .iter()
            .position(|m| m.path == at)
            .ok_or(FsError::Invalid)?;
        let nested = self
            .mounts
            .iter()
            .any(|m| m.path != at && path::is_within(&m.path, &at));
        if self.mounts[i].open > 0 || nested {
            return Err(FsError::Busy);
        }
        self.mounts[i].fs.sync()?;
        Ok(self.mounts.remove(i).fs)
    }

    pub fn mounts(&self) -> Vec<MountInfo> {
        self.mounts
            .iter()
            .map(|m| MountInfo {
                id: m.id,
                path: m.path.clone(),
                source: m.source.clone(),
                fs_type: m.fs.fs_type(),
                read_only: m.fs.read_only(),
            })
            .collect()
    }

    fn index(&self, id: MountId) -> Result<usize> {
        self.mounts
            .iter()
            .position(|m| m.id == id)
            .ok_or(FsError::NotFound)
    }

    /// The file system of a mount, for operations the VFS does not wrap.
    pub fn fs(&mut self, id: MountId) -> Result<&mut dyn FileSystem> {
        let i = self.index(id)?;
        Ok(&mut *self.mounts[i].fs)
    }

    /// Records an open file on `id` (unmount is refused while any exist).
    pub fn open_ref(&mut self, id: MountId) {
        if let Ok(i) = self.index(id) {
            self.mounts[i].open += 1;
        }
    }

    pub fn close_ref(&mut self, id: MountId) {
        if let Ok(i) = self.index(id) {
            self.mounts[i].open = self.mounts[i].open.saturating_sub(1);
        }
    }

    /// Mount holding normalised `p` and the path inside it.
    fn locate<'a>(&self, p: &'a str) -> Result<(usize, &'a str)> {
        let mut best: Option<usize> = None;
        for (i, m) in self.mounts.iter().enumerate() {
            if path::is_within(p, &m.path)
                && best.is_none_or(|b| m.path.len() > self.mounts[b].path.len())
            {
                best = Some(i);
            }
        }
        let i = best.ok_or(FsError::NotFound)?;
        let rest = if self.mounts[i].path == "/" {
            p
        } else {
            &p[self.mounts[i].path.len()..]
        };
        Ok((i, rest))
    }

    fn walk(&mut self, norm: &str, cred: &Cred) -> Result<Node> {
        let (i, rest) = self.locate(norm)?;
        let m = &mut self.mounts[i];
        let mut ino = m.fs.root();
        for comp in rest.split('/').filter(|c| !c.is_empty()) {
            let st = m.fs.stat(ino)?;
            if st.kind != Kind::Dir {
                return Err(FsError::NotDir);
            }
            if !may_access(&st, cred, access::EXEC) {
                return Err(FsError::Access);
            }
            ino = m.fs.lookup(ino, comp)?;
        }
        Ok(Node { mount: m.id, ino })
    }

    /// Finds the file at an absolute path.
    pub fn resolve(&mut self, p: &str, cred: &Cred) -> Result<Node> {
        let norm = path::normalize(p)?;
        self.walk(&norm, cred)
    }

    /// Resolves the parent directory of `p` for a change in it: the caller
    /// needs write and search permission there. Returns the parent, the
    /// normalised path and the last name. Mount points cannot be changed.
    fn parent_for_change(&mut self, p: &str, cred: &Cred) -> Result<(Node, String, String)> {
        let norm = path::normalize(p)?;
        let (parent, name) = path::split_last(&norm).ok_or(FsError::Busy)?;
        if self.mounts.iter().any(|m| m.path == norm) {
            return Err(FsError::Busy);
        }
        let (parent, name) = (String::from(parent), String::from(name));
        let dir = self.walk(&parent, cred)?;
        let i = self.index(dir.mount)?;
        let fs = &mut self.mounts[i].fs;
        let st = fs.stat(dir.ino)?;
        if st.kind != Kind::Dir {
            return Err(FsError::NotDir);
        }
        if fs.read_only() {
            return Err(FsError::ReadOnly);
        }
        if !may_access(&st, cred, access::WRITE | access::EXEC) {
            return Err(FsError::Access);
        }
        Ok((dir, norm, name))
    }

    pub fn stat(&mut self, node: Node) -> Result<Stat> {
        self.fs(node.mount)?.stat(node.ino)
    }

    pub fn stat_path(&mut self, p: &str, cred: &Cred) -> Result<Stat> {
        let n = self.resolve(p, cred)?;
        self.stat(n)
    }

    /// Creates a file or directory owned by `cred` with permission bits
    /// `mode`.
    pub fn create(
        &mut self,
        p: &str,
        kind: Kind,
        mode: u32,
        cred: &Cred,
        now: u64,
    ) -> Result<Node> {
        let (dir, _, name) = self.parent_for_change(p, cred)?;
        let attr = NewAttr {
            mode: mode & 0o7777,
            uid: cred.uid,
            gid: cred.gid,
        };
        let ino = self
            .fs(dir.mount)?
            .create(dir.ino, &name, kind, attr, now)?;
        Ok(Node {
            mount: dir.mount,
            ino,
        })
    }

    /// Removes a file or an empty directory.
    pub fn unlink(&mut self, p: &str, cred: &Cred, now: u64) -> Result<()> {
        let (dir, norm, name) = self.parent_for_change(p, cred)?;
        if self.mounts.iter().any(|m| path::is_within(&m.path, &norm)) {
            return Err(FsError::Busy);
        }
        self.fs(dir.mount)?.unlink(dir.ino, &name, now)
    }

    pub fn rename(&mut self, from: &str, to: &str, cred: &Cred, now: u64) -> Result<()> {
        let (fdir, fnorm, fname) = self.parent_for_change(from, cred)?;
        let (tdir, tnorm, tname) = self.parent_for_change(to, cred)?;
        if fdir.mount != tdir.mount {
            return Err(FsError::CrossDevice);
        }
        if fnorm == tnorm {
            return Ok(());
        }
        if path::is_within(&tnorm, &fnorm) {
            return Err(FsError::Invalid); // a directory into itself
        }
        if self.mounts.iter().any(|m| path::is_within(&m.path, &fnorm)) {
            return Err(FsError::Busy);
        }
        self.fs(fdir.mount)?
            .rename(fdir.ino, &fname, tdir.ino, &tname, now)
    }

    /// Changes permissions (owner or root) and ownership (root only).
    pub fn setattr(&mut self, p: &str, attr: SetAttr, cred: &Cred, now: u64) -> Result<()> {
        let n = self.resolve(p, cred)?;
        let st = self.stat(n)?;
        if cred.uid != 0 {
            if attr.uid.is_some_and(|u| u != st.uid) || attr.gid.is_some_and(|g| g != st.gid) {
                return Err(FsError::NotPermitted);
            }
            if attr.mode.is_some() && cred.uid != st.uid {
                return Err(FsError::NotPermitted);
            }
        }
        let fs = self.fs(n.mount)?;
        if fs.read_only() {
            return Err(FsError::ReadOnly);
        }
        fs.setattr(n.ino, attr, now)
    }

    /// Directory entry at `pos` in `dir` (opened at normalised `dir_path`),
    /// including mount points directly below it.
    pub fn readdir(
        &mut self,
        dir: Node,
        dir_path: &str,
        pos: u64,
    ) -> Result<Option<(DirEntry, u64)>> {
        if pos < MOUNT_POS
            && let Some(r) = self.fs(dir.mount)?.readdir(dir.ino, pos)?
        {
            return Ok(Some(r));
        }
        let mut k = pos.saturating_sub(MOUNT_POS) as usize;
        let children: Vec<(String, MountId)> = self
            .mounts
            .iter()
            .filter_map(|m| {
                let (parent, name) = path::split_last(&m.path)?;
                (parent == dir_path).then(|| (String::from(name), m.id))
            })
            .collect();
        while let Some((name, id)) = children.get(k) {
            k += 1;
            // Skip names the directory already lists.
            let fs = self.fs(dir.mount)?;
            if fs.lookup(dir.ino, name).is_ok() {
                continue;
            }
            let root = self.fs(*id)?.root();
            return Ok(Some((
                DirEntry {
                    ino: root,
                    kind: Kind::Dir,
                    name: name.clone(),
                },
                MOUNT_POS + k as u64,
            )));
        }
        Ok(None)
    }

    pub fn read(&mut self, n: Node, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.fs(n.mount)?.read(n.ino, offset, buf)
    }

    pub fn write(&mut self, n: Node, offset: u64, data: &[u8], now: u64) -> Result<usize> {
        let fs = self.fs(n.mount)?;
        if fs.read_only() {
            return Err(FsError::ReadOnly);
        }
        fs.write(n.ino, offset, data, now)
    }

    pub fn truncate(&mut self, n: Node, size: u64, now: u64) -> Result<()> {
        let fs = self.fs(n.mount)?;
        if fs.read_only() {
            return Err(FsError::ReadOnly);
        }
        fs.truncate(n.ino, size, now)
    }

    pub fn statfs(&mut self, n: Node) -> Result<FsStat> {
        self.fs(n.mount)?.statfs()
    }

    /// Syncs every mount; returns the first error but tries all.
    pub fn sync_all(&mut self) -> Result<()> {
        let mut r = Ok(());
        for m in &mut self.mounts {
            if let Err(e) = m.fs.sync() {
                r = r.and(Err(e));
            }
        }
        r
    }

    /// Reads a whole file (used to load programs).
    pub fn read_all(&mut self, p: &str, cred: &Cred) -> Result<Vec<u8>> {
        let n = self.resolve(p, cred)?;
        let st = self.stat(n)?;
        if st.kind == Kind::Dir {
            return Err(FsError::IsDir);
        }
        let mut buf = Vec::new();
        buf.try_reserve_exact(st.size as usize)
            .map_err(|_| FsError::NoMemory)?;
        buf.resize(st.size as usize, 0);
        let mut done = 0;
        while done < buf.len() {
            let k = self.read(n, done as u64, &mut buf[done..])?;
            if k == 0 {
                break;
            }
            done += k;
        }
        buf.truncate(done);
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemFs;

    const USER: Cred = Cred {
        uid: 1000,
        gid: 1000,
    };

    fn setup() -> Vfs {
        let mut v = Vfs::new();
        v.mount("/", "root", Box::new(MemFs::new(0o755, 1 << 20)))
            .unwrap();
        v.create("/mnt", Kind::Dir, 0o755, &Cred::ROOT, 1).unwrap();
        v.create("/home", Kind::Dir, 0o777, &Cred::ROOT, 1).unwrap();
        v.mount("/mnt/data", "memdisk", Box::new(MemFs::new(0o777, 1 << 20)))
            .unwrap();
        v
    }

    fn names(v: &mut Vfs, p: &str) -> Vec<String> {
        let n = v.resolve(p, &Cred::ROOT).unwrap();
        let mut out = Vec::new();
        let mut pos = 0;
        while let Some((e, next)) = v.readdir(n, p, pos).unwrap() {
            out.push(e.name);
            pos = next;
        }
        out
    }

    #[test]
    fn paths_cross_mounts() {
        let mut v = setup();
        let f = v
            .create("/mnt/data/a.txt", Kind::File, 0o644, &Cred::ROOT, 5)
            .unwrap();
        let root_id = v.resolve("/", &Cred::ROOT).unwrap().mount;
        assert_ne!(f.mount, root_id);
        v.write(f, 0, b"hello", 6).unwrap();
        assert_eq!(
            v.read_all("/mnt/data/../data/./a.txt", &Cred::ROOT)
                .unwrap(),
            b"hello"
        );
        assert_eq!(names(&mut v, "/mnt"), ["data"]);
        assert_eq!(names(&mut v, "/"), ["home", "mnt"]);
        assert_eq!(
            v.stat_path("/mnt/data", &Cred::ROOT).unwrap().kind,
            Kind::Dir
        );
    }

    #[test]
    fn rename_across_mounts_is_refused() {
        let mut v = setup();
        v.create("/home/x", Kind::File, 0o644, &Cred::ROOT, 1)
            .unwrap();
        assert_eq!(
            v.rename("/home/x", "/mnt/data/x", &Cred::ROOT, 2),
            Err(FsError::CrossDevice)
        );
        v.rename("/home/x", "/home/y", &Cred::ROOT, 2).unwrap();
        assert!(v.resolve("/home/y", &Cred::ROOT).is_ok());
        assert_eq!(v.resolve("/home/x", &Cred::ROOT), Err(FsError::NotFound));
    }

    #[test]
    fn mount_points_are_protected() {
        let mut v = setup();
        assert_eq!(v.unlink("/mnt/data", &Cred::ROOT, 1), Err(FsError::Busy));
        assert_eq!(v.unlink("/mnt", &Cred::ROOT, 1), Err(FsError::Busy));
        assert_eq!(
            v.rename("/mnt", "/mnt2", &Cred::ROOT, 1),
            Err(FsError::Busy)
        );
        let n = v.resolve("/mnt/data", &Cred::ROOT).unwrap();
        v.open_ref(n.mount);
        assert_eq!(v.unmount("/mnt/data").err(), Some(FsError::Busy));
        v.close_ref(n.mount);
        assert!(v.unmount("/mnt/data").is_ok());
        assert_eq!(names(&mut v, "/mnt"), Vec::<String>::new());
    }

    #[test]
    fn permissions_are_enforced() {
        let mut v = setup();
        // A user cannot create in a root-owned 0755 directory...
        assert_eq!(
            v.create("/mnt/x", Kind::File, 0o644, &USER, 1),
            Err(FsError::Access)
        );
        // ...but can in a world-writable one, and owns the result.
        let n = v.create("/home/u", Kind::Dir, 0o700, &USER, 1).unwrap();
        assert_eq!(v.stat(n).unwrap().uid, 1000);
        v.create("/home/u/secret", Kind::File, 0o600, &USER, 1)
            .unwrap();
        // Another user cannot search a 0700 directory.
        let other = Cred {
            uid: 1001,
            gid: 1001,
        };
        assert_eq!(v.resolve("/home/u/secret", &other), Err(FsError::Access));
        // Only root may give files away; only the owner may chmod.
        let give = SetAttr {
            uid: Some(0),
            ..Default::default()
        };
        assert_eq!(
            v.setattr("/home/u/secret", give, &USER, 2),
            Err(FsError::NotPermitted)
        );
        let chmod = SetAttr {
            mode: Some(0o644),
            ..Default::default()
        };
        v.setattr("/home/u/secret", chmod, &USER, 2).unwrap();
        assert_eq!(
            v.setattr("/home/u", chmod, &other, 2),
            Err(FsError::NotPermitted)
        );
        assert!(v.setattr("/home/u/secret", give, &Cred::ROOT, 2).is_ok());
    }

    #[test]
    fn directory_cannot_move_into_itself() {
        let mut v = setup();
        v.create("/home/a", Kind::Dir, 0o755, &Cred::ROOT, 1)
            .unwrap();
        v.create("/home/a/b", Kind::Dir, 0o755, &Cred::ROOT, 1)
            .unwrap();
        assert_eq!(
            v.rename("/home/a", "/home/a/b/c", &Cred::ROOT, 2),
            Err(FsError::Invalid)
        );
    }
}

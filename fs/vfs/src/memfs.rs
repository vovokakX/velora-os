//! A file system kept in memory: used for /tmp and as a reference in tests.

use crate::{
    DirEntry, FileSystem, FsError, FsStat, Ino, Kind, NewAttr, Result, SetAttr, Stat, path,
};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

struct MemNode {
    stat: Stat,
    data: Vec<u8>,
    children: BTreeMap<String, Ino>,
    parent: Ino,
}

pub struct MemFs {
    nodes: BTreeMap<Ino, MemNode>,
    next: Ino,
    /// Limit on file data, in bytes.
    capacity: u64,
    used: u64,
}

const ROOT: Ino = 1;

impl MemFs {
    /// An empty file system whose root has `mode` and is owned by root.
    pub fn new(root_mode: u32, capacity: u64) -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            ROOT,
            MemNode {
                stat: Stat {
                    ino: ROOT,
                    kind: Kind::Dir,
                    mode: root_mode,
                    uid: 0,
                    gid: 0,
                    nlink: 2,
                    size: 0,
                    allocated: 0,
                    mtime: 0,
                    ctime: 0,
                },
                data: Vec::new(),
                children: BTreeMap::new(),
                parent: ROOT,
            },
        );
        Self {
            nodes,
            next: 2,
            capacity,
            used: 0,
        }
    }

    fn node(&mut self, ino: Ino) -> Result<&mut MemNode> {
        self.nodes.get_mut(&ino).ok_or(FsError::NotFound)
    }

    fn dir(&mut self, ino: Ino) -> Result<&mut MemNode> {
        let n = self.node(ino)?;
        if n.stat.kind != Kind::Dir {
            return Err(FsError::NotDir);
        }
        Ok(n)
    }

    fn remove_node(&mut self, ino: Ino) {
        if let Some(n) = self.nodes.remove(&ino) {
            self.used -= n.data.len() as u64;
        }
    }

    /// True if `anc` is `ino` or one of its ancestors.
    fn is_ancestor(&self, anc: Ino, mut ino: Ino) -> bool {
        loop {
            if ino == anc {
                return true;
            }
            if ino == ROOT {
                return false;
            }
            ino = self.nodes[&ino].parent;
        }
    }
}

impl FileSystem for MemFs {
    fn fs_type(&self) -> &'static str {
        "memfs"
    }

    fn root(&self) -> Ino {
        ROOT
    }

    fn read_only(&self) -> bool {
        false
    }

    fn stat(&mut self, ino: Ino) -> Result<Stat> {
        Ok(self.node(ino)?.stat)
    }

    fn lookup(&mut self, dir: Ino, name: &str) -> Result<Ino> {
        self.dir(dir)?
            .children
            .get(name)
            .copied()
            .ok_or(FsError::NotFound)
    }

    fn readdir(&mut self, dir: Ino, pos: u64) -> Result<Option<(DirEntry, u64)>> {
        let d = self.dir(dir)?;
        let Some((name, &ino)) = d.children.iter().nth(pos as usize) else {
            return Ok(None);
        };
        let name = name.clone();
        let kind = self.nodes[&ino].stat.kind;
        Ok(Some((DirEntry { ino, kind, name }, pos + 1)))
    }

    fn read(&mut self, ino: Ino, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let n = self.node(ino)?;
        if n.stat.kind == Kind::Dir {
            return Err(FsError::IsDir);
        }
        let len = n.data.len() as u64;
        if offset >= len {
            return Ok(0);
        }
        let k = buf.len().min((len - offset) as usize);
        buf[..k].copy_from_slice(&n.data[offset as usize..offset as usize + k]);
        Ok(k)
    }

    fn write(&mut self, ino: Ino, offset: u64, data: &[u8], now: u64) -> Result<usize> {
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(FsError::TooBig)?;
        let (cap, used) = (self.capacity, self.used);
        let n = self.node(ino)?;
        if n.stat.kind == Kind::Dir {
            return Err(FsError::IsDir);
        }
        let old = n.data.len() as u64;
        if end > old {
            if used + (end - old) > cap {
                return Err(FsError::NoSpace);
            }
            n.data.resize(end as usize, 0);
        }
        n.data[offset as usize..end as usize].copy_from_slice(data);
        n.stat.size = n.data.len() as u64;
        n.stat.allocated = n.stat.size;
        n.stat.mtime = now;
        n.stat.ctime = now;
        let new = n.stat.size;
        self.used = used + new - old;
        Ok(data.len())
    }

    fn truncate(&mut self, ino: Ino, size: u64, now: u64) -> Result<()> {
        let (cap, used) = (self.capacity, self.used);
        let n = self.node(ino)?;
        if n.stat.kind == Kind::Dir {
            return Err(FsError::IsDir);
        }
        let old = n.data.len() as u64;
        if size > old && used + (size - old) > cap {
            return Err(FsError::NoSpace);
        }
        n.data.resize(size as usize, 0);
        n.stat.size = size;
        n.stat.allocated = size;
        n.stat.mtime = now;
        n.stat.ctime = now;
        self.used = used + size - old;
        Ok(())
    }

    fn create(&mut self, dir: Ino, name: &str, kind: Kind, attr: NewAttr, now: u64) -> Result<Ino> {
        path::check_name(name)?;
        if kind != Kind::File && kind != Kind::Dir {
            return Err(FsError::Unsupported);
        }
        let ino = self.next;
        let d = self.dir(dir)?;
        if d.children.contains_key(name) {
            return Err(FsError::Exists);
        }
        d.children.insert(String::from(name), ino);
        d.stat.mtime = now;
        if kind == Kind::Dir {
            d.stat.nlink += 1;
        }
        self.next += 1;
        self.nodes.insert(
            ino,
            MemNode {
                stat: Stat {
                    ino,
                    kind,
                    mode: attr.mode & 0o7777,
                    uid: attr.uid,
                    gid: attr.gid,
                    nlink: if kind == Kind::Dir { 2 } else { 1 },
                    size: 0,
                    allocated: 0,
                    mtime: now,
                    ctime: now,
                },
                data: Vec::new(),
                children: BTreeMap::new(),
                parent: dir,
            },
        );
        Ok(ino)
    }

    fn unlink(&mut self, dir: Ino, name: &str, now: u64) -> Result<()> {
        let ino = self.lookup(dir, name)?;
        let n = &self.nodes[&ino];
        let is_dir = n.stat.kind == Kind::Dir;
        if is_dir && !n.children.is_empty() {
            return Err(FsError::NotEmpty);
        }
        let d = self.dir(dir)?;
        d.children.remove(name);
        d.stat.mtime = now;
        if is_dir {
            d.stat.nlink -= 1;
        }
        self.remove_node(ino);
        Ok(())
    }

    fn rename(
        &mut self,
        from: Ino,
        from_name: &str,
        to: Ino,
        to_name: &str,
        now: u64,
    ) -> Result<()> {
        path::check_name(to_name)?;
        let ino = self.lookup(from, from_name)?;
        self.dir(to)?;
        let is_dir = self.nodes[&ino].stat.kind == Kind::Dir;
        if is_dir && self.is_ancestor(ino, to) {
            return Err(FsError::Invalid);
        }
        if let Ok(old) = self.lookup(to, to_name) {
            if old == ino {
                return Ok(());
            }
            let o = &self.nodes[&old];
            match (is_dir, o.stat.kind == Kind::Dir) {
                (true, false) => return Err(FsError::NotDir),
                (false, true) => return Err(FsError::IsDir),
                (true, true) if !o.children.is_empty() => return Err(FsError::NotEmpty),
                _ => {}
            }
            self.unlink(to, to_name, now)?;
        }
        let f = self.dir(from)?;
        f.children.remove(from_name);
        f.stat.mtime = now;
        if is_dir {
            f.stat.nlink -= 1;
        }
        let t = self.dir(to)?;
        t.children.insert(String::from(to_name), ino);
        t.stat.mtime = now;
        if is_dir {
            t.stat.nlink += 1;
        }
        let n = self.node(ino)?;
        n.parent = to;
        n.stat.ctime = now;
        Ok(())
    }

    fn setattr(&mut self, ino: Ino, attr: SetAttr, now: u64) -> Result<()> {
        let n = self.node(ino)?;
        if let Some(m) = attr.mode {
            n.stat.mode = m & 0o7777;
        }
        if let Some(u) = attr.uid {
            n.stat.uid = u;
        }
        if let Some(g) = attr.gid {
            n.stat.gid = g;
        }
        n.stat.ctime = now;
        Ok(())
    }

    fn statfs(&mut self) -> Result<FsStat> {
        Ok(FsStat {
            block_size: 1,
            blocks: self.capacity,
            free_blocks: self.capacity - self.used,
            inodes: 0,
            free_inodes: 0,
        })
    }
}

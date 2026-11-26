//! The initramfs (a ustar archive in memory) as a read-only file system.
//! Directories missing from the archive are created from file paths.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use velora_tar::{Archive, Kind as TarKind};
use velora_vfs::{DirEntry, FileSystem, FsError, Ino, Kind, Result, Stat};

struct TNode {
    name: String,
    kind: Kind,
    mode: u32,
    data: &'static [u8],
    children: Vec<Ino>,
}

pub struct TarFs {
    /// Index = inode number; 0 is unused, 1 is the root.
    nodes: Vec<TNode>,
}

const ROOT: Ino = 1;

impl TarFs {
    pub fn new(archive: &'static [u8]) -> Self {
        let dir = |name: &str| TNode {
            name: String::from(name),
            kind: Kind::Dir,
            mode: 0o755,
            data: &[],
            children: Vec::new(),
        };
        let mut fs = Self {
            nodes: vec![dir(""), dir("")],
        };
        for e in Archive::new(archive).flatten() {
            let mut buf = [0u8; 256];
            let p = e.path(&mut buf);
            if p.is_empty() {
                continue;
            }
            let mut parent = ROOT;
            let parts: Vec<&str> = p.split('/').filter(|c| !c.is_empty()).collect();
            for part in &parts[..parts.len() - 1] {
                parent = fs.child_or_dir(parent, part);
            }
            let last = parts[parts.len() - 1];
            match e.kind {
                TarKind::Directory => {
                    let d = fs.child_or_dir(parent, last);
                    fs.nodes[d as usize].mode = e.mode & 0o7777;
                }
                TarKind::File => {
                    if fs.child(parent, last).is_none() {
                        let ino = fs.nodes.len() as Ino;
                        fs.nodes.push(TNode {
                            name: String::from(last),
                            kind: Kind::File,
                            mode: e.mode & 0o7777,
                            data: e.data,
                            children: Vec::new(),
                        });
                        fs.nodes[parent as usize].children.push(ino);
                    }
                }
                TarKind::Other(_) => {}
            }
        }
        fs
    }

    pub fn file_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.kind == Kind::File).count()
    }

    fn child(&self, dir: Ino, name: &str) -> Option<Ino> {
        self.nodes[dir as usize]
            .children
            .iter()
            .copied()
            .find(|&c| self.nodes[c as usize].name == name)
    }

    fn child_or_dir(&mut self, dir: Ino, name: &str) -> Ino {
        if let Some(c) = self.child(dir, name) {
            return c;
        }
        let ino = self.nodes.len() as Ino;
        self.nodes.push(TNode {
            name: String::from(name),
            kind: Kind::Dir,
            mode: 0o755,
            data: &[],
            children: Vec::new(),
        });
        self.nodes[dir as usize].children.push(ino);
        ino
    }

    fn node(&self, ino: Ino) -> Result<&TNode> {
        if ino == 0 {
            return Err(FsError::NotFound);
        }
        self.nodes.get(ino as usize).ok_or(FsError::NotFound)
    }
}

impl FileSystem for TarFs {
    fn fs_type(&self) -> &'static str {
        "initramfs"
    }

    fn root(&self) -> Ino {
        ROOT
    }

    fn stat(&mut self, ino: Ino) -> Result<Stat> {
        let n = self.node(ino)?;
        Ok(Stat {
            ino,
            kind: n.kind,
            mode: n.mode,
            uid: 0,
            gid: 0,
            nlink: 1,
            size: n.data.len() as u64,
            allocated: n.data.len() as u64,
            mtime: 0,
            ctime: 0,
        })
    }

    fn lookup(&mut self, dir: Ino, name: &str) -> Result<Ino> {
        if self.node(dir)?.kind != Kind::Dir {
            return Err(FsError::NotDir);
        }
        self.child(dir, name).ok_or(FsError::NotFound)
    }

    fn readdir(&mut self, dir: Ino, pos: u64) -> Result<Option<(DirEntry, u64)>> {
        let d = self.node(dir)?;
        if d.kind != Kind::Dir {
            return Err(FsError::NotDir);
        }
        Ok(d.children.get(pos as usize).map(|&c| {
            let n = &self.nodes[c as usize];
            (
                DirEntry {
                    ino: c,
                    kind: n.kind,
                    name: n.name.clone(),
                },
                pos + 1,
            )
        }))
    }

    fn read(&mut self, ino: Ino, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let n = self.node(ino)?;
        if n.kind == Kind::Dir {
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
}

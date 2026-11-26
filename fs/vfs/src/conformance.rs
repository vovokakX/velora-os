//! A behaviour check any writable file system must pass. Host tests run it
//! on every file system; the kernel tests run it on real disks. Errors are
//! returned as text instead of panicking so the kernel can report them.

use crate::{FileSystem, FsError, Ino, Kind, NewAttr};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

type R = core::result::Result<(), String>;

const ATTR: NewAttr = NewAttr {
    mode: 0o644,
    uid: 0,
    gid: 0,
};
const NOW: u64 = 1_800_000_000;

macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        if !$cond {
            return Err(format!($($arg)*));
        }
    };
}

fn ok<T>(r: crate::Result<T>, what: &str) -> core::result::Result<T, String> {
    r.map_err(|e| format!("{}: {}", what, e))
}

fn pattern(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn read_all(
    fs: &mut dyn FileSystem,
    ino: Ino,
    chunk: usize,
) -> core::result::Result<Vec<u8>, String> {
    let size = ok(fs.stat(ino), "stat")?.size as usize;
    let mut out = vec![0u8; size];
    let mut off = 0;
    while off < size {
        let end = (off + chunk).min(size);
        let n = ok(fs.read(ino, off as u64, &mut out[off..end]), "read")?;
        ensure!(n > 0, "read returned 0 at {} of {}", off, size);
        off += n;
    }
    let mut extra = [0u8; 16];
    ensure!(
        ok(fs.read(ino, size as u64, &mut extra), "read at end")? == 0,
        "read past the end returned data"
    );
    Ok(out)
}

fn list(fs: &mut dyn FileSystem, dir: Ino) -> core::result::Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut pos = 0;
    while let Some((e, next)) = ok(fs.readdir(dir, pos), "readdir")? {
        ensure!(next > pos, "readdir position did not advance");
        ensure!(
            ok(fs.lookup(dir, &e.name), "lookup listed name")? == e.ino,
            "readdir inode differs from lookup for {}",
            e.name
        );
        names.push(e.name);
        pos = next;
    }
    names.sort();
    Ok(names)
}

/// Runs every check inside a new directory `name` in the root and removes
/// it afterwards. `big` is the size of the large-file check in bytes.
pub fn run(fs: &mut dyn FileSystem, name: &str, big: usize) -> R {
    let root = fs.root();
    let free_before = ok(fs.statfs(), "statfs")?.free_blocks;
    let top = ok(
        fs.create(root, name, Kind::Dir, ATTR, NOW),
        "mkdir test dir",
    )?;
    let st = ok(fs.stat(top), "stat dir")?;
    ensure!(st.kind == Kind::Dir, "new directory has kind {:?}", st.kind);
    ensure!(list(fs, top)?.is_empty(), "new directory is not empty");

    // Small file: write, read back, append, overwrite in the middle.
    let f = ok(fs.create(top, "hello.txt", Kind::File, ATTR, NOW), "create")?;
    ensure!(
        fs.create(top, "hello.txt", Kind::File, ATTR, NOW) == Err(FsError::Exists),
        "creating an existing name did not fail with Exists"
    );
    ok(fs.write(f, 0, b"Hello, Velora", NOW), "write")?;
    ok(fs.write(f, 13, b"!\n", NOW), "append")?;
    ok(fs.write(f, 7, b"VELORA", NOW), "overwrite")?;
    ensure!(
        read_all(fs, f, 5)? == b"Hello, VELORA!\n",
        "small file content wrong"
    );
    ensure!(ok(fs.stat(f), "stat")?.size == 15, "small file size wrong");

    // A write past the end leaves zeros in the gap.
    let g = ok(fs.create(top, "gap", Kind::File, ATTR, NOW), "create gap")?;
    ok(fs.write(g, 10_000, b"end", NOW), "write past end")?;
    let data = read_all(fs, g, 4096)?;
    ensure!(data.len() == 10_003, "gap file size {}", data.len());
    ensure!(
        data[..10_000].iter().all(|&b| b == 0),
        "gap is not zero-filled"
    );

    // Large file written in odd-sized pieces, read in other sizes.
    let big_data = pattern(big, 7);
    let b = ok(
        fs.create(top, "big.bin", Kind::File, ATTR, NOW),
        "create big",
    )?;
    let mut off = 0;
    while off < big_data.len() {
        let end = (off + 7001).min(big_data.len());
        let n = ok(
            fs.write(b, off as u64, &big_data[off..end], NOW),
            "write big",
        )?;
        ensure!(n == end - off, "short write");
        off = end;
    }
    ensure!(
        read_all(fs, b, 4096)? == big_data,
        "large file differs (4 KiB reads)"
    );
    ensure!(
        read_all(fs, b, 65_537)? == big_data,
        "large file differs (64 KiB reads)"
    );

    // Truncate down and up again: the regrown part reads as zeros.
    ok(fs.truncate(b, 5000, NOW), "truncate down")?;
    ok(fs.truncate(b, 9000, NOW), "truncate up")?;
    let t = read_all(fs, b, 4096)?;
    ensure!(t[..5000] == big_data[..5000], "data before the cut changed");
    ensure!(
        t[5000..].iter().all(|&x| x == 0),
        "regrown part is not zero"
    );

    // Names: Unicode, spaces, long names.
    let long = "n".repeat(200);
    for n in ["Длинное имя файла.txt", "with space", long.as_str()] {
        let i = ok(
            fs.create(top, n, Kind::File, ATTR, NOW),
            "create unicode/long name",
        )?;
        ensure!(
            ok(fs.lookup(top, n), "lookup unicode/long name")? == i,
            "lookup gave another inode for {}",
            n
        );
    }

    // Many entries in one directory.
    let many = ok(fs.create(top, "many", Kind::Dir, ATTR, NOW), "mkdir many")?;
    let count = 150;
    for i in 0..count {
        let name = format!("file-{:03}", i);
        let ino = ok(fs.create(many, &name, Kind::File, ATTR, NOW), "create many")?;
        ok(fs.write(ino, 0, name.as_bytes(), NOW), "write many")?;
    }
    let names = list(fs, many)?;
    ensure!(
        names.len() == count,
        "readdir listed {} of {} entries",
        names.len(),
        count
    );
    for i in (0..count).step_by(2) {
        ok(
            fs.unlink(many, &format!("file-{:03}", i), NOW),
            "unlink many",
        )?;
    }
    let names = list(fs, many)?;
    ensure!(
        names.len() == count / 2,
        "after unlink {} entries remain",
        names.len()
    );
    let i = ok(fs.lookup(many, "file-149"), "lookup survivor")?;
    ensure!(
        read_all(fs, i, 100)? == b"file-149",
        "survivor content wrong"
    );
    ensure!(
        fs.lookup(many, "file-000") == Err(FsError::NotFound),
        "removed file still found"
    );

    // Errors.
    ensure!(
        fs.unlink(top, "many", NOW) == Err(FsError::NotEmpty),
        "removing a non-empty directory did not fail with NotEmpty"
    );
    ensure!(
        fs.lookup(f, "x") == Err(FsError::NotDir),
        "lookup in a file did not fail with NotDir"
    );
    ensure!(
        fs.unlink(top, "missing", NOW) == Err(FsError::NotFound),
        "unlink of a missing name did not fail with NotFound"
    );

    // Renames: same directory, across directories, replacing a file,
    // moving a directory.
    ok(
        fs.rename(top, "hello.txt", top, "renamed.txt", NOW),
        "rename",
    )?;
    ensure!(
        fs.lookup(top, "hello.txt") == Err(FsError::NotFound),
        "old name still exists"
    );
    let r = ok(fs.lookup(top, "renamed.txt"), "lookup renamed")?;
    ensure!(
        read_all(fs, r, 64)? == b"Hello, VELORA!\n",
        "renamed file content wrong"
    );
    ok(
        fs.rename(top, "renamed.txt", many, "moved.txt", NOW),
        "rename across dirs",
    )?;
    ok(
        fs.rename(many, "moved.txt", many, "file-149", NOW),
        "rename over a file",
    )?;
    let r = ok(fs.lookup(many, "file-149"), "lookup replaced")?;
    ensure!(
        read_all(fs, r, 64)? == b"Hello, VELORA!\n",
        "replacing rename kept the old content"
    );
    let sub = ok(fs.create(top, "sub", Kind::Dir, ATTR, NOW), "mkdir sub")?;
    ok(fs.rename(top, "many", sub, "many2", NOW), "move directory")?;
    let m = ok(fs.lookup(sub, "many2"), "lookup moved dir")?;
    ensure!(
        list(fs, m)?.len() == count / 2,
        "moved directory lost entries"
    );

    // Clean up everything and check that the space came back.
    for n in list(fs, m)? {
        ok(fs.unlink(m, &n, NOW), "cleanup")?;
    }
    ok(fs.unlink(sub, "many2", NOW), "cleanup")?;
    ok(fs.unlink(top, "sub", NOW), "cleanup")?;
    for n in list(fs, top)? {
        ok(fs.unlink(top, &n, NOW), "cleanup")?;
    }
    ok(fs.unlink(root, name, NOW), "remove test dir")?;
    ok(fs.sync(), "sync")?;
    let free_after = ok(fs.statfs(), "statfs")?.free_blocks;
    ensure!(
        free_after == free_before,
        "free blocks {} after cleanup, {} before",
        free_after,
        free_before
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::MemFs;

    #[test]
    fn memfs_passes() {
        let mut fs = MemFs::new(0o755, 16 << 20);
        super::run(&mut fs, "conformance", 300_000).unwrap();
    }
}

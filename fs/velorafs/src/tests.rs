use super::*;
use velora_vfs::{Cred, MemDisk, Vfs, conformance};

const MIB: u64 = 1024 * 1024;
const ATTR: NewAttr = NewAttr {
    mode: 0o644,
    uid: 0,
    gid: 0,
};

fn opts() -> FormatOptions<'static> {
    FormatOptions {
        label: "test",
        now: 1_700_000_000,
        uuid: [7; 16],
    }
}

fn fresh(size: u64) -> VeloraFs<MemDisk> {
    let mut d = MemDisk::new(512, size / 512);
    format(&mut d, &opts()).unwrap();
    VeloraFs::mount(d, false).unwrap()
}

fn remount(fs: VeloraFs<MemDisk>) -> VeloraFs<MemDisk> {
    let img = fs.into_device().durable_image();
    VeloraFs::mount(MemDisk::from_image(512, img), false).unwrap()
}

fn assert_clean(fs: &mut VeloraFs<MemDisk>) {
    let r = fs.check();
    assert!(r.is_clean(), "fsck: {:?}", r.errors);
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u32 * 7 + seed as u32 * 13 + (i / 4096) as u32) as u8)
        .collect()
}

fn read_file(fs: &mut VeloraFs<MemDisk>, ino: Ino) -> Vec<u8> {
    let size = fs.stat(ino).unwrap().size as usize;
    let mut v = vec![0u8; size];
    assert_eq!(fs.read(ino, 0, &mut v).unwrap(), size);
    v
}

fn lookup_path(fs: &mut VeloraFs<MemDisk>, p: &str) -> Option<Ino> {
    let mut ino = fs.root();
    for c in p.split('/').filter(|c| !c.is_empty()) {
        ino = fs.lookup(ino, c).ok()?;
    }
    Some(ino)
}

#[test]
fn format_and_probe() {
    let mut d = MemDisk::new(512, 64 * MIB / 512);
    assert!(probe(&mut d).is_none());
    let sb = format(&mut d, &opts()).unwrap();
    assert_eq!(probe(&mut d).unwrap(), sb);
    assert_eq!(sb.label_str(), "test");
    assert_eq!(sb.total_blocks, 16384);
    let mut fs = VeloraFs::mount(d, false).unwrap();
    assert!(!fs.recovered());
    let root = fs.stat(ROOT_INO).unwrap();
    assert_eq!((root.kind, root.mode, root.nlink), (Kind::Dir, 0o755, 2));
    assert_clean(&mut fs);
}

#[test]
fn too_small_device_is_refused() {
    let mut d = MemDisk::new(512, 100);
    assert_eq!(format(&mut d, &opts()).err(), Some(FsError::NoSpace));
}

#[test]
fn passes_conformance() {
    let mut fs = fresh(64 * MIB);
    conformance::run(&mut fs, "conformance", 3 * MIB as usize).unwrap();
    assert_clean(&mut fs);
    // And again after a remount, with the journal in use.
    let mut fs = remount(fs);
    conformance::run(&mut fs, "again", 300_000).unwrap();
    assert_clean(&mut fs);
}

#[test]
fn files_persist_across_remount() {
    let mut fs = fresh(16 * MIB);
    let root = fs.root();
    let d = fs.create(root, "docs", Kind::Dir, ATTR, 5).unwrap();
    let f = fs.create(d, "отчёт.txt", Kind::File, ATTR, 5).unwrap();
    let data = pattern(100_000, 3);
    fs.write(f, 0, &data, 6).unwrap();
    let attr = SetAttr {
        mode: Some(0o600),
        uid: Some(1000),
        gid: Some(100),
    };
    fs.setattr(f, attr, 7).unwrap();
    let mut fs = remount(fs);
    let f = lookup_path(&mut fs, "/docs/отчёт.txt").unwrap();
    assert_eq!(read_file(&mut fs, f), data);
    let st = fs.stat(f).unwrap();
    assert_eq!(
        (st.mode, st.uid, st.gid, st.mtime, st.ctime),
        (0o600, 1000, 100, 6, 7)
    );
    assert_eq!(st.allocated, 25 * 4096);
    assert_clean(&mut fs);
}

#[test]
fn works_under_the_vfs() {
    let mut v = Vfs::new();
    v.mount("/", "mem", Box::new(fresh(8 * MIB))).unwrap();
    let open_root = SetAttr {
        mode: Some(0o777),
        ..Default::default()
    };
    v.setattr("/", open_root, &Cred::ROOT, 1).unwrap();
    let n = v
        .create("/f", Kind::File, 0o600, &Cred { uid: 5, gid: 5 }, 1)
        .unwrap();
    v.write(n, 0, b"x", 1).unwrap();
    assert_eq!(v.read_all("/f", &Cred::ROOT).unwrap(), b"x");
    assert_eq!(v.stat_path("/f", &Cred::ROOT).unwrap().uid, 5);
}

#[test]
fn sparse_file_uses_extent_block() {
    let mut fs = fresh(16 * MIB);
    let root = fs.root();
    let free0 = fs.statfs().unwrap().free_blocks;
    let f = fs.create(root, "sparse", Kind::File, ATTR, 1).unwrap();
    // One byte in every other block: 40 extents that cannot merge.
    for i in 0..40u64 {
        fs.write(f, i * 2 * 4096, &[i as u8 + 1], 1).unwrap();
    }
    let st = fs.stat(f).unwrap();
    assert_eq!(st.allocated, 41 * 4096, "40 data blocks + 1 extent block");
    let data = read_file(&mut fs, f);
    for i in 0..40usize {
        assert_eq!(data[i * 8192], i as u8 + 1);
        if i < 39 {
            assert_eq!(data[i * 8192 + 1], 0);
        }
    }
    assert_clean(&mut fs);
    let mut fs = remount(fs);
    let f = lookup_path(&mut fs, "/sparse").unwrap();
    assert_eq!(read_file(&mut fs, f), data);
    fs.truncate(f, 4096 * 6, 2).unwrap();
    assert_eq!(
        fs.stat(f).unwrap().allocated,
        3 * 4096,
        "extent block freed"
    );
    fs.unlink(root, "sparse", 3).unwrap();
    assert_eq!(fs.statfs().unwrap().free_blocks, free0);
    assert_clean(&mut fs);
}

#[test]
fn directory_grows_over_many_blocks() {
    let mut fs = fresh(16 * MIB);
    let root = fs.root();
    let d = fs.create(root, "big", Kind::Dir, ATTR, 1).unwrap();
    let long = "z".repeat(200);
    for i in 0..300 {
        fs.create(d, &format!("{}{}", long, i), Kind::File, ATTR, 1)
            .unwrap();
    }
    assert!(fs.stat(d).unwrap().size >= 15 * 4096);
    let mut fs = remount(fs);
    let d = lookup_path(&mut fs, "/big").unwrap();
    let mut pos = 0;
    let mut n = 0;
    while let Some((_, next)) = fs.readdir(d, pos).unwrap() {
        pos = next;
        n += 1;
    }
    assert_eq!(n, 300);
    assert_clean(&mut fs);
}

#[test]
fn running_out_of_space_leaves_a_consistent_fs() {
    let mut fs = fresh(4 * MIB);
    let root = fs.root();
    let f = fs.create(root, "fill", Kind::File, ATTR, 1).unwrap();
    let chunk = vec![0xaau8; 256 * 1024];
    let mut off = 0;
    loop {
        match fs.write(f, off, &chunk, 1) {
            Ok(_) => off += chunk.len() as u64,
            Err(FsError::NoSpace) => break,
            Err(e) => panic!("unexpected {e}"),
        }
    }
    let free = fs.statfs().unwrap().free_blocks;
    assert!(free < 64);
    // The failed write changed nothing.
    assert_eq!(fs.stat(f).unwrap().size, off);
    assert_clean(&mut fs);
    // Creating files still works until inodes or blocks run out.
    fs.unlink(root, "fill", 2).unwrap();
    assert_clean(&mut fs);
}

#[test]
fn committed_transaction_is_replayed() {
    let mut fs = fresh(8 * MIB);
    let root = fs.root();
    fs.create(root, "before", Kind::File, ATTR, 1).unwrap();
    fs.stop_after_commit = true;
    let f = fs.create(root, "after", Kind::File, ATTR, 2).unwrap();
    // The machine "stopped": nothing else can be written.
    assert_eq!(fs.write(f, 0, b"x", 3), Err(FsError::Io));
    // Home locations were never written, yet the file exists after mount.
    let img = fs.into_device().durable_image();
    let mut fs = VeloraFs::mount(MemDisk::from_image(512, img), false).unwrap();
    assert!(fs.recovered());
    assert!(lookup_path(&mut fs, "/after").is_some());
    assert!(lookup_path(&mut fs, "/before").is_some());
    assert_clean(&mut fs);
    // A second mount has nothing to replay.
    let fs = remount(fs);
    assert!(!fs.recovered());
}

#[test]
fn corruption_is_detected() {
    let mut fs = fresh(8 * MIB);
    let root = fs.root();
    let d = fs.create(root, "dir", Kind::Dir, ATTR, 1).unwrap();
    let f = fs.create(d, "file", Kind::File, ATTR, 1).unwrap();
    let sb = fs.superblock().clone();
    let mut img = fs.into_device().durable_image();
    // Flip a bit in the directory block of "dir" (the second data block
    // allocated after the root's).
    let dir_blk = sb.data_start + 1;
    img[dir_blk as usize * 4096 + 20] ^= 0x40;
    let mut fs = VeloraFs::mount(MemDisk::from_image(512, img.clone()), false).unwrap();
    assert_eq!(fs.lookup(d, "file"), Err(FsError::Corrupt));
    assert!(!fs.check().is_clean());
    // Flip a bit in the inode of "file".
    img[dir_blk as usize * 4096 + 20] ^= 0x40;
    let off = (sb.itable_start * 4096 + f * 256 + 16) as usize;
    img[off] ^= 1;
    let mut fs = VeloraFs::mount(MemDisk::from_image(512, img.clone()), false).unwrap();
    assert_eq!(fs.stat(f), Err(FsError::Corrupt));
    assert!(!fs.check().is_clean());
    // A bad superblock checksum refuses the mount.
    img[off] ^= 1;
    img[20] ^= 1;
    assert!(VeloraFs::mount(MemDisk::from_image(512, img), false).is_err());
}

#[test]
fn read_only_mount_refuses_changes() {
    let fs = fresh(8 * MIB);
    let img = fs.into_device().durable_image();
    let mut fs = VeloraFs::mount(MemDisk::from_image(512, img), true).unwrap();
    let root = fs.root();
    assert_eq!(
        fs.create(root, "x", Kind::File, ATTR, 1),
        Err(FsError::ReadOnly)
    );
}

/// Runs the crash scenario; errors after the power cut are expected.
fn scenario(fs: &mut VeloraFs<MemDisk>) {
    let root = fs.root();
    let _ = (|| -> Result<()> {
        let a = fs.create(root, "a", Kind::File, ATTR, 10)?;
        fs.write(a, 0, &pattern(20_000, 1), 10)?;
        let d = fs.create(root, "d", Kind::Dir, ATTR, 11)?;
        fs.rename(root, "a", d, "b", 12)?;
        fs.unlink(root, "c", 13)?;
        let big = fs.create(root, "big", Kind::File, ATTR, 14)?;
        fs.write(big, 0, &pattern(200_000, 2), 14)?;
        fs.truncate(big, 1000, 15)?;
        Ok(())
    })();
}

fn crash_setup() -> Vec<u8> {
    let mut fs = fresh(8 * MIB);
    let root = fs.root();
    let k = fs.create(root, "keep", Kind::File, ATTR, 1).unwrap();
    fs.write(k, 0, &pattern(10_000, 9), 1).unwrap();
    fs.create(root, "c", Kind::File, ATTR, 1).unwrap();
    fs.into_device().durable_image()
}

fn check_after_crash(img: Vec<u8>, n: u64, cache: bool) {
    let ctx = format!("cut after {} writes, cache {}", n, cache);
    let mut fs = VeloraFs::mount(MemDisk::from_image(512, img), false)
        .unwrap_or_else(|e| panic!("{ctx}: mount failed: {e}"));
    let r = fs.check();
    assert!(r.is_clean(), "{ctx}: fsck {:?}", r.errors);
    let k = lookup_path(&mut fs, "/keep").expect("keep missing");
    assert_eq!(
        read_file(&mut fs, k),
        pattern(10_000, 9),
        "{ctx}: keep changed"
    );
    let a = lookup_path(&mut fs, "/a");
    let b = lookup_path(&mut fs, "/d/b");
    assert!(!(a.is_some() && b.is_some()), "{ctx}: rename not atomic");
    for f in [a, b].into_iter().flatten() {
        let data = read_file(&mut fs, f);
        assert!(
            data.is_empty() || data == pattern(20_000, 1),
            "{ctx}: a/b content"
        );
    }
    if let Some(b) = b {
        assert_eq!(
            read_file(&mut fs, b).len(),
            20_000,
            "{ctx}: renamed before written"
        );
    }
    if let Some(big) = lookup_path(&mut fs, "/big") {
        let data = read_file(&mut fs, big);
        let full = pattern(200_000, 2);
        assert!(
            data.is_empty() || data == full || data == full[..1000],
            "{ctx}: big has {} bytes",
            data.len()
        );
        assert!(
            lookup_path(&mut fs, "/c").is_none(),
            "{ctx}: ops out of order"
        );
    }
    // The file system stays usable.
    let root = fs.root();
    let x = fs.create(root, "x", Kind::File, ATTR, 20).unwrap();
    fs.write(x, 0, b"after", 20).unwrap();
    assert_clean(&mut fs);
}

#[test]
fn power_cut_at_every_write_is_recovered() {
    let base = crash_setup();
    // Count the writes of a complete run.
    let mut fs = VeloraFs::mount(MemDisk::from_image(512, base.clone()), false).unwrap();
    scenario(&mut fs);
    let total = fs.into_device().writes;
    assert!(total > 20);
    for cache in [false, true] {
        for n in 0..=total {
            let mut d = MemDisk::from_image(512, base.clone());
            d.set_write_cache(cache);
            d.cut_power_after(n);
            let mut fs = VeloraFs::mount(d, false).unwrap();
            scenario(&mut fs);
            check_after_crash(fs.into_device().durable_image(), n, cache);
        }
    }
}

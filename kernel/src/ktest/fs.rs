//! Stage 7 tests: VFS, devfs, /tmp, VeloraFS and FAT32 on a real disk.
//!
//! The test runner attaches a blank 128 MiB "fs" disk and boots the test
//! image twice with it (see tests/qemu_run.py):
//! - boot 1 creates VeloraFS on the first 64 MiB and FAT32 on the rest,
//!   checks them, leaves files behind and finally stops in the middle of
//!   a VeloraFS transaction (journal committed, home blocks not written),
//!   as a power cut would;
//! - boot 2 finds VeloraFS mounted at boot with the journal replayed and
//!   checks that every file from boot 1 is intact.
//!
//! After each boot the runner checks both file systems on the host with
//! `velora-fsck`.

use crate::drivers::block::{self, BlockDevice};
use crate::exec::{self, ExecError};
use crate::fs::{self, DiskIo};
use crate::{kprint, kprintln, sched};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use velora_vfs::{Cred, FileSystem, FsError, Kind, MemFs, SetAttr, Slice, conformance};
use velorafs::VeloraFs;

const MIB: u64 = 1024 * 1024;
const FS_DISK_SIZE: u64 = 128 * MIB;
const HALF_SECTORS: u64 = 64 * MIB / 512;

fn fs_disk() -> Arc<dyn BlockDevice> {
    block::devices()
        .into_iter()
        .find(|d| d.size_bytes() == FS_DISK_SIZE)
        .expect("no 128 MiB test disk for the file system tests")
}

fn velora_part() -> Slice<DiskIo> {
    Slice {
        dev: DiskIo::new(fs_disk()),
        start: 0,
        count: HALF_SECTORS,
    }
}

fn fat_part() -> Slice<DiskIo> {
    Slice {
        dev: DiskIo::new(fs_disk()),
        start: HALF_SECTORS,
        count: HALF_SECTORS,
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u32 * 13 + seed as u32 + (i / 4096) as u32) as u8)
        .collect()
}

fn run(argv: &[&str]) -> i64 {
    let pid = exec::spawn(argv[0], argv, &[]).expect("spawn failed");
    sched::wait_pid(pid).expect("process vanished")
}

pub(crate) fn write_file(p: &str, data: &[u8]) {
    let now = fs::now();
    fs::with(|v| {
        let n = match v.resolve(p, &Cred::ROOT) {
            Ok(n) => {
                v.truncate(n, 0, now)?;
                n
            }
            Err(_) => v.create(p, Kind::File, 0o644, &Cred::ROOT, now)?,
        };
        v.write(n, 0, data, now).map(|_| ())
    })
    .unwrap_or_else(|e| panic!("write {}: {}", p, e));
}

pub(crate) fn read_file(p: &str) -> Result<Vec<u8>, FsError> {
    fs::with(|v| v.read_all(p, &Cred::ROOT))
}

fn mkdir(p: &str) {
    fs::with(|v| v.create(p, Kind::Dir, 0o755, &Cred::ROOT, fs::now()))
        .unwrap_or_else(|e| panic!("mkdir {}: {}", p, e));
}

fn conformance_at(at: &str, name: &str, big: usize) {
    let r = fs::with(|v| {
        let id = v.resolve(at, &Cred::ROOT).unwrap().mount;
        let fsys = v.fs(id).unwrap();
        conformance::run(fsys, name, big)
    });
    if let Err(e) = r {
        panic!("conformance on {}: {}", at, e);
    }
}

fn fsck_at(at: &str) {
    let errors = fs::with(|v| {
        let id = v.resolve(at, &Cred::ROOT).unwrap().mount;
        v.fs(id).unwrap().fsck()
    })
    .expect("fsck failed to run");
    assert!(errors.is_empty(), "fsck on {}: {:?}", at, errors);
}

// --- tests --------------------------------------------------------------------

pub fn vfs_mounted_at_boot() {
    let mounts = fs::with(|v| v.mounts());
    let paths: Vec<&str> = mounts.iter().map(|m| m.path.as_str()).collect();
    kprint!("({:?}) ", paths);
    for (p, ty) in [("/", "initramfs"), ("/dev", "devfs"), ("/tmp", "memfs")] {
        assert!(
            mounts.iter().any(|m| m.path == p && m.fs_type == ty),
            "{} ({}) not mounted",
            p,
            ty
        );
    }
    let hello = read_file("/bin/hello").expect("/bin/hello");
    assert!(velora_exe::elf::probe(&hello));
    let st = fs::with(|v| v.stat_path("/tmp", &Cred::ROOT)).unwrap();
    assert_eq!((st.kind, st.mode), (Kind::Dir, 0o1777));
    assert_eq!(
        fs::with(|v| v.stat_path("/bin/nothing", &Cred::ROOT)).err(),
        Some(FsError::NotFound)
    );
}

pub fn devfs_exposes_disks() {
    let p = "/dev/vd0";
    let st = fs::with(|v| v.stat_path(p, &Cred::ROOT)).expect("/dev/vd0");
    assert_eq!(st.kind, Kind::BlockDevice);
    assert_eq!(st.size, 32 * MIB);
    let mut sector = vec![0u8; 512];
    let n = fs::with(|v| {
        let node = v.resolve(p, &Cred::ROOT)?;
        v.read(node, 512, &mut sector)
    })
    .unwrap();
    assert_eq!(n, 512);
    assert_eq!(&sector[..18], b"VELORA-DISK-VIRTIO");
    // Unaligned access is refused.
    let r = fs::with(|v| {
        let node = v.resolve(p, &Cred::ROOT)?;
        v.read(node, 100, &mut sector)
    });
    assert_eq!(r, Err(FsError::Invalid));
}

pub fn memfs_conformance() {
    let mut m = MemFs::new(0o755, 16 * MIB);
    conformance::run(&mut m, "conformance", 1_000_000).unwrap();
}

pub fn programs_need_execute_permission() {
    let hello = read_file("/bin/hello").unwrap();
    write_file("/tmp/hello-copy", &hello);
    assert_eq!(
        exec::spawn("/tmp/hello-copy", &["/tmp/hello-copy"], &[]).err(),
        Some(ExecError::Access)
    );
    let chmod = SetAttr {
        mode: Some(0o755),
        ..Default::default()
    };
    fs::with(|v| v.setattr("/tmp/hello-copy", chmod, &Cred::ROOT, 0)).unwrap();
    assert_eq!(run(&["/tmp/hello-copy", "from", "/tmp"]), 0);
    assert_eq!(
        exec::spawn("/tmp", &["/tmp"], &[]).err(),
        Some(ExecError::Access)
    );
    fs::with(|v| v.unlink("/tmp/hello-copy", &Cred::ROOT, 0)).unwrap();
}

pub fn velorafs_on_disk() {
    let disk = fs_disk();
    let at = format!("/mnt/{}", disk.name());
    let auto = fs::automounted()
        .into_iter()
        .find(|m| m.device == disk.name());
    match auto {
        None => velorafs_first_boot(&at),
        Some(m) => {
            assert_eq!(m.fs_type, "velorafs");
            velorafs_second_boot(&at, m.recovered);
        }
    }
}

fn velorafs_first_boot(at: &str) {
    let mut part = velora_part();
    let opts = velorafs::FormatOptions {
        label: "ktest",
        now: fs::now(),
        uuid: [0x5a; 16],
    };
    let sb = velorafs::format(&mut part, &opts).expect("format VeloraFS");
    kprint!("({} blocks, {} inodes) ", sb.total_blocks, sb.inode_count);
    let vfs_fs = VeloraFs::mount(part, false).expect("mount VeloraFS");
    fs::with(|v| v.mount(at, "ktest", Box::new(vfs_fs))).unwrap();

    conformance_at(at, "conformance", 2 * MIB as usize);
    let dir = format!("{}/persist", at);
    mkdir(&dir);
    write_file(&format!("{}/hello.txt", dir), b"written in boot 1\n");
    write_file(&format!("{}/big.bin", dir), &pattern(300_000, 1));
    write_file(&format!("{}/boot-count", at), b"1");
    let status = run(&["/bin/vtest", "files", at]);
    assert_eq!(status, 0, "vtest files on VeloraFS: {} failures", status);

    // A process that exits with a file open must not keep the mount busy.
    let hold = format!("{}/hello.txt", dir);
    assert_eq!(run(&["/bin/vtest", "child-hold", &hold]), 0);
    fsck_at(at);
    let fsys = fs::with(|v| v.unmount(at)).expect("unmount (files left open?)");
    drop(fsys);

    // Leave a committed but not checkpointed transaction behind.
    let mut direct = VeloraFs::mount(velora_part(), false).unwrap();
    assert!(!direct.recovered());
    direct.stop_after_commit = true;
    let root = direct.root();
    let attr = velora_vfs::NewAttr {
        mode: 0o644,
        uid: 0,
        gid: 0,
    };
    direct
        .create(root, "replay-me", Kind::File, attr, fs::now())
        .expect("create before the simulated power cut");
    assert_eq!(direct.write(2, 0, b"x", 0), Err(FsError::Io));
    drop(direct);
    kprintln!();
    kprint!("ktest: fs: boot 1: VeloraFS created, journal left with a committed transaction ... ");
}

fn velorafs_second_boot(at: &str, recovered: bool) {
    assert!(recovered, "the journal was not replayed at mount");
    let dir = format!("{}/persist", at);
    assert_eq!(
        read_file(&format!("{}/hello.txt", dir)).unwrap(),
        b"written in boot 1\n"
    );
    assert_eq!(
        read_file(&format!("{}/big.bin", dir)).unwrap(),
        pattern(300_000, 1)
    );
    assert_eq!(read_file(&format!("{}/boot-count", at)).unwrap(), b"1");
    let st = fs::with(|v| v.stat_path(&format!("{}/replay-me", at), &Cred::ROOT))
        .expect("file from the replayed transaction is missing");
    assert_eq!(st.size, 0);
    fsck_at(at);
    let status = run(&["/bin/vtest", "files", at]);
    assert_eq!(status, 0, "vtest files on VeloraFS: {} failures", status);
    conformance_at(at, "conformance-2", MIB as usize);
    write_file(&format!("{}/boot-count", at), b"2");
    fsck_at(at);
    fs::with(|v| v.sync_all()).unwrap();
    kprintln!();
    kprint!("ktest: fs: boot 2: files from boot 1 intact, journal replayed, fsck clean ... ");
}

pub fn fat32_on_disk() {
    let at = "/mnt/fat";
    let mut part = fat_part();
    let first = velora_fat::volume::probe(&mut part).is_none();
    if first {
        velora_fat::format_fat32(&mut part, "VELORA", 0x7e57).expect("format FAT32");
    }
    let mut vol = velora_fat::FatVolume::mount(part, false).expect("mount FAT32");
    assert_eq!(vol.fat_type(), velora_fat::FatType::Fat32);
    // The deliberate panic at the end of the tests is logged here; the
    // host reads the log after boot 1 and boot 2 finds it.
    crate::crashlog::prepare(&mut vol, fs_disk(), HALF_SECTORS, at).expect("panic log");
    fs::with(|v| v.mount(at, "ktest-fat", Box::new(vol))).unwrap();
    let efi = pattern(50_000, 2);
    if first {
        mkdir("/mnt/fat/EFI");
        mkdir("/mnt/fat/EFI/BOOT");
        write_file("/mnt/fat/EFI/BOOT/BOOTX64.EFI", &efi);
        write_file("/mnt/fat/Velora notes.txt", "Файл на FAT32\n".as_bytes());
        conformance_at(at, "conformance", MIB as usize);
        let status = run(&["/bin/vtest", "files", at]);
        assert_eq!(status, 0, "vtest files on FAT32: {} failures", status);
        kprint!("(created) ");
    } else {
        // Names match without regard to case on FAT.
        assert_eq!(read_file("/mnt/fat/efi/boot/bootx64.efi").unwrap(), efi);
        assert_eq!(
            read_file("/mnt/fat/Velora notes.txt").unwrap(),
            "Файл на FAT32\n".as_bytes()
        );
        conformance_at(at, "conformance-2", MIB as usize);
        let prev = read_file("/mnt/fat/velora-panic.prev.log").expect("panic log of boot 1");
        let text = String::from_utf8_lossy(&prev);
        assert!(text.starts_with("VELORA PANIC LOG"), "bad panic log header");
        assert!(
            text.contains("ktest: deliberate panic") && text.contains("KERNEL PANIC"),
            "the panic log of boot 1 has no report"
        );
        kprint!("(files and the panic log from boot 1 intact) ");
    }
    drop(fs::with(|v| v.unmount(at)).expect("unmount FAT32"));
}

/// The VFS refuses what the permission bits forbid (checked with a
/// non-root user on /tmp).
pub fn permissions_are_enforced() {
    let user = Cred {
        uid: 1000,
        gid: 1000,
    };
    let other = Cred {
        uid: 1001,
        gid: 1001,
    };
    let now = fs::now();
    fs::with(|v| -> Result<(), FsError> {
        v.create("/tmp/u", Kind::Dir, 0o700, &user, now)?;
        v.create("/tmp/u/f", Kind::File, 0o600, &user, now)?;
        assert_eq!(v.resolve("/tmp/u/f", &other), Err(FsError::Access));
        assert_eq!(
            v.create("/tmp/u/g", Kind::File, 0o600, &other, now),
            Err(FsError::Access)
        );
        assert_eq!(
            v.create("/etc/x", Kind::File, 0o600, &user, now),
            Err(FsError::ReadOnly)
        );
        assert!(v.resolve("/tmp/u/f", &user).is_ok());
        v.unlink("/tmp/u/f", &user, now)?;
        v.unlink("/tmp/u", &user, now)
    })
    .unwrap();
    let st = fs::with(|v| v.stat_path("/dev/vd0", &Cred::ROOT)).unwrap();
    assert!(!velora_vfs::may_access(
        &st,
        &user,
        velora_vfs::access::READ
    ));
}

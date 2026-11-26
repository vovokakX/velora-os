//! velora-setup: installs Velora OS from the Live system onto a disk.
//!
//! ```text
//! velora-setup                                   asks everything
//! velora-setup --auto --disk vd0 --user anna [--yes]
//! ```
//!
//! With `--auto` the passwords come from the environment variables
//! `SETUP_ROOT_PASSWORD` and `SETUP_USER_PASSWORD`, and `--yes` skips the
//! confirmation (used by the tests).
//!
//! The disk gets a GPT with an EFI system partition (FAT32, 64 MiB:
//! Limine, the kernel and limine.conf) and a VeloraFS root partition with
//! the programs, /etc and the home directory. The file systems are written
//! through `/dev/<disk>` with the same crates the kernel uses; nothing is
//! mounted.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use velora_gpt::Guid;
use velora_vfs::{BlockIo, FileSystem, FsError, Kind, NewAttr, Slice};
use vlibc::fs::{self, File, flags, mode, whence};
use vlibc::{Errno, env, io, print, println, process, term, time};

vlibc::main!(main);

const ESP_MIB: u64 = 64;
const MIN_DISK_MIB: u64 = 128;
const SECTOR: u64 = 512;
const SHARE: &str = "/usr/lib/velora";
const USER_UID: u32 = 1000;

// --- disk access through /dev ------------------------------------------------

/// A whole disk opened through devfs.
struct DevDisk {
    file: File,
    sectors: u64,
}

fn io_err(_: Errno) -> FsError {
    FsError::Io
}

impl BlockIo for DevDisk {
    fn sector_size(&self) -> usize {
        SECTOR as usize
    }

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> velora_vfs::Result<()> {
        self.file
            .seek((lba * SECTOR) as i64, whence::SET)
            .map_err(io_err)?;
        let mut done = 0;
        while done < buf.len() {
            let n = self.file.read(&mut buf[done..]).map_err(io_err)?;
            if n == 0 {
                return Err(FsError::Io);
            }
            done += n;
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> velora_vfs::Result<()> {
        self.file
            .seek((lba * SECTOR) as i64, whence::SET)
            .map_err(io_err)?;
        self.file.write_all(buf).map_err(io_err)
    }

    fn flush(&mut self) -> velora_vfs::Result<()> {
        fs::sync().map_err(io_err)
    }
}

fn open_disk(name: &str) -> Result<DevDisk, String> {
    let path = format!("/dev/{}", name);
    let st = fs::stat(&path).map_err(|e| format!("{}: {}", path, e))?;
    let file =
        File::open_with(&path, flags::READ_WRITE, 0).map_err(|e| format!("{}: {}", path, e))?;
    Ok(DevDisk {
        file,
        sectors: st.size / SECTOR,
    })
}

/// Whole disks (not partitions) with their size in bytes.
fn disks() -> Vec<(String, u64)> {
    let Ok(entries) = fs::read_dir("/dev") else {
        return Vec::new();
    };
    let devs: Vec<(String, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let st = fs::stat(&format!("/dev/{}", e.name)).ok()?;
            (st.mode & mode::TYPE_MASK == mode::BLOCK_DEVICE).then_some((e.name, st.size))
        })
        .collect();
    devs.iter()
        .filter(|(n, _)| {
            // "vd0p1" is a partition when "vd0" exists.
            !n.rfind('p').is_some_and(|i| {
                i > 0
                    && n[i + 1..].chars().all(|c| c.is_ascii_digit())
                    && !n[i + 1..].is_empty()
                    && devs.iter().any(|(d, _)| d == &n[..i])
            })
        })
        .cloned()
        .collect()
}

/// Disks with something mounted from them under /mnt.
fn in_use(disk: &str) -> bool {
    fs::read_dir("/mnt").is_ok_and(|d| {
        d.flatten().any(|e| {
            e.name == disk
                || e.name
                    .strip_prefix(disk)
                    .is_some_and(|r| r.starts_with('p'))
        })
    })
}

// --- writing files into a file system ----------------------------------------

struct Writer<'a> {
    fs: &'a mut dyn FileSystem,
    now: u64,
}

impl Writer<'_> {
    fn dir_of(&mut self, path: &str) -> velora_vfs::Result<(u64, String)> {
        let mut ino = self.fs.root();
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let (last, dirs) = parts.split_last().ok_or(FsError::Invalid)?;
        for d in dirs {
            ino = self.fs.lookup(ino, d)?;
        }
        Ok((ino, String::from(*last)))
    }

    fn mkdir(&mut self, path: &str, mode: u32, uid: u32, gid: u32) -> velora_vfs::Result<()> {
        let (dir, name) = self.dir_of(path)?;
        let attr = NewAttr { mode, uid, gid };
        match self.fs.create(dir, &name, Kind::Dir, attr, self.now) {
            Ok(_) | Err(FsError::Exists) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn file(
        &mut self,
        path: &str,
        data: &[u8],
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> velora_vfs::Result<()> {
        let (dir, name) = self.dir_of(path)?;
        let attr = NewAttr { mode, uid, gid };
        let ino = self.fs.create(dir, &name, Kind::File, attr, self.now)?;
        if !data.is_empty() {
            self.fs.write(ino, 0, data, self.now)?;
        }
        Ok(())
    }
}

// --- questions ---------------------------------------------------------------

fn read_line() -> Option<String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        let n = io::read(0, &mut buf).ok()?;
        if n == 0 {
            return (!out.is_empty()).then(|| String::from_utf8_lossy(&out).into_owned());
        }
        out.extend_from_slice(&buf[..n]);
        if out.ends_with(b"\n") {
            out.pop();
            return Some(String::from_utf8_lossy(&out).into_owned());
        }
    }
}

fn ask(q: &str) -> Option<String> {
    print!("{}", q);
    read_line().map(|s| String::from(s.trim()))
}

fn ask_password(who: &str) -> Option<String> {
    loop {
        print!("Password for {}: ", who);
        term::set_mode(term::NO_ECHO);
        let a = read_line();
        print!("Repeat the password: ");
        let b = read_line();
        term::set_mode(0);
        let (a, b) = (a?, b?);
        if a.is_empty() {
            println!("The password must not be empty.");
        } else if a != b {
            println!("The passwords differ, try again.");
        } else {
            return Some(a);
        }
    }
}

fn valid_user_name(n: &str) -> bool {
    let mut cs = n.chars();
    !n.is_empty()
        && n.len() <= 32
        && n != "root"
        && cs
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

struct Plan {
    disk: String,
    user: String,
    root_password: String,
    user_password: String,
}

fn random_guid() -> Guid {
    let mut r = [0u8; 16];
    process::random(&mut r);
    Guid::from_random(r)
}

fn hash(password: &str) -> String {
    let mut salt = [0u8; 16];
    process::random(&mut salt);
    velora_auth::hash_password(password, &salt, velora_auth::DEFAULT_ITERATIONS)
}

// --- installation ------------------------------------------------------------

fn step(n: u32, what: &str) {
    println!("[{}/6] {}", n, what);
}

fn install(plan: &Plan) -> Result<(), String> {
    let now = time::realtime().map(|t| t.as_secs()).unwrap_or(0);
    let kernel =
        fs::read(&format!("{}/velora", SHARE)).map_err(|e| format!("kernel image: {}", e))?;
    let efi =
        fs::read(&format!("{}/BOOTX64.EFI", SHARE)).map_err(|e| format!("bootloader: {}", e))?;

    step(1, "partitioning (GPT: EFI system partition and root)");
    let mut disk = open_disk(&plan.disk)?;
    let esp_guid = random_guid();
    let root_guid = random_guid();
    let parts = velora_gpt::velora_layout(disk.sectors, ESP_MIB, esp_guid, root_guid)
        .map_err(|e| format!("partition layout: {}", e))?;
    velora_gpt::write(&mut disk, random_guid(), &parts).map_err(|e| format!("GPT: {}", e))?;
    let (esp, root) = (&parts[0], &parts[1]);

    step(2, "formatting the EFI system partition (FAT32)");
    let mut esp_dev = Slice {
        dev: disk,
        start: esp.first_lba,
        count: esp.sectors(),
    };
    let mut volid = [0u8; 4];
    process::random(&mut volid);
    velora_fat::format_fat32(&mut esp_dev, "VELORA-EFI", u32::from_le_bytes(volid))
        .map_err(|e| format!("FAT32: {}", e))?;
    let mut vol =
        velora_fat::FatVolume::mount(esp_dev, false).map_err(|e| format!("FAT32: {}", e))?;
    let conf = format!(
        "# Velora OS (installed by velora-setup)\ntimeout: 0\nserial: yes\nverbose: no\n\n/Velora OS\n    protocol: limine\n    path: boot():/boot/velora\n    cmdline: velora.root=PARTUUID={}\n",
        root_guid
    );
    {
        let mut w = Writer { fs: &mut vol, now };
        let e = |e: FsError| format!("EFI partition: {}", e);
        w.mkdir("/EFI", 0o755, 0, 0).map_err(e)?;
        w.mkdir("/EFI/BOOT", 0o755, 0, 0).map_err(e)?;
        w.mkdir("/boot", 0o755, 0, 0).map_err(e)?;
        w.file("/EFI/BOOT/BOOTX64.EFI", &efi, 0o644, 0, 0)
            .map_err(e)?;
        w.file("/boot/velora", &kernel, 0o644, 0, 0).map_err(e)?;
        w.file("/boot/limine.conf", conf.as_bytes(), 0o644, 0, 0)
            .map_err(e)?;
    }
    vol.sync().map_err(|e| format!("EFI partition: {}", e))?;
    let disk = vol.into_device().dev;

    step(3, "formatting the root partition (VeloraFS)");
    let mut root_dev = Slice {
        dev: disk,
        start: root.first_lba,
        count: root.sectors(),
    };
    let opts = velorafs::FormatOptions {
        label: "velora-root",
        now,
        uuid: root_guid.0,
    };
    velorafs::format(&mut root_dev, &opts).map_err(|e| format!("VeloraFS: {}", e))?;
    let mut vfs =
        velorafs::VeloraFs::mount(root_dev, false).map_err(|e| format!("VeloraFS: {}", e))?;

    step(4, "copying the system");
    let mut w = Writer { fs: &mut vfs, now };
    let e = |e: FsError| format!("root partition: {}", e);
    for (d, m) in [
        ("/bin", 0o755),
        ("/etc", 0o755),
        ("/home", 0o755),
        ("/root", 0o700),
        ("/tmp", 0o1777),
        ("/dev", 0o755),
        ("/mnt", 0o755),
    ] {
        w.mkdir(d, m, 0, 0).map_err(e)?;
    }
    let programs = fs::read_dir("/bin").map_err(|x| format!("/bin: {}", x))?;
    let mut copied = 0;
    for ent in programs.flatten() {
        if ent.name == "velora-setup" || ent.name == "vtest" {
            continue;
        }
        let p = format!("/bin/{}", ent.name);
        let st = fs::stat(&p).map_err(|x| format!("{}: {}", p, x))?;
        let data = fs::read(&p).map_err(|x| format!("{}: {}", p, x))?;
        w.file(&p, &data, st.mode & 0o7777, 0, 0).map_err(e)?;
        copied += 1;
    }
    let motd = fs::read("/etc/motd").unwrap_or_default();
    w.file("/etc/motd", &motd, 0o644, 0, 0).map_err(e)?;
    println!("      {} programs copied", copied);

    step(5, &format!("creating the user {}", plan.user));
    let passwd = format!(
        "root:x:0:0:root:/root:/bin/vsh\n{u}:x:{id}:{id}:{u}:/home/{u}:/bin/vsh\n",
        u = plan.user,
        id = USER_UID
    );
    let group = format!("root:x:0:\n{}:x:{}:\n", plan.user, USER_UID);
    let shadow = format!(
        "root:{}:0:0:99999:7:::\n{}:{}:0:0:99999:7:::\n",
        hash(&plan.root_password),
        plan.user,
        hash(&plan.user_password)
    );
    w.file("/etc/passwd", passwd.as_bytes(), 0o644, 0, 0)
        .map_err(e)?;
    w.file("/etc/group", group.as_bytes(), 0o644, 0, 0)
        .map_err(e)?;
    w.file("/etc/shadow", shadow.as_bytes(), 0o600, 0, 0)
        .map_err(e)?;
    w.mkdir(&format!("/home/{}", plan.user), 0o700, USER_UID, USER_UID)
        .map_err(e)?;

    step(6, "checking the root file system");
    let problems = vfs.check();
    if !problems.is_clean() {
        return Err(format!(
            "the new file system has errors: {:?}",
            problems.errors
        ));
    }
    vfs.sync().map_err(|x| format!("sync: {}", x))?;
    Ok(())
}

fn usage() -> i32 {
    println!("usage: velora-setup");
    println!("       velora-setup --auto --disk NAME --user NAME [--yes]");
    println!("       (--auto reads SETUP_ROOT_PASSWORD and SETUP_USER_PASSWORD)");
    2
}

fn main() -> i32 {
    let args = env::args();
    let argv: Vec<&str> = args.iter().skip(1).collect();
    if argv.contains(&"-h") || argv.contains(&"--help") {
        return usage();
    }
    if process::getuid().0 != 0 {
        println!("velora-setup: run it as root");
        return 1;
    }
    let live = fs::statfs("/").is_ok_and(|s| s.fs_type.starts_with(b"initramfs"));
    if !live {
        println!("velora-setup: this is not the Live system; Velora OS is already installed");
        return 1;
    }
    let all = disks();
    let auto = argv.contains(&"--auto");
    let opt = |name: &str| {
        argv.iter()
            .position(|a| *a == name)
            .and_then(|i| argv.get(i + 1))
            .map(|s| String::from(*s))
    };

    println!("Velora OS {} installer", env!("CARGO_PKG_VERSION"));
    println!();
    if all.is_empty() {
        println!("No disks found.");
        return 1;
    }
    println!("Disks:");
    for (n, size) in &all {
        println!(
            "  {:<10} {:>8} MiB{}",
            n,
            size / (1024 * 1024),
            if in_use(n) { "  (in use)" } else { "" }
        );
    }
    println!();

    let plan = if auto {
        let (Some(disk), Some(user)) = (opt("--disk"), opt("--user")) else {
            return usage();
        };
        let (Some(rp), Some(up)) = (
            env::var("SETUP_ROOT_PASSWORD"),
            env::var("SETUP_USER_PASSWORD"),
        ) else {
            println!("velora-setup: set SETUP_ROOT_PASSWORD and SETUP_USER_PASSWORD");
            return 2;
        };
        Plan {
            disk,
            user,
            root_password: rp.into(),
            user_password: up.into(),
        }
    } else {
        let Some(disk) = ask("Disk to install on: ") else {
            return 1;
        };
        let user = loop {
            let Some(u) = ask("Name of the new user (lowercase letters, digits): ") else {
                return 1;
            };
            if valid_user_name(&u) {
                break u;
            }
            println!("Not a valid user name.");
        };
        let Some(user_password) = ask_password(&user) else {
            return 1;
        };
        let Some(root_password) = ask_password("root") else {
            return 1;
        };
        Plan {
            disk,
            user,
            root_password,
            user_password,
        }
    };

    let Some((_, size)) = all.iter().find(|(n, _)| *n == plan.disk) else {
        println!("velora-setup: no disk named {}", plan.disk);
        return 1;
    };
    if !valid_user_name(&plan.user) {
        println!("velora-setup: '{}' is not a valid user name", plan.user);
        return 1;
    }
    if plan.root_password.is_empty() || plan.user_password.is_empty() {
        println!("velora-setup: passwords must not be empty");
        return 1;
    }
    if *size < MIN_DISK_MIB * 1024 * 1024 {
        println!(
            "velora-setup: {} is too small (at least {} MiB)",
            plan.disk, MIN_DISK_MIB
        );
        return 1;
    }
    if in_use(&plan.disk) {
        println!(
            "velora-setup: {} is mounted under /mnt; it cannot be erased now",
            plan.disk
        );
        return 1;
    }
    println!(
        "Velora OS will be installed on {} ({} MiB) with the user {}.",
        plan.disk,
        size / (1024 * 1024),
        plan.user
    );
    println!("ALL DATA ON {} WILL BE ERASED.", plan.disk);
    if !argv.contains(&"--yes") {
        let answer = ask(&format!("Type 'yes' to erase {} and install: ", plan.disk));
        if answer.as_deref() != Some("yes") {
            println!("Nothing was changed.");
            return 1;
        }
    }
    match install(&plan) {
        Ok(()) => {
            println!();
            println!("Installation complete.");
            println!("Remove the installation media and type 'reboot'.");
            0
        }
        Err(e) => {
            println!();
            println!("velora-setup: installation failed: {}", e);
            println!("The disk may be partly written; run velora-setup again.");
            1
        }
    }
}

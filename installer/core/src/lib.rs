//! velora-install: the installer's core, shared by the terminal installer
//! (`velora-setup`) and the graphical one (`velora-installer`).
//!
//! [`install`] writes the whole system onto any [`BlockIo`] disk: a GPT
//! with an EFI system partition (FAT32: Limine, the kernel, limine.conf)
//! and a VeloraFS root (programs, fonts, /etc, the user's home). The files
//! come from a [`Source`]: the running Live system on Velora (module
//! `live`, feature `velora`), memory in the host tests. Progress is
//! reported in the four phases the graphical installer shows.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod settings;

#[cfg(feature = "velora")]
pub mod live;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use settings::Settings;
use velora_gpt::Guid;
use velora_vfs::{BlockIo, FileSystem, FsError, Kind, NewAttr, Slice};

pub const DEFAULT_ESP_MIB: u64 = 64;
pub const MIN_DISK_MIB: u64 = 128;
pub const USER_UID: u32 = 1000;
const MIB: u64 = 1024 * 1024;

/// How the disk is divided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Size of the EFI system partition.
    pub esp_mib: u64,
    /// Size of the root partition; `None` uses the rest of the disk.
    pub root_mib: Option<u64>,
    /// Label of the root file system.
    pub root_label: String,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            esp_mib: DEFAULT_ESP_MIB,
            root_mib: None,
            root_label: String::from("velora-root"),
        }
    }
}

/// What to install.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub user: String,
    pub user_password: String,
    pub root_password: String,
    pub layout: Layout,
    pub settings: Settings,
}

/// What is wrong with a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    BadUserName,
    EmptyPassword,
    DiskTooSmall { min_mib: u64 },
    PartitionsTooLarge,
    BadLabel,
}

impl Problem {
    pub fn text(&self, lang: settings::Lang) -> String {
        use settings::Lang::*;
        match (self, lang) {
            (Problem::BadUserName, Ru) => String::from(
                "Имя пользователя: строчные латинские буквы, цифры, «_» и «-», с буквы, не root",
            ),
            (Problem::BadUserName, En) => String::from(
                "User name: lowercase letters, digits, '_' and '-', starting with a letter, not root",
            ),
            (Problem::EmptyPassword, Ru) => String::from("Пароль не может быть пустым"),
            (Problem::EmptyPassword, En) => String::from("The password must not be empty"),
            (Problem::DiskTooSmall { min_mib }, Ru) => format!("Диск меньше {} МиБ", min_mib),
            (Problem::DiskTooSmall { min_mib }, En) => {
                format!("The disk is smaller than {} MiB", min_mib)
            }
            (Problem::PartitionsTooLarge, Ru) => String::from("Разделы не помещаются на диск"),
            (Problem::PartitionsTooLarge, En) => String::from("The partitions do not fit on the disk"),
            (Problem::BadLabel, Ru) => String::from("Метка: до 16 латинских букв, цифр, «-» и «_»"),
            (Problem::BadLabel, En) => String::from("Label: up to 16 letters, digits, '-' and '_'"),
        }
    }
}

/// User names the system accepts: lowercase letters, digits, `_`, `-`,
/// starting with a letter or `_`, at most 32 characters, not "root".
pub fn valid_user_name(n: &str) -> bool {
    let mut cs = n.chars();
    !n.is_empty()
        && n.len() <= 32
        && n != "root"
        && cs
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn valid_label(l: &str) -> bool {
    !l.is_empty()
        && l.len() <= 16
        && l
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Checks the user and the layout for a disk of `disk_bytes`.
pub fn check(plan: &Plan, disk_bytes: u64) -> Result<(), Problem> {
    check_user(plan)?;
    check_layout(&plan.layout, disk_bytes)
}

pub fn check_user(plan: &Plan) -> Result<(), Problem> {
    if !valid_user_name(&plan.user) {
        return Err(Problem::BadUserName);
    }
    if plan.user_password.is_empty() || plan.root_password.is_empty() {
        return Err(Problem::EmptyPassword);
    }
    Ok(())
}

pub fn check_layout(l: &Layout, disk_bytes: u64) -> Result<(), Problem> {
    if disk_bytes < MIN_DISK_MIB * MIB {
        return Err(Problem::DiskTooSmall {
            min_mib: MIN_DISK_MIB,
        });
    }
    if !valid_label(&l.root_label) {
        return Err(Problem::BadLabel);
    }
    // 1 MiB before the ESP and about 1 MiB for the backup GPT.
    let need = 2 + l.esp_mib + l.root_mib.unwrap_or(32);
    if need * MIB > disk_bytes || l.root_mib.is_some_and(|r| r < 32) {
        return Err(Problem::PartitionsTooLarge);
    }
    Ok(())
}

/// The partitions [`install`] writes for `layout` on a disk of `sectors`.
pub fn partitions(
    layout: &Layout,
    sectors: u64,
    esp_guid: Guid,
    root_guid: Guid,
) -> Result<Vec<velora_gpt::Partition>, FsError> {
    let mut parts = velora_gpt::velora_layout(sectors, layout.esp_mib, esp_guid, root_guid)?;
    if let Some(mib) = layout.root_mib {
        let root = &mut parts[1];
        let last = root.first_lba + mib * 2048 - 1;
        if last > root.last_lba {
            return Err(FsError::NoSpace);
        }
        root.last_lba = last;
    }
    Ok(parts)
}

/// A file of the Live system to copy.
pub struct SourceFile {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
}

/// Where the installed files come from.
pub trait Source {
    /// Programs and data copied to the root (absolute paths).
    fn files(&mut self) -> Result<Vec<SourceFile>, String>;
    /// The kernel for /boot/velora on the EFI partition.
    fn kernel(&mut self) -> Result<Vec<u8>, String>;
    /// Limine for /EFI/BOOT/BOOTX64.EFI.
    fn bootloader(&mut self) -> Result<Vec<u8>, String>;
    fn random(&mut self, buf: &mut [u8]);
    /// Seconds since 1970 (0 if unknown).
    fn now(&self) -> u64;
}

/// The phases the graphical installer lists, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    CopyFiles,
    Bootloader,
    Configure,
    Finish,
}

impl Phase {
    pub const ALL: [Phase; 4] = [
        Phase::CopyFiles,
        Phase::Bootloader,
        Phase::Configure,
        Phase::Finish,
    ];

    pub fn name(self, lang: settings::Lang) -> &'static str {
        use settings::Lang::*;
        match (self, lang) {
            (Phase::CopyFiles, Ru) => "Копирование файлов",
            (Phase::CopyFiles, En) => "Copying files",
            (Phase::Bootloader, Ru) => "Установка загрузчика",
            (Phase::Bootloader, En) => "Installing the bootloader",
            (Phase::Configure, Ru) => "Настройка системы",
            (Phase::Configure, En) => "Setting up the system",
            (Phase::Finish, Ru) => "Завершение установки",
            (Phase::Finish, En) => "Finishing the installation",
        }
    }

    /// Where the phase starts and ends in the overall progress (0..1000).
    fn span(self) -> (u32, u32) {
        match self {
            Phase::CopyFiles => (0, 700),
            Phase::Bootloader => (700, 800),
            Phase::Configure => (800, 950),
            Phase::Finish => (950, 1000),
        }
    }
}

/// Progress of [`install`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    /// Overall, 0 to 1000.
    pub permille: u32,
}

fn report(progress: &mut dyn FnMut(Progress), phase: Phase, part: u64, whole: u64) {
    let (a, b) = phase.span();
    let f = (part.min(whole) * 1000).checked_div(whole).unwrap_or(0);
    progress(Progress {
        phase,
        permille: a + ((b - a) as u64 * f / 1000) as u32,
    });
}

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

    /// Creates the directories leading to `path` (0755, root).
    fn parents(&mut self, path: &str) -> velora_vfs::Result<()> {
        let mut at = String::new();
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        for d in &parts[..parts.len().saturating_sub(1)] {
            at.push('/');
            at.push_str(d);
            self.mkdir(&at, 0o755, 0, 0)?;
        }
        Ok(())
    }

    fn file(&mut self, path: &str, data: &[u8], mode: u32, uid: u32) -> velora_vfs::Result<()> {
        self.parents(path)?;
        let (dir, name) = self.dir_of(path)?;
        let attr = NewAttr {
            mode,
            uid,
            gid: uid,
        };
        let ino = self.fs.create(dir, &name, Kind::File, attr, self.now)?;
        if !data.is_empty() {
            self.fs.write(ino, 0, data, self.now)?;
        }
        Ok(())
    }
}

fn guid(src: &mut dyn Source) -> Guid {
    let mut r = [0u8; 16];
    src.random(&mut r);
    Guid::from_random(r)
}

fn hash(src: &mut dyn Source, password: &str) -> String {
    let mut salt = [0u8; 16];
    src.random(&mut salt);
    velora_auth::hash_password(password, &salt, velora_auth::DEFAULT_ITERATIONS)
}

/// The text of /etc/passwd, /etc/group and the shadow lines' layout.
pub fn account_files(user: &str) -> (String, String) {
    let passwd = format!(
        "root:x:0:0:root:/root:/bin/vsh\n{u}:x:{id}:{id}:{u}:/home/{u}:/bin/vsh\n",
        u = user,
        id = USER_UID
    );
    let group = format!("root:x:0:\n{}:x:{}:\n", user, USER_UID);
    (passwd, group)
}

/// Installs the system onto `disk`, erasing it. Returns the disk.
pub fn install<D: BlockIo>(
    disk: D,
    plan: &Plan,
    src: &mut dyn Source,
    progress: &mut dyn FnMut(Progress),
) -> Result<D, String> {
    let now = src.now();
    let mut disk = disk;
    check(plan, disk.sector_count() * disk.sector_size() as u64)
        .map_err(|p| p.text(settings::Lang::En))?;
    report(progress, Phase::CopyFiles, 0, 1);
    let files = src.files()?;
    let kernel = src.kernel()?;
    let efi = src.bootloader()?;

    // Partitions.
    let esp_guid = guid(src);
    let root_guid = guid(src);
    let parts = partitions(&plan.layout, disk.sector_count(), esp_guid, root_guid)
        .map_err(|e| format!("partition layout: {}", e))?;
    let disk_guid = guid(src);
    velora_gpt::write(&mut disk, disk_guid, &parts).map_err(|e| format!("GPT: {}", e))?;
    let (esp, root) = (parts[0].clone(), parts[1].clone());

    // 1. The root file system and the files.
    let mut root_dev = Slice {
        dev: disk,
        start: root.first_lba,
        count: root.sectors(),
    };
    let opts = velorafs::FormatOptions {
        label: &plan.layout.root_label,
        now,
        uuid: root_guid.0,
    };
    velorafs::format(&mut root_dev, &opts).map_err(|e| format!("VeloraFS: {}", e))?;
    let mut vfs =
        velorafs::VeloraFs::mount(root_dev, false).map_err(|e| format!("VeloraFS: {}", e))?;
    {
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
        let total: u64 = files.iter().map(|f| f.data.len() as u64).sum();
        let mut done = 0u64;
        for f in &files {
            w.file(&f.path, &f.data, f.mode & 0o7777, 0)
                .map_err(|x| format!("{}: {}", f.path, x))?;
            done += f.data.len() as u64;
            report(progress, Phase::CopyFiles, done, total);
        }
    }
    vfs.sync().map_err(|x| format!("sync: {}", x))?;
    let disk = vfs.into_device().dev;

    // 2. The EFI system partition with the bootloader.
    report(progress, Phase::Bootloader, 0, 1);
    let mut esp_dev = Slice {
        dev: disk,
        start: esp.first_lba,
        count: esp.sectors(),
    };
    let mut volid = [0u8; 4];
    src.random(&mut volid);
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
        w.file("/EFI/BOOT/BOOTX64.EFI", &efi, 0o644, 0).map_err(e)?;
        report(progress, Phase::Bootloader, 1, 3);
        w.file("/boot/velora", &kernel, 0o644, 0).map_err(e)?;
        report(progress, Phase::Bootloader, 2, 3);
        w.file("/boot/limine.conf", conf.as_bytes(), 0o644, 0)
            .map_err(e)?;
    }
    vol.sync().map_err(|e| format!("EFI partition: {}", e))?;
    let disk = vol.into_device().dev;

    // 3. Users and settings.
    report(progress, Phase::Configure, 0, 3);
    let root_dev = Slice {
        dev: disk,
        start: root.first_lba,
        count: root.sectors(),
    };
    let mut vfs =
        velorafs::VeloraFs::mount(root_dev, false).map_err(|e| format!("VeloraFS: {}", e))?;
    {
        let mut w = Writer { fs: &mut vfs, now };
        let e = |e: FsError| format!("root partition: {}", e);
        let (passwd, group) = account_files(&plan.user);
        let root_hash = hash(src, &plan.root_password);
        report(progress, Phase::Configure, 1, 3);
        let user_hash = hash(src, &plan.user_password);
        report(progress, Phase::Configure, 2, 3);
        let shadow = format!(
            "root:{}:0:0:99999:7:::\n{}:{}:0:0:99999:7:::\n",
            root_hash, plan.user, user_hash
        );
        w.file("/etc/passwd", passwd.as_bytes(), 0o644, 0)
            .map_err(e)?;
        w.file("/etc/group", group.as_bytes(), 0o644, 0)
            .map_err(e)?;
        w.file("/etc/shadow", shadow.as_bytes(), 0o600, 0)
            .map_err(e)?;
        w.mkdir(&format!("/home/{}", plan.user), 0o700, USER_UID, USER_UID)
            .map_err(e)?;
        for (path, text) in plan.settings.files() {
            w.file(path, text.as_bytes(), 0o644, 0).map_err(e)?;
        }
    }

    // 4. Check and write everything out.
    report(progress, Phase::Finish, 0, 2);
    let problems = vfs.check();
    if !problems.is_clean() {
        return Err(format!(
            "the new file system has errors: {:?}",
            problems.errors
        ));
    }
    vfs.sync().map_err(|x| format!("sync: {}", x))?;
    report(progress, Phase::Finish, 2, 2);
    Ok(vfs.into_device().dev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use velora_vfs::MemDisk;

    struct Fake(u8);

    impl Source for Fake {
        fn files(&mut self) -> Result<Vec<SourceFile>, String> {
            Ok(alloc::vec![
                SourceFile {
                    path: String::from("/bin/vsh"),
                    data: alloc::vec![0x7f; 50_000],
                    mode: 0o755,
                },
                SourceFile {
                    path: String::from("/usr/share/fonts/Inter-Regular.ttf"),
                    data: alloc::vec![1; 3000],
                    mode: 0o644,
                },
                SourceFile {
                    path: String::from("/etc/motd"),
                    data: b"hello\n".to_vec(),
                    mode: 0o644,
                },
            ])
        }
        fn kernel(&mut self) -> Result<Vec<u8>, String> {
            Ok(alloc::vec![0xaa; 100_000])
        }
        fn bootloader(&mut self) -> Result<Vec<u8>, String> {
            Ok(alloc::vec![0xbb; 20_000])
        }
        fn random(&mut self, buf: &mut [u8]) {
            for b in buf {
                self.0 = self.0.wrapping_mul(13).wrapping_add(7);
                *b = self.0;
            }
        }
        fn now(&self) -> u64 {
            1_800_000_000
        }
    }

    fn plan() -> Plan {
        Plan {
            user: String::from("anna"),
            user_password: String::from("secret"),
            root_password: String::from("rootpw"),
            ..Plan::default()
        }
    }

    fn read(fs: &mut dyn FileSystem, path: &str) -> Vec<u8> {
        let mut ino = fs.root();
        for p in path.split('/').filter(|p| !p.is_empty()) {
            ino = fs.lookup(ino, p).unwrap_or_else(|e| panic!("{}: {:?}", path, e));
        }
        let size = fs.stat(ino).unwrap().size as usize;
        let mut buf = alloc::vec![0; size];
        let n = fs.read(ino, 0, &mut buf).unwrap();
        buf.truncate(n);
        buf
    }

    #[test]
    fn installs_onto_a_disk() {
        let disk = MemDisk::new(512, 256 * 2048);
        let mut steps = Vec::new();
        let mut p = plan();
        p.settings.theme = 1;
        let disk = install(disk, &p, &mut Fake(1), &mut |pr| steps.push(pr)).unwrap();
        // Progress goes through the phases in order and only forwards.
        assert!(steps.windows(2).all(|w| w[0].permille <= w[1].permille));
        assert!(steps.windows(2).all(|w| w[0].phase <= w[1].phase));
        for ph in Phase::ALL {
            assert!(steps.iter().any(|s| s.phase == ph));
        }
        assert_eq!(steps.last().unwrap().permille, 1000);
        // The partitions and both file systems.
        let mut disk = disk;
        let parts = velora_gpt::read(&mut disk).unwrap().unwrap();
        assert_eq!(parts.len(), 2);
        let root = parts[1].clone();
        let esp = parts[0].clone();
        let mut vfs = velorafs::VeloraFs::mount(
            Slice {
                dev: disk,
                start: root.first_lba,
                count: root.sectors(),
            },
            true,
        )
        .unwrap();
        assert_eq!(read(&mut vfs, "/bin/vsh").len(), 50_000);
        assert_eq!(read(&mut vfs, "/etc/motd"), b"hello\n");
        assert_eq!(read(&mut vfs, "/usr/share/fonts/Inter-Regular.ttf").len(), 3000);
        let passwd = String::from_utf8(read(&mut vfs, "/etc/passwd")).unwrap();
        assert!(passwd.contains("anna:x:1000:1000:anna:/home/anna:/bin/vsh"));
        let shadow = String::from_utf8(read(&mut vfs, "/etc/shadow")).unwrap();
        let user_hash = shadow.lines().nth(1).unwrap().split(':').nth(1).unwrap();
        assert!(velora_auth::verify_password("secret", user_hash));
        let desktop = String::from_utf8(read(&mut vfs, "/etc/velora/desktop.conf")).unwrap();
        assert!(desktop.contains("theme=velora-light"));
        assert!(vfs.check().is_clean());
        let disk = vfs.into_device().dev;
        let mut fat = velora_fat::FatVolume::mount(
            Slice {
                dev: disk,
                start: esp.first_lba,
                count: esp.sectors(),
            },
            true,
        )
        .unwrap();
        assert_eq!(read(&mut fat, "/boot/velora").len(), 100_000);
        let conf = String::from_utf8(read(&mut fat, "/boot/limine.conf")).unwrap();
        assert!(conf.contains(&format!("velora.root=PARTUUID={}", root.unique_guid)));
    }

    #[test]
    fn manual_layout_and_checks() {
        let mut p = plan();
        p.layout = Layout {
            esp_mib: 128,
            root_mib: Some(100),
            root_label: String::from("main"),
        };
        let disk = MemDisk::new(512, 512 * 2048);
        let mut disk = install(disk, &p, &mut Fake(9), &mut |_| {}).unwrap();
        let parts = velora_gpt::read(&mut disk).unwrap().unwrap();
        assert_eq!(parts[0].sectors(), 128 * 2048);
        assert_eq!(parts[1].sectors(), 100 * 2048);
        let big = 1024 * MIB;
        assert_eq!(check(&p, 64 * MIB), Err(Problem::DiskTooSmall { min_mib: 128 }));
        p.layout.root_mib = Some(2000);
        assert_eq!(check(&p, big), Err(Problem::PartitionsTooLarge));
        p.layout.root_mib = None;
        p.layout.root_label = String::from("bad label!");
        assert_eq!(check(&p, big), Err(Problem::BadLabel));
        p.layout.root_label = String::from("ok");
        p.user = String::from("Root");
        assert_eq!(check(&p, big), Err(Problem::BadUserName));
        p.user = String::from("bob");
        p.root_password.clear();
        assert_eq!(check(&p, big), Err(Problem::EmptyPassword));
        assert!(valid_user_name("_x-1") && !valid_user_name("root") && !valid_user_name("1a"));
    }
}

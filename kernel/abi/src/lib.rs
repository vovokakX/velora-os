//! The Velora native ABI, shared by the kernel and user space (vlibc).
//!
//! Calling convention: `syscall` with the number in `rax`, arguments in
//! `rdi, rsi, rdx, r10, r8, r9`. The result comes back in `rax`: a value
//! `>= 0` on success or `-errno` on failure. `rcx` and `r11` are clobbered.
//! See docs/syscalls.md.

#![cfg_attr(not(test), no_std)]

/// System call numbers.
pub mod nr {
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const YIELD: u64 = 3;
    pub const GETPID: u64 = 4;
    /// spawn(path, path_len, argv: *const StrRef, argc, envp: *const StrRef, envc) -> pid
    pub const SPAWN: u64 = 5;
    /// wait(pid, status: *mut i64) -> pid
    pub const WAIT: u64 = 6;
    /// mmap(addr, len, prot, flags) -> addr
    pub const MMAP: u64 = 7;
    /// munmap(addr, len) -> 0
    pub const MUNMAP: u64 = 8;
    /// open(path, path_len, flags, mode) -> fd
    pub const OPEN: u64 = 9;
    /// close(fd) -> 0
    pub const CLOSE: u64 = 10;
    /// seek(fd, offset: i64, whence) -> new offset
    pub const SEEK: u64 = 11;
    /// stat(path, path_len, out: *mut Stat) -> 0
    pub const STAT: u64 = 12;
    /// readdir(fd, buf, len) -> bytes written (packed `DirentHeader` + name)
    pub const READDIR: u64 = 13;
    /// mkdir(path, path_len, mode) -> 0
    pub const MKDIR: u64 = 14;
    /// unlink(path, path_len) -> 0 (files and empty directories)
    pub const UNLINK: u64 = 15;
    /// rename(old, old_len, new, new_len) -> 0
    pub const RENAME: u64 = 16;
    /// pipe(fds: *mut [u64; 2]) -> 0; fds[0] reads, fds[1] writes
    pub const PIPE: u64 = 17;
    /// dup2(old, new) -> new
    pub const DUP2: u64 = 18;
    /// chdir(path, path_len) -> 0
    pub const CHDIR: u64 = 19;
    /// kill(pid, status) -> 0: ends the process with `status`
    pub const KILL: u64 = 20;
    pub const CLOCK_GETTIME: u64 = 21;
    /// sysinfo(out: *mut SysInfo) -> 0
    pub const SYSINFO: u64 = 22;
    /// reboot(cmd) (root only): `reboot::RESTART` or `reboot::POWER_OFF`
    pub const REBOOT: u64 = 23;
    /// setuid(uid, gid) -> 0 (root only): sets user and group
    pub const SETUID: u64 = 24;
    pub const SLEEP_MS: u64 = 25;
    /// fstat(fd, out: *mut Stat) -> 0
    pub const FSTAT: u64 = 26;
    /// sync() -> 0: writes all cached file system data to the disks
    pub const SYNC: u64 = 27;
    /// ftruncate(fd, size) -> 0
    pub const FTRUNCATE: u64 = 28;
    /// statfs(path, path_len, out: *mut FsStat) -> 0
    pub const STATFS: u64 = 29;
    // 30: mount (not implemented).
    /// umount(path, path_len) -> 0 (root only): unmounts a disk mounted
    /// at boot under /mnt. `EBUSY` while files on it are open.
    pub const UMOUNT: u64 = 31;
    /// getcwd(buf, len) -> length of the path (no NUL)
    pub const GETCWD: u64 = 32;
    /// getuid() -> uid | gid << 32
    pub const GETUID: u64 = 33;
    /// proclist(buf, len) -> bytes written (`ProcEntry` records)
    pub const PROCLIST: u64 = 34;
    /// chmod(path, path_len, mode) -> 0
    pub const CHMOD: u64 = 35;
    /// chown(path, path_len, uid, gid) -> 0 (root only)
    pub const CHOWN: u64 = 36;
    /// dup(fd) -> new fd (lowest free)
    pub const DUP: u64 = 37;
    /// tty(op, arg): console terminal control, see [`super::tty`]
    pub const TTY: u64 = 38;
    /// getrandom(buf, len) -> bytes written (at most 256 per call)
    pub const GETRANDOM: u64 = 39;
    /// socket(kind) -> fd; kind: `sock::UDP` or `sock::TCP`
    pub const SOCKET: u64 = 40;
    /// bind(fd, port) -> 0
    pub const BIND: u64 = 41;
    /// connect(fd, ip, port) -> 0 (TCP: waits for the handshake)
    pub const CONNECT: u64 = 42;
    /// listen(fd) -> 0 (TCP, after bind)
    pub const LISTEN: u64 = 43;
    /// accept(fd) -> new fd (waits for a connection)
    pub const ACCEPT: u64 = 44;
    /// sendto(fd, buf, len, ip, port) -> bytes (ip 0: the connected peer)
    pub const SENDTO: u64 = 45;
    /// recvfrom(fd, buf, len, from: *mut SockAddr) -> bytes (0: peer closed)
    pub const RECVFROM: u64 = 46;
    /// netinfo(out: *mut NetInfo) -> 0
    pub const NETINFO: u64 = 47;
    /// ping(ip, seq, timeout_ms) -> round trip in microseconds
    pub const PING: u64 = 48;
    /// shutdown(fd) -> 0: ends the sending side of a TCP connection
    pub const SHUTDOWN: u64 = 49;
    /// display(op, arg): the screen for graphical programs (see `display`)
    pub const DISPLAY: u64 = 50;
    /// diskinfo(path, path_len, out: *mut DiskInfo) -> 0
    pub const DISKINFO: u64 = 51;
    /// input_wait(timeout_ms) -> 1 when a key or pointer event is waiting
    /// (on `/dev/keyboard`, `/dev/pointer` or `/dev/mouse`), 0 after the
    /// timeout. Returns at once if one is already waiting.
    pub const INPUT_WAIT: u64 = 52;
}

/// Error numbers (returned negated).
pub mod errno {
    pub const EPERM: i64 = 1;
    pub const ENOENT: i64 = 2;
    pub const ESRCH: i64 = 3;
    pub const EINTR: i64 = 4;
    pub const EIO: i64 = 5;
    pub const E2BIG: i64 = 7;
    pub const ENOEXEC: i64 = 8;
    pub const EBADF: i64 = 9;
    pub const ECHILD: i64 = 10;
    pub const EAGAIN: i64 = 11;
    pub const ENOMEM: i64 = 12;
    pub const EFAULT: i64 = 14;
    pub const EACCES: i64 = 13;
    pub const EBUSY: i64 = 16;
    pub const EEXIST: i64 = 17;
    pub const EXDEV: i64 = 18;
    pub const ENOTDIR: i64 = 20;
    pub const EISDIR: i64 = 21;
    pub const EINVAL: i64 = 22;
    pub const EMFILE: i64 = 24;
    pub const EFBIG: i64 = 27;
    pub const ENODEV: i64 = 19;
    pub const ENOSPC: i64 = 28;
    pub const ESPIPE: i64 = 29;
    pub const EROFS: i64 = 30;
    pub const ENAMETOOLONG: i64 = 36;
    pub const ENOSYS: i64 = 38;
    pub const EPIPE: i64 = 32;
    pub const ENOTEMPTY: i64 = 39;
    pub const ENOTSOCK: i64 = 88;
    pub const EDESTADDRREQ: i64 = 89;
    pub const EADDRINUSE: i64 = 98;
    pub const ENETDOWN: i64 = 100;
    pub const ENETUNREACH: i64 = 101;
    pub const ECONNRESET: i64 = 104;
    pub const EISCONN: i64 = 106;
    pub const ENOTCONN: i64 = 107;
    pub const ETIMEDOUT: i64 = 110;
    pub const ECONNREFUSED: i64 = 111;
    /// The file system found inconsistent metadata (Linux: EUCLEAN).
    pub const EUCLEAN: i64 = 117;
}

/// `open` flags. The access mode is the low two bits.
pub mod open {
    pub const READ: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ_WRITE: u64 = 2;
    pub const ACCESS_MASK: u64 = 3;
    /// Create the file if it does not exist.
    pub const CREATE: u64 = 0o100;
    /// With CREATE: fail with EEXIST if the file exists.
    pub const EXCLUSIVE: u64 = 0o200;
    /// Cut the file to length 0.
    pub const TRUNCATE: u64 = 0o1000;
    /// Every write goes to the end of the file.
    pub const APPEND: u64 = 0o2000;
    /// Fail with ENOTDIR unless the path is a directory.
    pub const DIRECTORY: u64 = 0o200000;
}

/// `seek` origins.
pub mod whence {
    pub const SET: u64 = 0;
    pub const CURRENT: u64 = 1;
    pub const END: u64 = 2;
}

/// File type bits of `Stat::mode` (POSIX values).
pub mod mode {
    pub const TYPE_MASK: u32 = 0o170000;
    pub const DIR: u32 = 0o040000;
    pub const FILE: u32 = 0o100000;
    pub const CHAR_DEVICE: u32 = 0o020000;
    pub const BLOCK_DEVICE: u32 = 0o060000;
    pub const FIFO: u32 = 0o010000;
    pub const SOCKET: u32 = 0o140000;
    /// Permission bits (rwx for owner, group, others) plus setuid/setgid/sticky.
    pub const PERM_MASK: u32 = 0o7777;
}

/// File information written by `stat` and `fstat`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    /// Device (mount) number and inode number: together they identify a file.
    pub dev: u64,
    pub ino: u64,
    /// Type and permission bits, see [`mode`].
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    /// Space used on disk, in bytes.
    pub allocated: u64,
    /// Seconds since 1970-01-01 UTC.
    pub mtime: u64,
    pub ctime: u64,
}

/// File system information written by `statfs`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FsStat {
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub inodes: u64,
    pub free_inodes: u64,
    /// File system type, NUL-padded ASCII ("velorafs", "fat32", ...).
    pub fs_type: [u8; 16],
    pub read_only: u64,
}

/// One `readdir` record: the header, then `name_len` bytes of UTF-8 name,
/// then padding to a multiple of 8 (`rec_len` covers all of it).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirentHeader {
    pub ino: u64,
    pub rec_len: u16,
    pub name_len: u16,
    /// File type bits (`mode::DIR`, `mode::FILE`, ...) shifted right by 12.
    pub kind: u8,
    pub _pad: [u8; 3],
}

/// `reboot` commands.
pub mod reboot {
    pub const RESTART: u64 = 0;
    pub const POWER_OFF: u64 = 1;
}

/// `tty` operations on the console.
pub mod tty {
    /// Set the mode bits (`RAW`, `NO_ECHO`); returns the old ones.
    pub const SET_MODE: u64 = 0;
    /// The process that Ctrl+C ends in line mode (0 = none).
    pub const SET_FOREGROUND: u64 = 1;
    /// Returns columns << 32 | rows.
    pub const SIZE: u64 = 2;
    /// Returns the character cell size in pixels, width << 32 | height
    /// (`ENODEV` without a screen: the console is the serial port only).
    pub const CELL_SIZE: u64 = 3;
    /// Mode bit: deliver every key at once, without echo or line editing.
    /// Special keys arrive as ANSI sequences (arrows `ESC [ A`..`D`, Home
    /// `ESC [ H`, End `ESC [ F`, Delete `ESC [ 3 ~`), Backspace as 0x7f.
    pub const RAW: u64 = 1;
    /// Mode bit (line mode): do not echo typed characters (passwords).
    pub const NO_ECHO: u64 = 2;
}

/// `display` operations. One process at a time owns the screen; while it
/// does, the text console stops drawing (its text is kept and shown again
/// when the owner releases the screen or exits).
pub mod display {
    /// Writes `DisplayInfo` to `arg`; `ENODEV` without a frame buffer.
    pub const INFO: u64 = 0;
    /// Maps the frame buffer into the caller and takes the screen; returns
    /// the address. `EBUSY` if another process owns it.
    pub const MAP: u64 = 1;
    /// Unmaps the frame buffer and gives the screen back to the console.
    pub const RELEASE: u64 = 2;
}

/// The frame buffer, as `display(INFO)` writes it. Pixels are 32 bits;
/// a colour channel is at bits `shift .. shift + 8`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub pitch: u32,
    pub bits_per_pixel: u32,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
    pub _pad: [u8; 5],
}

/// What `diskinfo` writes about a block device.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskInfo {
    pub size_bytes: u64,
    pub sector_size: u32,
    /// `disk_kind` value.
    pub kind: u32,
    /// Model name in UTF-8, NUL-padded.
    pub model: [u8; 48],
}

pub mod disk_kind {
    pub const OTHER: u32 = 0;
    pub const VIRTIO: u32 = 1;
    pub const SATA: u32 = 2;
    pub const NVME: u32 = 3;
    pub const USB: u32 = 4;
    pub const PARTITION: u32 = 5;
    /// Flag added to the kind of the disk the system was started from
    /// (and of its partitions).
    pub const BOOT: u32 = 0x100;
    /// The kind without flags.
    pub const MASK: u32 = 0xff;
}

/// `/dev/keyboard`: key presses and releases as 8-byte packets, for
/// graphical programs. Reads do not wait: 0 bytes means no event. Only
/// keys read there are taken from the console.
pub mod key {
    /// Packet: `code: u16`, `flags: u8`, reserved `u8`, `ch: u32` (the
    /// character the press typed, 0 for none), little-endian.
    pub const PACKET: usize = 8;
    pub const PRESSED: u8 = 1;
    pub const SHIFT: u8 = 2;
    pub const CTRL: u8 = 4;
    pub const ALT: u8 = 8;
    /// The Super (Windows) key is held.
    pub const SUPER: u8 = 16;

    pub const OTHER: u16 = 0;
    pub const ESCAPE: u16 = 1;
    pub const TAB: u16 = 2;
    pub const ENTER: u16 = 3;
    pub const BACKSPACE: u16 = 4;
    pub const DELETE: u16 = 5;
    pub const LEFT: u16 = 6;
    pub const RIGHT: u16 = 7;
    pub const UP: u16 = 8;
    pub const DOWN: u16 = 9;
    pub const HOME: u16 = 10;
    pub const END: u16 = 11;
    pub const PAGE_UP: u16 = 12;
    pub const PAGE_DOWN: u16 = 13;
    pub const SPACE: u16 = 14;
    /// A key that types a character (see `ch`). With Ctrl held, a letter
    /// key comes as `CHAR` with the letter in `ch` (lower case), not as
    /// the control character it types on the console.
    pub const CHAR: u16 = 15;
    /// F1 ... F12 are `F1 + n - 1`.
    pub const F1: u16 = 16;
    pub const F12: u16 = 27;
    /// The modifier keys themselves (presses and releases; the flags
    /// give the state after the event).
    pub const SHIFT_KEY: u16 = 28;
    pub const CTRL_KEY: u16 = 29;
    pub const ALT_KEY: u16 = 30;
    pub const SUPER_KEY: u16 = 31;
    pub const INSERT: u16 = 32;
}

/// `/dev/pointer`: every mouse and tablet event as an 8-byte packet;
/// reads do not wait (0 bytes: no event). `/dev/mouse` gives only the
/// relative movements, in its older 4-byte form.
pub mod pointer {
    /// Packet: `flags: u8`, `buttons: u8` (bit 0 left, 1 right, 2
    /// middle), `wheel: i8` (positive away from the user), reserved `u8`,
    /// then `x: i16` and `y: i16`, little-endian: a movement, or with
    /// [`ABSOLUTE`] a position from 0 to [`ABS_MAX`] across the screen.
    pub const PACKET: usize = 8;
    pub const ABSOLUTE: u8 = 1;
    pub const ABS_MAX: i32 = 32767;
}

/// System information written by `sysinfo`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SysInfo {
    pub uptime_ns: u64,
    /// Memory managed by the kernel and in use, in bytes.
    pub mem_total: u64,
    pub mem_used: u64,
    pub processes: u64,
    /// Milliseconds from kernel start to the end of boot.
    pub boot_ms: u64,
    pub cpus: u64,
    /// NUL-padded ASCII.
    pub version: [u8; 16],
    pub cpu: [u8; 48],
}

impl Default for SysInfo {
    fn default() -> Self {
        Self {
            uptime_ns: 0,
            mem_total: 0,
            mem_used: 0,
            processes: 0,
            boot_ms: 0,
            cpus: 0,
            version: [0; 16],
            cpu: [0; 48],
        }
    }
}

/// One process in the `proclist` output.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u64,
    pub parent: u64,
    pub uid: u32,
    /// 0 = running, 1 = exited (not yet collected).
    pub state: u32,
    pub threads: u32,
    pub _pad: u32,
    /// User memory in bytes.
    pub memory: u64,
    /// NUL-padded program name.
    pub name: [u8; 32],
}

/// Socket kinds for `socket`.
pub mod sock {
    pub const UDP: u64 = 1;
    pub const TCP: u64 = 2;
}

/// An IPv4 address and port (`ip` as a.b.c.d = a << 24 | ...).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SockAddr {
    pub ip: u32,
    pub port: u16,
    pub _pad: u16,
}

/// Network interface information written by `netinfo`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetInfo {
    /// Bit 0: a network card exists; bit 1: an address is configured.
    pub flags: u32,
    pub ip: u32,
    pub netmask: u32,
    pub gateway: u32,
    pub dns: u32,
    pub mac: [u8; 6],
    pub _pad: [u8; 2],
    pub rx_frames: u64,
    pub tx_frames: u64,
}

/// Most file descriptors one process can have open.
pub const MAX_FDS: usize = 64;
/// Longest path accepted by the file system calls.
pub const MAX_PATH: usize = 4096;
/// Longest file name (one path component), in bytes.
pub const MAX_NAME: usize = 255;

/// Clock ids for `clock_gettime`.
pub mod clock {
    /// Wall-clock time: seconds since 1970-01-01 UTC.
    pub const REALTIME: u64 = 0;
    /// Time since boot, never goes backwards.
    pub const MONOTONIC: u64 = 1;
}

/// `mmap` protection bits. Writable and executable together is refused
/// (W^X); PROT_NONE mappings are not supported.
pub mod prot {
    pub const READ: u64 = 1;
    pub const WRITE: u64 = 2;
    pub const EXEC: u64 = 4;
}

/// `mmap` flags.
pub mod map {
    /// Map exactly at `addr`; fails with EEXIST if a page there is in use.
    pub const FIXED: u64 = 1;
}

/// A string passed to the kernel (`spawn` arguments and environment).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StrRef {
    pub ptr: u64,
    pub len: u64,
}

/// Limits for `spawn`.
pub const MAX_ARGS: usize = 256;
pub const MAX_ARG_BYTES: usize = 64 * 1024;

/// Auxiliary vector keys placed on the initial stack.
pub mod auxv {
    pub const NULL: u64 = 0;
    pub const PAGESZ: u64 = 6;
    pub const ENTRY: u64 = 9;
}

/// Standard file descriptors. Every process starts with all three open on
/// the console (0 reads the keyboard, 1 and 2 write to the screen).
pub const STDIN: u64 = 0;
pub const STDOUT: u64 = 1;
pub const STDERR: u64 = 2;

/// `struct timespec` as written by `clock_gettime`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timespec {
    pub seconds: u64,
    pub nanoseconds: u64,
}

/// Exit status of a process killed by a CPU exception: 128 + vector,
/// like shells report signals.
pub const fn fault_exit_status(vector: u8) -> i64 {
    128 + vector as i64
}

/// Lowest address user programs may use.
pub const USER_MIN: u64 = 0x0000_0000_0040_0000;
/// One past the highest user address. The last page below the canonical
/// hole stays unmapped so `sysret` never returns to a non-canonical RIP.
pub const USER_MAX: u64 = 0x0000_7fff_ffff_f000;

const _: () = assert!(USER_MAX.is_multiple_of(4096));
const _: () = assert!(USER_MIN < USER_MAX && USER_MAX < 0x0000_8000_0000_0000);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_constants() {
        assert_eq!(core::mem::size_of::<Timespec>(), 16);
        assert_eq!(fault_exit_status(14), 142);
        assert_eq!(core::mem::size_of::<Stat>(), 64);
        assert_eq!(core::mem::size_of::<DirentHeader>(), 16);
        assert_eq!(core::mem::size_of::<FsStat>(), 64);
        assert_eq!(core::mem::size_of::<SysInfo>(), 112);
        assert_eq!(core::mem::size_of::<ProcEntry>(), 72);
        assert_eq!(core::mem::size_of::<NetInfo>(), 48);
        assert_eq!(core::mem::size_of::<SockAddr>(), 8);
        assert_eq!(core::mem::size_of::<DisplayInfo>(), 24);
        assert_eq!(core::mem::size_of::<DiskInfo>(), 64);
    }

    #[test]
    fn syscall_numbers_are_unique() {
        let all = [
            nr::EXIT,
            nr::WRITE,
            nr::READ,
            nr::YIELD,
            nr::GETPID,
            nr::SPAWN,
            nr::WAIT,
            nr::MMAP,
            nr::MUNMAP,
            nr::OPEN,
            nr::CLOSE,
            nr::SEEK,
            nr::STAT,
            nr::READDIR,
            nr::MKDIR,
            nr::UNLINK,
            nr::RENAME,
            nr::CLOCK_GETTIME,
            nr::SLEEP_MS,
            nr::FSTAT,
            nr::SYNC,
            nr::FTRUNCATE,
            nr::STATFS,
            nr::PIPE,
            nr::DUP2,
            nr::CHDIR,
            nr::KILL,
            nr::SYSINFO,
            nr::REBOOT,
            nr::SETUID,
            nr::GETCWD,
            nr::GETUID,
            nr::PROCLIST,
            nr::CHMOD,
            nr::CHOWN,
            nr::DUP,
            nr::TTY,
            nr::GETRANDOM,
            nr::SOCKET,
            nr::BIND,
            nr::CONNECT,
            nr::LISTEN,
            nr::ACCEPT,
            nr::SENDTO,
            nr::RECVFROM,
            nr::NETINFO,
            nr::PING,
            nr::SHUTDOWN,
            nr::DISPLAY,
            nr::DISKINFO,
        ];
        for (i, a) in all.iter().enumerate() {
            assert!(!all[i + 1..].contains(a), "duplicate syscall number {a}");
        }
    }
}

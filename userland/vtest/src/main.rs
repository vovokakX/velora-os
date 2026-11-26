//! /bin/vtest: checks vlibc and the system calls from user space.
//!
//! `vtest` (or `vtest all`) runs every check and exits with the number of
//! failures. `vtest files DIR` runs only the file checks inside DIR (used
//! on disk file systems). The other modes are children started by the
//! checks: `child-exit N`, `child-args ...`, `child-env`, `child-segv`,
//! `child-hold PATH`, `child-echo TEXT`, `child-fds`, `child-user DIR`,
//! `child-wait`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use vlibc::abi::errno;
use vlibc::fs::{self, File, flags, whence};
use vlibc::mem::{self, map, prot};
use vlibc::{Errno, env, heap, io, println, process, time};

vlibc::main!(main);

const SELF: &str = "/bin/vtest";

fn main() -> i32 {
    let args = env::args();
    match args.get(1) {
        None | Some("all") => run_all(),
        Some("files") => {
            let dir = args.get(2).unwrap_or("/tmp");
            let mut r = Runner {
                passed: 0,
                failed: 0,
            };
            file_checks(&mut r, dir);
            println!(
                "vtest: files in {}: {} passed, {} failed",
                dir, r.passed, r.failed
            );
            r.failed as i32
        }
        Some("child-exit") => args.get(2).and_then(|s| s.parse().ok()).unwrap_or(255),
        Some("child-args") => args.len() as i32,
        Some("child-env") => match env::var("VELORA_TEST") {
            Some("yes") => 0,
            _ => 1,
        },
        Some("child-echo") => {
            println!("{}", args.get(2).unwrap_or(""));
            0
        }
        Some("child-fds") => (3..16)
            .filter(|&fd| {
                let mut st = fs::Stat::default();
                // SAFETY: `st` is writable and has the kernel's layout.
                unsafe {
                    vlibc::sys::syscall2(vlibc::abi::nr::FSTAT, fd, &mut st as *mut fs::Stat as u64)
                        == 0
                }
            })
            .count() as i32,
        Some("child-user") => child_user(args.get(2).unwrap_or("/tmp")),
        Some("child-wait") => loop {
            time::sleep_ms(1000);
        },
        Some("child-hold") => {
            // Exits with the file still open: the kernel must close it.
            let _f = args.get(2).map(File::open);
            process::exit(0)
        }
        Some("child-segv") => {
            // SAFETY: deliberately invalid; the kernel must end this process.
            unsafe { core::ptr::write_volatile(16 as *mut u64, 1) };
            0
        }
        Some(other) => {
            println!("vtest: unknown mode {}", other);
            2
        }
    }
}

struct Runner {
    passed: u32,
    failed: u32,
}

impl Runner {
    fn check(&mut self, name: &str, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.passed += 1;
                println!("vtest: ok   {}", name);
            }
            Err(e) => {
                self.failed += 1;
                println!("vtest: FAIL {}: {}", name, e);
            }
        }
    }
}

macro_rules! ensure {
    ($cond:expr, $($fmt:tt)*) => {
        if !$cond {
            return Err(format!($($fmt)*));
        }
    };
}

fn run_all() -> i32 {
    let mut r = Runner {
        passed: 0,
        failed: 0,
    };
    r.check("args", args());
    r.check("environment", environment());
    r.check("heap collections", heap_collections());
    r.check("heap reuse", heap_reuse());
    r.check("clock and sleep", clock_and_sleep());
    r.check("wall clock", wall_clock());
    r.check("mmap and munmap", mmap_munmap());
    r.check("mmap refuses W+X and bad input", mmap_rejections());
    r.check("write with a bad pointer", bad_pointer());
    r.check("spawn and wait: exit status", spawn_exit_status());
    r.check("spawn passes arguments", spawn_args());
    r.check("spawn passes the environment", spawn_env());
    r.check("child killed by a fault", child_fault());
    r.check("spawn of a missing program", spawn_missing());
    r.check("wait on a non-child", wait_non_child());
    file_checks(&mut r, "/tmp");
    r.check("read-only root and devices", root_and_devices());
    r.check("standard descriptors are the console", std_descriptors());
    r.check("pipes: data, end of input, EPIPE", pipe_basics());
    r.check("pipe to a child's standard output", pipe_to_child());
    r.check("dup and dup2", dup_and_dup2());
    r.check("working directory and relative paths", working_directory());
    r.check(
        "users: setuid, permissions, chmod, chown",
        users_and_permissions(),
    );
    r.check("kill a process", kill_a_process());
    r.check("process list and system information", system_information());
    r.check("random bytes", random_bytes());
    r.check("network: interface, UDP and TCP sockets", network());
    println!("vtest: {} passed, {} failed", r.passed, r.failed);
    r.failed as i32
}

fn args() -> Result<(), String> {
    let a = env::args();
    ensure!(a.get(0) == Some(SELF), "argv[0] = {:?}", a.get(0));
    ensure!(a.get(1) == Some("all"), "argv[1] = {:?}", a.get(1));
    ensure!(a.get(a.len()).is_none(), "argv not terminated");
    Ok(())
}

fn environment() -> Result<(), String> {
    ensure!(
        env::var("VELORA_TEST") == Some("yes"),
        "VELORA_TEST = {:?}",
        env::var("VELORA_TEST")
    );
    ensure!(env::var("MISSING").is_none(), "unexpected variable");
    Ok(())
}

fn heap_collections() -> Result<(), String> {
    let v: Vec<u64> = (0..10_000).collect();
    ensure!(v.iter().sum::<u64>() == 49_995_000, "wrong sum");
    let mut m = BTreeMap::new();
    for i in 0..1000u32 {
        m.insert(format!("key{}", i), i);
    }
    ensure!(m["key777"] == 777, "map lookup");
    let big = alloc::vec![0x5au8; 3 * 1024 * 1024];
    ensure!(big.iter().all(|&b| b == 0x5a), "large allocation");
    Ok(())
}

fn heap_reuse() -> Result<(), String> {
    let before = heap::stats().pages_mapped;
    for _ in 0..20 {
        let v = alloc::vec![1u8; 512 * 1024];
        core::hint::black_box(&v);
    }
    let grown = heap::stats().pages_mapped - before;
    ensure!(
        grown <= 128,
        "heap grew by {} pages for 20 equal allocations",
        grown
    );
    Ok(())
}

fn clock_and_sleep() -> Result<(), String> {
    let t0 = time::monotonic().map_err(|e| format!("{}", e))?;
    time::sleep_ms(30);
    let t1 = time::monotonic().map_err(|e| format!("{}", e))?;
    let ms = (t1 - t0).as_millis();
    ensure!(ms >= 30, "slept only {} ms", ms);
    Ok(())
}

fn wall_clock() -> Result<(), String> {
    let now = time::realtime().map_err(|e| format!("{}", e))?;
    ensure!(
        now.as_secs() > 1_700_000_000,
        "time {} s is before 2023",
        now.as_secs()
    );
    Ok(())
}

fn mmap_munmap() -> Result<(), String> {
    let len = 3 * 4096;
    let p = mem::mmap(0, len, prot::READ | prot::WRITE, 0).map_err(|e| format!("mmap: {}", e))?;
    // SAFETY: the mapping is `len` bytes, readable and writable.
    let s = unsafe { core::slice::from_raw_parts_mut(p, len) };
    ensure!(s.iter().all(|&b| b == 0), "mmap memory not zeroed");
    s.fill(0xab);
    ensure!(s[len - 1] == 0xab, "write lost");
    let again = mem::mmap(p as u64, 4096, prot::READ, map::FIXED);
    ensure!(
        again == Err(Errno(errno::EEXIST)),
        "fixed mmap over a mapping: {:?}",
        again
    );
    // SAFETY: nothing uses the mapping after this.
    unsafe { mem::munmap(p, len) }.map_err(|e| format!("munmap: {}", e))?;
    let q = mem::mmap(p as u64, 4096, prot::READ | prot::WRITE, map::FIXED)
        .map_err(|e| format!("remap: {}", e))?;
    // SAFETY: freshly mapped page.
    ensure!(unsafe { *q } == 0, "remapped page not zeroed");
    // SAFETY: nothing uses the mapping after this.
    unsafe { mem::munmap(q, 4096) }.map_err(|e| format!("munmap: {}", e))?;
    Ok(())
}

fn mmap_rejections() -> Result<(), String> {
    let wx = mem::mmap(0, 4096, prot::READ | prot::WRITE | prot::EXEC, 0);
    ensure!(wx == Err(Errno(errno::EINVAL)), "W+X mapping: {:?}", wx);
    let zero = mem::mmap(0, 0, prot::READ, 0);
    ensure!(
        zero == Err(Errno(errno::EINVAL)),
        "zero-length mapping: {:?}",
        zero
    );
    let kernel = mem::mmap(0xffff_8000_0000_0000, 4096, prot::READ, map::FIXED);
    ensure!(
        kernel == Err(Errno(errno::EINVAL)),
        "mapping in the kernel half: {:?}",
        kernel
    );
    let unaligned = mem::mmap(0x2000_0000_0001, 4096, prot::READ, map::FIXED);
    ensure!(
        unaligned == Err(Errno(errno::EINVAL)),
        "unaligned fixed mapping: {:?}",
        unaligned
    );
    Ok(())
}

fn bad_pointer() -> Result<(), String> {
    // SAFETY: the kernel checks the pointer and refuses it.
    let bad = unsafe { core::slice::from_raw_parts(0x10 as *const u8, 8) };
    let r = io::write(1, bad);
    ensure!(r == Err(Errno(errno::EFAULT)), "write returned {:?}", r);
    Ok(())
}

fn run_child(args: &[&str], env: &[&str]) -> Result<i64, String> {
    let pid = process::spawn(SELF, args, env).map_err(|e| format!("spawn: {}", e))?;
    ensure!(pid != process::getpid(), "child has the parent's pid");
    process::wait(pid).map_err(|e| format!("wait: {}", e))
}

fn spawn_exit_status() -> Result<(), String> {
    let status = run_child(&[SELF, "child-exit", "42"], &[])?;
    ensure!(status == 42, "status {}", status);
    Ok(())
}

fn spawn_args() -> Result<(), String> {
    let status = run_child(&[SELF, "child-args", "a", "б", "c d"], &[])?;
    ensure!(status == 5, "child saw {} arguments", status);
    Ok(())
}

fn spawn_env() -> Result<(), String> {
    let status = run_child(&[SELF, "child-env"], &["VELORA_TEST=yes"])?;
    ensure!(status == 0, "child did not see VELORA_TEST");
    let status = run_child(&[SELF, "child-env"], &[])?;
    ensure!(status == 1, "child saw a variable it was not given");
    Ok(())
}

fn child_fault() -> Result<(), String> {
    let status = run_child(&[SELF, "child-segv"], &[])?;
    ensure!(
        status == 142,
        "status {} (expected 142 = page fault)",
        status
    );
    Ok(())
}

fn spawn_missing() -> Result<(), String> {
    let r = process::spawn("/bin/does-not-exist", &["x"], &[]);
    ensure!(r == Err(Errno(errno::ENOENT)), "spawn returned {:?}", r);
    let r = process::spawn("/etc/motd", &["x"], &[]);
    ensure!(
        r == Err(Errno(errno::EACCES)),
        "spawning a file without execute permission returned {:?}",
        r
    );
    // Executable bits set, but not a program.
    let p = "/tmp/vtest-not-a-program";
    let mut f =
        File::open_with(p, flags::WRITE | flags::CREATE | flags::TRUNCATE, 0o755).map_err(e)?;
    f.write_all(b"echo hello\n").map_err(e)?;
    drop(f);
    let r = process::spawn(p, &["x"], &[]);
    fs::remove(p).map_err(e)?;
    ensure!(
        r == Err(Errno(errno::ENOEXEC)),
        "spawning a text file returned {:?}",
        r
    );
    Ok(())
}

fn wait_non_child() -> Result<(), String> {
    let r = process::wait(process::getpid());
    ensure!(
        r == Err(Errno(errno::ECHILD)),
        "wait on self returned {:?}",
        r
    );
    let r = process::wait(99_999);
    ensure!(
        r == Err(Errno(errno::ECHILD)),
        "wait on unknown pid returned {:?}",
        r
    );
    Ok(())
}

// --- files ----------------------------------------------------------------

fn e(x: Errno) -> String {
    format!("{}", x)
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u32 * 31 + seed as u32 + (i / 1000) as u32) as u8)
        .collect()
}

fn expect_err<T>(r: Result<T, Errno>, want: i64, what: &str) -> Result<(), String> {
    match r {
        Ok(_) => Err(format!("{}: succeeded, expected errno {}", what, want)),
        Err(Errno(got)) if got == want => Ok(()),
        Err(x) => Err(format!("{}: {} instead of errno {}", what, x, want)),
    }
}

fn file_checks(r: &mut Runner, dir: &str) {
    let base = format!("{}/vtest-{}", dir, process::getpid());
    if let Err(x) = fs::mkdir(&base, 0o755) {
        r.check(
            &format!("files in {}", dir),
            Err(format!("mkdir {}: {}", base, x)),
        );
        return;
    }
    r.check(&format!("{}: write and read back", dir), write_read(&base));
    r.check(
        &format!("{}: seek and partial reads", dir),
        seek_reads(&base),
    );
    r.check(
        &format!("{}: append and truncate", dir),
        append_truncate(&base),
    );
    r.check(&format!("{}: directories and listing", dir), listing(&base));
    r.check(&format!("{}: rename and remove", dir), rename_remove(&base));
    r.check(&format!("{}: error codes", dir), file_errors(&base));
    r.check(&format!("{}: descriptor limit", dir), fd_limit(&base));
    r.check(&format!("{}: clean up", dir), fs::remove(&base).map_err(e));
}

fn write_read(base: &str) -> Result<(), String> {
    let p = format!("{}/data.bin", base);
    let data = pattern(100_000, 1);
    let mut f = File::create(&p).map_err(e)?;
    f.write_all(&data).map_err(e)?;
    f.close().map_err(e)?;
    ensure!(fs::read(&p).map_err(e)? == data, "content differs");
    let st = fs::stat(&p).map_err(e)?;
    ensure!(st.size == 100_000, "size {}", st.size);
    ensure!(
        !fs::is_dir(&st) && st.mode & 0o777 == 0o644,
        "mode {:o}",
        st.mode
    );
    fs::sync().map_err(e)?;
    Ok(())
}

fn seek_reads(base: &str) -> Result<(), String> {
    let p = format!("{}/data.bin", base);
    let data = pattern(100_000, 1);
    let mut f = File::open(&p).map_err(e)?;
    ensure!(
        f.seek(50_000, whence::SET).map_err(e)? == 50_000,
        "seek SET"
    );
    let mut b = [0u8; 10];
    ensure!(f.read(&mut b).map_err(e)? == 10, "short read");
    ensure!(b[..] == data[50_000..50_010], "data after seek");
    ensure!(f.seek(-10, whence::END).map_err(e)? == 99_990, "seek END");
    ensure!(
        f.read(&mut b).map_err(e)? == 10 && b[..] == data[99_990..],
        "tail"
    );
    ensure!(f.read(&mut b).map_err(e)? == 0, "read past the end");
    ensure!(
        f.seek(-5, whence::CURRENT).map_err(e)? == 99_995,
        "seek CURRENT"
    );
    expect_err(
        f.seek(-1, whence::SET),
        errno::EINVAL,
        "seek before the start",
    )?;
    expect_err(
        f.write(b"x"),
        errno::EBADF,
        "write to a read-only descriptor",
    )?;
    Ok(())
}

fn append_truncate(base: &str) -> Result<(), String> {
    let p = format!("{}/log.txt", base);
    fs::write(&p, b"one\n").map_err(e)?;
    let mut f = File::open_with(&p, flags::WRITE | flags::APPEND, 0).map_err(e)?;
    f.write_all(b"two\n").map_err(e)?;
    f.seek(0, whence::SET).map_err(e)?;
    f.write_all(b"three\n").map_err(e)?;
    drop(f);
    ensure!(fs::read(&p).map_err(e)? == b"one\ntwo\nthree\n", "append");
    let mut f = File::open_with(&p, flags::READ_WRITE, 0).map_err(e)?;
    f.set_len(3).map_err(e)?;
    ensure!(f.stat().map_err(e)?.size == 3, "truncate");
    f.set_len(6).map_err(e)?;
    ensure!(
        f.read_to_end().map_err(e)? == b"one\0\0\0",
        "grown part is not zero"
    );
    Ok(())
}

fn listing(base: &str) -> Result<(), String> {
    let d = format!("{}/sub", base);
    fs::mkdir(&d, 0o750).map_err(e)?;
    for n in ["a", "b", "файл с пробелом.txt"] {
        fs::write(&format!("{}/{}", d, n), n.as_bytes()).map_err(e)?;
    }
    fs::mkdir(&format!("{}/inner", d), 0o755).map_err(e)?;
    let mut names: Vec<(String, bool)> = Vec::new();
    for ent in fs::read_dir(&d).map_err(e)? {
        let ent = ent.map_err(e)?;
        names.push((ent.name, ent.is_dir));
    }
    names.sort();
    let want = [
        ("a", false),
        ("b", false),
        ("inner", true),
        ("файл с пробелом.txt", false),
    ];
    ensure!(names.len() == want.len(), "listed {:?}", names);
    for ((n, d), (wn, wd)) in names.iter().zip(want) {
        ensure!(n == wn && *d == wd, "listed {:?}", names);
    }
    let st = fs::stat(&d).map_err(e)?;
    ensure!(fs::is_dir(&st), "not a directory");
    Ok(())
}

fn rename_remove(base: &str) -> Result<(), String> {
    let d = format!("{}/sub", base);
    fs::rename(&format!("{}/a", d), &format!("{}/inner/moved", d)).map_err(e)?;
    expect_err(fs::stat(&format!("{}/a", d)), errno::ENOENT, "old name")?;
    ensure!(
        fs::read(&format!("{}/inner/moved", d)).map_err(e)? == b"a",
        "moved content"
    );
    fs::rename(&format!("{}/b", d), &format!("{}/inner/moved", d)).map_err(e)?;
    ensure!(
        fs::read(&format!("{}/inner/moved", d)).map_err(e)? == b"b",
        "replacing rename"
    );
    expect_err(
        fs::remove(&d),
        errno::ENOTEMPTY,
        "remove a non-empty directory",
    )?;
    fs::remove(&format!("{}/inner/moved", d)).map_err(e)?;
    fs::remove(&format!("{}/inner", d)).map_err(e)?;
    fs::remove(&format!("{}/файл с пробелом.txt", d)).map_err(e)?;
    fs::remove(&d).map_err(e)?;
    expect_err(fs::remove(&d), errno::ENOENT, "remove twice")?;
    fs::remove(&format!("{}/data.bin", base)).map_err(e)?;
    fs::remove(&format!("{}/log.txt", base)).map_err(e)?;
    Ok(())
}

fn file_errors(base: &str) -> Result<(), String> {
    let p = format!("{}/x", base);
    expect_err(File::open(&p), errno::ENOENT, "open a missing file")?;
    fs::write(&p, b"x").map_err(e)?;
    expect_err(
        File::open_with(&p, flags::WRITE | flags::CREATE | flags::EXCLUSIVE, 0o644),
        errno::EEXIST,
        "exclusive create",
    )?;
    expect_err(
        File::open_with(base, flags::WRITE, 0),
        errno::EISDIR,
        "open a directory for writing",
    )?;
    expect_err(
        fs::mkdir(base, 0o755),
        errno::EEXIST,
        "mkdir of an existing directory",
    )?;
    expect_err(
        fs::stat(&format!("{}/y", p)),
        errno::ENOTDIR,
        "path through a file",
    )?;
    expect_err(fs::read_dir(&p), errno::ENOTDIR, "list a file")?;
    expect_err(
        fs::rename(&p, "/dev/x"),
        errno::EXDEV,
        "rename across file systems",
    )?;
    expect_err(
        io::read(999, &mut [0u8; 4]),
        errno::EBADF,
        "read from a bad descriptor",
    )?;
    fs::remove(&p).map_err(e)?;
    Ok(())
}

fn fd_limit(base: &str) -> Result<(), String> {
    let p = format!("{}/fd", base);
    fs::write(&p, b"").map_err(e)?;
    let mut open = Vec::new();
    let err = loop {
        match File::open(&p) {
            Ok(f) => open.push(f),
            Err(x) => break x,
        }
        if open.len() > 100 {
            return Err(String::from("no descriptor limit"));
        }
    };
    ensure!(err == Errno(errno::EMFILE), "error {}", err);
    ensure!(open.len() == 64 - 3, "{} files opened", open.len());
    drop(open);
    File::open(&p).map_err(e)?;
    fs::remove(&p).map_err(e)?;
    Ok(())
}

fn root_and_devices() -> Result<(), String> {
    let motd = fs::read("/etc/motd").map_err(e)?;
    ensure!(motd.starts_with(b"Velora OS"), "motd content");
    expect_err(
        fs::write("/etc/new", b"x"),
        errno::EROFS,
        "write to the initramfs",
    )?;
    expect_err(
        fs::remove("/bin/hello"),
        errno::EROFS,
        "remove from the initramfs",
    )?;
    let st = fs::stat("/bin/hello").map_err(e)?;
    ensure!(st.mode & 0o111 != 0, "/bin/hello not executable");
    let mut null = File::open_with("/dev/null", flags::READ_WRITE, 0).map_err(e)?;
    ensure!(null.write(b"gone").map_err(e)? == 4, "/dev/null write");
    ensure!(null.read(&mut [1u8; 8]).map_err(e)? == 0, "/dev/null read");
    let mut zero = File::open("/dev/zero").map_err(e)?;
    let mut b = [1u8; 16];
    ensure!(
        zero.read(&mut b).map_err(e)? == 16 && b == [0; 16],
        "/dev/zero"
    );
    let names: Vec<String> = fs::read_dir("/")
        .map_err(e)?
        .filter_map(|x| x.ok())
        .map(|x| x.name)
        .collect();
    for want in ["bin", "dev", "etc", "mnt", "tmp"] {
        ensure!(
            names.iter().any(|n| n == want),
            "/ lacks {} ({:?})",
            want,
            names
        );
    }
    let st = fs::statfs("/tmp").map_err(e)?;
    ensure!(st.fs_type.starts_with(b"memfs"), "/tmp type");
    Ok(())
}

fn std_descriptors() -> Result<(), String> {
    for fd in 0..3 {
        let mut st = fs::Stat::default();
        // SAFETY: `st` is writable and has the kernel's layout.
        let r = unsafe {
            vlibc::sys::syscall2(vlibc::abi::nr::FSTAT, fd, &mut st as *mut fs::Stat as u64)
        };
        ensure!(r == 0, "fstat({}) = {}", fd, r);
        ensure!(
            st.mode & fs::mode::TYPE_MASK == fs::mode::CHAR_DEVICE,
            "fd {} mode {:o}",
            fd,
            st.mode
        );
    }
    let console = File::open_with("/dev/console", flags::WRITE, 0).map_err(e)?;
    io::write_all(console.fd(), b"vtest: written through /dev/console\n").map_err(e)?;
    Ok(())
}

// --- stage 8: pipes, descriptors, directories, users, processes -----------

fn read_to_end(fd: u64) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        let n = io::read(fd, &mut buf).map_err(e)?;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

fn pipe_basics() -> Result<(), String> {
    let (r, w) = io::pipe().map_err(e)?;
    io::write_all(w, b"through the pipe").map_err(e)?;
    fs::close(w).map_err(e)?;
    ensure!(read_to_end(r)? == b"through the pipe", "pipe content");
    fs::close(r).map_err(e)?;
    // Writing when nobody reads fails with EPIPE.
    let (r, w) = io::pipe().map_err(e)?;
    fs::close(r).map_err(e)?;
    expect_err(
        io::write(w, b"x"),
        errno::EPIPE,
        "write to a pipe without readers",
    )?;
    fs::close(w).map_err(e)?;
    // A large write blocks until a reader... here it fits the 64 KiB buffer.
    let (r, w) = io::pipe().map_err(e)?;
    let big = pattern(60_000, 3);
    io::write_all(w, &big).map_err(e)?;
    fs::close(w).map_err(e)?;
    ensure!(read_to_end(r)? == big, "60 KB through a pipe");
    fs::close(r).map_err(e)?;
    Ok(())
}

/// Runs `vtest ARGS` with its stdout on a pipe and returns the output.
fn child_output(args: &[&str]) -> Result<(Vec<u8>, i64), String> {
    let (r, w) = io::pipe().map_err(e)?;
    let saved = io::dup(1).map_err(e)?;
    io::dup2(w, 1).map_err(e)?;
    let spawned = process::spawn(SELF, args, &[]);
    io::dup2(saved, 1).map_err(e)?;
    fs::close(saved).map_err(e)?;
    fs::close(w).map_err(e)?;
    let pid = spawned.map_err(e)?;
    let out = read_to_end(r)?;
    fs::close(r).map_err(e)?;
    let status = process::wait(pid).map_err(e)?;
    Ok((out, status))
}

fn pipe_to_child() -> Result<(), String> {
    let (out, status) = child_output(&[SELF, "child-echo", "hello parent"])?;
    ensure!(status == 0, "child status {}", status);
    ensure!(
        out == b"hello parent\n",
        "child wrote {:?}",
        String::from_utf8_lossy(&out)
    );
    // Only descriptors 0-2 are inherited.
    let extra = File::open("/etc/motd").map_err(e)?;
    let (_, status) = child_output(&[SELF, "child-fds"])?;
    drop(extra);
    ensure!(status == 0, "child saw {} extra descriptors", status);
    Ok(())
}

fn dup_and_dup2() -> Result<(), String> {
    let d = io::dup(1).map_err(e)?;
    ensure!(d > 2, "dup returned {}", d);
    io::write_all(d, b"vtest: written through a dup of stdout\n").map_err(e)?;
    ensure!(io::dup2(d, 20).map_err(e)? == 20, "dup2 result");
    io::write_all(20, b"vtest: written through descriptor 20\n").map_err(e)?;
    fs::close(20).map_err(e)?;
    fs::close(d).map_err(e)?;
    expect_err(io::dup(40), errno::EBADF, "dup of a closed descriptor")?;
    expect_err(io::dup2(1, 64), errno::EBADF, "dup2 past the limit")?;
    Ok(())
}

fn working_directory() -> Result<(), String> {
    let dir = format!("/tmp/vtest-cwd-{}", process::getpid());
    fs::mkdir(&dir, 0o755).map_err(e)?;
    fs::chdir(&dir).map_err(e)?;
    ensure!(fs::getcwd().map_err(e)? == dir, "getcwd after chdir");
    fs::write("rel.txt", b"relative").map_err(e)?;
    ensure!(
        fs::read(&format!("{}/rel.txt", dir)).map_err(e)? == b"relative",
        "relative write"
    );
    fs::chdir("..").map_err(e)?;
    ensure!(fs::getcwd().map_err(e)? == "/tmp", "chdir ..");
    expect_err(fs::chdir("/etc/motd"), errno::ENOTDIR, "chdir to a file")?;
    expect_err(
        fs::chdir("/nothing"),
        errno::ENOENT,
        "chdir to a missing directory",
    )?;
    // Relative program paths start at the working directory.
    fs::chdir("/bin").map_err(e)?;
    let pid = process::spawn("./hello", &["./hello"], &[]).map_err(e)?;
    ensure!(process::wait(pid).map_err(e)? == 0, "./hello status");
    fs::chdir("/").map_err(e)?;
    fs::remove(&format!("{}/rel.txt", dir)).map_err(e)?;
    fs::remove(&dir).map_err(e)?;
    Ok(())
}

fn users_and_permissions() -> Result<(), String> {
    ensure!(
        process::getuid() == (0, 0),
        "tests run as root, got {:?}",
        process::getuid()
    );
    let dir = format!("/tmp/vtest-users-{}", process::getpid());
    fs::mkdir(&dir, 0o777).map_err(e)?;
    let (out, status) = child_output(&[SELF, "child-user", &dir])?;
    ensure!(
        status == 0,
        "child-user failed: {}",
        String::from_utf8_lossy(&out)
    );
    let st = fs::stat(&format!("{}/owned", dir)).map_err(e)?;
    ensure!(
        (st.uid, st.gid) == (1000, 1000),
        "owner {}:{}",
        st.uid,
        st.gid
    );
    // Root may change mode and owner.
    let f = format!("{}/owned", dir);
    fs::chmod(&f, 0o600).map_err(e)?;
    fs::chown(&f, 0, 0).map_err(e)?;
    let st = fs::stat(&f).map_err(e)?;
    ensure!(st.mode & 0o777 == 0o600 && st.uid == 0, "chmod/chown");
    fs::remove(&f).map_err(e)?;
    fs::remove(&dir).map_err(e)?;
    Ok(())
}

/// Child: becomes user 1000 and checks what it may no longer do.
fn child_user(dir: &str) -> i32 {
    let r = (|| -> Result<(), String> {
        process::setuid(1000, 1000).map_err(e)?;
        ensure!(process::getuid() == (1000, 1000), "getuid after setuid");
        expect_err(process::setuid(0, 0), errno::EPERM, "setuid back to root")?;
        expect_err(File::open("/etc/shadow"), errno::EACCES, "read /etc/shadow")?;
        expect_err(
            fs::chown("/tmp", 1000, 1000),
            errno::EPERM,
            "chown as a user",
        )?;
        let e2 = process::reboot(vlibc::abi::reboot::RESTART);
        ensure!(e2 == Errno(errno::EPERM), "reboot as a user: {}", e2);
        fs::write(&format!("{}/owned", dir), b"mine").map_err(e)?;
        expect_err(
            fs::chmod("/tmp", 0o777),
            errno::EPERM,
            "chmod of root's directory",
        )?;
        Ok(())
    })();
    match r {
        Ok(()) => 0,
        Err(msg) => {
            println!("{}", msg);
            1
        }
    }
}

fn kill_a_process() -> Result<(), String> {
    let pid = process::spawn(SELF, &[SELF, "child-wait"], &[]).map_err(e)?;
    time::sleep_ms(20);
    ensure!(
        process::list()
            .map_err(e)?
            .iter()
            .any(|p| p.pid == pid && p.state == 0),
        "child not listed as running"
    );
    process::kill(pid, 77).map_err(e)?;
    ensure!(process::wait(pid).map_err(e)? == 77, "status after kill");
    expect_err(
        process::kill(999_999, 1),
        errno::ESRCH,
        "kill of a missing process",
    )?;
    Ok(())
}

fn system_information() -> Result<(), String> {
    let me = process::getpid();
    let list = process::list().map_err(e)?;
    let mine = list
        .iter()
        .find(|p| p.pid == me)
        .ok_or("own process not listed")?;
    ensure!(
        process::field_str(&mine.name) == "vtest",
        "own name {:?}",
        process::field_str(&mine.name)
    );
    ensure!(mine.memory > 0 && mine.threads == 1, "own entry {:?}", mine);
    let s = process::sysinfo().map_err(e)?;
    ensure!(
        s.mem_total > s.mem_used && s.mem_used > 0,
        "memory {} / {}",
        s.mem_used,
        s.mem_total
    );
    ensure!(process::field_str(&s.version).starts_with("0."), "version");
    ensure!(s.processes >= 1 && s.cpus >= 1 && s.uptime_ns > 0, "counts");
    let (cols, rows) = vlibc::term::size();
    ensure!(cols >= 40 && rows >= 10, "console size {}x{}", cols, rows);
    ensure!(vlibc::term::is_console(1), "stdout is not the console");
    Ok(())
}

fn random_bytes() -> Result<(), String> {
    let mut a = [0u8; 300];
    let mut b = [0u8; 300];
    process::random(&mut a);
    process::random(&mut b);
    ensure!(a != b, "two calls gave the same bytes");
    ensure!(
        a.iter().filter(|&&x| x == 0).count() < 30,
        "too many zero bytes"
    );
    Ok(())
}

/// Sockets. With an address (DHCP on QEMU's user network) the gateway is
/// pinged and ports are bound; without one the error path is checked.
fn network() -> Result<(), String> {
    use vlibc::net::{self, Ipv4, TcpListener, TcpStream, UdpSocket};
    let info = net::netinfo().map_err(e)?;
    ensure!(info.flags & 1 != 0, "no network card");
    if info.flags & 2 == 0 {
        // Without an address only the error paths can be checked.
        expect_err(
            TcpStream::connect(Ipv4::new(10, 0, 2, 2), 80),
            errno::ENETUNREACH,
            "connect without an address",
        )?;
        return Ok(());
    }
    ensure!(
        Ipv4(info.ip) == Ipv4::new(10, 0, 2, 15),
        "address {}",
        Ipv4(info.ip)
    );
    net::ping(Ipv4(info.gateway), 9, 3000).map_err(e)?;
    let u = UdpSocket::bind(4000).map_err(e)?;
    expect_err(
        UdpSocket::bind(4000),
        errno::EADDRINUSE,
        "second bind of a UDP port",
    )?;
    // A datagram to QEMU's DNS server address is sent (no answer expected).
    ensure!(
        u.send_to(b"x", Ipv4(info.dns), 9).map_err(e)? == 1,
        "UDP send"
    );
    let l = TcpListener::bind(7070).map_err(e)?;
    expect_err(
        TcpListener::bind(7070),
        errno::EADDRINUSE,
        "second listen on a port",
    )?;
    drop(l);
    // The port is free again once the listener is closed.
    drop(TcpListener::bind(7070).map_err(e)?);
    Ok(())
}

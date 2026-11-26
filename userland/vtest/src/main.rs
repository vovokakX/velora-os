//! /bin/vtest: checks vlibc and the system calls from user space.
//!
//! `vtest` (or `vtest all`) runs every check and exits with the number of
//! failures. The other modes are children started by the checks:
//! `child-exit N`, `child-args ...`, `child-env`, `child-segv`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use vlibc::abi::errno;
use vlibc::mem::{self, map, prot};
use vlibc::{Errno, env, heap, io, println, process, time};

vlibc::main!(main);

const SELF: &str = "/bin/vtest";

fn main() -> i32 {
    let args = env::args();
    match args.get(1) {
        None | Some("all") => run_all(),
        Some("child-exit") => args.get(2).and_then(|s| s.parse().ok()).unwrap_or(255),
        Some("child-args") => args.len() as i32,
        Some("child-env") => match env::var("VELORA_TEST") {
            Some("yes") => 0,
            _ => 1,
        },
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
    r.check("mmap and munmap", mmap_munmap());
    r.check("mmap refuses W+X and bad input", mmap_rejections());
    r.check("write with a bad pointer", bad_pointer());
    r.check("spawn and wait: exit status", spawn_exit_status());
    r.check("spawn passes arguments", spawn_args());
    r.check("spawn passes the environment", spawn_env());
    r.check("child killed by a fault", child_fault());
    r.check("spawn of a missing program", spawn_missing());
    r.check("wait on a non-child", wait_non_child());
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

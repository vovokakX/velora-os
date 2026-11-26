//! Process, user and system calls added in stage 8.

use super::uaccess;
use crate::{acpi, arch, mm, power, sched, time, tty};
use velora_abi::{ProcEntry, SysInfo, errno, reboot as cmd, tty as op};
use velora_vfs::Cred;

fn as_bytes<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: the ABI structs are plain `repr(C)` data, fully initialised.
    unsafe { core::slice::from_raw_parts((v as *const T).cast::<u8>(), size_of::<T>()) }
}

fn copy_str(dst: &mut [u8], s: &str) {
    let n = s.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&s.as_bytes()[..n]);
}

/// Ends process `pid` with `status`. Root may end any process, others only
/// their own.
pub fn kill(pid: u64, status: u64) -> i64 {
    let Some(uid) = sched::uid_of(pid) else {
        return -errno::ESRCH;
    };
    let me = sched::current_cred();
    if me.uid != 0 && me.uid != uid {
        return -errno::EPERM;
    }
    if sched::kill(pid, status as i32 as i64) {
        0
    } else {
        -errno::ESRCH
    }
}

pub fn sysinfo(out: u64) -> i64 {
    let u = mm::usage();
    let mut info = SysInfo {
        uptime_ns: time::uptime_ns(),
        mem_total: u.managed,
        mem_used: u.in_use,
        processes: sched::processes()
            .iter()
            .filter(|p| p.exited.is_none())
            .count() as u64,
        boot_ms: time::boot_done_ms(),
        cpus: acpi::get().madt.lapics.iter().filter(|l| l.enabled).count() as u64,
        ..Default::default()
    };
    copy_str(&mut info.version, crate::VERSION);
    let mut brand = [0u8; 48];
    copy_str(&mut info.cpu, arch::cpu::brand_string(&mut brand).trim());
    if uaccess::write(out, as_bytes(&info)) {
        0
    } else {
        -errno::EFAULT
    }
}

pub fn reboot(what: u64) -> i64 {
    if sched::current_cred().uid != 0 {
        return -errno::EPERM;
    }
    match what {
        cmd::RESTART => power::restart(),
        cmd::POWER_OFF => power::power_off(),
        _ => -errno::EINVAL,
    }
}

/// Changes user and group. Only root may do this; it is how `login`
/// becomes the user who logged in.
pub fn setuid(uid: u64, gid: u64) -> i64 {
    if sched::current_cred().uid != 0 {
        return -errno::EPERM;
    }
    if uid > u32::MAX as u64 || gid > u32::MAX as u64 {
        return -errno::EINVAL;
    }
    sched::set_cred(Cred {
        uid: uid as u32,
        gid: gid as u32,
    });
    0
}

pub fn getuid() -> i64 {
    let c = sched::current_cred();
    (c.uid as i64) | ((c.gid as i64) << 32)
}

pub fn proclist(buf: u64, len: u64) -> i64 {
    let size = size_of::<ProcEntry>();
    let mut out = alloc::vec::Vec::new();
    for p in sched::processes() {
        if out.len() + size > len as usize {
            break;
        }
        let mut e = ProcEntry {
            pid: p.pid,
            parent: p.parent,
            uid: p.uid,
            state: p.exited.is_some() as u32,
            threads: p.threads as u32,
            memory: p.user_pages * mm::PAGE_SIZE,
            ..Default::default()
        };
        copy_str(&mut e.name, &p.name);
        out.extend_from_slice(as_bytes(&e));
    }
    if uaccess::write(buf, &out) {
        out.len() as i64
    } else {
        -errno::EFAULT
    }
}

pub fn tty(what: u64, arg: u64) -> i64 {
    match what {
        op::SET_MODE => tty::set_mode(arg) as i64,
        op::SET_FOREGROUND => {
            tty::set_foreground(arg);
            0
        }
        op::SIZE => {
            let (c, r) = tty::size();
            ((c << 32) | r) as i64
        }
        _ => -errno::EINVAL,
    }
}

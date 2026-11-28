//! Process, user and system calls added in stage 8.

use super::uaccess;
use crate::{arch, mm, power, sched, time, tty};
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
        cpus: crate::smp::online() as u64,
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

/// The pseudo-terminal of the caller's descriptor 0, if it is one.
fn own_pty() -> Option<alloc::sync::Arc<crate::pty::Pty>> {
    let f = sched::fd_get(0)?;
    match &f.lock().handle {
        crate::fs::Handle::PtySlave(p) => Some(p.clone()),
        _ => None,
    }
}

pub fn tty(what: u64, arg: u64) -> i64 {
    if let Some(p) = own_pty() {
        return match what {
            op::SET_MODE => p.set_mode(arg) as i64,
            op::SET_FOREGROUND => {
                p.set_foreground(arg);
                0
            }
            op::SIZE => {
                let (c, r) = p.size();
                ((c << 32) | r) as i64
            }
            // No images in a pseudo-terminal: programs use text.
            op::CELL_SIZE => -errno::ENODEV,
            _ => -errno::EINVAL,
        };
    }
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
        op::CELL_SIZE => match tty::cell_size() {
            Some((w, h)) => ((w << 32) | h) as i64,
            None => -errno::ENODEV,
        },
        _ => -errno::EINVAL,
    }
}

/// Pseudo-terminal operations (see `velora_abi::pty`).
pub fn pty(what: u64, a: u64, b: u64) -> i64 {
    use alloc::sync::Arc;
    use crate::sync::SpinLock;
    let (cols, rows) = ((b >> 32) as u32, b as u32);
    match what {
        velora_abi::pty::OPEN => {
            if !uaccess::write(a, &[0u8; 16]) {
                return -errno::EFAULT;
            }
            let (m, s) = crate::fs::new_pty(cols, rows);
            let Some(mfd) = sched::fd_install(Arc::new(SpinLock::new(m))) else {
                return -errno::EMFILE;
            };
            let Some(sfd) = sched::fd_install(Arc::new(SpinLock::new(s))) else {
                drop(sched::fd_remove(mfd));
                return -errno::EMFILE;
            };
            let mut out = [0u8; 16];
            out[..8].copy_from_slice(&mfd.to_le_bytes());
            out[8..].copy_from_slice(&sfd.to_le_bytes());
            if uaccess::write(a, &out) { 0 } else { -errno::EFAULT }
        }
        velora_abi::pty::RESIZE => {
            let Some(f) = sched::fd_get(a) else {
                return -errno::EBADF;
            };
            match &f.lock().handle {
                crate::fs::Handle::PtyMaster(p) => {
                    p.resize(cols, rows);
                    0
                }
                _ => -errno::EINVAL,
            }
        }
        _ => -errno::EINVAL,
    }
}

/// Random bytes from RDRAND when the CPU has it, mixed with the TSC; a
/// TSC-based generator otherwise. Used for salts and GUIDs.
pub fn getrandom(buf: u64, len: u64) -> i64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
    let n = len.min(256) as usize;
    let has_rdrand = arch::cpu::cpuid(1, 0).ecx & (1 << 30) != 0;
    let mut out = alloc::vec![0u8; n];
    for chunk in out.chunks_mut(8) {
        let mut v = 0u64;
        if has_rdrand {
            for _ in 0..10 {
                let ok: u8;
                // SAFETY: RDRAND is available (CPUID.1:ECX[30]).
                unsafe {
                    core::arch::asm!("rdrand {v}", "setc {ok}", v = out(reg) v, ok = out(reg_byte) ok)
                };
                if ok != 0 {
                    break;
                }
            }
        }
        // splitmix64 over the TSC and a running state.
        let mut z = STATE
            .fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
            .wrapping_add(arch::cpu::rdtsc());
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        let bytes = (v ^ z).to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    if uaccess::write(buf, &out) {
        n as i64
    } else {
        -errno::EFAULT
    }
}

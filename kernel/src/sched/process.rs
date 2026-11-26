//! User processes: an address space plus one or more threads.

use super::{
    KERNEL_PID, Pid, Priority, Sched, State, Tid, make_ready, mark_dead, new_thread, with,
};
use crate::arch::cpu;
use crate::fs::{FdTable, FileRef};
use crate::mm::paging::{AddressSpace, UserKind};
use crate::mm::{PAGE_SIZE, frame, phys_to_virt};
use alloc::string::String;
use alloc::vec::Vec;
use velora_vfs::Cred;

/// Where flat test programs are loaded (until the ELF loader in stage 5).
pub const FLAT_CODE_BASE: u64 = 0x0000_0000_0040_0000;
/// A zeroed read/write data area every flat program gets.
pub const FLAT_DATA_BASE: u64 = 0x0000_0000_0060_0000;
pub const FLAT_DATA_PAGES: u64 = 16;
/// Top of the initial user stack. The page above it is never mapped.
pub const USER_STACK_TOP: u64 = 0x0000_7fff_fff0_0000;
pub const USER_STACK_PAGES: u64 = 32;
/// Window for non-fixed `mmap` requests.
pub const MMAP_BASE: u64 = 0x0000_2000_0000_0000;
pub const MMAP_END: u64 = 0x0000_4000_0000_0000;

/// Which system call table a process uses. Only the native Velora ABI
/// exists today; a Win32 personality (for .exe programs) is planned and
/// will be served by a user-space translation layer on top of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Personality {
    Native,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcState {
    Alive,
    /// Exited; waiting for `wait_pid` to collect the status.
    Zombie(i64),
}

pub(super) struct Process {
    pub pid: Pid,
    pub name: String,
    pub parent: Pid,
    pub aspace: Option<AddressSpace>,
    pub threads: Vec<Tid>,
    pub state: ProcState,
    pub personality: Personality,
    pub exit_waiters: Vec<Tid>,
    /// Parent exited: nobody will wait, so the kernel removes it on exit.
    pub detached: bool,
    /// Next address for non-fixed `mmap` requests.
    pub mmap_next: u64,
    pub files: FdTable,
    /// User and group the process acts as (root until users arrive).
    pub cred: Cred,
}

#[derive(Clone, Debug)]
pub struct ProcessInfo {
    pub pid: Pid,
    pub name: String,
    pub parent: Pid,
    pub threads: usize,
    pub exited: Option<i64>,
    pub user_pages: u64,
}

impl Process {
    pub fn info(&self) -> ProcessInfo {
        ProcessInfo {
            pid: self.pid,
            name: self.name.clone(),
            parent: self.parent,
            threads: self.threads.len(),
            exited: match self.state {
                ProcState::Zombie(c) => Some(c),
                ProcState::Alive => None,
            },
            user_pages: self.aspace.as_ref().map(|a| a.user_pages()).unwrap_or(0),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SpawnError {
    OutOfMemory,
    TooLarge,
}

/// Creates a process from a flat binary: `code` is copied to
/// `FLAT_CODE_BASE` (read+execute) and started there with a fresh stack
/// and a zeroed data area at `FLAT_DATA_BASE`.
pub fn spawn_flat(name: &str, code: &[u8], priority: Priority) -> Result<Pid, SpawnError> {
    if code.is_empty() || code.len() > 1024 * 1024 {
        return Err(SpawnError::TooLarge);
    }
    let aspace = AddressSpace::new().map_err(|_| SpawnError::OutOfMemory)?;
    let pages = (code.len() as u64).div_ceil(PAGE_SIZE);
    for i in 0..pages {
        let phys = frame::alloc_zeroed().ok_or(SpawnError::OutOfMemory)?;
        let off = (i * PAGE_SIZE) as usize;
        let chunk = &code[off..code.len().min(off + PAGE_SIZE as usize)];
        // SAFETY: the frame was just allocated; HHDM maps it.
        unsafe {
            core::ptr::copy_nonoverlapping(
                chunk.as_ptr(),
                phys_to_virt(phys) as *mut u8,
                chunk.len(),
            )
        };
        if aspace
            .map(FLAT_CODE_BASE + i * PAGE_SIZE, phys, UserKind::Code)
            .is_err()
        {
            frame::free(phys);
            return Err(SpawnError::OutOfMemory);
        }
    }
    aspace
        .map_new(FLAT_DATA_BASE, FLAT_DATA_PAGES, UserKind::Data)
        .map_err(|_| SpawnError::OutOfMemory)?;
    aspace
        .map_new(
            USER_STACK_TOP - USER_STACK_PAGES * PAGE_SIZE,
            USER_STACK_PAGES,
            UserKind::Data,
        )
        .map_err(|_| SpawnError::OutOfMemory)?;
    spawn_user(
        name,
        aspace,
        FLAT_CODE_BASE,
        USER_STACK_TOP,
        Personality::Native,
        priority,
    )
}

/// Creates a process with a prepared address space and starts its first
/// thread at `entry` with stack pointer `rsp`. The parent is the current
/// process (pid 0 when called from a kernel thread).
pub fn spawn_user(
    name: &str,
    aspace: AddressSpace,
    entry: u64,
    rsp: u64,
    personality: Personality,
    priority: Priority,
) -> Result<Pid, SpawnError> {
    let was = cpu::irq_save();
    let result = with(|s| {
        let pid = s.next_pid;
        s.next_pid += 1;
        let parent = s.threads[&s.current].pid;
        let cred = s.processes.get(&parent).map_or(Cred::ROOT, |p| p.cred);
        s.processes.insert(
            pid,
            Process {
                pid,
                name: String::from(name),
                parent,
                aspace: Some(aspace),
                threads: Vec::new(),
                state: ProcState::Alive,
                personality,
                exit_waiters: Vec::new(),
                detached: false,
                mmap_next: MMAP_BASE,
                files: FdTable::standard(),
                cred,
            },
        );
        match new_thread(
            s,
            name,
            pid,
            priority,
            super::user_thread_start,
            0,
            Some((entry, rsp)),
        ) {
            Some(tid) => {
                s.processes.get_mut(&pid).unwrap().threads.push(tid);
                Ok(pid)
            }
            None => {
                s.processes.remove(&pid);
                Err(SpawnError::OutOfMemory)
            }
        }
    });
    cpu::irq_restore(was);
    result
}

/// Parent of `pid`, if the process exists.
pub fn parent_of(pid: Pid) -> Option<Pid> {
    with(|s| s.processes.get(&pid).map(|p| p.parent))
}

/// Reserves `len` bytes of address space for a non-fixed `mmap` in the
/// current process and returns the start. A guard page is left after it.
pub fn reserve_mmap_range(len: u64) -> Option<u64> {
    with(|s| {
        let pid = s.threads[&s.current].pid;
        let p = s.processes.get_mut(&pid)?;
        let start = p.mmap_next;
        let next = start.checked_add(len)?.checked_add(PAGE_SIZE)?;
        if next > MMAP_END {
            return None;
        }
        p.mmap_next = next;
        Some(start)
    })
}

/// Bookkeeping when thread `tid` of `pid` has exited.
pub(super) fn thread_exited(s: &mut Sched, pid: Pid, _tid: Tid, code: i64) {
    let alive = s
        .processes
        .get(&pid)
        .map(|p| {
            p.threads
                .iter()
                .any(|t| s.threads.get(t).is_some_and(|t| t.state != State::Dead))
        })
        .unwrap_or(false);
    if !alive {
        become_zombie(s, pid, code);
    }
}

fn become_zombie(s: &mut Sched, pid: Pid, status: i64) {
    let Some(p) = s.processes.get_mut(&pid) else {
        return;
    };
    if p.state != ProcState::Alive {
        return;
    }
    p.state = ProcState::Zombie(status);
    // Close its files (the VFS lock may be taken under the scheduler lock).
    drop(core::mem::take(&mut p.files));
    let waiters = core::mem::take(&mut p.exit_waiters);
    for w in waiters {
        make_ready(s, w);
    }
    // Nobody can wait for the children of an exited process any more.
    for child in s.processes.values_mut().filter(|c| c.parent == pid) {
        child.parent = KERNEL_PID;
        child.detached = true;
    }
}

/// Kills every thread of `pid` and makes it a zombie with `status`.
/// The current thread, if it belongs to `pid`, is left for the caller to
/// end with `exit_current`.
pub(super) fn terminate(s: &mut Sched, pid: Pid, status: i64) {
    let tids = s
        .processes
        .get(&pid)
        .map(|p| p.threads.clone())
        .unwrap_or_default();
    for t in tids {
        if t != s.current {
            mark_dead(s, t);
        }
    }
    become_zombie(s, pid, status);
}

/// Kills process `pid` with `status`. Returns false if there is no such
/// living process. Killing the current process does not return.
pub fn kill(pid: Pid, status: i64) -> bool {
    if pid == KERNEL_PID {
        return false;
    }
    let was = cpu::irq_save();
    let (found, is_current) = with(|s| {
        let found = s
            .processes
            .get(&pid)
            .is_some_and(|p| p.state == ProcState::Alive);
        if found {
            terminate(s, pid, status);
        }
        (found, s.threads[&s.current].pid == pid)
    });
    if found && is_current {
        super::exit_current(status);
    }
    cpu::irq_restore(was);
    found
}

/// Waits until process `pid` exits, removes it and returns its status.
/// Returns `None` if there is no such process.
pub fn wait_pid(pid: Pid) -> Option<i64> {
    let was = cpu::irq_save();
    let status = loop {
        let state = with(|s| {
            let cur = s.current;
            let p = s.processes.get_mut(&pid)?;
            match p.state {
                ProcState::Zombie(c) => Some(Some(c)),
                ProcState::Alive => {
                    p.exit_waiters.push(cur);
                    Some(None)
                }
            }
        });
        match state {
            None => break None,
            Some(Some(c)) => break Some(c),
            Some(None) => super::block_current(None),
        }
    };
    if status.is_some() {
        // Make sure the dead threads are reaped before dropping the process.
        super::finish_switch();
        let aspace = with(|s| s.processes.remove(&pid).and_then(|mut p| p.aspace.take()));
        drop(aspace);
    }
    cpu::irq_restore(was);
    status
}

/// Credentials of the current process (root for kernel threads).
pub fn current_cred() -> Cred {
    with(|s| {
        let pid = s.threads[&s.current].pid;
        s.processes.get(&pid).map_or(Cred::ROOT, |p| p.cred)
    })
}

/// The open file behind descriptor `fd` of the current process.
pub fn fd_get(fd: u64) -> Option<FileRef> {
    with(|s| {
        let pid = s.threads[&s.current].pid;
        s.processes.get(&pid)?.files.get(fd)
    })
}

/// Gives `f` the lowest free descriptor; `None` when the table is full.
pub fn fd_install(f: FileRef) -> Option<u64> {
    with(|s| {
        let pid = s.threads[&s.current].pid;
        s.processes.get_mut(&pid)?.files.install(f)
    })
}

/// Removes descriptor `fd`. The caller drops the result outside the
/// scheduler lock.
pub fn fd_remove(fd: u64) -> Option<FileRef> {
    with(|s| {
        let pid = s.threads[&s.current].pid;
        s.processes.get_mut(&pid)?.files.remove(fd)
    })
}

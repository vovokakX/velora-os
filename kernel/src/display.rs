//! The screen for graphical programs (`display` system call).
//!
//! One process at a time maps the frame buffer into its address space and
//! draws on it directly; meanwhile the text console only updates its RAM
//! copy of the screen. When the owner releases the screen or exits, the
//! console copy is shown again. The release after an exit happens on the
//! next console output or when a parent collects the process ([`poll`]),
//! not inside the scheduler.

use crate::console;
use crate::mm::PAGE_SIZE;
use crate::mm::paging::UserKind;
use crate::sched;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicBool, Ordering};
use velora_abi::errno;

struct Owner {
    pid: u64,
    addr: u64,
    len: u64,
}

static OWNER: SpinLock<Option<Owner>> = SpinLock::new(None);
/// The owner exited: show the console at the next chance.
static SHOW_PENDING: AtomicBool = AtomicBool::new(false);

/// True if process `pid` owns the screen.
pub fn is_owner(pid: u64) -> bool {
    OWNER.lock().as_ref().is_some_and(|o| o.pid == pid)
}

pub fn info(out: u64) -> i64 {
    let Some((info, _, _)) = console::display() else {
        return -errno::ENODEV;
    };
    // SAFETY: DisplayInfo is plain data without padding bytes left
    // uninitialised (the padding is an explicit field).
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&info as *const velora_abi::DisplayInfo).cast::<u8>(),
            core::mem::size_of::<velora_abi::DisplayInfo>(),
        )
    };
    if !crate::syscall::uaccess::write(out, bytes) {
        return -errno::EFAULT;
    }
    0
}

pub fn map() -> i64 {
    poll();
    let Some((_, phys, size)) = console::display() else {
        return -errno::ENODEV;
    };
    let pid = sched::current_pid();
    let mut owner = OWNER.lock();
    if let Some(o) = owner.as_ref() {
        return if o.pid == pid {
            o.addr as i64
        } else {
            -errno::EBUSY
        };
    }
    let len = size.next_multiple_of(PAGE_SIZE);
    let Some(addr) = sched::reserve_mmap_range(len) else {
        return -errno::ENOMEM;
    };
    let mapped = sched::with_address_space(pid, |a| {
        let mut off = 0;
        while off < len {
            if a.map(
                addr + off,
                (phys & !(PAGE_SIZE - 1)) + off,
                UserKind::Framebuffer,
            )
            .is_err()
            {
                let mut v = 0;
                while v < off {
                    a.unmap(addr + v);
                    v += PAGE_SIZE;
                }
                return false;
            }
            off += PAGE_SIZE;
        }
        true
    })
    .unwrap_or(false);
    if !mapped {
        return -errno::ENOMEM;
    }
    if !console::set_hidden(true) {
        crate::kwarn!("display: the console has no back buffer; text may draw over the program");
    }
    *owner = Some(Owner { pid, addr, len });
    (addr + (phys & (PAGE_SIZE - 1))) as i64
}

pub fn release() -> i64 {
    let pid = sched::current_pid();
    let mut owner = OWNER.lock();
    match owner.as_ref() {
        Some(o) if o.pid == pid => {
            let (addr, len) = (o.addr, o.len);
            sched::with_address_space(pid, |a| {
                let mut v = 0;
                while v < len {
                    a.unmap(addr + v);
                    v += PAGE_SIZE;
                }
            });
            *owner = None;
            drop(owner);
            console::set_hidden(false);
            0
        }
        _ => -errno::EINVAL,
    }
}

/// Called when a process becomes a zombie (inside the scheduler: only
/// notes that the console must come back).
pub fn process_exited(pid: u64) {
    // try_lock: the scheduler must not wait here. If the lock is busy the
    // owner is mapping or releasing, so it is alive or being handled.
    if let Some(mut owner) = OWNER.try_lock()
        && owner.as_ref().is_some_and(|o| o.pid == pid)
    {
        *owner = None;
        SHOW_PENDING.store(true, Ordering::SeqCst);
    }
}

/// Shows the console again if the screen's owner has exited.
pub fn poll() {
    if SHOW_PENDING.swap(false, Ordering::SeqCst) {
        console::set_hidden(false);
    }
}

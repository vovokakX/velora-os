//! System call dispatch. The process's personality selects the table;
//! today only the native Velora ABI exists (see docs/syscalls.md).

mod files;
mod native;

use crate::arch::x86_64::syscall::SyscallFrame;
use crate::sched::{self, Personality};
use velora_abi::USER_MAX;

/// Called by the `syscall` entry code with interrupts disabled.
#[unsafe(no_mangle)]
extern "C" fn velora_syscall_dispatch(frame: &mut SyscallFrame) {
    let result = match sched::current_personality() {
        Personality::Native => native::dispatch(frame),
    };
    frame.rax = result as u64;
    // `sysret` to a non-canonical RIP would fault in kernel mode on some
    // CPUs. The last user page is never mapped, so a valid program cannot
    // get here; anything else is ended instead of returned to.
    if frame.rcx >= USER_MAX {
        crate::kwarn!(
            "process {}: return address {:#x} out of range",
            sched::current_pid(),
            frame.rcx
        );
        sched::exit_process(velora_abi::fault_exit_status(13));
    }
}

/// Copying between the current process and the kernel, after checking
/// that the user range is mapped with the right permissions.
pub mod uaccess {
    use crate::sched;

    fn check(addr: u64, len: u64, write: bool) -> bool {
        let pid = sched::current_pid();
        sched::with_address_space(pid, |a| a.check_user_range(addr, len, write)).unwrap_or(false)
    }

    /// Borrows `len` bytes of user memory for reading.
    pub fn read_slice<'a>(addr: u64, len: u64) -> Option<&'a [u8]> {
        if !check(addr, len, false) {
            return None;
        }
        // SAFETY: the range is mapped and user-accessible in the active
        // address space; the kernel is not preemptible here, so the
        // mapping cannot change while the slice is used.
        Some(unsafe { core::slice::from_raw_parts(addr as *const u8, len as usize) })
    }

    /// Copies `data` to user memory at `addr`.
    pub fn write(addr: u64, data: &[u8]) -> bool {
        if !check(addr, data.len() as u64, true) {
            return false;
        }
        // SAFETY: as in `read_slice`, plus the range is writable.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, data.len()) };
        true
    }
}

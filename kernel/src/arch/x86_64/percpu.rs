//! Per-CPU data reached through the GS segment base.
//!
//! In the kernel GS_BASE points at [`PerCpu`]; in user mode the two GS
//! bases are swapped (`swapgs`), so user code sees KERNEL_GS_BASE (0).
//! The field offsets are used by the `syscall` entry code.

use super::cpu;
use crate::sync::RacyCell;

const MSR_GS_BASE: u32 = 0xc000_0101;
const MSR_KERNEL_GS_BASE: u32 = 0xc000_0102;

#[repr(C)]
pub struct PerCpu {
    /// Address of this structure (offset 0).
    pub self_ptr: u64,
    /// Kernel stack top of the current thread (offset 8), used by `syscall`.
    pub kernel_rsp: u64,
    /// Scratch slot for the user stack pointer during `syscall` (offset 16).
    pub user_rsp: u64,
}

pub const OFFSET_KERNEL_RSP: usize = 8;
pub const OFFSET_USER_RSP: usize = 16;

static BSP: RacyCell<PerCpu> = RacyCell::new(PerCpu {
    self_ptr: 0,
    kernel_rsp: 0,
    user_rsp: 0,
});

/// Points GS at the boot CPU's per-CPU data. Must run after the GDT is
/// loaded (loading a GS selector clears the base).
pub fn init() {
    let p = BSP.get();
    // SAFETY: single-threaded init; the MSRs exist on every x86_64 CPU.
    unsafe {
        (*p).self_ptr = p as u64;
        cpu::wrmsr(MSR_GS_BASE, p as u64);
        cpu::wrmsr(MSR_KERNEL_GS_BASE, 0);
    }
}

/// Sets the stack used on the next `syscall` from user mode.
pub fn set_kernel_rsp(top: u64) {
    // SAFETY: one CPU; called with interrupts disabled during a switch.
    unsafe { (*BSP.get()).kernel_rsp = top };
}

pub fn gs_base() -> u64 {
    // SAFETY: reading an MSR that always exists.
    unsafe { cpu::rdmsr(MSR_GS_BASE) }
}

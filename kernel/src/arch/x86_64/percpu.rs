//! Per-CPU data reached through the GS segment base.
//!
//! In the kernel GS_BASE points at this CPU's [`PerCpu`]; in user mode
//! the two GS bases are swapped (`swapgs`), so user code sees
//! KERNEL_GS_BASE (0). The first field offsets are used by the `syscall`
//! entry code.

use super::cpu;
use super::gdt::Tss;
use crate::sync::RacyCell;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

const MSR_GS_BASE: u32 = 0xc000_0101;
const MSR_KERNEL_GS_BASE: u32 = 0xc000_0102;

/// Most CPUs the kernel starts (more are left idle in the firmware).
pub const MAX_CPUS: usize = 64;

#[repr(C)]
pub struct PerCpu {
    /// Address of this structure (offset 0).
    pub self_ptr: u64,
    /// Kernel stack top of the current thread (offset 8), used by `syscall`.
    pub kernel_rsp: u64,
    /// Scratch slot for the user stack pointer during `syscall` (offset 16).
    pub user_rsp: u64,
    /// Index of this CPU: 0 for the boot CPU, then 1, 2, ...
    pub index: u64,
    /// Nesting depth of the big kernel lock held by this CPU.
    pub bkl_depth: u64,
    /// This CPU's task state segment.
    pub tss: *mut Tss,
}

pub const OFFSET_KERNEL_RSP: usize = 8;
pub const OFFSET_USER_RSP: usize = 16;

static BSP: RacyCell<PerCpu> = RacyCell::new(PerCpu {
    self_ptr: 0,
    kernel_rsp: 0,
    user_rsp: 0,
    index: 0,
    bkl_depth: 0,
    tss: core::ptr::null_mut(),
});

/// Set once the boot CPU's GS base points at its data.
static READY: AtomicBool = AtomicBool::new(false);

/// True once [`init`] ran (before it, the boot CPU has no per-CPU data).
#[inline]
pub fn ready() -> bool {
    READY.load(Ordering::Relaxed)
}

fn install(p: *mut PerCpu) {
    // SAFETY: each CPU sets its own GS base once, during its bring-up,
    // after its GDT is loaded (loading a GS selector clears the base).
    unsafe {
        (*p).self_ptr = p as u64;
        cpu::wrmsr(MSR_GS_BASE, p as u64);
        cpu::wrmsr(MSR_KERNEL_GS_BASE, 0);
    }
}

/// Points GS at the boot CPU's per-CPU data.
pub fn init(tss: *mut Tss) {
    let p = BSP.get();
    // SAFETY: single-threaded boot.
    unsafe { (*p).tss = tss };
    install(p);
    READY.store(true, Ordering::SeqCst);
}

/// Per-CPU data for application processor `index` (allocated once).
pub fn init_ap(index: usize, tss: *mut Tss) {
    let p = alloc::boxed::Box::leak(alloc::boxed::Box::new(PerCpu {
        self_ptr: 0,
        kernel_rsp: 0,
        user_rsp: 0,
        index: index as u64,
        bkl_depth: 0,
        tss,
    }));
    install(p);
}

/// This CPU's data. Valid in kernel mode once `init`/`init_ap` ran.
#[inline]
pub fn this() -> &'static mut PerCpu {
    let p: u64;
    // SAFETY: GS_BASE points at this CPU's PerCpu, whose first field is
    // its own address.
    unsafe {
        asm!("mov {}, gs:[0]", out(reg) p, options(nostack, preserves_flags, readonly));
        &mut *(p as *mut PerCpu)
    }
}

/// Index of the CPU running this code.
#[inline]
pub fn index() -> usize {
    if !ready() {
        return 0; // the boot CPU, before `init`
    }
    this().index as usize
}

/// Sets the stack used on the next `syscall` from user mode.
pub fn set_kernel_rsp(top: u64) {
    this().kernel_rsp = top;
}

pub fn gs_base() -> u64 {
    // SAFETY: reading an MSR that always exists.
    unsafe { cpu::rdmsr(MSR_GS_BASE) }
}

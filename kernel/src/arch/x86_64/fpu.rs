//! FPU/SSE state for user programs.
//!
//! The kernel itself is built without SSE (soft-float target), so only user
//! threads use these registers. Every thread has a 512-byte FXSAVE area
//! that is saved and restored on each context switch.

use crate::sync::RacyCell;
use core::arch::asm;

#[repr(C, align(16))]
#[derive(Clone)]
pub struct FxState(pub [u8; 512]);

const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR0_NE: u64 = 1 << 5;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;

static INITIAL: RacyCell<FxState> = RacyCell::new(FxState([0; 512]));

/// Enables x87 and SSE for user code and records a clean initial state.
pub fn init() {
    // SAFETY: standard CR0/CR4 setup for SSE; INITIAL is written once here
    // before any thread exists.
    unsafe {
        let mut cr0: u64;
        asm!("mov {}, cr0", out(reg) cr0);
        cr0 = (cr0 & !(CR0_EM | CR0_TS)) | CR0_MP | CR0_NE;
        asm!("mov cr0, {}", in(reg) cr0);
        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4);
        cr4 |= CR4_OSFXSR | CR4_OSXMMEXCPT;
        asm!("mov cr4, {}", in(reg) cr4);
        let mxcsr: u32 = 0x1f80; // all exceptions masked, round to nearest
        asm!("fninit", "ldmxcsr [{}]", in(reg) &mxcsr);
        asm!("fxsave64 [{}]", in(reg) INITIAL.get().cast::<u8>());
    }
}

/// A fresh state for a new thread.
pub fn initial() -> FxState {
    // SAFETY: INITIAL is only written in `init`.
    unsafe { (*INITIAL.get()).clone() }
}

#[inline]
pub fn save(area: &mut FxState) {
    // SAFETY: the area is 512 bytes and 16-byte aligned.
    unsafe { asm!("fxsave64 [{}]", in(reg) area.0.as_mut_ptr(), options(nostack)) }
}

#[inline]
pub fn restore(area: &FxState) {
    // SAFETY: the area holds a state produced by fxsave.
    unsafe { asm!("fxrstor64 [{}]", in(reg) area.0.as_ptr(), options(nostack)) }
}

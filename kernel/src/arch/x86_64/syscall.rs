//! `syscall`/`sysret` entry. The handler runs on the current thread's
//! kernel stack with interrupts disabled (the kernel is not preemptible
//! inside system calls; blocking calls switch threads explicitly).

use super::{cpu, gdt, percpu};
use core::arch::global_asm;

const MSR_STAR: u32 = 0xc000_0081;
const MSR_LSTAR: u32 = 0xc000_0082;
const MSR_FMASK: u32 = 0xc000_0084;
const EFER_SCE: u64 = 1 << 0;

/// Registers saved by the entry code. `rax` holds the call number on entry
/// and the result on exit.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r9: u64,
    pub r8: u64,
    pub r10: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rax: u64,
    /// User RIP to return to.
    pub rcx: u64,
    /// User RFLAGS.
    pub r11: u64,
    pub user_rsp: u64,
}

global_asm!(
    r#"
    .section .text.syscall, "ax", @progbits
    .global velora_syscall_entry
velora_syscall_entry:
    swapgs
    movq %rsp, %gs:{user_rsp}
    movq %gs:{kernel_rsp}, %rsp
    pushq %gs:{user_rsp}
    pushq %r11
    pushq %rcx
    pushq %rax
    pushq %rdi
    pushq %rsi
    pushq %rdx
    pushq %r10
    pushq %r8
    pushq %r9
    pushq %rbx
    pushq %rbp
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, %rdi
    cld
    call velora_syscall_dispatch
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %rbp
    popq %rbx
    popq %r9
    popq %r8
    popq %r10
    popq %rdx
    popq %rsi
    popq %rdi
    popq %rax
    popq %rcx
    popq %r11
    popq %rsp
    swapgs
    sysretq
"#,
    user_rsp = const percpu::OFFSET_USER_RSP,
    kernel_rsp = const percpu::OFFSET_KERNEL_RSP,
    options(att_syntax)
);

unsafe extern "C" {
    fn velora_syscall_entry();
}

/// Programs the `syscall` MSRs.
pub fn init() {
    // SAFETY: standard SYSCALL setup. STAR: kernel CS = 0x08 (SS = 0x10);
    // sysret uses 0x1b + 16 = user CS 0x2b and 0x1b + 8 = user SS 0x23.
    // The RPL bits must be in STAR itself: Intel CPUs force RPL 3 on
    // sysret, but AMD CPUs copy them, and a user SS with RPL 0 makes the
    // next iretq back to user mode fail with #GP.
    unsafe {
        let efer = cpu::rdmsr(cpu::MSR_EFER);
        cpu::wrmsr(cpu::MSR_EFER, efer | EFER_SCE);
        let star = ((gdt::USER_CODE32 as u64) << 48) | ((gdt::KERNEL_CODE as u64) << 32);
        cpu::wrmsr(MSR_STAR, star);
        cpu::wrmsr(MSR_LSTAR, velora_syscall_entry as *const () as u64);
        // Cleared on entry: TF, IF, DF, AC.
        cpu::wrmsr(MSR_FMASK, (1 << 8) | (1 << 9) | (1 << 10) | (1 << 18));
    }
}

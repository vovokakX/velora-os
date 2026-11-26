//! Low-level context switching and the first entry into user mode.

use super::gdt;
use core::arch::{asm, global_asm};

// switch_context(old_rsp: *mut u64, new_rsp: u64)
//
// Saves the callee-saved registers on the current stack, stores the stack
// pointer into *old_rsp, loads new_rsp and restores that thread's
// registers. Must be called with interrupts disabled.
global_asm!(
    r#"
    .section .text.context, "ax", @progbits
    .global velora_switch_context
velora_switch_context:
    pushq %rbp
    pushq %rbx
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, (%rdi)
    movq %rsi, %rsp
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %rbx
    popq %rbp
    ret

    # First code a new thread runs. r12 = entry function, r13 = argument.
    .global velora_thread_trampoline
velora_thread_trampoline:
    call velora_finish_switch
    sti
    movq %r13, %rdi
    call *%r12
    movq %rax, %rdi
    call velora_thread_return
    ud2
"#,
    options(att_syntax)
);

unsafe extern "C" {
    fn velora_switch_context(old_rsp: *mut u64, new_rsp: u64);
    fn velora_thread_trampoline();
}

/// Switches stacks to another thread.
///
/// # Safety
/// Interrupts must be disabled; `new_rsp` must come from a previous switch
/// or from [`prepare_stack`].
#[inline]
pub unsafe fn switch(old_rsp: *mut u64, new_rsp: u64) {
    unsafe { velora_switch_context(old_rsp, new_rsp) }
}

/// Builds the initial stack of a new thread so that the first switch to it
/// enters `entry(arg)` through the trampoline. Returns the saved RSP.
///
/// # Safety
/// `top` must be the 16-byte aligned top of a mapped, unused stack.
pub unsafe fn prepare_stack(top: u64, entry: extern "C" fn(u64) -> u64, arg: u64) -> u64 {
    // Layout popped by switch_context: r15, r14, r13, r12, rbx, rbp, ret.
    // After the `ret` RSP equals `top`, which is 16-byte aligned as the
    // trampoline's calls require.
    let words: [u64; 7] = [
        0,                                            // r15
        0,                                            // r14
        arg,                                          // r13
        entry as usize as u64,                        // r12
        0,                                            // rbx
        0,                                            // rbp
        velora_thread_trampoline as *const () as u64, // return address
    ];
    let rsp = top - (words.len() as u64) * 8;
    // SAFETY: caller guarantees the stack is mapped and unused.
    unsafe { core::ptr::copy_nonoverlapping(words.as_ptr(), rsp as *mut u64, words.len()) };
    rsp
}

/// Enters ring 3 at `rip` with stack `rsp`, interrupts enabled. All
/// general-purpose registers are cleared so no kernel data leaks.
///
/// # Safety
/// The current address space must map `rip` and `rsp` for user access, and
/// TSS.rsp0 / the per-CPU kernel stack must point at this thread's stack.
pub unsafe fn enter_user(rip: u64, rsp: u64) -> ! {
    unsafe {
        asm!(
            "cli",
            "push {ss}",
            "push {rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "swapgs",
            "xor eax, eax",
            "xor ebx, ebx",
            "xor ecx, ecx",
            "xor edx, edx",
            "xor esi, esi",
            "xor edi, edi",
            "xor ebp, ebp",
            "xor r8d, r8d",
            "xor r9d, r9d",
            "xor r10d, r10d",
            "xor r11d, r11d",
            "xor r12d, r12d",
            "xor r13d, r13d",
            "xor r14d, r14d",
            "xor r15d, r15d",
            "iretq",
            ss = in(reg) gdt::USER_DATA as u64,
            rsp = in(reg) rsp,
            rflags = in(reg) 0x202u64,
            cs = in(reg) gdt::USER_CODE as u64,
            rip = in(reg) rip,
            options(noreturn)
        )
    }
}

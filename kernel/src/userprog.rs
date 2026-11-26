//! Small flat user programs embedded in the kernel, used to test ring 3,
//! system calls, isolation and preemption before the ELF loader exists
//! (stage 5). They are position-independent: the kernel copies their bytes
//! to the user address 0x400000. Each has a zeroed data area at 0x600000.
//!
//! System call numbers used here: 0 exit, 1 write, 3 yield, 4 getpid,
//! 21 clock_gettime, 25 sleep_ms (see velora-abi).

use core::arch::global_asm;

global_asm!(
    r#"
    .section .rodata.userprog, "a"

    // Prints a greeting and exits with 0.
    .balign 16
    .global uprog_hello_start
uprog_hello_start:
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + .Lhello_msg]
    mov edx, 18
    syscall
    xor edi, edi
    xor eax, eax
    syscall
    ud2
.Lhello_msg:
    .ascii "hello from ring 3\n"
    .global uprog_hello_end
uprog_hello_end:

    // Exits with its own pid.
    .balign 16
    .global uprog_getpid_start
uprog_getpid_start:
    mov eax, 4
    syscall
    mov edi, eax
    xor eax, eax
    syscall
    ud2
    .global uprog_getpid_end
uprog_getpid_end:

    // write() with a kernel pointer: exits with -result (expected 14, EFAULT).
    .balign 16
    .global uprog_efault_start
uprog_efault_start:
    mov eax, 1
    mov edi, 1
    movabs rsi, 0xffff800000000000
    mov edx, 10
    syscall
    neg rax
    mov rdi, rax
    xor eax, eax
    syscall
    ud2
    .global uprog_efault_end
uprog_efault_end:

    // Unknown system call: exits with -result (expected 38, ENOSYS).
    .balign 16
    .global uprog_enosys_start
uprog_enosys_start:
    mov eax, 999
    syscall
    neg rax
    mov rdi, rax
    xor eax, eax
    syscall
    ud2
    .global uprog_enosys_end
uprog_enosys_end:

    // Writes 0x55 to its data area and exits with what it reads back.
    .balign 16
    .global uprog_writemem_start
uprog_writemem_start:
    mov dword ptr [0x600000], 0x55
    mov edi, dword ptr [0x600000]
    xor eax, eax
    syscall
    ud2
    .global uprog_writemem_end
uprog_writemem_end:

    // Exits with the first word of its (fresh) data area: expected 0.
    .balign 16
    .global uprog_readmem_start
uprog_readmem_start:
    mov edi, dword ptr [0x600000]
    xor eax, eax
    syscall
    ud2
    .global uprog_readmem_end
uprog_readmem_end:

    // Reads kernel memory: must be killed with a page fault.
    .balign 16
    .global uprog_kread_start
uprog_kread_start:
    movabs rax, 0xffffffff80000000
    mov rax, qword ptr [rax]
    xor edi, edi
    xor eax, eax
    syscall
    ud2
    .global uprog_kread_end
uprog_kread_end:

    // Privileged instruction: must be killed with #GP.
    .balign 16
    .global uprog_cli_start
uprog_cli_start:
    cli
    xor edi, edi
    xor eax, eax
    syscall
    ud2
    .global uprog_cli_end
uprog_cli_end:

    // Writes to its own code: must be killed with a page fault (W^X).
    .balign 16
    .global uprog_selfmod_start
uprog_selfmod_start:
    lea rax, [rip]
    mov byte ptr [rax], 0x90
    xor edi, edi
    xor eax, eax
    syscall
    ud2
    .global uprog_selfmod_end
uprog_selfmod_end:

    // Exits with the RPL of its stack segment after a system call
    // (expected 3; AMD CPUs take it from STAR on sysret).
    .balign 16
    .global uprog_ssrpl_start
uprog_ssrpl_start:
    mov eax, 4
    syscall
    mov ax, ss
    movzx edi, ax
    and edi, 3
    xor eax, eax
    syscall
    ud2
    .global uprog_ssrpl_end
uprog_ssrpl_end:

    // Spins forever, counting in its data area (preemption test).
    .balign 16
    .global uprog_spin_start
uprog_spin_start:
.Lspin:
    inc qword ptr [0x600000]
    jmp .Lspin
    .global uprog_spin_end
uprog_spin_end:

    // Keeps a pid-specific value in xmm0 for a few million iterations and
    // exits with 1 if another process's value ever shows up there.
    .balign 16
    .global uprog_sse_start
uprog_sse_start:
    mov eax, 4
    syscall
    imul rax, rax, 0x01010101
    movq xmm0, rax
    mov rbx, rax
    mov r12d, 3000000
.Lsse_loop:
    movq rdx, xmm0
    cmp rdx, rbx
    jne .Lsse_fail
    dec r12d
    jnz .Lsse_loop
    xor edi, edi
    xor eax, eax
    syscall
.Lsse_fail:
    mov edi, 1
    xor eax, eax
    syscall
    ud2
    .global uprog_sse_end
uprog_sse_end:

    // Sleeps 50 ms, yields 10 times, exits with 0.
    .balign 16
    .global uprog_sleepyield_start
uprog_sleepyield_start:
    mov eax, 25
    mov edi, 50
    syscall
    mov r12d, 10
.Lyield:
    mov eax, 3
    syscall
    dec r12d
    jnz .Lyield
    xor edi, edi
    xor eax, eax
    syscall
    ud2
    .global uprog_sleepyield_end
uprog_sleepyield_end:

    // clock_gettime around a 20 ms sleep must show >= 20 ms; a bad clock
    // id must give -22 (EINVAL). Exits 0 on success, 1 on failure.
    .balign 16
    .global uprog_clock_start
uprog_clock_start:
    sub rsp, 32
    mov eax, 21
    mov edi, 1
    mov rsi, rsp
    syscall
    test rax, rax
    jnz .Lclock_fail
    mov r12, qword ptr [rsp]
    mov r13, qword ptr [rsp + 8]
    mov eax, 25
    mov edi, 20
    syscall
    mov eax, 21
    mov edi, 1
    lea rsi, [rsp + 16]
    syscall
    test rax, rax
    jnz .Lclock_fail
    mov rax, qword ptr [rsp + 16]
    sub rax, r12
    imul rax, rax, 1000000000
    add rax, qword ptr [rsp + 24]
    sub rax, r13
    cmp rax, 20000000
    jb .Lclock_fail
    mov eax, 21
    mov edi, 77
    mov rsi, rsp
    syscall
    cmp rax, -22
    jne .Lclock_fail
    xor edi, edi
    xor eax, eax
    syscall
.Lclock_fail:
    mov edi, 1
    xor eax, eax
    syscall
    ud2
    .global uprog_clock_end
uprog_clock_end:
"#
);

macro_rules! programs {
    ($($name:ident: $start:ident, $end:ident;)*) => {
        unsafe extern "C" {
            $(static $start: u8; static $end: u8;)*
        }
        $(
            pub fn $name() -> &'static [u8] {
                let s = &raw const $start;
                let e = &raw const $end;
                // SAFETY: both labels delimit one program in .rodata.
                unsafe { core::slice::from_raw_parts(s, e as usize - s as usize) }
            }
        )*
    };
}

programs! {
    hello: uprog_hello_start, uprog_hello_end;
    getpid: uprog_getpid_start, uprog_getpid_end;
    efault: uprog_efault_start, uprog_efault_end;
    enosys: uprog_enosys_start, uprog_enosys_end;
    write_mem: uprog_writemem_start, uprog_writemem_end;
    read_mem: uprog_readmem_start, uprog_readmem_end;
    read_kernel: uprog_kread_start, uprog_kread_end;
    privileged: uprog_cli_start, uprog_cli_end;
    self_modify: uprog_selfmod_start, uprog_selfmod_end;
    ss_rpl: uprog_ssrpl_start, uprog_ssrpl_end;
    spin: uprog_spin_start, uprog_spin_end;
    sse: uprog_sse_start, uprog_sse_end;
    sleep_yield: uprog_sleepyield_start, uprog_sleepyield_end;
    clock: uprog_clock_start, uprog_clock_end;
}

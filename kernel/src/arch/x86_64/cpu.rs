//! Small wrappers around privileged x86_64 instructions.

use core::arch::asm;

#[inline]
pub fn halt() {
    // SAFETY: `hlt` only pauses the CPU until the next interrupt.
    unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)) }
}

/// Stops this CPU for good: interrupts off, then `hlt` in a loop.
pub fn halt_forever() -> ! {
    disable_interrupts();
    loop {
        halt();
    }
}

#[inline]
pub fn disable_interrupts() {
    // SAFETY: clearing IF is always allowed in ring 0.
    unsafe { asm!("cli", options(nomem, nostack)) }
}

#[inline]
pub fn enable_interrupts() {
    // SAFETY: the IDT is loaded before anything calls this.
    unsafe { asm!("sti", options(nomem, nostack)) }
}

#[inline]
pub fn rflags() -> u64 {
    let r: u64;
    // SAFETY: reading RFLAGS has no side effects.
    unsafe { asm!("pushfq", "pop {}", out(reg) r, options(nomem, preserves_flags)) }
    r
}

pub const RFLAGS_IF: u64 = 1 << 9;

/// Disables interrupts and returns whether they were enabled before.
#[inline]
pub fn irq_save() -> bool {
    let was_enabled = rflags() & RFLAGS_IF != 0;
    disable_interrupts();
    was_enabled
}

#[inline]
pub fn irq_restore(was_enabled: bool) {
    if was_enabled {
        enable_interrupts();
    }
}

macro_rules! read_cr {
    ($name:ident, $reg:literal) => {
        #[inline]
        pub fn $name() -> u64 {
            let v: u64;
            // SAFETY: reading a control register has no side effects.
            unsafe { asm!(concat!("mov {}, ", $reg), out(reg) v, options(nomem, nostack, preserves_flags)) }
            v
        }
    };
}

read_cr!(read_cr0, "cr0");
read_cr!(read_cr2, "cr2");
read_cr!(read_cr3, "cr3");
read_cr!(read_cr4, "cr4");

#[inline]
pub fn read_rsp() -> u64 {
    let v: u64;
    // SAFETY: reads the stack pointer only.
    unsafe { asm!("mov {}, rsp", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}

#[inline]
pub fn read_rbp() -> u64 {
    let v: u64;
    // SAFETY: reads the frame pointer only.
    unsafe { asm!("mov {}, rbp", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}

#[inline]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: `rdtsc` is unprivileged and side-effect free.
    unsafe { asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) }
    ((hi as u64) << 32) | lo as u64
}

pub struct CpuidResult {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

pub fn cpuid(leaf: u32, subleaf: u32) -> CpuidResult {
    let r = core::arch::x86_64::__cpuid_count(leaf, subleaf);
    CpuidResult {
        eax: r.eax,
        ebx: r.ebx,
        ecx: r.ecx,
        edx: r.edx,
    }
}

/// CPU brand string ("QEMU Virtual CPU", "AMD Ryzen ..."), trimmed.
pub fn brand_string(buf: &mut [u8; 48]) -> &str {
    if cpuid(0x8000_0000, 0).eax < 0x8000_0004 {
        return "unknown";
    }
    for (i, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
        let r = cpuid(leaf, 0);
        for (j, reg) in [r.eax, r.ebx, r.ecx, r.edx].iter().enumerate() {
            let off = i * 16 + j * 4;
            buf[off..off + 4].copy_from_slice(&reg.to_le_bytes());
        }
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(48);
    core::str::from_utf8(&buf[..end])
        .unwrap_or("unknown")
        .trim()
}

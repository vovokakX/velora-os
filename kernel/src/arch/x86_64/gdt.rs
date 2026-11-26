//! Global Descriptor Table and Task State Segment.
//!
//! The selector layout is chosen for `syscall`/`sysret` (stage 4):
//! STAR expects user code32, user data and user code64 in that order.

use crate::sync::RacyCell;
use core::arch::asm;
use core::mem::size_of;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const USER_CODE32: u16 = 0x18 | 3;
pub const USER_DATA: u16 = 0x20 | 3;
pub const USER_CODE: u16 = 0x28 | 3;
pub const TSS_SELECTOR: u16 = 0x30;

/// IST slots (1-based, as used in IDT entries).
pub const IST_DOUBLE_FAULT: u8 = 1;
pub const IST_NMI: u8 = 2;
pub const IST_MACHINE_CHECK: u8 = 3;

const IST_STACK_SIZE: usize = 16 * 1024;

#[repr(C, packed(4))]
pub struct Tss {
    _reserved0: u32,
    pub rsp: [u64; 3],
    _reserved1: u64,
    pub ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    pub iomap_base: u16,
}

#[repr(C, align(16))]
struct Stack([u8; IST_STACK_SIZE]);

static TSS: RacyCell<Tss> = RacyCell::new(Tss {
    _reserved0: 0,
    rsp: [0; 3],
    _reserved1: 0,
    ist: [0; 7],
    _reserved2: 0,
    _reserved3: 0,
    iomap_base: size_of::<Tss>() as u16,
});

static IST_STACKS: RacyCell<[Stack; 3]> = RacyCell::new([
    Stack([0; IST_STACK_SIZE]),
    Stack([0; IST_STACK_SIZE]),
    Stack([0; IST_STACK_SIZE]),
]);

const GDT_ENTRIES: usize = 8;
static GDT: RacyCell<[u64; GDT_ENTRIES]> = RacyCell::new([0; GDT_ENTRIES]);

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

pub fn init() {
    // SAFETY: runs once on the boot CPU before interrupts are enabled.
    unsafe {
        let tss = &mut *TSS.get();
        let stacks = &*IST_STACKS.get();
        // The TSS is packed, so fields are copied out and back instead of
        // being borrowed in place.
        let mut ist = [0u64; 7];
        for (i, s) in stacks.iter().enumerate() {
            ist[i] = s.0.as_ptr() as u64 + IST_STACK_SIZE as u64;
        }
        tss.ist = ist;

        let gdt = &mut *GDT.get();
        gdt[0] = 0;
        gdt[1] = 0x00af_9a00_0000_ffff; // kernel code, 64-bit
        gdt[2] = 0x00cf_9200_0000_ffff; // kernel data
        gdt[3] = 0x00cf_fa00_0000_ffff; // user code, 32-bit (unused, sysret layout)
        gdt[4] = 0x00cf_f200_0000_ffff; // user data
        gdt[5] = 0x00af_fa00_0000_ffff; // user code, 64-bit
        let (lo, hi) = tss_descriptor(TSS.get() as u64, size_of::<Tss>() as u64 - 1);
        gdt[6] = lo;
        gdt[7] = hi;

        let ptr = DescriptorPointer {
            limit: (size_of::<[u64; GDT_ENTRIES]>() - 1) as u16,
            base: gdt.as_ptr() as u64,
        };
        asm!("lgdt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));

        // Reload CS with a far return, then the data segments.
        asm!(
            "push {sel}",
            "lea {tmp}, [rip + 55f]",
            "push {tmp}",
            "retfq",
            "55:",
            sel = in(reg) KERNEL_CODE as u64,
            tmp = lateout(reg) _,
            options(preserves_flags),
        );
        asm!(
            "mov ds, {0:x}",
            "mov es, {0:x}",
            "mov ss, {0:x}",
            in(reg) KERNEL_DATA,
            options(nostack, preserves_flags),
        );
        asm!(
            "mov fs, {0:x}",
            "mov gs, {0:x}",
            in(reg) 0u16,
            options(nostack, preserves_flags),
        );
        asm!("ltr {0:x}", in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
    }
}

fn tss_descriptor(base: u64, limit: u64) -> (u64, u64) {
    let mut lo = limit & 0xffff;
    lo |= (base & 0xff_ffff) << 16;
    lo |= 0x89 << 40; // present, type = 64-bit available TSS
    lo |= ((limit >> 16) & 0xf) << 48;
    lo |= ((base >> 24) & 0xff) << 56;
    (lo, base >> 32)
}

/// Current code segment selector.
pub fn current_cs() -> u16 {
    let cs: u16;
    // SAFETY: reading CS is side-effect free.
    unsafe { asm!("mov {0:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags)) }
    cs
}

/// Currently loaded task register selector.
pub fn current_tr() -> u16 {
    let tr: u16;
    // SAFETY: `str` only reads the task register.
    unsafe { asm!("str {0:x}", out(reg) tr, options(nomem, nostack, preserves_flags)) }
    tr
}

/// Address of the IST stack top for a slot, for tests and diagnostics.
pub fn ist_top(slot: u8) -> u64 {
    // SAFETY: TSS is initialised in `init` and not changed afterwards.
    let ist = unsafe { (*TSS.get()).ist };
    ist[slot as usize - 1]
}

/// Sets the stack the CPU switches to on an interrupt from ring 3.
pub fn set_rsp0(top: u64) {
    // SAFETY: TSS is a static; single CPU, called with interrupts disabled
    // during a context switch. Packed fields are copied, not borrowed.
    unsafe {
        let tss = &mut *TSS.get();
        let mut rsp = tss.rsp;
        rsp[0] = top;
        tss.rsp = rsp;
    }
}

pub fn rsp0() -> u64 {
    // SAFETY: plain read of a static.
    let rsp = unsafe { (*TSS.get()).rsp };
    rsp[0]
}

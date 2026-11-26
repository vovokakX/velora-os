//! Global descriptor table and task state segment, one pair per CPU.
//!
//! Layout (fixed, `syscall`/`sysret` depend on it):
//! 0x00 null, 0x08 kernel code, 0x10 kernel data, 0x18 user code 32-bit
//! (unused, keeps the sysret layout), 0x20 user data, 0x28 user code,
//! 0x30 TSS (two slots). Each CPU has its own TSS (its own `rsp0` and IST
//! stacks), so each has its own GDT holding that TSS's descriptor.

use super::percpu;
use crate::sync::RacyCell;
use alloc::boxed::Box;
use core::arch::asm;
use core::mem::size_of;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const USER_CODE32: u16 = 0x18 | 3;
pub const USER_DATA: u16 = 0x20 | 3;
pub const USER_CODE: u16 = 0x28 | 3;
pub const TSS_SELECTOR: u16 = 0x30;

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

const fn empty_tss() -> Tss {
    Tss {
        _reserved0: 0,
        rsp: [0; 3],
        _reserved1: 0,
        ist: [0; 7],
        _reserved2: 0,
        _reserved3: 0,
        iomap_base: size_of::<Tss>() as u16,
    }
}

#[repr(C, align(16))]
struct Stack([u8; IST_STACK_SIZE]);

const GDT_ENTRIES: usize = 8;

/// The tables of one CPU.
#[repr(C, align(16))]
struct CpuTables {
    gdt: [u64; GDT_ENTRIES],
    tss: Tss,
}

static BSP_TABLES: RacyCell<CpuTables> = RacyCell::new(CpuTables {
    gdt: [0; GDT_ENTRIES],
    tss: empty_tss(),
});

static BSP_IST: RacyCell<[Stack; 3]> = RacyCell::new([
    Stack([0; IST_STACK_SIZE]),
    Stack([0; IST_STACK_SIZE]),
    Stack([0; IST_STACK_SIZE]),
]);

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

/// Fills and loads the tables, reloads the segment registers and the
/// task register.
///
/// # Safety
/// `t` and the IST stacks must stay valid forever; runs once per CPU.
unsafe fn load(t: *mut CpuTables, ist_stacks: *const [Stack; 3]) {
    unsafe {
        let t = &mut *t;
        let mut ist = [0u64; 7];
        for (i, s) in (*ist_stacks).iter().enumerate() {
            ist[i] = s.0.as_ptr() as u64 + IST_STACK_SIZE as u64;
        }
        t.tss.ist = ist;

        let gdt = &mut t.gdt;
        gdt[0] = 0;
        gdt[1] = 0x00af_9a00_0000_ffff; // kernel code, 64-bit
        gdt[2] = 0x00cf_9200_0000_ffff; // kernel data
        gdt[3] = 0x00cf_fa00_0000_ffff; // user code, 32-bit (unused, sysret layout)
        gdt[4] = 0x00cf_f200_0000_ffff; // user data
        gdt[5] = 0x00af_fa00_0000_ffff; // user code, 64-bit
        let (lo, hi) = tss_descriptor(&raw const t.tss as u64, size_of::<Tss>() as u64 - 1);
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

/// Tables of the boot CPU. Returns its TSS (for the per-CPU data).
pub fn init() -> *mut Tss {
    // SAFETY: runs once on the boot CPU with interrupts disabled; the
    // tables are statics.
    unsafe {
        load(BSP_TABLES.get(), BSP_IST.get());
        &raw mut (*BSP_TABLES.get()).tss
    }
}

/// Tables of an application processor, allocated once and never freed.
/// Needs the heap. Returns the CPU's TSS.
pub fn init_ap() -> *mut Tss {
    let t = Box::leak(Box::new(CpuTables {
        gdt: [0; GDT_ENTRIES],
        tss: empty_tss(),
    }));
    // SAFETY: zeroed bytes are a valid `Stack`; the allocation is leaked
    // and so lives forever.
    let stacks: &'static mut [Stack; 3] = unsafe { Box::leak(Box::new_zeroed().assume_init()) };
    // SAFETY: runs once on this CPU during bring-up.
    unsafe {
        load(t, stacks);
        &raw mut t.tss
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

/// Current code segment selector (for tests).
pub fn current_cs() -> u16 {
    let cs: u16;
    // SAFETY: reading CS has no side effects.
    unsafe { asm!("mov {0:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags)) }
    cs
}

/// Current task register (for tests).
pub fn current_tr() -> u16 {
    let tr: u16;
    // SAFETY: `str` only reads the task register.
    unsafe { asm!("str {0:x}", out(reg) tr, options(nomem, nostack, preserves_flags)) }
    tr
}

fn this_tss() -> *mut Tss {
    let t = if percpu::ready() {
        percpu::this().tss
    } else {
        core::ptr::null_mut()
    };
    if t.is_null() {
        // Before the per-CPU data exists: the boot CPU.
        // SAFETY: address of a static.
        unsafe { &raw mut (*BSP_TABLES.get()).tss }
    } else {
        t
    }
}

/// Address of this CPU's IST stack top for a slot (tests, diagnostics).
pub fn ist_top(slot: u8) -> u64 {
    // SAFETY: the TSS is set up in `init`/`init_ap`; packed fields are
    // copied, not borrowed.
    let ist = unsafe { (*this_tss()).ist };
    ist[slot as usize - 1]
}

/// Sets the stack this CPU switches to on an interrupt from ring 3.
pub fn set_rsp0(top: u64) {
    // SAFETY: only this CPU uses its TSS; called with interrupts disabled
    // during a context switch. Packed fields are copied, not borrowed.
    unsafe {
        let tss = this_tss();
        let mut rsp = (*tss).rsp;
        rsp[0] = top;
        (*tss).rsp = rsp;
    }
}

pub fn rsp0() -> u64 {
    // SAFETY: plain read of this CPU's TSS.
    let rsp = unsafe { (*this_tss()).rsp };
    rsp[0]
}

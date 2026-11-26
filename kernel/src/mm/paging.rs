//! 4-level page tables.
//!
//! The kernel builds its own tables at boot instead of keeping Limine's:
//! - HHDM: every RAM-like region of the memory map at `hhdm_offset + phys`,
//!   read/write, no-execute; 2 MiB pages where alignment allows; the
//!   framebuffer write-combining.
//! - Kernel image: `.text` read+execute, `.rodata` read-only, `.data`/`.bss`
//!   read/write no-execute (W^X).
//!
//! Nothing is mapped in the lower half.

use super::{PAGE_SIZE, frame, phys_to_virt};
use crate::arch::cpu;
use crate::boot::{self, limine::MemoryKind};
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const PWT: u64 = 1 << 3;
pub const PCD: u64 = 1 << 4;
/// PS bit in PDPT/PD entries (huge page).
pub const HUGE: u64 = 1 << 7;
/// PAT bit in a 4 KiB PTE (same position as HUGE).
pub const PAT_4K: u64 = 1 << 7;
pub const GLOBAL: u64 = 1 << 8;
pub const NO_EXECUTE: u64 = 1 << 63;
const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
const HUGE_2M: u64 = 2 * 1024 * 1024;

/// Leaf permission sets for kernel mappings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Code: readable and executable.
    Code,
    /// Read-only data.
    ReadOnly,
    /// Read/write data, never executable.
    Data,
    /// Write-combining device memory (framebuffer).
    Framebuffer,
    /// Uncached device memory (MMIO registers).
    Mmio,
}

static NX_ENABLED: AtomicBool = AtomicBool::new(false);
static PAT_AVAILABLE: AtomicBool = AtomicBool::new(false);
static KERNEL_PML4: AtomicU64 = AtomicU64::new(0);
static TABLE_FRAMES: AtomicU64 = AtomicU64::new(0);
/// Serialises page table edits.
static LOCK: SpinLock<()> = SpinLock::new(());

fn nx() -> u64 {
    if NX_ENABLED.load(Ordering::Relaxed) {
        NO_EXECUTE
    } else {
        0
    }
}

fn leaf_flags(kind: Kind) -> u64 {
    let base = PRESENT | GLOBAL;
    match kind {
        Kind::Code => base,
        Kind::ReadOnly => base | nx(),
        Kind::Data => base | WRITABLE | nx(),
        Kind::Framebuffer => {
            // Limine programs PAT entry 5 as write-combining: PAT=1 PCD=0 PWT=1.
            let cache = if PAT_AVAILABLE.load(Ordering::Relaxed) {
                PAT_4K | PWT
            } else {
                PCD
            };
            base | WRITABLE | nx() | cache
        }
        Kind::Mmio => base | WRITABLE | nx() | PCD | PWT,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum MapError {
    AlreadyMapped,
    OutOfMemory,
    /// The range overlaps a 2 MiB page.
    HugePageInTheWay,
}

fn table(phys: u64) -> &'static mut [u64; 512] {
    // SAFETY: page table frames are mapped in the HHDM and only edited
    // while holding LOCK.
    unsafe { &mut *(phys_to_virt(phys) as *mut [u64; 512]) }
}

fn indices(virt: u64) -> [usize; 4] {
    [
        ((virt >> 39) & 0x1ff) as usize,
        ((virt >> 30) & 0x1ff) as usize,
        ((virt >> 21) & 0x1ff) as usize,
        ((virt >> 12) & 0x1ff) as usize,
    ]
}

fn new_table() -> Result<u64, MapError> {
    let phys = frame::alloc_zeroed().ok_or(MapError::OutOfMemory)?;
    TABLE_FRAMES.fetch_add(1, Ordering::Relaxed);
    Ok(phys)
}

/// Returns the table referenced by `entry`, creating it if needed.
fn next_level(entry: &mut u64) -> Result<u64, MapError> {
    next_level_with(entry, 0)
}

/// Like `next_level`, adding `extra` (USER for user mappings) to a newly
/// created intermediate entry.
fn next_level_with(entry: &mut u64, extra: u64) -> Result<u64, MapError> {
    if *entry & PRESENT == 0 {
        let t = new_table()?;
        // Intermediate entries are permissive; leaves decide the rights.
        *entry = t | PRESENT | WRITABLE | extra;
    } else if *entry & HUGE != 0 {
        return Err(MapError::HugePageInTheWay);
    }
    Ok(*entry & ADDR_MASK)
}

fn map_4k_in(pml4: u64, virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
    let [i4, i3, i2, i1] = indices(virt);
    let extra = flags & USER;
    let pdpt = next_level_with(&mut table(pml4)[i4], extra)?;
    let pd = next_level_with(&mut table(pdpt)[i3], extra)?;
    let pt = next_level_with(&mut table(pd)[i2], extra)?;
    let e = &mut table(pt)[i1];
    if *e & PRESENT != 0 {
        return Err(MapError::AlreadyMapped);
    }
    *e = (phys & ADDR_MASK) | flags;
    Ok(())
}

fn map_2m_in(pml4: u64, virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
    let [i4, i3, i2, _] = indices(virt);
    let pdpt = next_level(&mut table(pml4)[i4])?;
    let pd = next_level(&mut table(pdpt)[i3])?;
    let e = &mut table(pd)[i2];
    if *e & PRESENT != 0 {
        return Err(MapError::AlreadyMapped);
    }
    *e = (phys & ADDR_MASK) | flags | HUGE;
    Ok(())
}

/// Physical address and leaf flags for `virt` in the given tables.
fn translate_in(pml4: u64, virt: u64) -> Option<(u64, u64)> {
    let [i4, i3, i2, i1] = indices(virt);
    let e4 = table(pml4)[i4];
    if e4 & PRESENT == 0 {
        return None;
    }
    let e3 = table(e4 & ADDR_MASK)[i3];
    if e3 & PRESENT == 0 {
        return None;
    }
    if e3 & HUGE != 0 {
        let base = e3 & 0x000f_ffff_c000_0000;
        return Some((base + (virt & 0x3fff_ffff), e3));
    }
    let e2 = table(e3 & ADDR_MASK)[i2];
    if e2 & PRESENT == 0 {
        return None;
    }
    if e2 & HUGE != 0 {
        let base = e2 & 0x000f_ffff_ffe0_0000;
        return Some((base + (virt & (HUGE_2M - 1)), e2));
    }
    let e1 = table(e2 & ADDR_MASK)[i1];
    if e1 & PRESENT == 0 {
        return None;
    }
    Some(((e1 & ADDR_MASK) + (virt & 0xfff), e1))
}

/// Maps `[phys, phys + len)` at `virt`, using 2 MiB pages where both
/// addresses are aligned. Pages that are already mapped are skipped
/// (neighbouring regions may share a partial page).
fn map_range_in(
    pml4: u64,
    virt: u64,
    phys: u64,
    len: u64,
    flags: u64,
    allow_huge: bool,
) -> Result<(), MapError> {
    let mut off = 0;
    while off < len {
        let (v, p) = (virt + off, phys + off);
        if allow_huge && v % HUGE_2M == 0 && p % HUGE_2M == 0 && len - off >= HUGE_2M {
            match map_2m_in(pml4, v, p, flags) {
                Ok(()) | Err(MapError::AlreadyMapped) => {
                    off += HUGE_2M;
                    continue;
                }
                Err(MapError::HugePageInTheWay) => {}
                Err(e) => return Err(e),
            }
        }
        match map_4k_in(pml4, v, p, flags) {
            Ok(()) | Err(MapError::AlreadyMapped) | Err(MapError::HugePageInTheWay) => {}
            Err(e) => return Err(e),
        }
        off += PAGE_SIZE;
    }
    Ok(())
}

unsafe extern "C" {
    static __kernel_start: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __kernel_end: u8;
}

macro_rules! sym {
    ($s:ident) => {
        // Taking the address of an extern static is safe.
        (&raw const $s) as u64
    };
}

pub struct KernelSections {
    pub start: u64,
    pub text: (u64, u64),
    pub rodata: (u64, u64),
    pub data: (u64, u64),
    pub end: u64,
}

pub fn kernel_sections() -> KernelSections {
    KernelSections {
        start: sym!(__kernel_start),
        text: (sym!(__text_start), sym!(__text_end)),
        rodata: (sym!(__rodata_start), sym!(__rodata_end)),
        data: (sym!(__data_start), sym!(__kernel_end)),
        end: sym!(__kernel_end),
    }
}

pub fn kernel_image_size() -> u64 {
    let s = kernel_sections();
    s.end - s.start
}

pub fn table_frames() -> u64 {
    TABLE_FRAMES.load(Ordering::Relaxed)
}

fn enable_nx() {
    if !cpu::has_nx() {
        return;
    }
    // SAFETY: EFER exists on every x86_64 CPU; setting NXE only enables
    // the no-execute bit in page tables.
    unsafe {
        let efer = cpu::rdmsr(cpu::MSR_EFER);
        if efer & cpu::EFER_NXE == 0 {
            cpu::wrmsr(cpu::MSR_EFER, efer | cpu::EFER_NXE);
        }
    }
    NX_ENABLED.store(true, Ordering::SeqCst);
}

pub fn init() {
    enable_nx();
    PAT_AVAILABLE.store(cpu::has_pat(), Ordering::SeqCst);
    let info = boot::info();
    let _guard = LOCK.lock();
    let pml4 = new_table().expect("out of memory for page tables");

    // Pre-create all upper-half PDPTs so the kernel half of every future
    // address space can share them by copying the top-level entries.
    for i in 256..512 {
        next_level(&mut table(pml4)[i]).expect("out of memory for page tables");
    }

    // Higher-half direct map.
    for r in info.regions() {
        let kind = match r.kind {
            MemoryKind::Reserved | MemoryKind::BadMemory => continue,
            MemoryKind::Framebuffer => Kind::Framebuffer,
            _ => Kind::Data,
        };
        let start = r.base & !(PAGE_SIZE - 1);
        let end = r.end().next_multiple_of(PAGE_SIZE);
        let huge = kind == Kind::Data;
        map_range_in(
            pml4,
            info.hhdm_offset + start,
            start,
            end - start,
            leaf_flags(kind),
            huge,
        )
        .expect("out of memory while building the HHDM");
    }

    // Kernel image with per-section permissions.
    let s = kernel_sections();
    let to_phys = |v: u64| v - info.kernel_virt + info.kernel_phys;
    let sections = [
        (s.start, s.text.0, Kind::Data), // Limine request block
        (s.text.0, s.text.1, Kind::Code),
        (s.rodata.0, s.rodata.1, Kind::ReadOnly),
        (s.data.0, s.data.1, Kind::Data),
    ];
    for (from, to, kind) in sections {
        let from = from & !(PAGE_SIZE - 1);
        let to = to.next_multiple_of(PAGE_SIZE);
        let mut v = from;
        while v < to {
            map_4k_in(pml4, v, to_phys(v), leaf_flags(kind)).expect("kernel image mapping failed");
            v += PAGE_SIZE;
        }
    }

    // SAFETY: the new tables map the kernel image, the HHDM (which holds
    // the current boot stack and the Limine structures) and the framebuffer.
    unsafe { cpu::write_cr3(pml4) };
    KERNEL_PML4.store(pml4, Ordering::SeqCst);
}

fn kernel_pml4() -> u64 {
    let p = KERNEL_PML4.load(Ordering::SeqCst);
    assert!(p != 0, "kernel page tables not initialised");
    p
}

/// Maps one 4 KiB page in the kernel address space.
pub fn map(virt: u64, phys: u64, kind: Kind) -> Result<(), MapError> {
    let _guard = LOCK.lock();
    map_4k_in(kernel_pml4(), virt, phys, leaf_flags(kind))?;
    cpu::invlpg(virt);
    Ok(())
}

/// Unmaps one 4 KiB page and returns the physical address it mapped.
/// The frame itself is not freed.
pub fn unmap(virt: u64) -> Option<u64> {
    let _guard = LOCK.lock();
    let [i4, i3, i2, i1] = indices(virt);
    let e4 = table(kernel_pml4())[i4];
    if e4 & PRESENT == 0 {
        return None;
    }
    let e3 = table(e4 & ADDR_MASK)[i3];
    if e3 & PRESENT == 0 || e3 & HUGE != 0 {
        return None;
    }
    let e2 = table(e3 & ADDR_MASK)[i2];
    if e2 & PRESENT == 0 || e2 & HUGE != 0 {
        return None;
    }
    let e = &mut table(e2 & ADDR_MASK)[i1];
    if *e & PRESENT == 0 {
        return None;
    }
    let phys = *e & ADDR_MASK;
    *e = 0;
    cpu::invlpg(virt);
    Some(phys)
}

/// Physical address and leaf flags of a kernel virtual address.
pub fn translate(virt: u64) -> Option<(u64, u64)> {
    let _guard = LOCK.lock();
    translate_in(kernel_pml4(), virt)
}

pub fn nx_enabled() -> bool {
    NX_ENABLED.load(Ordering::Relaxed)
}

/// Physical address of the kernel's top-level table.
pub fn kernel_root() -> u64 {
    kernel_pml4()
}

// --- user address spaces ----------------------------------------------------

/// Available-to-software PTE bit: the frame is not owned by this address
/// space and must not be freed with it (shared mappings such as a future
/// framebuffer mapping for Lumi).
pub const BORROWED: u64 = 1 << 9;

const USER_HALF_END: u64 = 0x0000_8000_0000_0000;

/// Permissions of a user page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserKind {
    /// Readable and executable, not writable.
    Code,
    /// Read-only, not executable.
    ReadOnly,
    /// Readable and writable, not executable.
    Data,
}

fn user_flags(kind: UserKind) -> u64 {
    let base = PRESENT | USER;
    match kind {
        UserKind::Code => base,
        UserKind::ReadOnly => base | nx(),
        UserKind::Data => base | WRITABLE | nx(),
    }
}

/// A process address space: its own lower half, the shared kernel half.
pub struct AddressSpace {
    pml4: u64,
}

impl AddressSpace {
    /// Creates an empty user address space. The kernel half points at the
    /// same PDPTs as the kernel tables, so kernel mappings made later are
    /// visible in every process.
    pub fn new() -> Result<Self, MapError> {
        let _guard = LOCK.lock();
        let pml4 = new_table()?;
        let kernel = table(kernel_pml4());
        let t = table(pml4);
        t[256..].copy_from_slice(&kernel[256..]);
        Ok(Self { pml4 })
    }

    pub fn root(&self) -> u64 {
        self.pml4
    }

    /// Maps one user page. `virt` must be in the user range.
    pub fn map(&self, virt: u64, phys: u64, kind: UserKind) -> Result<(), MapError> {
        assert!(virt < USER_HALF_END, "user mapping in the kernel half");
        let _guard = LOCK.lock();
        map_4k_in(self.pml4, virt, phys, user_flags(kind))
    }

    /// Allocates zeroed frames and maps `pages` pages starting at `virt`.
    pub fn map_new(&self, virt: u64, pages: u64, kind: UserKind) -> Result<(), MapError> {
        for i in 0..pages {
            let phys = frame::alloc_zeroed().ok_or(MapError::OutOfMemory)?;
            if let Err(e) = self.map(virt + i * PAGE_SIZE, phys, kind) {
                frame::free(phys);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Physical address and flags for a user address.
    pub fn translate(&self, virt: u64) -> Option<(u64, u64)> {
        let _guard = LOCK.lock();
        translate_in(self.pml4, virt)
    }

    /// True if `[addr, addr + len)` is mapped for user access (and
    /// writable, if `write`). Used to validate system call pointers.
    pub fn check_user_range(&self, addr: u64, len: u64, write: bool) -> bool {
        if len == 0 {
            return true;
        }
        let Some(end) = addr.checked_add(len) else {
            return false;
        };
        if addr < velora_abi::USER_MIN || end > velora_abi::USER_MAX {
            return false;
        }
        let _guard = LOCK.lock();
        let mut page = addr & !(PAGE_SIZE - 1);
        while page < end {
            match translate_in(self.pml4, page) {
                Some((_, f)) if f & USER != 0 && (!write || f & WRITABLE != 0) => {}
                _ => return false,
            }
            page += PAGE_SIZE;
        }
        true
    }

    /// Makes this address space the active one.
    ///
    /// # Safety
    /// Interrupts disabled. The kernel half is shared, so kernel code and
    /// stacks stay mapped.
    pub unsafe fn activate(&self) {
        unsafe { cpu::write_cr3(self.pml4) }
    }

    /// Number of user pages mapped (for diagnostics and tests).
    pub fn user_pages(&self) -> u64 {
        let _guard = LOCK.lock();
        let mut n = 0;
        walk_user(self.pml4, &mut |_| n += 1);
        n
    }
}

/// Calls `f(leaf)` for every present 4 KiB leaf in the lower half.
fn walk_user(pml4: u64, f: &mut impl FnMut(u64)) {
    for i4 in 0..256 {
        let e4 = table(pml4)[i4];
        if e4 & PRESENT == 0 {
            continue;
        }
        for i3 in 0..512 {
            let e3 = table(e4 & ADDR_MASK)[i3];
            if e3 & PRESENT == 0 || e3 & HUGE != 0 {
                continue;
            }
            for i2 in 0..512 {
                let e2 = table(e3 & ADDR_MASK)[i2];
                if e2 & PRESENT == 0 || e2 & HUGE != 0 {
                    continue;
                }
                for &e in table(e2 & ADDR_MASK).iter() {
                    if e & PRESENT != 0 {
                        f(e);
                    }
                }
            }
        }
    }
}

fn free_table(phys: u64) {
    frame::free(phys);
    TABLE_FRAMES.fetch_sub(1, Ordering::Relaxed);
}

impl Drop for AddressSpace {
    /// Frees every owned user frame and every lower-half page table.
    /// The address space must not be active.
    fn drop(&mut self) {
        let _guard = LOCK.lock();
        assert_ne!(
            cpu::read_cr3() & ADDR_MASK,
            self.pml4,
            "dropping the active address space"
        );
        walk_user(self.pml4, &mut |leaf| {
            if leaf & BORROWED == 0 {
                frame::free(leaf & ADDR_MASK);
            }
        });
        for i4 in 0..256 {
            let e4 = table(self.pml4)[i4];
            if e4 & PRESENT == 0 {
                continue;
            }
            for i3 in 0..512 {
                let e3 = table(e4 & ADDR_MASK)[i3];
                if e3 & PRESENT == 0 || e3 & HUGE != 0 {
                    continue;
                }
                for i2 in 0..512 {
                    let e2 = table(e3 & ADDR_MASK)[i2];
                    if e2 & PRESENT != 0 && e2 & HUGE == 0 {
                        free_table(e2 & ADDR_MASK);
                    }
                }
                free_table(e3 & ADDR_MASK);
            }
            free_table(e4 & ADDR_MASK);
        }
        free_table(self.pml4);
    }
}

/// Switches to the kernel tables (before freeing a process's tables).
///
/// # Safety
/// Interrupts disabled.
pub unsafe fn activate_kernel() {
    unsafe { cpu::write_cr3(kernel_pml4()) }
}

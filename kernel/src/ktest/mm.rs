//! Stage 2 tests: frames, page tables, heap, stacks, console back buffer.

use super::TEST_FB_SIZE;
use crate::arch::x86_64::exceptions;
use crate::boot;
use crate::console::framebuffer::Framebuffer;
use crate::mm::{self, PAGE_SIZE, frame, heap, kstack, paging};
use crate::sync::RacyCell;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;

const MIB: u64 = 1024 * 1024;
/// Unused virtual address in the MMIO window, for map/unmap tests.
const TEST_VA: u64 = 0xffff_d0ff_ff00_0000;

/// Reads `addr` and expects a page fault. Returns (error code, CR2).
fn expect_read_fault(addr: u64) -> (u64, u64) {
    exceptions::arm_recovery(exceptions::PAGE_FAULT, 0);
    // SAFETY: the read faults and the recovery hook resumes at label 77.
    unsafe {
        asm!(
            "lea {tmp}, [rip + 77f]",
            "mov qword ptr [{slot}], {tmp}",
            "mov {tmp}, qword ptr [{addr}]",
            "77:",
            slot = in(reg) exceptions::recover_rip_slot(),
            addr = in(reg) addr,
            tmp = out(reg) _,
        );
    }
    let (v, err, cr2) = exceptions::take_caught().expect("no page fault");
    assert_eq!(v, exceptions::PAGE_FAULT);
    (err, cr2)
}

/// Writes one byte to `addr` and expects a page fault.
fn expect_write_fault(addr: u64) -> (u64, u64) {
    exceptions::arm_recovery(exceptions::PAGE_FAULT, 0);
    // SAFETY: the write faults (read-only page) and execution resumes at
    // label 77, so nothing is modified.
    unsafe {
        asm!(
            "lea {tmp}, [rip + 77f]",
            "mov qword ptr [{slot}], {tmp}",
            "mov byte ptr [{addr}], 0x90",
            "77:",
            slot = in(reg) exceptions::recover_rip_slot(),
            addr = in(reg) addr,
            tmp = out(reg) _,
        );
    }
    let (v, err, cr2) = exceptions::take_caught().expect("no page fault");
    assert_eq!(v, exceptions::PAGE_FAULT);
    (err, cr2)
}

const PF_PRESENT: u64 = 1;
const PF_WRITE: u64 = 2;
const PF_FETCH: u64 = 16;

pub fn frames_alloc_and_free() {
    // Reserve first: growing the Vec would take frames for the heap.
    let mut got: Vec<u64> = Vec::with_capacity(100);
    let before = frame::stats().free_frames;
    got.extend((0..100).map(|_| frame::alloc().expect("out of frames")));
    assert!(got.iter().all(|&p| p != 0 && p % PAGE_SIZE == 0));
    assert_eq!(frame::stats().free_frames, before - 100);
    got.sort_unstable();
    got.dedup();
    assert_eq!(got.len(), 100, "a frame was handed out twice");
    for &p in &got {
        frame::free(p);
    }
    assert_eq!(frame::stats().free_frames, before);
    assert!(frame::try_free(got[0]).is_err(), "double free not detected");
}

pub fn frames_contiguous_and_aligned() {
    let p = frame::alloc_contiguous(16, 16).expect("no contiguous frames");
    assert_eq!(p % (16 * PAGE_SIZE), 0);
    for i in 0..16 {
        frame::free(p + i * PAGE_SIZE);
    }
}

pub fn frames_reachable_through_hhdm() {
    let p = frame::alloc_zeroed().unwrap();
    let v = mm::phys_to_virt(p) as *mut u64;
    // SAFETY: freshly allocated frame, mapped in the HHDM.
    unsafe {
        assert_eq!(v.read(), 0);
        for i in 0..512 {
            v.add(i).write(i as u64 * 0x0101_0101);
        }
        assert_eq!(v.add(511).read(), 511 * 0x0101_0101);
    }
    assert_eq!(paging::translate(v as u64).unwrap().0, p);
    frame::free(p);
}

static RODATA_VALUE: [u8; 16] = *b"velora rodata 16";
static DATA_VALUE: RacyCell<u64> = RacyCell::new(7);

pub fn kernel_sections_have_wx_permissions() {
    let info = boot::info();
    let code = kernel_sections_have_wx_permissions as *const () as u64;
    let (phys, flags) = paging::translate(code).expect("code not mapped");
    assert_eq!(phys, code - info.kernel_virt + info.kernel_phys);
    assert_eq!(flags & paging::WRITABLE, 0, ".text is writable");
    assert_eq!(flags & paging::NO_EXECUTE, 0, ".text is not executable");

    let ro = RODATA_VALUE.as_ptr() as u64;
    let (_, flags) = paging::translate(ro).expect("rodata not mapped");
    assert_eq!(flags & paging::WRITABLE, 0, ".rodata is writable");

    let data = DATA_VALUE.get() as u64;
    let (_, flags) = paging::translate(data).expect("data not mapped");
    assert_ne!(flags & paging::WRITABLE, 0, ".data is read-only");
    if paging::nx_enabled() {
        assert_ne!(flags & paging::NO_EXECUTE, 0, ".data is executable");
        let (_, flags) = paging::translate(ro).unwrap();
        assert_ne!(flags & paging::NO_EXECUTE, 0, ".rodata is executable");
    }
}

pub fn hhdm_maps_usable_memory() {
    let info = boot::info();
    for r in info
        .regions()
        .iter()
        .filter(|r| r.kind == boot::limine::MemoryKind::Usable)
    {
        for phys in [r.base, r.end() - PAGE_SIZE] {
            let (p, flags) = paging::translate(info.hhdm_offset + phys).expect("HHDM gap");
            assert_eq!(p, phys);
            assert_ne!(flags & paging::WRITABLE, 0);
        }
    }
    // Nothing is mapped in the lower half.
    assert!(paging::translate(0x1000).is_none());
    assert!(paging::translate(0x0000_7fff_ffff_f000).is_none());
}

pub fn write_to_text_faults() {
    let target = write_to_text_faults as *const () as u64;
    let (err, cr2) = expect_write_fault(target);
    assert_eq!(cr2, target);
    assert_eq!(err & (PF_PRESENT | PF_WRITE), PF_PRESENT | PF_WRITE);
}

pub fn write_to_rodata_faults() {
    let target = RODATA_VALUE.as_ptr() as u64;
    let (err, _) = expect_write_fault(target);
    assert_eq!(err & (PF_PRESENT | PF_WRITE), PF_PRESENT | PF_WRITE);
    assert_eq!(&RODATA_VALUE, b"velora rodata 16");
}

static CODE_IN_DATA: RacyCell<[u8; 16]> = RacyCell::new([0; 16]);

pub fn execute_from_data_faults() {
    if !paging::nx_enabled() {
        crate::kprint!("(skipped: no NX) ");
        return;
    }
    let buf = CODE_IN_DATA.get();
    // SAFETY: writes a `ret` into a static buffer that is never executed.
    unsafe { (*buf)[0] = 0xc3 };
    exceptions::arm_recovery(exceptions::PAGE_FAULT, 0);
    // SAFETY: jumping to an NX page faults on the fetch; the recovery hook
    // resumes at label 77 with the stack unchanged (jmp, not call).
    unsafe {
        asm!(
            "lea {tmp}, [rip + 77f]",
            "mov qword ptr [{slot}], {tmp}",
            "jmp {target}",
            "77:",
            slot = in(reg) exceptions::recover_rip_slot(),
            target = in(reg) buf as u64,
            tmp = out(reg) _,
        );
    }
    let (v, err, cr2) = exceptions::take_caught().expect("no fault on NX page");
    assert_eq!(v, exceptions::PAGE_FAULT);
    assert_eq!(cr2, buf as u64);
    assert_eq!(err & (PF_PRESENT | PF_FETCH), PF_PRESENT | PF_FETCH);
}

pub fn null_page_is_unmapped() {
    let (err, cr2) = expect_read_fault(0);
    assert_eq!(cr2, 0);
    assert_eq!(err & PF_PRESENT, 0);
}

pub fn map_and_unmap_page() {
    let phys = frame::alloc_zeroed().unwrap();
    paging::map(TEST_VA, phys, paging::Kind::Data).unwrap();
    assert_eq!(
        paging::map(TEST_VA, phys, paging::Kind::Data),
        Err(paging::MapError::AlreadyMapped)
    );
    // SAFETY: TEST_VA is mapped to `phys` right above.
    unsafe { (TEST_VA as *mut u64).write_volatile(0xfeed_beef) };
    // SAFETY: the same frame through the HHDM.
    let via_hhdm = unsafe { (mm::phys_to_virt(phys) as *const u64).read_volatile() };
    assert_eq!(via_hhdm, 0xfeed_beef);
    assert_eq!(paging::unmap(TEST_VA), Some(phys));
    assert_eq!(paging::unmap(TEST_VA), None);
    let (err, cr2) = expect_read_fault(TEST_VA);
    assert_eq!(cr2, TEST_VA);
    assert_eq!(err & PF_PRESENT, 0);
    frame::free(phys);
}

pub fn heap_collections_work() {
    let b = Box::new([7u64; 64]);
    assert_eq!(b.iter().sum::<u64>(), 7 * 64);
    let mut v: Vec<u32> = Vec::new();
    for i in 0..10_000 {
        v.push(i);
    }
    assert_eq!(v.iter().map(|&x| x as u64).sum::<u64>(), 49_995_000);
    let mut s = String::new();
    for i in 0..100 {
        s.push_str(&alloc::format!("{i},"));
    }
    assert!(s.starts_with("0,1,2,") && s.ends_with("98,99,"));
    let mut m = BTreeMap::new();
    for i in 0..500u32 {
        m.insert(i.wrapping_mul(2_654_435_761), i);
    }
    assert_eq!(m.len(), 500);
}

pub fn heap_reuses_freed_pages() {
    let v = alloc::vec![0xabu8; 4 * MIB as usize];
    assert!(v.iter().all(|&b| b == 0xab));
    let mapped = heap::stats().pages_mapped;
    drop(v);
    let w = alloc::vec![0u8; 4 * MIB as usize];
    assert_eq!(
        heap::stats().pages_mapped,
        mapped,
        "freed pages were not reused"
    );
    drop(w);
}

pub fn heap_stress() {
    let start = heap::stats();
    let mut live: Vec<(Vec<u8>, u8)> = Vec::new();
    let mut seed = 0x9e37_79b9u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for step in 0..3000u32 {
        if live.len() > 200 || (rnd() % 3 == 0 && !live.is_empty()) {
            let i = rnd() as usize % live.len();
            let (buf, v) = live.swap_remove(i);
            assert!(
                buf.iter().all(|&b| b == v),
                "heap corruption at step {}",
                step
            );
        } else {
            let size = if rnd() % 8 == 0 {
                4096 + rnd() as usize % 30_000
            } else {
                1 + rnd() as usize % 700
            };
            let v = (step % 251) as u8;
            live.push((alloc::vec![v; size], v));
        }
    }
    drop(live);
    assert_eq!(heap::stats().bytes_in_use, start.bytes_in_use, "heap leak");
}

pub fn kernel_stack_guard_page_faults() {
    let s = kstack::alloc(2).expect("no stack");
    assert_eq!(s.top - s.bottom, 2 * PAGE_SIZE);
    assert_eq!(s.bottom - s.guard, PAGE_SIZE);
    // SAFETY: the top word of the new stack is mapped.
    unsafe { ((s.top - 8) as *mut u64).write_volatile(1) };
    let (err, cr2) = expect_read_fault(s.guard + PAGE_SIZE - 8);
    assert_eq!(cr2, s.guard + PAGE_SIZE - 8);
    assert_eq!(err & PF_PRESENT, 0);
}

pub fn running_on_kernel_stack() {
    let rsp = crate::arch::cpu::read_rsp();
    assert!(
        (kstack::STACKS_START..kstack::STACKS_END).contains(&rsp),
        "still on the bootloader stack: {:#x}",
        rsp
    );
}

pub fn bootloader_memory_released() {
    let u = mm::usage();
    assert_eq!(u.bootloader, 0, "bootloader memory not released");
    let m = boot::memory_summary();
    assert!(u.managed >= (m.usable + m.reclaimable) * 9 / 10);
    // The captured boot info survives the release.
    assert!(boot::cmdline_has("velora.test"));
}

static TEST_FB: RacyCell<[u32; TEST_FB_SIZE]> = RacyCell::new([0; TEST_FB_SIZE]);

pub fn back_buffer_scrolls_like_screen() {
    let (w, h) = (super::TW, super::TH);
    // SAFETY: test buffer is static and used by one test at a time.
    let mut fb = unsafe { Framebuffer::from_raw(TEST_FB.get().cast(), w, h, w * 4) };
    for y in 0..h {
        for x in 0..w {
            fb.put_native(x, y, (y * 1000 + x) as u32);
        }
    }
    fb.enable_back_buffer();
    assert!(fb.has_back_buffer());
    fb.scroll_up(16, 0);
    // SAFETY: reading the "screen" memory directly.
    let screen = unsafe { &*TEST_FB.get() };
    for y in 0..h {
        for x in 0..w {
            let expect = if y + 16 < h {
                ((y + 16) * 1000 + x) as u32
            } else {
                0
            };
            assert_eq!(fb.get_native(x, y), Some(expect), "back buffer wrong");
            assert_eq!(screen[y * w + x], expect, "screen wrong");
        }
    }
}

pub fn idle_memory_below_64_mib() {
    let u = mm::usage();
    crate::kprint!("({} KiB in use) ", u.in_use / 1024);
    assert!(u.in_use < 64 * MIB, "kernel uses {} KiB", u.in_use / 1024);
}

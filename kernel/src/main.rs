//! Velora OS kernel.

#![no_std]
#![no_main]
// Some interfaces (user selectors, MMIO mappings) exist for later stages.
#![allow(dead_code)]

extern crate alloc;

mod arch;
mod boot;
mod console;
#[cfg(feature = "ktest")]
mod ktest;
mod mm;
mod panic;
mod sync;

use arch::x86_64::serial;
use boot::limine;
use core::sync::atomic::{AtomicU64, Ordering};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Size of the kernel's main stack (the boot stack belongs to Limine).
const MAIN_STACK_PAGES: u64 = 16;

static BOOT_TSC: AtomicU64 = AtomicU64::new(0);
static SCREEN: AtomicU64 = AtomicU64::new(0);

/// Entry point, jumped to by Limine with interrupts disabled and a 64 KiB
/// stack in bootloader-reclaimable memory.
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    BOOT_TSC.store(arch::cpu::rdtsc(), Ordering::Relaxed);
    serial::init();

    if !limine::BASE_REVISION.is_supported() {
        // Without a supported base revision the memory layout is not what
        // the kernel expects, so stop before touching anything else.
        kprintln!(
            "velora: bootloader does not support Limine base revision {} (loaded {:?})",
            limine::REQUESTED_BASE_REVISION,
            limine::BASE_REVISION.loaded_revision()
        );
        arch::fatal_exit();
    }

    if let Some((w, h)) = console::init() {
        SCREEN.store(((w as u64) << 32) | h as u64, Ordering::Relaxed);
    }

    kprintln!();
    kprintln!("\x1b[1;35mVelora OS\x1b[0m {} (x86_64)", VERSION);
    kprintln!();

    arch::init();
    kok!("GDT, TSS and IDT loaded");

    if let Err(e) = boot::capture() {
        panic!("boot information incomplete: {}", e);
    }
    mm::init();
    kok!(
        "memory manager: own page tables (NX {}), frame allocator, kernel heap",
        if mm::paging::nx_enabled() {
            "on"
        } else {
            "unavailable"
        }
    );
    if console::enable_back_buffer() {
        kok!("console back buffer enabled");
    }

    let stack = mm::kstack::alloc(MAIN_STACK_PAGES).expect("no memory for the kernel stack");
    // SAFETY: nothing on the boot stack is needed after this point.
    unsafe { mm::kstack::switch_to(&stack, kmain_on_kernel_stack) }
}

/// Continues boot on the kernel's own stack, so the boot stack (bootloader
/// memory) can be released.
extern "C" fn kmain_on_kernel_stack() -> ! {
    let reclaimed = boot::release_bootloader_memory();
    kok!("released {} KiB of bootloader memory", reclaimed / 1024);

    report_boot_info();

    #[cfg(feature = "ktest")]
    ktest::run(BOOT_TSC.load(Ordering::Relaxed));

    #[cfg(not(feature = "ktest"))]
    idle()
}

#[cfg(not(feature = "ktest"))]
fn idle() -> ! {
    let cycles = arch::cpu::rdtsc().wrapping_sub(BOOT_TSC.load(Ordering::Relaxed));
    kok!(
        "boot complete ({} M TSC cycles since kernel entry)",
        cycles / 1_000_000
    );
    kprintln!("Stage 2 kernel: nothing else to run yet. System idle.");
    // Interrupts are not enabled until stage 3, so this halts for good.
    arch::cpu::halt_forever();
}

fn report_boot_info() {
    let info = boot::info();
    kinfo!("bootloader: {}", info.bootloader());
    match SCREEN.load(Ordering::Relaxed) {
        0 => kwarn!("no usable framebuffer, serial console only"),
        s => kok!("framebuffer console {}x{}", s >> 32, s & 0xffff_ffff),
    }
    if serial::is_present() {
        kok!("serial console on COM1");
    }
    let mut brand = [0u8; 48];
    kinfo!("cpu: {}", arch::cpu::brand_string(&mut brand));
    let m = boot::memory_summary();
    kinfo!(
        "memory map: {} MiB usable, {} MiB bootloader-reclaimable, {} regions",
        m.usable / (1024 * 1024),
        m.reclaimable / (1024 * 1024),
        m.regions
    );
    if info.regions_dropped > 0 {
        kwarn!(
            "{} memory map entries did not fit and were ignored",
            info.regions_dropped
        );
    }
    kinfo!("higher-half direct map at {:#018x}", info.hhdm_offset);
    kinfo!(
        "kernel at phys {:#x}, virt {:#x}",
        info.kernel_phys,
        info.kernel_virt
    );
    let u = mm::usage();
    let kib = |b: u64| b / 1024;
    kinfo!(
        "memory in use: {} KiB of {} KiB (kernel image {}, page tables {}, heap {}, stacks {}, frame bitmap {} KiB)",
        kib(u.in_use),
        kib(u.managed),
        kib(u.kernel_image),
        kib(u.page_tables),
        kib(u.heap),
        kib(u.stacks),
        kib(u.frame_bitmap)
    );
    if !info.cmdline().is_empty() {
        kinfo!("cmdline: {}", info.cmdline());
    }
}

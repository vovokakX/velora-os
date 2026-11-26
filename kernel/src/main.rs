//! Velora OS kernel.

#![no_std]
#![no_main]
// Some interfaces (user selectors, memory kinds) exist for later stages.
#![allow(dead_code)]

mod arch;
mod boot;
mod console;
#[cfg(feature = "ktest")]
mod ktest;
mod panic;
mod sync;

use arch::x86_64::serial;
use boot::limine;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Entry point, jumped to by Limine with interrupts disabled and a 64 KiB
/// stack in bootloader-reclaimable memory.
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    let tsc_start = arch::cpu::rdtsc();
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

    let screen = console::init();

    kprintln!();
    kprintln!("\x1b[1;35mVelora OS\x1b[0m {} (x86_64)", VERSION);
    kprintln!();

    arch::init();
    kok!("GDT, TSS and IDT loaded");

    report_boot_info(screen);

    #[cfg(feature = "ktest")]
    ktest::run(tsc_start);

    #[cfg(not(feature = "ktest"))]
    idle(tsc_start)
}

#[cfg(not(feature = "ktest"))]
fn idle(tsc_start: u64) -> ! {
    let cycles = arch::cpu::rdtsc().wrapping_sub(tsc_start);
    kok!(
        "boot complete ({} M TSC cycles since kernel entry)",
        cycles / 1_000_000
    );
    kprintln!("Stage 1 kernel: nothing else to run yet. System idle.");
    // Interrupts are not enabled until stage 3, so this halts for good.
    arch::cpu::halt_forever();
}

fn report_boot_info(screen: Option<(usize, usize)>) {
    if let Some(bl) = limine::BOOTLOADER_INFO.response() {
        kinfo!("bootloader: {} {}", bl.name(), bl.version());
    }
    match screen {
        Some((w, h)) => kok!("framebuffer console {}x{}", w, h),
        None => kwarn!("no usable framebuffer, serial console only"),
    }
    if serial::is_present() {
        kok!("serial console on COM1");
    }
    let mut brand = [0u8; 48];
    kinfo!("cpu: {}", arch::cpu::brand_string(&mut brand));
    if let Some(m) = boot::memory_summary() {
        kinfo!(
            "memory: {} MiB usable, {} MiB reclaimable, {} regions",
            m.usable / (1024 * 1024),
            m.reclaimable / (1024 * 1024),
            m.regions
        );
    } else {
        kwarn!("bootloader did not provide a memory map");
    }
    if let Some(h) = limine::HHDM.response() {
        kinfo!("higher-half direct map at {:#018x}", h.offset);
    }
    if let Some(k) = limine::EXECUTABLE_ADDRESS.response() {
        kinfo!(
            "kernel at phys {:#x}, virt {:#x}",
            k.physical_base,
            k.virtual_base
        );
    }
    let cmdline = boot::cmdline();
    if !cmdline.is_empty() {
        kinfo!("cmdline: {}", cmdline);
    }
}

//! Velora OS kernel.

#![no_std]
#![no_main]
// Some interfaces (user selectors, MMIO mappings) exist for later stages.
#![allow(dead_code)]

extern crate alloc;

mod acpi;
mod arch;
mod boot;
mod console;
mod drivers;
mod exec;
mod fs;
mod initramfs;
mod input;
#[cfg(not(feature = "ktest"))]
mod kbdtest;
#[cfg(feature = "ktest")]
mod ktest;
mod mm;
mod panic;
mod sched;
mod sync;
mod syscall;
mod time;
#[cfg(feature = "ktest")]
mod userprog;

use arch::x86_64::{apic, ioapic, pic, serial};
use boot::limine;
use core::sync::atomic::{AtomicU64, Ordering};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Size of the kernel's main stack (the boot stack belongs to Limine).
const MAIN_STACK_PAGES: u64 = 16;

static SCREEN: AtomicU64 = AtomicU64::new(0);
static MAIN_STACK: sync::RacyCell<Option<mm::kstack::KernelStack>> = sync::RacyCell::new(None);

/// Entry point, jumped to by Limine with interrupts disabled and a 64 KiB
/// stack in bootloader-reclaimable memory.
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    time::mark_boot();
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
    kok!("GDT, TSS, IDT, syscall entry and SSE state set up");

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
    // SAFETY: written once here, read once on the new stack.
    unsafe { *MAIN_STACK.get() = Some(stack) };
    // SAFETY: nothing on the boot stack is needed after this point.
    unsafe { mm::kstack::switch_to(&stack, kmain_on_kernel_stack) }
}

/// Continues boot on the kernel's own stack, so the boot stack (bootloader
/// memory) can be released.
extern "C" fn kmain_on_kernel_stack() -> ! {
    let reclaimed = boot::release_bootloader_memory();
    kok!("released {} KiB of bootloader memory", reclaimed / 1024);

    report_boot_info();
    init_interrupts();
    // SAFETY: set in `kmain` before the switch; nothing else touches it.
    let main_stack = unsafe { (*MAIN_STACK.get()).take() }.expect("main stack not recorded");
    sched::init(main_stack);
    match initramfs::init() {
        Ok((files, size)) => kok!("initramfs: {} files, {} KiB", files, size / 1024),
        Err(e) => kwarn!("initramfs: {}", e),
    }
    drivers::init();
    fs::init();
    fs::automount();
    arch::cpu::enable_interrupts();
    kok!(
        "scheduler running (preemptive, {} Hz time slices)",
        time::TICK_HZ
    );

    #[cfg(feature = "ktest")]
    ktest::run();

    #[cfg(not(feature = "ktest"))]
    {
        run_first_user_program();
        kok!("boot complete in {} ms", time::uptime_ms());
        kbdtest::run()
    }
}

/// Runs /bin/hello from the initramfs to show that programs load from
/// files and run in user mode.
#[cfg(not(feature = "ktest"))]
fn run_first_user_program() {
    let argv = ["/bin/hello", "from", "initramfs"];
    let envp = ["PATH=/bin", "HOME=/"];
    match exec::spawn("/bin/hello", &argv, &envp) {
        Ok(pid) => match sched::wait_pid(pid) {
            Some(0) => kok!(
                "user mode works: /bin/hello (pid {}) exited with status 0",
                pid
            ),
            other => kwarn!("/bin/hello (pid {}) ended with {:?}", pid, other),
        },
        Err(e) => kwarn!("cannot start /bin/hello: {:?}", e),
    }
}

/// ACPI, interrupt controllers, timer and keyboard. Interrupts stay off;
/// the caller enables them once the scheduler exists.
fn init_interrupts() {
    let acpi = acpi::init().unwrap_or_else(|e| panic!("ACPI: {}", e));
    let madt = &acpi.madt;
    kok!(
        "ACPI {} tables (rev {}), {} CPU(s), {} I/O APIC(s)",
        acpi.tables.len(),
        acpi.rsdp.revision,
        madt.lapics.iter().filter(|l| l.enabled).count(),
        madt.ioapics.len()
    );
    pic::disable();
    let nmis: alloc::vec::Vec<apic::LintNmi> = madt
        .nmis
        .iter()
        .map(|n| apic::LintNmi {
            lint: n.lint,
            active_low: n.polarity == velora_acpi::Polarity::ActiveLow,
            level: n.trigger == velora_acpi::Trigger::Level,
        })
        .collect();
    apic::init(madt.lapic_address, &nmis).unwrap_or_else(|e| panic!("local APIC: {}", e));
    ioapic::init(madt).unwrap_or_else(|e| panic!("I/O APIC: {}", e));
    kok!(
        "local APIC {} (id {}, {} mode), legacy PIC masked",
        apic::version(),
        apic::id(),
        if apic::is_x2apic() { "x2APIC" } else { "xAPIC" }
    );
    match time::init() {
        Ok((tsc_hz, per_ms)) => kok!(
            "timer {} Hz; TSC {}.{:02} GHz, APIC timer {} kHz",
            time::TICK_HZ,
            tsc_hz / 1_000_000_000,
            tsc_hz / 10_000_000 % 100,
            per_ms
        ),
        Err(e) => panic!("timer calibration: {}", e),
    }
    match drivers::ps2::init(madt) {
        Ok(()) => kok!("PS/2 keyboard (layouts EN, RU; Alt+Shift switches)"),
        Err(e) => kwarn!("PS/2 keyboard: {}", e),
    }
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
        "memory in use: {} KiB of {} KiB (kernel image {}, initramfs {}, page tables {}, heap {}, stacks {}, frame bitmap {} KiB)",
        kib(u.in_use),
        kib(u.managed),
        kib(u.kernel_image),
        kib(u.modules),
        kib(u.page_tables),
        kib(u.heap),
        kib(u.stacks),
        kib(u.frame_bitmap)
    );
    if !info.cmdline().is_empty() {
        kinfo!("cmdline: {}", info.cmdline());
    }
}

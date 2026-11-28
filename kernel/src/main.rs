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
mod crashlog;
mod display;
mod drivers;
mod exec;
mod fs;
mod initramfs;
mod input;
#[cfg(feature = "ktest")]
mod ktest;
mod mm;
mod net;
mod panic;
mod power;
mod pty;
mod sched;
mod smp;
mod sync;
mod syscall;
mod time;
mod tty;
#[cfg(feature = "ktest")]
mod userprog;

use arch::x86_64::{apic, hpet, ioapic, pic, pmtimer, serial};
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
    // The other CPUs must leave the bootloader's memory before it is freed.
    let parked = smp::start_aps();
    let reclaimed = boot::release_bootloader_memory();
    kok!("released {} KiB of bootloader memory", reclaimed / 1024);

    report_boot_info();
    init_interrupts();
    // SAFETY: set in `kmain` before the switch; nothing else touches it.
    let main_stack = unsafe { (*MAIN_STACK.get()).take() }.expect("main stack not recorded");
    sched::init(main_stack);
    match initramfs::init() {
        Ok((files, size)) => kok!("initramfs: {} files, {} KiB", files, size / 1024),
        Err(e) => kinfo!("initramfs: {}", e),
    }
    drivers::init();
    drivers::xhci::start_thread();
    tty::init();
    match power::init(&acpi::get().madt) {
        Ok(what) => kok!("power management: {}", what),
        Err(e) => kwarn!("power management: {}", e),
    }
    let online = smp::release_aps();
    if online == parked + 1 && online == smp::reported() {
        kok!("{} CPU(s) online", online);
    } else {
        kwarn!(
            "{} of {} CPU(s) online ({} started)",
            online,
            smp::reported(),
            parked + 1
        );
    }
    fs::init();
    fs::automount();
    net::init();
    arch::cpu::enable_interrupts();
    kok!(
        "scheduler running (preemptive, {} Hz time slices)",
        time::TICK_HZ
    );

    #[cfg(feature = "ktest")]
    ktest::run();

    #[cfg(not(feature = "ktest"))]
    {
        time::mark_boot_done();
        kok!("boot complete in {} ms", time::uptime_ms());
        run_sessions()
    }
}

/// The Live system first shows the start menu (`/bin/velora-start`:
/// graphical installation, installation in the terminal or the Live
/// system). Then the graphical shell `/bin/lumi` runs (on an installed
/// system always, on the Live system when the menu chose the desktop),
/// again each time its user logs out. When Lumi cannot run (no screen,
/// no fonts, `session=console` in desktop.conf: it ends with another
/// status) or `velora.text` is on the command line, `/bin/login` runs on
/// the console instead, again each time a session ends. `velora.nomenu`
/// skips the menu; `velora.uitest` (the menu reports its widgets for
/// automatic tests) and `velora.noanim` (no animations) are passed to it.
#[cfg(not(feature = "ktest"))]
fn run_sessions() -> ! {
    let mut desktop = !fs::is_live();
    if fs::is_live() && !boot::cmdline_has("velora.nomenu") {
        tty::reset();
        let mut argv = alloc::vec!["/bin/velora-start"];
        if boot::cmdline_has("velora.uitest") {
            argv.push("--uitest");
        }
        if boot::cmdline_has("velora.noanim") {
            argv.push("--no-animations");
        }
        match exec::spawn("/bin/velora-start", &argv, &["PATH=/bin"]) {
            Ok(pid) => {
                // Status 3: the Live system with the desktop.
                desktop = sched::wait_pid(pid) == Some(3);
            }
            Err(e) => kwarn!("cannot start /bin/velora-start: {:?}", e),
        }
    }
    if boot::cmdline_has("velora.text") {
        desktop = false;
    }
    // Lumi's recent crashes (uptime in ms) and the last status, which the
    // next Lumi writes to its log.
    let mut crashes: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    let mut crashed: Option<alloc::string::String> = None;
    loop {
        tty::reset();
        if desktop {
            let mut argv = alloc::vec!["/bin/lumi"];
            if let Some(s) = crashed.as_deref() {
                argv.push("--crashed");
                argv.push(s);
            }
            match exec::spawn("/bin/lumi", &argv, &["PATH=/bin"]) {
                Ok(pid) => match sched::wait_pid(pid) {
                    // Logged out: the login screen again.
                    Some(0) => {
                        crashed = None;
                        continue;
                    }
                    // No screen or fonts, or session=console.
                    Some(2) => kinfo!("lumi ended with status 2; console login"),
                    status => {
                        // A crash: start Lumi again, unless it keeps
                        // crashing (three times in a minute).
                        let now = time::uptime_ms();
                        crashes.retain(|&t| now - t < 60_000);
                        crashes.push(now);
                        let code = status.unwrap_or(-1);
                        crashed = Some(alloc::format!("{}", code));
                        if crashes.len() < 3 {
                            kwarn!("lumi crashed (status {}); starting it again", code);
                            continue;
                        }
                        kwarn!(
                            "lumi crashed 3 times in a minute (status {}); console login",
                            code
                        );
                    }
                },
                Err(e) => kwarn!("cannot start /bin/lumi: {:?}", e),
            }
            desktop = false;
            tty::reset();
        }
        match exec::spawn("/bin/login", &["/bin/login"], &["PATH=/bin"]) {
            Ok(pid) => {
                sched::wait_pid(pid);
            }
            Err(e) => {
                kwarn!("cannot start /bin/login: {:?}", e);
                kprintln!("No login program; the system is idle.");
                loop {
                    sched::sleep_ms(60_000);
                }
            }
        }
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
    if let Some(h) = acpi.hpet {
        match hpet::init(h.address) {
            Ok(hz) => kok!("HPET at {:#x}, {} kHz", h.address, hz / 1000),
            Err(e) => kwarn!("HPET: {}", e),
        }
    }
    if acpi.fadt.pm_timer_port != 0 {
        let _ = pmtimer::init(acpi.fadt.pm_timer_port, acpi.fadt.pm_timer_32bit);
    }
    match time::init() {
        Ok(t) => {
            for (r, e) in &t.failed {
                kwarn!("calibration against the {}: {}", r.name(), e);
            }
            kok!(
                "timer {} Hz; TSC {}.{:02} GHz{}, APIC timer {} kHz (calibrated against the {})",
                time::TICK_HZ,
                t.tsc_hz / 1_000_000_000,
                t.tsc_hz / 10_000_000 % 100,
                if time::tsc_invariant() {
                    ", invariant"
                } else {
                    ""
                },
                t.timer_per_ms,
                t.reference.name()
            );
            if !time::tsc_invariant() {
                kwarn!("the TSC is not invariant: time may drift in deep power states");
            }
        }
        Err(e) => panic!("timer calibration: {}", e),
    }
    match drivers::ps2::init(madt) {
        Ok(mouse) => kok!(
            "PS/2 keyboard (layouts EN, RU; Alt+Shift switches){}",
            if mouse { ", PS/2 mouse" } else { "" }
        ),
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

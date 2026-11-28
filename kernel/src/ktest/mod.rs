//! In-kernel test runner, built with `--features ktest`.
//!
//! Tests run in order after early boot. A failing test panics; the panic
//! handler prints the report and exits QEMU with a failure status. The last
//! step is a deliberate panic that checks the panic path itself.

use crate::arch::x86_64::{exceptions, gdt, idt, qemu, serial};
use crate::console::font;
use crate::console::framebuffer::Framebuffer;
use crate::console::text::{self, TextConsole};
use crate::sync::RacyCell;
use crate::{boot, kprint, kprintln};
use core::arch::asm;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

mod drivers;
mod exec;
mod fs;
mod irq;
mod mm;
mod net;
mod sched;
mod smp;
mod user;

struct Test {
    name: &'static str,
    run: fn(),
}

macro_rules! tests {
    ($($f:path),* $(,)?) => { &[$(Test { name: stringify!($f), run: $f }),*] };
}

static TESTS: &[Test] = tests![
    serial_is_present,
    boot_info_is_present,
    font_covers_ascii_and_cyrillic,
    font_glyph_matches_source,
    text_console_draws_glyph,
    text_console_wraps_and_scrolls,
    text_console_scale_reverse_and_cursor,
    ansi_sgr_colours,
    ansi_cursor_position,
    text_console_kitty_graphics,
    text_console_answers_queries,
    hidden_console_keeps_drawing_in_ram,
    key_events_become_packets,
    pointer_events_become_packets,
    gdt_and_tss_loaded,
    idt_loaded,
    breakpoint_resumes,
    page_fault_is_caught,
    invalid_opcode_is_caught,
    divide_error_is_caught,
    general_protection_is_caught,
    exception_report_format,
    // stage 2: memory management
    mm::frames_alloc_and_free,
    mm::frames_contiguous_and_aligned,
    mm::frames_reachable_through_hhdm,
    mm::kernel_sections_have_wx_permissions,
    mm::hhdm_maps_usable_memory,
    mm::write_to_text_faults,
    mm::write_to_rodata_faults,
    mm::execute_from_data_faults,
    mm::null_page_is_unmapped,
    mm::map_and_unmap_page,
    mm::heap_collections_work,
    mm::heap_reuses_freed_pages,
    mm::heap_stress,
    mm::kernel_stack_guard_page_faults,
    mm::running_on_kernel_stack,
    mm::bootloader_memory_released,
    mm::back_buffer_scrolls_like_screen,
    mm::idle_memory_below_64_mib,
    // stage 3: interrupts, timer, keyboard
    irq::acpi_tables_found,
    irq::local_apic_enabled,
    irq::legacy_pic_masked,
    irq::keyboard_irq_routed,
    irq::tsc_calibrated,
    irq::reference_clocks_agree,
    irq::acpi_power_management,
    irq::timer_ticks_at_expected_rate,
    irq::handler_registry_dispatches,
    irq::keyboard_input,
    irq::no_stray_interrupts,
    // stage 4: threads, processes, ring 3, system calls
    sched::scheduler_started,
    sched::kernel_threads_are_preempted,
    sched::sleep_blocks_for_the_requested_time,
    sched::user_hello,
    sched::user_getpid,
    sched::user_bad_pointer_gets_efault,
    sched::user_unknown_syscall_gets_enosys,
    sched::user_memory_is_isolated,
    sched::user_cannot_read_kernel_memory,
    sched::user_cannot_run_privileged_instructions,
    sched::user_code_is_not_writable,
    sched::user_stack_segment_has_rpl_3_after_syscall,
    sched::user_processes_are_preempted,
    sched::user_sse_registers_are_per_thread,
    sched::user_sleep_and_yield,
    sched::user_clock_gettime,
    // stage 14: multiprocessing
    smp::all_cpus_online,
    smp::tsc_synchronized,
    smp::task_priority_sticks,
    smp::programs_run_in_parallel,
    smp::kill_on_another_cpu,
    smp::big_kernel_lock_balanced,
    sched::processes_do_not_leak_memory,
    sched::wait_for_unknown_process,
    // stage 5: programs from files
    exec::initramfs_has_programs,
    exec::elf_program_runs,
    exec::user_space_test_suite,
    exec::missing_and_non_executable_files,
    exec::pe_files_are_recognised_but_not_run,
    exec::broken_elf_is_rejected,
    exec::too_many_arguments_are_refused,
    exec::programs_do_not_leak_memory,
    // stage 6: PCI, disks, RTC
    drivers::pci_finds_devices,
    drivers::three_disks_found,
    drivers::ahci_read_write,
    drivers::nvme_read_write,
    drivers::virtio_blk_read_write,
    drivers::disks_are_independent,
    drivers::rtc_matches_bootloader_time,
    drivers::usb_devices_found,
    drivers::usb_storage_files,
    drivers::usb_mouse_moves,
    // stage 7: file systems
    fs::vfs_mounted_at_boot,
    fs::devfs_exposes_disks,
    fs::memfs_conformance,
    fs::permissions_are_enforced,
    fs::programs_need_execute_permission,
    fs::velorafs_on_disk,
    fs::fat32_on_disk,
    // stage 8: shell, commands, users
    user::pipe_buffer,
    user::acpi_power_off_data,
    user::passwords_are_hashed,
    user::shell_pipelines_and_variables,
    user::shell_script_and_exit_status,
    user::commands_work,
    // stage 10: network
    net::dhcp_and_ping_gateway,
];

static EXPECTING_PANIC: AtomicBool = AtomicBool::new(false);

/// TSC when the running test started (0 between tests).
static TEST_STARTED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static WATCHDOG_FIRED: AtomicBool = AtomicBool::new(false);

/// Called by the boot CPU on every timer interrupt, before it takes the
/// big kernel lock: a test that runs for more than 60 s is stuck, so the
/// scheduler state is printed and the run ends.
pub fn watchdog(frame: &crate::arch::x86_64::idt::InterruptFrame) {
    let start = TEST_STARTED.load(Ordering::Relaxed);
    let hz = crate::time::tsc_hz();
    if start == 0 || hz == 0 || crate::arch::cpu::rdtsc() - start < 60 * hz {
        return;
    }
    if WATCHDOG_FIRED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::console::emergency(|w| {
        let _ = writeln!(w, "\nktest: watchdog: the test runs for more than 60 s");
        let _ = writeln!(
            w,
            "CPU 0 was at RIP {:#x} RFLAGS {:#x}",
            frame.rip, frame.rflags
        );
        let _ = crate::panic::write_backtrace(w, frame.rip, frame.rbp);
        for c in 1..crate::smp::online() {
            match crate::smp::where_is(c) {
                Some((rip, rbp, flags)) => {
                    let _ = writeln!(w, "CPU {} was at RIP {:#x} RFLAGS {:#x}", c, rip, flags);
                    let _ = crate::panic::write_backtrace(w, rip, rbp);
                }
                None => {
                    let _ = writeln!(w, "CPU {} did not answer", c);
                }
            }
        }
        crate::arch::x86_64::irq::debug_dump(w);
        crate::sched::debug_dump(w);
        crate::smp::debug_dump(w);
    });
    crate::arch::fatal_exit();
}

pub fn run() -> ! {
    kprintln!("ktest: boot took {} ms", crate::time::uptime_ms());
    kprintln!("ktest: running {} tests", TESTS.len());
    for t in TESTS {
        kprint!("ktest: {} ... ", t.name);
        TEST_STARTED.store(crate::arch::cpu::rdtsc(), Ordering::SeqCst);
        (t.run)();
        TEST_STARTED.store(0, Ordering::SeqCst);
        kprintln!("ok");
    }
    kprintln!("ktest: {} passed, 0 failed", TESTS.len());
    kprintln!("ktest: checking the panic path (a panic report is expected below)");
    EXPECTING_PANIC.store(true, Ordering::SeqCst);
    panic!("ktest: deliberate panic");
}

/// Called by the panic handler after the report is printed.
pub fn on_panic() {
    if EXPECTING_PANIC.load(Ordering::SeqCst) {
        kprintln!("ktest: panic path ok");
        kprintln!("ktest: ALL PASSED");
        qemu::exit(qemu::ExitCode::Success);
    }
    kprintln!("ktest: FAILED");
}

// --- helpers ----------------------------------------------------------------

pub(crate) struct BufWriter<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> BufWriter<N> {
    fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }
    pub(crate) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap()
    }
}

impl<const N: usize> Write for BufWriter<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let b = s.as_bytes();
        if self.len + b.len() > N {
            return Err(fmt::Error);
        }
        self.buf[self.len..self.len + b.len()].copy_from_slice(b);
        self.len += b.len();
        Ok(())
    }
}

// A 4x3-cell test screen in ordinary memory.
pub(crate) const TW: usize = 32;
pub(crate) const TH: usize = 48;
pub(crate) const TEST_FB_SIZE: usize = TW * TH;
static TEST_FB: RacyCell<[u32; TW * TH]> = RacyCell::new([0; TW * TH]);

pub(crate) fn test_console() -> TextConsole {
    // SAFETY: tests run one at a time; the buffer is static.
    let fb = unsafe { Framebuffer::from_raw(TEST_FB.get().cast(), TW, TH, TW * 4) };
    TextConsole::new(fb, 1)
}

// --- tests ------------------------------------------------------------------

fn serial_is_present() {
    assert!(serial::is_present(), "COM1 loopback test failed");
}

fn boot_info_is_present() {
    let m = boot::memory_summary();
    assert!(
        m.usable >= 16 * 1024 * 1024,
        "less than 16 MiB usable memory"
    );
    let info = boot::info();
    assert!(info.hhdm_offset >= 0xffff_8000_0000_0000, "bad HHDM offset");
    assert!(
        info.bootloader().starts_with("Limine"),
        "bootloader name missing"
    );
    assert!(
        boot::cmdline_has("velora.test"),
        "test cmdline flag missing"
    );
}

fn font_covers_ascii_and_cyrillic() {
    for c in (0x20u8..=0x7e).map(char::from) {
        assert!(font::glyph(c).is_some(), "missing glyph");
    }
    for c in ['Ж', 'я', 'Ё', '─', '█'] {
        assert!(font::glyph(c).is_some(), "missing non-ASCII glyph");
    }
    assert!(font::glyph_count() > 300);
}

fn font_glyph_matches_source() {
    // 'A' in Spleen 8x16: rows 2..12 = 7c c6 c6 c6 fe c6 c6 c6 c6 c6
    let a = font::glyph('A').unwrap();
    assert_eq!(a[2], 0x7c);
    assert_eq!(a[6], 0xfe);
    assert_eq!(a[0], 0x00);
}

fn text_console_draws_glyph() {
    let mut c = test_console();
    assert_eq!((c.cols, c.rows), (4, 3));
    c.write_str("A");
    let fb = c.framebuffer();
    let fg = fb.native(text::DEFAULT_FG);
    let bg = fb.native(text::DEFAULT_BG);
    // Row 2 of 'A' is 0x7c = .XXXXX..
    assert_eq!(fb.get_native(0, 2), Some(bg));
    assert_eq!(fb.get_native(1, 2), Some(fg));
    assert_eq!(fb.get_native(5, 2), Some(fg));
    assert_eq!(fb.get_native(6, 2), Some(bg));
    assert_eq!((c.row, c.col), (0, 1));
}

/// Font scale 2, reverse video and the cursor (an inverted underline).
fn text_console_scale_reverse_and_cursor() {
    // SAFETY: tests run one at a time; the buffer is static.
    let fb = unsafe { Framebuffer::from_raw(TEST_FB.get().cast(), TW, TH, TW * 4) };
    let mut c = TextConsole::new(fb, 2);
    assert_eq!((c.cols, c.rows), (2, 1));
    c.write_str("A");
    let (fg, bg, inv) = {
        let fb = c.framebuffer();
        let bg = fb.native(text::DEFAULT_BG);
        (fb.native(text::DEFAULT_FG), bg, bg ^ fb.native(0xff_ffff))
    };
    let fb = c.framebuffer();
    // Row 2 of 'A' (.XXXXX..) is pixel rows 4 and 5, each dot 2x2 pixels.
    assert_eq!(fb.get_native(1, 4), Some(bg));
    assert_eq!(fb.get_native(2, 4), Some(fg));
    assert_eq!(fb.get_native(3, 5), Some(fg));
    assert_eq!(fb.get_native(12, 5), Some(bg));
    // The cursor is in cell 1: its bottom 4 pixel rows are inverted.
    assert_eq!(fb.get_native(20, 31), Some(inv));
    assert_eq!(fb.get_native(20, 27), Some(bg));
    c.write_str("[?25l");
    assert_eq!(
        c.framebuffer().get_native(20, 31),
        Some(bg),
        "hidden cursor drawn"
    );
    c.write_str("[7mB[27m");
    // Reverse video: the background of 'B' has the foreground colour.
    assert_eq!(c.framebuffer().get_native(16, 0), Some(fg));
    c.write_str("[?25h[H");
    assert_eq!(c.framebuffer().get_native(0, 31), Some(inv));
}

fn text_console_wraps_and_scrolls() {
    let mut c = test_console();
    c.write_str("abcde"); // 4 columns: wraps after 'd'
    assert_eq!((c.row, c.col), (1, 1));
    c.write_str("\n\n\nX");
    // Three rows: the screen must have scrolled and stay on the last row.
    assert_eq!((c.row, c.col), (2, 1));
    c.write_str("\t");
    assert_eq!(c.col, 4); // tab stops at the right edge
    c.write_str("Y"); // and the next character wraps
    assert_eq!((c.row, c.col), (2, 1));
}

fn ansi_sgr_colours() {
    let mut c = test_console();
    c.write_str("\x1b[31m");
    assert_eq!(c.fg, text::PALETTE[1]);
    c.write_str("\x1b[1;32m");
    assert_eq!(c.fg, text::PALETTE[10]);
    c.write_str("\x1b[44m");
    assert_eq!(c.bg, text::PALETTE[4]);
    c.write_str("\x1b[0m");
    assert_eq!((c.fg, c.bg), (text::DEFAULT_FG, text::DEFAULT_BG));
    c.write_str("\x1b[96m");
    assert_eq!(c.fg, text::PALETTE[14]);
    c.write_str("\x1b[m");
    assert_eq!(c.fg, text::DEFAULT_FG);
}

fn ansi_cursor_position() {
    let mut c = test_console();
    c.write_str("\x1b[2;3H");
    assert_eq!((c.row, c.col), (1, 2));
    c.write_str("\x1b[H");
    assert_eq!((c.row, c.col), (0, 0));
    c.write_str("\x1b[9;9H"); // clamped to the screen
    assert_eq!((c.row, c.col), (2, 3));
}

/// Kitty graphics on the 4x3-cell test screen (cells of 8x16 pixels).
fn text_console_kitty_graphics() {
    use velora_gfx::kitty::transmit;
    let mut c = test_console();
    // 2x2 RGB (red, green / blue, white) stretched over 2x1 cells.
    let rgb = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
    c.write_str(&transmit("a=T,f=24,s=2,v=2,c=2,r=1,i=7", &rgb));
    assert_eq!(c.take_replies(), b"\x1b_Gi=7;OK\x1b\\");
    let px = |c: &TextConsole, x, y| {
        c.framebuffer()
            .rgb(c.framebuffer().get_native(x, y).unwrap())
    };
    assert_eq!(px(&c, 0, 0), 0xff0000);
    assert_eq!(px(&c, 15, 7), 0x00ff00);
    assert_eq!(px(&c, 0, 8), 0x0000ff);
    assert_eq!(px(&c, 15, 15), 0xffffff);
    // The cursor moved past the image, on its last row.
    assert_eq!((c.row, c.col), (0, 2));

    // Transparent and half-transparent pixels, cursor kept (C=1), no
    // id: no answer.
    c.write_str(&transmit("a=T,f=32,s=1,v=1,C=1", &[255, 255, 255, 0]));
    assert_eq!(px(&c, 16, 0), text::DEFAULT_BG);
    c.write_str(&transmit(
        "a=T,f=32,s=1,v=1,c=1,r=1,C=1",
        &[255, 255, 255, 128],
    ));
    let half = velora_gfx::image::blend([128, 128, 128, 128], text::DEFAULT_BG);
    assert_eq!(px(&c, 16, 0), half);
    assert_eq!(px(&c, 23, 13), half); // rows 14-15: the cursor
    assert_eq!((c.row, c.col), (0, 2));
    assert!(c.take_replies().is_empty());

    // Store, then show by id; unknown ids and PNG data are refused.
    c.write_str(&transmit("a=t,f=24,s=1,v=1,i=9", &[1, 2, 3]));
    assert_eq!(c.take_replies(), b"\x1b_Gi=9;OK\x1b\\");
    c.write_str("\x1b[2;1H\x1b_Ga=p,i=9,c=1,r=1\x1b\\");
    assert_eq!(px(&c, 0, 16), 0x010203);
    assert_eq!(px(&c, 7, 31), 0x010203);
    assert_eq!((c.row, c.col), (1, 1));
    assert_eq!(c.take_replies(), b"\x1b_Gi=9;OK\x1b\\");
    c.write_str("\x1b_Ga=p,i=99\x1b\\\x1b_Ga=T,f=100,i=5;AAAA\x1b\\");
    let r = c.take_replies();
    assert!(r.starts_with(b"\x1b_Gi=99;ENOENT:"), "no ENOENT answer");
    assert!(
        r.windows(14).any(|w| w == b"\x1b_Gi=5;EINVAL:"),
        "no EINVAL answer"
    );

    // On the last row, a two-row image scrolls the screen by one row.
    c.write_str("\x1b[3;1H");
    c.write_str(&transmit("a=T,f=24,s=1,v=1,c=1,r=2,C=1", &[200, 0, 0]));
    assert_eq!((c.row, c.col), (1, 0));
    assert_eq!(px(&c, 0, 16), 0xc80000);
    assert_eq!(px(&c, 0, 47), 0xc80000);
    // The old row 1 (the stored image) moved up to row 0.
    assert_eq!(px(&c, 0, 0), 0x010203);

    // An APC string cut short by another escape sequence is dropped and
    // the sequence still works; text after an image is drawn normally.
    c.write_str("\x1b_Gi=3,a=q;AA\x1b[31m");
    assert_eq!(c.fg, text::PALETTE[1]);
    assert!(c.take_replies().is_empty());
    c.write_str("\x1b[0mAB");
    assert_eq!((c.row, c.col), (1, 2));
}

/// Device attributes and size queries, as programs use them to detect
/// graphics support and the cell size.
/// While a graphical program owns the screen, console text changes only
/// the RAM copy; showing the console again puts it on the screen.
fn hidden_console_keeps_drawing_in_ram() {
    let mut c = test_console();
    c.framebuffer_mut().enable_back_buffer();
    let screen_px = |x: usize, y: usize| {
        // SAFETY: tests run one at a time; inside the static buffer.
        unsafe { TEST_FB.get().cast::<u32>().add(y * TW + x).read_volatile() }
    };
    c.framebuffer_mut().fill_rect(0, 0, TW, TH, 0x000000);
    assert_eq!(screen_px(5, 5), 0);
    assert!(c.framebuffer_mut().set_hidden(true));
    c.framebuffer_mut().fill_rect(0, 0, TW, TH, 0x123456);
    c.write_str("\n\n\n\nscroll");
    assert_eq!(screen_px(5, 5), 0, "hidden console wrote to the screen");
    assert!(c.framebuffer_mut().set_hidden(false));
    assert_ne!(screen_px(5, 5), 0, "the console did not come back");
    assert_eq!(
        screen_px(TW - 1, 0),
        c.framebuffer().get_native(TW - 1, 0).unwrap()
    );
}

fn key_events_become_packets() {
    use velora_abi::key;
    use velora_input::{KeyCode, KeyEvent, Layout, Modifiers};
    let ev = |k, ch, shift| KeyEvent {
        key: k,
        pressed: true,
        ch,
        mods: Modifiers {
            shift,
            ..Modifiers::default()
        },
        layout: Layout::Russian,
    };
    let p = crate::input::key_packet(&ev(KeyCode::Tab, Some('\t'), true));
    assert_eq!(u16::from_le_bytes([p[0], p[1]]), key::TAB);
    assert_eq!(p[2], key::PRESSED | key::SHIFT);
    assert_eq!(&p[4..], &[0, 0, 0, 0]);
    let p = crate::input::key_packet(&ev(KeyCode::Q, Some('й'), false));
    assert_eq!(u16::from_le_bytes([p[0], p[1]]), key::CHAR);
    assert_eq!(u32::from_le_bytes([p[4], p[5], p[6], p[7]]), 'й' as u32);
    let p = crate::input::key_packet(&ev(KeyCode::Left, None, false));
    assert_eq!(u16::from_le_bytes([p[0], p[1]]), key::LEFT);
}

fn pointer_events_become_packets() {
    use crate::input::MouseEvent;
    use velora_abi::pointer;
    let rel = MouseEvent {
        buttons: 1,
        dx: -3,
        dy: 4,
        wheel: 1,
        abs: None,
    };
    let p = crate::input::packet(&rel);
    assert_eq!(p.len(), pointer::PACKET);
    assert_eq!((p[0], p[1], p[2] as i8), (0, 1, 1));
    assert_eq!(i16::from_le_bytes([p[4], p[5]]), -3);
    assert_eq!(i16::from_le_bytes([p[6], p[7]]), 4);
    let abs = MouseEvent {
        abs: Some((32767, 100)),
        ..MouseEvent::default()
    };
    let p = crate::input::packet(&abs);
    assert_eq!(p[0], pointer::ABSOLUTE);
    assert_eq!(i16::from_le_bytes([p[4], p[5]]) as i32, pointer::ABS_MAX);
    assert_eq!(i16::from_le_bytes([p[6], p[7]]), 100);
}

fn text_console_answers_queries() {
    let mut c = test_console();
    c.write_str(velora_gfx::kitty::QUERY);
    let r = c.take_replies();
    assert_eq!(
        velora_gfx::kitty::support(&r),
        velora_gfx::kitty::Support::Yes
    );
    c.write_str("\x1b[16t\x1b[14t\x1b[18t");
    assert_eq!(c.take_replies(), b"\x1b[6;16;8t\x1b[4;48;32t\x1b[8;3;4t");
    // Secondary attributes are not answered and leave no text behind.
    c.write_str("\x1b[>c");
    assert!(c.take_replies().is_empty());
    assert_eq!((c.row, c.col), (0, 0));
}

fn gdt_and_tss_loaded() {
    assert_eq!(gdt::current_cs(), gdt::KERNEL_CODE);
    assert_eq!(gdt::current_tr(), gdt::TSS_SELECTOR);
    let top = gdt::ist_top(gdt::IST_DOUBLE_FAULT);
    assert!(top != 0 && top.is_multiple_of(16), "bad double-fault stack");
}

fn idt_loaded() {
    let (limit, base) = idt::current();
    assert_eq!(limit, 256 * 16 - 1);
    assert!(base >= 0xffff_8000_0000_0000);
}

fn breakpoint_resumes() {
    let before = exceptions::breakpoint_count();
    // SAFETY: int3 is handled and returns to the next instruction.
    unsafe { asm!("int3") }
    assert_eq!(exceptions::breakpoint_count(), before + 1);
}

fn page_fault_is_caught() {
    exceptions::arm_recovery(exceptions::PAGE_FAULT, 0);
    // SAFETY: the read faults (lower half is unmapped) and the recovery
    // hook resumes at label 77.
    unsafe {
        asm!(
            "lea {tmp}, [rip + 77f]",
            "mov qword ptr [{slot}], {tmp}",
            "mov {tmp}, qword ptr [{addr}]",
            "77:",
            slot = in(reg) exceptions::recover_rip_slot(),
            addr = in(reg) 0x1000_0000u64,
            tmp = out(reg) _,
        );
    }
    let (v, err, cr2) = exceptions::take_caught().expect("page fault not raised");
    assert_eq!(v, exceptions::PAGE_FAULT);
    assert_eq!(cr2, 0x1000_0000);
    assert_eq!(err & 0b111, 0, "expected kernel read of a non-present page");
}

fn invalid_opcode_is_caught() {
    exceptions::arm_recovery(exceptions::INVALID_OPCODE, 0);
    // SAFETY: ud2 faults; the hook resumes at label 77.
    unsafe {
        asm!(
            "lea {tmp}, [rip + 77f]",
            "mov qword ptr [{slot}], {tmp}",
            "ud2",
            "77:",
            slot = in(reg) exceptions::recover_rip_slot(),
            tmp = out(reg) _,
        );
    }
    let (v, ..) = exceptions::take_caught().expect("#UD not raised");
    assert_eq!(v, exceptions::INVALID_OPCODE);
}

fn divide_error_is_caught() {
    exceptions::arm_recovery(exceptions::DIVIDE_ERROR, 0);
    // SAFETY: division by zero faults; the hook resumes at label 77.
    unsafe {
        asm!(
            "lea {tmp}, [rip + 77f]",
            "mov qword ptr [{slot}], {tmp}",
            "xor edx, edx",
            "xor eax, eax",
            "xor ecx, ecx",
            "div ecx",
            "77:",
            slot = in(reg) exceptions::recover_rip_slot(),
            tmp = out(reg) _,
            out("eax") _, out("ecx") _, out("edx") _,
        );
    }
    let (v, ..) = exceptions::take_caught().expect("#DE not raised");
    assert_eq!(v, exceptions::DIVIDE_ERROR);
}

fn general_protection_is_caught() {
    exceptions::arm_recovery(exceptions::GENERAL_PROTECTION, 0);
    // SAFETY: a non-canonical address raises #GP; resume at label 77.
    unsafe {
        asm!(
            "lea {tmp}, [rip + 77f]",
            "mov qword ptr [{slot}], {tmp}",
            "mov {tmp}, qword ptr [{addr}]",
            "77:",
            slot = in(reg) exceptions::recover_rip_slot(),
            addr = in(reg) 0x8000_0000_0000_0000u64,
            tmp = out(reg) _,
        );
    }
    let (v, ..) = exceptions::take_caught().expect("#GP not raised");
    assert_eq!(v, exceptions::GENERAL_PROTECTION);
}

fn exception_report_format() {
    let frame = idt::InterruptFrame {
        vector: exceptions::PAGE_FAULT,
        error_code: 0x2,
        rip: 0xffff_ffff_8000_1234,
        cs: gdt::KERNEL_CODE as u64,
        rax: 0xabcd,
        ..Default::default()
    };
    let mut w = BufWriter::<4096>::new();
    exceptions::write_report(&mut w, &frame, 0xdead_beef).unwrap();
    let s = w.as_str();
    assert!(s.contains("#PF Page fault (vector 14) in kernel mode"));
    assert!(s.contains("write to a non-present page (kernel)"));
    assert!(s.contains("CR2=0x00000000deadbeef"));
    assert!(s.contains("RIP=0xffffffff80001234"));
    assert!(s.contains("RAX=0x000000000000abcd"));
    assert!(s.contains("backtrace:"));
}

//! Stage 3 tests: ACPI, APIC, I/O APIC, timer, interrupt dispatch, keyboard.

use crate::arch::cpu;
use crate::arch::x86_64::idt::InterruptFrame;
use crate::arch::x86_64::{apic, hpet, ioapic, irq, pic, pmtimer};
use crate::{acpi, input, kprintln, time};
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

pub fn acpi_tables_found() {
    let a = acpi::get();
    assert!(a.rsdp.revision >= 2, "QEMU q35 provides ACPI 2.0+");
    assert!(a.tables.iter().any(|(s, _)| s == b"APIC"));
    assert!(a.tables.iter().any(|(s, _)| s == b"FACP"), "no FADT");
    assert!(a.madt.lapics.iter().any(|l| l.enabled));
    assert!(!a.madt.ioapics.is_empty());
}

pub fn local_apic_enabled() {
    assert!(apic::software_enabled());
    let initial_id = cpu::cpuid(1, 0).ebx >> 24;
    assert_eq!(apic::id(), initial_id);
    assert!(apic::version() >= 0x10, "integrated APIC expected");
}

pub fn legacy_pic_masked() {
    assert_eq!(pic::masks(), 0xffff);
}

pub fn keyboard_irq_routed() {
    let (gsi, _, _) = acpi::get().madt.isa_irq(1);
    let (vector, masked) = ioapic::entry(gsi).expect("keyboard GSI has no I/O APIC");
    assert_eq!(vector, irq::VECTOR_KEYBOARD);
    assert!(!masked);
}

pub fn tsc_calibrated() {
    let hz = time::tsc_hz();
    assert!((100_000_000..10_000_000_000).contains(&hz), "TSC {} Hz", hz);
    let a = time::uptime_ns();
    time::delay_us(1000);
    let b = time::uptime_ns();
    assert!(b - a >= 1_000_000, "delay_us(1000) took {} ns", b - a);
}

/// QEMU q35 has an HPET and an ACPI PM timer; all reference clocks must
/// measure the same TSC rate as the boot calibration (within 5 %).
pub fn reference_clocks_agree() {
    assert!(hpet::available(), "no HPET on q35");
    assert!(hpet::hz() > 1_000_000, "HPET {} Hz", hpet::hz());
    assert!(pmtimer::available(), "no ACPI PM timer on q35");
    let boot = time::tsc_hz();
    for r in apic::Reference::ALL {
        let was = cpu::irq_save();
        let hz = apic::measure_tsc_hz(r);
        cpu::irq_restore(was);
        let hz = hz.unwrap_or_else(|e| panic!("{}: {}", r.name(), e));
        let diff = hz.abs_diff(boot);
        kprintln!("ktest:   {}: TSC {} kHz", r.name(), hz / 1000);
        assert!(
            diff * 20 < boot,
            "{} measured {} Hz, boot {} Hz",
            r.name(),
            hz,
            boot
        );
    }
}

pub fn timer_ticks_at_expected_rate() {
    assert!(cpu::interrupts_enabled());
    let t0 = time::ticks();
    let start = cpu::rdtsc();
    time::sleep_ms(300);
    let elapsed_ms = time::tsc_to_ns(cpu::rdtsc() - start) / 1_000_000;
    let ticks = time::ticks() - t0;
    crate::kprint!("({} ticks in {} ms) ", ticks, elapsed_ms);
    assert!(elapsed_ms >= 300);
    // 100 Hz -> about 30 ticks; emulation is allowed to be off by 2x.
    let expected = elapsed_ms * time::TICK_HZ / 1000;
    assert!(
        ticks * 2 >= expected && ticks <= expected * 2,
        "{} ticks, expected ~{}",
        ticks,
        expected
    );
}

static SOFT_IRQS: AtomicU64 = AtomicU64::new(0);

fn soft_handler(frame: &mut InterruptFrame) {
    assert_eq!(frame.vector, 0x40);
    SOFT_IRQS.fetch_add(1, Ordering::SeqCst);
}

pub fn handler_registry_dispatches() {
    irq::set_handler(0x40, soft_handler);
    // SAFETY: vector 0x40 has a handler that only counts.
    unsafe {
        core::arch::asm!("int 0x40");
        core::arch::asm!("int 0x40");
    }
    irq::clear_handler(0x40);
    assert_eq!(SOFT_IRQS.load(Ordering::SeqCst), 2);
}

/// End-to-end keyboard test: the test runner types keys through the QEMU
/// monitor (see KTEST_KEYS in tests/qemu_run.py).
pub fn keyboard_input() {
    while input::pop().is_some() {}
    kprintln!();
    kprintln!("ktest: keyboard: waiting for input");
    let mut text = String::new();
    while text.matches('\n').count() < 2 {
        let ev = input::wait(Some(20_000)).expect("no keyboard input within 20 s");
        if let (true, Some(c)) = (ev.pressed, ev.ch) {
            text.push(c);
        }
    }
    crate::kprint!("ktest: keyboard: got {:?} ", text);
    assert_eq!(text, "velA\nйЦ\n");
    assert_eq!(input::dropped(), 0);
}

pub fn no_stray_interrupts() {
    let s = irq::stats();
    assert_eq!(s.unhandled, 0, "unhandled interrupts");
    assert_eq!(s.apic_errors, 0, "APIC errors");
}

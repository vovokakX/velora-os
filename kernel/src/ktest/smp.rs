//! Stage 14 tests: every CPU online, programs on several CPUs at once.

use crate::sched::{self, Pid, Priority};
use crate::{acpi, kprint, mm, smp, userprog};
use alloc::vec::Vec;

const KILLED: i64 = 128 + 9;

/// The counter the `spin` program increments in its data page.
fn counter(pid: Pid) -> u64 {
    sched::with_address_space(pid, |a| {
        let (phys, _) = a.translate(sched::FLAT_DATA_BASE).expect("data not mapped");
        // SAFETY: the frame belongs to the live process; read via the HHDM.
        unsafe { (mm::phys_to_virt(phys) as *const u64).read_volatile() }
    })
    .expect("no such process")
}

fn spin() -> Pid {
    sched::spawn_flat("spin", userprog::spin(), Priority::Normal).expect("spawn")
}

fn stop(pid: Pid) {
    assert!(sched::kill(pid, KILLED));
    assert_eq!(sched::wait_pid(pid), Some(KILLED));
}

/// The TSCs of all CPUs agree (time and sleeping rely on them).
pub fn tsc_synchronized() {
    assert_eq!(smp::tsc_skew(), 0, "the CPUs' TSCs differ");
}

/// A processor waiting for the big kernel lock raises its task priority
/// so the reschedule IPI cannot enter and take a second ticket. The value
/// must stick (it did not under WHPX when written through the APIC).
pub fn task_priority_sticks() {
    use crate::arch::x86_64::apic;
    let was = crate::arch::x86_64::cpu::irq_save();
    apic::set_task_priority(0xe);
    let got = apic::task_priority();
    apic::set_task_priority(0);
    crate::arch::x86_64::cpu::irq_restore(was);
    assert_eq!(got, 0xe, "the task priority did not stick");
    assert_eq!(apic::task_priority(), 0);
}

pub fn all_cpus_online() {
    let enabled = acpi::get().madt.lapics.iter().filter(|l| l.enabled).count();
    assert!(smp::online() >= 2, "the tests run with -smp 4");
    assert_eq!(smp::online(), enabled.min(smp::MAX_CPUS), "CPUs online");
    for i in 0..smp::online() {
        assert!(smp::apic_id(i).is_some(), "CPU {} has no APIC id", i);
    }
    // Every CPU has run its idle thread (it took timer interrupts).
    let idle = sched::idle_ticks();
    assert_eq!(idle.len(), smp::online());
}

/// Programs started together land on different CPUs and run at the same
/// time: together they count clearly further than one program alone. If
/// they shared one CPU, the total would be about the same as alone.
pub fn programs_run_in_parallel() {
    let n = smp::online().min(4);
    let solo = spin();
    sched::sleep_ms(300);
    let alone = counter(solo);
    stop(solo);

    let pids: Vec<Pid> = (0..n).map(|_| spin()).collect();
    sched::sleep_ms(300);
    let counts: Vec<u64> = pids.iter().map(|&p| counter(p)).collect();
    let mut cpus: Vec<usize> = sched::processes()
        .iter()
        .filter(|p| pids.contains(&p.pid))
        .map(|p| p.cpu)
        .collect();
    for &p in &pids {
        stop(p);
    }
    cpus.sort_unstable();
    cpus.dedup();
    kprint!("(alone {}, together {:?}, CPUs {:?}) ", alone, counts, cpus);
    assert_eq!(cpus.len(), n, "the programs did not get their own CPUs");
    // Together they must do clearly more work than one alone (on one
    // CPU the total would be about the same). Each one is not required to
    // keep its full speed: in a virtual machine the CPUs may share the
    // host's cores.
    let total: u64 = counts.iter().sum();
    assert!(
        counts.iter().all(|&c| c > 0) && total * 10 > alone * 13,
        "programs did not run in parallel: {:?} together vs {} alone",
        counts,
        alone
    );
}

/// Killing a program that runs on another CPU stops it at once.
pub fn kill_on_another_cpu() {
    let pids: Vec<Pid> = (0..smp::online()).map(|_| spin()).collect();
    sched::sleep_ms(50);
    for &p in &pids {
        stop(p);
    }
    sched::sleep_ms(30);
    assert!(
        !sched::threads()
            .iter()
            .any(|t| t.name == "spin" && t.state != sched::State::Dead),
        "a killed program still runs"
    );
}

/// The big kernel lock was never released by a CPU that did not hold it.
pub fn big_kernel_lock_balanced() {
    assert_eq!(smp::bkl_mismatches(), 0, "unbalanced big kernel lock");
    assert!(smp::bkl_held(), "the test thread runs without the lock");
}

//! Stage 4 tests: threads, scheduler, processes, ring 3, system calls.

use crate::arch::cpu;
use crate::arch::x86_64::{gdt, percpu};
use crate::mm::{self, frame, kstack};
use crate::sched::{self, Priority, State};
use crate::{kprint, time, userprog};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use velora_abi::{errno, fault_exit_status};

const KILLED: i64 = 128 + 9;

fn run(name: &str, code: &[u8]) -> i64 {
    let pid = sched::spawn_flat(name, code, Priority::Normal).expect("spawn failed");
    sched::wait_pid(pid).expect("process vanished")
}

pub fn scheduler_started() {
    let t = sched::threads();
    assert!(
        t.iter()
            .any(|t| t.name == "kmain" && t.state == State::Running)
    );
    // One idle thread per CPU.
    let idle = t
        .iter()
        .filter(|t| t.name.starts_with("idle") && t.priority == Priority::Idle)
        .count();
    assert_eq!(idle, crate::smp::online(), "idle threads");
    assert_eq!(gdt::rsp0() % 16, 0);
    assert_ne!(percpu::gs_base(), 0, "GS base not set");
}

static COUNTERS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn counting_thread(i: u64) -> u64 {
    // No yields and no sleeps: only the timer can take the CPU away.
    while !STOP.load(Ordering::Relaxed) {
        COUNTERS[i as usize].fetch_add(1, Ordering::Relaxed);
    }
    i + 100
}

pub fn kernel_threads_are_preempted() {
    let stacks_before = kstack::mapped_pages();
    STOP.store(false, Ordering::SeqCst);
    // All three on one CPU, so only the timer can share it among them
    // (on several CPUs the test would also measure the big kernel lock).
    let cpu = crate::smp::online() - 1;
    for i in 0..3 {
        sched::spawn_kernel_on("counter", counting_thread, i, Priority::Normal, cpu)
            .expect("spawn");
    }
    sched::sleep_ms(300);
    STOP.store(true, Ordering::SeqCst);
    let counts: [u64; 3] = core::array::from_fn(|i| COUNTERS[i].load(Ordering::SeqCst));
    kprint!("({:?}) ", counts);
    assert!(
        counts.iter().all(|&c| c > 0),
        "a busy thread never ran: {:?}",
        counts
    );
    // Let them finish and be reaped.
    for _ in 0..50 {
        if !sched::threads().iter().any(|t| t.name == "counter") {
            break;
        }
        sched::sleep_ms(10);
    }
    assert!(
        !sched::threads().iter().any(|t| t.name == "counter"),
        "threads not reaped"
    );
    // Freed stacks stay mapped in a reuse cache (no TLB shootdown on
    // SMP); at most the three new stacks were added.
    assert!(kstack::cached() >= 3, "thread stacks not returned");
    assert!(
        kstack::mapped_pages() <= stacks_before + 3 * 8,
        "thread stacks leaked"
    );
}

pub fn sleep_blocks_for_the_requested_time() {
    let start = cpu::rdtsc();
    sched::sleep_ms(50);
    let ms = time::tsc_to_ns(cpu::rdtsc() - start) / 1_000_000;
    assert!((50..500).contains(&ms), "slept {} ms", ms);
}

pub fn user_hello() {
    assert_eq!(run("hello", userprog::hello()), 0);
}

pub fn user_getpid() {
    let pid = sched::spawn_flat("getpid", userprog::getpid(), Priority::Normal).unwrap();
    assert_eq!(sched::wait_pid(pid), Some(pid as i64));
}

pub fn user_bad_pointer_gets_efault() {
    assert_eq!(run("efault", userprog::efault()), errno::EFAULT);
}

pub fn user_unknown_syscall_gets_enosys() {
    assert_eq!(run("enosys", userprog::enosys()), errno::ENOSYS);
}

pub fn user_memory_is_isolated() {
    // Run the reader while the writer still exists: same virtual address,
    // different physical pages.
    let w = sched::spawn_flat("writer", userprog::write_mem(), Priority::Normal).unwrap();
    let r = sched::spawn_flat("reader", userprog::read_mem(), Priority::Normal).unwrap();
    assert_eq!(sched::wait_pid(w), Some(0x55));
    assert_eq!(sched::wait_pid(r), Some(0));
}

pub fn user_cannot_read_kernel_memory() {
    assert_eq!(run("kread", userprog::read_kernel()), fault_exit_status(14));
}

pub fn user_cannot_run_privileged_instructions() {
    assert_eq!(run("cli", userprog::privileged()), fault_exit_status(13));
}

pub fn user_code_is_not_writable() {
    assert_eq!(
        run("selfmod", userprog::self_modify()),
        fault_exit_status(14)
    );
}

fn data_word(pid: u64) -> u64 {
    sched::with_address_space(pid, |a| {
        let (phys, _) = a.translate(sched::FLAT_DATA_BASE).expect("data not mapped");
        // SAFETY: the frame belongs to the live process; read via the HHDM.
        unsafe { (mm::phys_to_virt(phys) as *const u64).read_volatile() }
    })
    .expect("no such process")
}

pub fn user_stack_segment_has_rpl_3_after_syscall() {
    assert_eq!(
        run("ssrpl", userprog::ss_rpl()),
        3,
        "sysret left a user SS with RPL != 3"
    );
    let star = unsafe { crate::arch::cpu::rdmsr(0xc000_0081) };
    assert_eq!((star >> 48) & 3, 3, "STAR[63:48] has no RPL 3");
}

pub fn user_processes_are_preempted() {
    let a = sched::spawn_flat("spin-a", userprog::spin(), Priority::Normal).unwrap();
    let b = sched::spawn_flat("spin-b", userprog::spin(), Priority::Normal).unwrap();
    sched::sleep_ms(200);
    let (ca, cb) = (data_word(a), data_word(b));
    kprint!("(counters {} / {}) ", ca, cb);
    assert!(ca > 0 && cb > 0, "a spinning process never ran");
    assert!(sched::kill(a, KILLED));
    assert!(sched::kill(b, KILLED));
    assert_eq!(sched::wait_pid(a), Some(KILLED));
    assert_eq!(sched::wait_pid(b), Some(KILLED));
    assert!(!sched::kill(a, KILLED), "dead process killed twice");
}

pub fn user_sse_registers_are_per_thread() {
    let a = sched::spawn_flat("sse-a", userprog::sse(), Priority::Normal).unwrap();
    let b = sched::spawn_flat("sse-b", userprog::sse(), Priority::Normal).unwrap();
    assert_eq!(
        sched::wait_pid(a),
        Some(0),
        "xmm0 of process a was corrupted"
    );
    assert_eq!(
        sched::wait_pid(b),
        Some(0),
        "xmm0 of process b was corrupted"
    );
}

pub fn user_sleep_and_yield() {
    let start = cpu::rdtsc();
    assert_eq!(run("sleepy", userprog::sleep_yield()), 0);
    let ms = time::tsc_to_ns(cpu::rdtsc() - start) / 1_000_000;
    assert!(ms >= 50, "user sleep returned after {} ms", ms);
}

pub fn user_clock_gettime() {
    assert_eq!(run("clock", userprog::clock()), 0);
}

pub fn processes_do_not_leak_memory() {
    // Warm up once so the heap and page-table caches are populated.
    run("hello", userprog::hello());
    let before = frame::stats().free_frames;
    let tables = mm::paging::table_frames();
    for _ in 0..5 {
        assert_eq!(run("hello", userprog::hello()), 0);
        assert_eq!(run("kread", userprog::read_kernel()), fault_exit_status(14));
    }
    let after = frame::stats().free_frames;
    // The kernel heap may grow by a few pages for bookkeeping.
    assert!(
        before - after.min(before) <= 4,
        "{} frames leaked",
        before - after
    );
    assert_eq!(mm::paging::table_frames(), tables, "page tables leaked");
    assert!(sched::processes().is_empty(), "processes left behind");
}

pub fn wait_for_unknown_process() {
    assert_eq!(sched::wait_pid(9999), None);
    assert!(!sched::kill(9999, KILLED));
}

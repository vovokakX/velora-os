//! Threads, processes and the preemptive scheduler.
//!
//! - Round robin within three priorities (High, Normal, Idle). The idle
//!   thread runs only when nothing else is ready.
//! - A thread runs for one timer tick (10 ms) before it is preempted.
//! - The kernel is not preemptible inside system calls (interrupts are off
//!   there); kernel threads and user code are preempted by the timer.
//! - Dead threads are reaped by the next thread after the switch, because a
//!   thread cannot free the stack it is running on.

mod process;
mod wait;

#[cfg_attr(not(feature = "ktest"), allow(unused_imports))]
pub use process::{
    FLAT_DATA_BASE, Personality, ProcessInfo, USER_STACK_PAGES, USER_STACK_TOP, kill, parent_of,
    reserve_mmap_range, spawn_flat, spawn_user, wait_pid,
};
pub use wait::WaitQueue;

use crate::arch::cpu;
use crate::arch::x86_64::{context, fpu, gdt, percpu};
use crate::mm::kstack::{self, KernelStack};
use crate::mm::paging;
use crate::sync::SpinLock;
use crate::time;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use process::Process;

pub type Tid = u64;
pub type Pid = u64;

/// Pid 0 means "kernel thread, no user address space".
pub const KERNEL_PID: Pid = 0;
const KERNEL_STACK_PAGES: u64 = 8;
/// Timer ticks a thread may run before it is preempted.
const TIME_SLICE: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    High = 0,
    Normal = 1,
    Idle = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ready,
    Running,
    Sleeping,
    Blocked,
    Dead,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Ready => "ready",
            State::Running => "running",
            State::Sleeping => "sleeping",
            State::Blocked => "blocked",
            State::Dead => "dead",
        }
    }
}

struct Thread {
    tid: Tid,
    pid: Pid,
    name: String,
    state: State,
    priority: Priority,
    /// Saved stack pointer while not running.
    rsp: u64,
    stack: KernelStack,
    fx: Box<fpu::FxState>,
    /// Tick at which a sleeping (or timed blocked) thread wakes.
    wake_at: Option<u64>,
    /// Where a user thread starts: (rip, rsp).
    user_entry: Option<(u64, u64)>,
    ticks: u64,
}

struct Sched {
    threads: BTreeMap<Tid, Box<Thread>>,
    ready: [VecDeque<Tid>; 3],
    current: Tid,
    idle: Tid,
    dead: Vec<Tid>,
    processes: BTreeMap<Pid, Process>,
    next_tid: Tid,
    next_pid: Pid,
    slice_left: u32,
}

static SCHED: SpinLock<Option<Sched>> = SpinLock::new(None);
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

fn with<R>(f: impl FnOnce(&mut Sched) -> R) -> R {
    let mut g = SCHED.lock();
    f(g.as_mut().expect("scheduler not initialised"))
}

/// True once `init` has run.
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

/// Turns the current boot code into thread 1 ("kmain") running on
/// `main_stack` and creates the idle thread. Interrupts must be disabled.
pub fn init(main_stack: KernelStack) {
    let kmain = Box::new(Thread {
        tid: 1,
        pid: KERNEL_PID,
        name: String::from("kmain"),
        state: State::Running,
        priority: Priority::Normal,
        rsp: 0,
        stack: main_stack,
        fx: Box::new(fpu::initial()),
        wake_at: None,
        user_entry: None,
        ticks: 0,
    });
    let mut s = Sched {
        threads: BTreeMap::new(),
        ready: [VecDeque::new(), VecDeque::new(), VecDeque::new()],
        current: 1,
        idle: 0,
        dead: Vec::new(),
        processes: BTreeMap::new(),
        next_tid: 2,
        next_pid: 1,
        slice_left: TIME_SLICE,
    };
    s.threads.insert(1, kmain);
    gdt::set_rsp0(main_stack.top);
    percpu::set_kernel_rsp(main_stack.top);
    *SCHED.lock() = Some(s);
    let idle =
        spawn_kernel("idle", idle_loop, 0, Priority::Idle).expect("cannot create the idle thread");
    with(|s| {
        // The idle thread is picked only when every queue is empty.
        s.idle = idle;
        for q in s.ready.iter_mut() {
            q.retain(|&t| t != idle);
        }
    });
    RUNNING.store(true, Ordering::Release);
}

extern "C" fn idle_loop(_: u64) -> u64 {
    loop {
        cpu::enable_interrupts_and_halt();
    }
}

fn new_thread(
    s: &mut Sched,
    name: &str,
    pid: Pid,
    priority: Priority,
    entry: extern "C" fn(u64) -> u64,
    arg: u64,
    user_entry: Option<(u64, u64)>,
) -> Option<Tid> {
    let stack = kstack::alloc(KERNEL_STACK_PAGES)?;
    let tid = s.next_tid;
    s.next_tid += 1;
    // SAFETY: the stack was just allocated and is unused.
    let rsp = unsafe { context::prepare_stack(stack.top, entry, arg) };
    s.threads.insert(
        tid,
        Box::new(Thread {
            tid,
            pid,
            name: String::from(name),
            state: State::Ready,
            priority,
            rsp,
            stack,
            fx: Box::new(fpu::initial()),
            wake_at: None,
            user_entry,
            ticks: 0,
        }),
    );
    s.ready[priority as usize].push_back(tid);
    Some(tid)
}

/// Starts a kernel thread running `entry(arg)`. Its return value becomes
/// the thread's exit code.
pub fn spawn_kernel(
    name: &str,
    entry: extern "C" fn(u64) -> u64,
    arg: u64,
    priority: Priority,
) -> Option<Tid> {
    with(|s| new_thread(s, name, KERNEL_PID, priority, entry, arg, None))
}

/// Entry of every user thread: jump to its user entry point.
extern "C" fn user_thread_start(_: u64) -> u64 {
    let (rip, rsp) = with(|s| s.threads[&s.current].user_entry).expect("user thread without entry");
    cpu::disable_interrupts();
    // SAFETY: the scheduler activated this thread's address space and set
    // TSS.rsp0 / the per-CPU kernel stack to this thread's stack.
    unsafe { context::enter_user(rip, rsp) }
}

pub fn current_tid() -> Tid {
    with(|s| s.current)
}

pub fn current_pid() -> Pid {
    with(|s| s.threads[&s.current].pid)
}

/// Picks the next thread and switches to it. Interrupts must be disabled.
fn schedule() {
    let switch = {
        let mut g = SCHED.lock();
        let s = g.as_mut().expect("scheduler not initialised");
        let cur = s.current;
        {
            let t = s.threads.get_mut(&cur).unwrap();
            if t.state == State::Running {
                t.state = State::Ready;
                if cur != s.idle {
                    s.ready[t.priority as usize].push_back(cur);
                }
            }
        }
        let next = s
            .ready
            .iter_mut()
            .find_map(|q| q.pop_front())
            .unwrap_or(s.idle);
        s.slice_left = TIME_SLICE;
        if next == cur {
            s.threads.get_mut(&cur).unwrap().state = State::Running;
            None
        } else {
            let old_pid = s.threads[&cur].pid;
            let new_pid = s.threads[&next].pid;
            {
                let old = s.threads.get_mut(&cur).unwrap();
                fpu::save(&mut old.fx);
            }
            let n = s.threads.get_mut(&next).unwrap();
            n.state = State::Running;
            fpu::restore(&n.fx);
            gdt::set_rsp0(n.stack.top);
            percpu::set_kernel_rsp(n.stack.top);
            let new_rsp = n.rsp;
            if old_pid != new_pid {
                match s.processes.get(&new_pid).and_then(|p| p.aspace.as_ref()) {
                    // SAFETY: interrupts are off; the kernel half is shared.
                    Some(a) => unsafe { a.activate() },
                    None => unsafe { paging::activate_kernel() },
                }
            }
            s.current = next;
            let old_rsp: *mut u64 = &mut s.threads.get_mut(&cur).unwrap().rsp;
            Some((old_rsp, new_rsp))
        }
    };
    if let Some((old, new)) = switch {
        // SAFETY: interrupts are disabled; both stacks are valid kernel
        // stacks; the Box holding `old` stays alive while the thread exists.
        unsafe { context::switch(old, new) };
        finish_switch();
    }
}

/// Runs on the new thread right after a switch: frees dead threads.
#[unsafe(no_mangle)]
extern "C" fn velora_finish_switch() {
    finish_switch();
}

fn finish_switch() {
    let mut stacks = Vec::new();
    let mut spaces = Vec::new();
    with(|s| {
        let current = s.current;
        let dead: Vec<Tid> = s.dead.iter().copied().filter(|&t| t != current).collect();
        s.dead.retain(|&t| t == current);
        for tid in dead {
            if let Some(t) = s.threads.remove(&tid) {
                stacks.push(t.stack);
            }
        }
        for p in s.processes.values_mut() {
            if p.threads.iter().all(|t| !s.threads.contains_key(t))
                && let Some(a) = p.aspace.take()
            {
                spaces.push(a);
            }
        }
        let current_pid = s.threads[&current].pid;
        s.processes.retain(|&pid, p| {
            let gone = p.detached
                && matches!(p.state, process::ProcState::Zombie(_))
                && p.aspace.is_none()
                && pid != current_pid;
            !gone
        });
    });
    // Freed outside the scheduler lock (they take the page table lock).
    for st in stacks {
        kstack::free(st);
    }
    drop(spaces);
}

/// Called from the trampoline when a thread's entry function returns.
#[unsafe(no_mangle)]
extern "C" fn velora_thread_return(code: u64) -> ! {
    exit_current(code as i64)
}

/// Ends the current thread. If it was the last thread of a user process,
/// the process exits with `code`.
pub fn exit_current(code: i64) -> ! {
    cpu::disable_interrupts();
    with(|s| {
        let cur = s.current;
        let pid = s.threads[&cur].pid;
        mark_dead(s, cur);
        if pid != KERNEL_PID {
            process::thread_exited(s, pid, cur, code);
        }
    });
    schedule();
    unreachable!("a dead thread was scheduled again");
}

/// Ends the whole current process with `status`.
pub fn exit_process(status: i64) -> ! {
    let pid = current_pid();
    if pid != KERNEL_PID {
        cpu::disable_interrupts();
        with(|s| process::terminate(s, pid, status));
    }
    exit_current(status)
}

fn mark_dead(s: &mut Sched, tid: Tid) {
    if let Some(t) = s.threads.get_mut(&tid) {
        if t.state == State::Dead {
            return;
        }
        t.state = State::Dead;
        t.wake_at = None;
        for q in s.ready.iter_mut() {
            q.retain(|&x| x != tid);
        }
        s.dead.push(tid);
    }
}

/// Gives up the CPU to another ready thread, if any.
pub fn yield_now() {
    let was = cpu::irq_save();
    schedule();
    cpu::irq_restore(was);
}

/// Puts the current thread to sleep for at least `ms` milliseconds.
///
/// Sleeping is counted in timer ticks, but the tick and the TSC are
/// calibrated separately and can disagree by a few percent, so the TSC
/// decides when the time is really up.
pub fn sleep_ms(ms: u64) {
    let deadline = cpu::rdtsc() + time::tsc_hz() * ms / 1000;
    let was = cpu::irq_save();
    loop {
        let now_tsc = cpu::rdtsc();
        if now_tsc >= deadline {
            break;
        }
        let left_ms = time::tsc_to_ns(deadline - now_tsc).div_ceil(1_000_000);
        let ticks = (left_ms * time::TICK_HZ).div_ceil(1000).max(1);
        with(|s| {
            let now = time::ticks();
            let t = s.threads.get_mut(&s.current).unwrap();
            t.state = State::Sleeping;
            t.wake_at = Some(now + ticks);
        });
        schedule();
    }
    cpu::irq_restore(was);
}

/// Blocks the current thread until `wake` is called (or the optional
/// deadline tick passes). Interrupts must be disabled by the caller, who
/// re-checks its condition afterwards (wakeups can be spurious).
pub fn block_current(deadline: Option<u64>) {
    with(|s| {
        let t = s.threads.get_mut(&s.current).unwrap();
        t.state = State::Blocked;
        t.wake_at = deadline;
    });
    schedule();
}

fn make_ready(s: &mut Sched, tid: Tid) {
    let current_prio = s.threads.get(&s.current).map(|t| t.priority);
    let idle = s.idle;
    let current = s.current;
    if let Some(t) = s.threads.get_mut(&tid)
        && matches!(t.state, State::Blocked | State::Sleeping)
    {
        t.state = State::Ready;
        t.wake_at = None;
        s.ready[t.priority as usize].push_back(tid);
        if current == idle || current_prio.is_some_and(|p| t.priority < p) {
            NEED_RESCHED.store(true, Ordering::Release);
        }
    }
}

/// Wakes a blocked or sleeping thread.
pub fn wake(tid: Tid) {
    with(|s| make_ready(s, tid));
}

/// Timer interrupt hook: wakes sleepers and accounts the time slice.
pub fn on_tick(now: u64) {
    if !is_running() {
        return;
    }
    let mut g = SCHED.lock();
    let Some(s) = g.as_mut() else { return };
    let due: Vec<Tid> = s
        .threads
        .values()
        .filter(|t| t.wake_at.is_some_and(|w| w <= now))
        .map(|t| t.tid)
        .collect();
    for tid in due {
        make_ready(s, tid);
    }
    let cur = s.current;
    if let Some(t) = s.threads.get_mut(&cur) {
        t.ticks += 1;
    }
    if cur == s.idle {
        if s.ready.iter().any(|q| !q.is_empty()) {
            NEED_RESCHED.store(true, Ordering::Release);
        }
    } else {
        s.slice_left = s.slice_left.saturating_sub(1);
        if s.slice_left == 0 {
            NEED_RESCHED.store(true, Ordering::Release);
        }
    }
}

/// Called after every hardware interrupt (after EOI): switches threads if
/// the timer or a wakeup asked for it. Interrupts are disabled here.
pub fn preempt_if_needed() {
    if is_running() && NEED_RESCHED.swap(false, Ordering::AcqRel) {
        schedule();
    }
}

#[derive(Clone, Debug)]
pub struct ThreadInfo {
    pub tid: Tid,
    pub pid: Pid,
    pub name: String,
    pub state: State,
    pub priority: Priority,
    pub ticks: u64,
}

/// Snapshot of all threads (for `ps` and tests).
pub fn threads() -> Vec<ThreadInfo> {
    with(|s| {
        s.threads
            .values()
            .map(|t| ThreadInfo {
                tid: t.tid,
                pid: t.pid,
                name: t.name.clone(),
                state: t.state,
                priority: t.priority,
                ticks: t.ticks,
            })
            .collect()
    })
}

/// Snapshot of all processes.
pub fn processes() -> Vec<ProcessInfo> {
    with(|s| s.processes.values().map(|p| p.info()).collect())
}

/// Runs `f` with the address space of process `pid` (for tests and
/// debugging tools that inspect process memory).
pub fn with_address_space<R>(pid: Pid, f: impl FnOnce(&paging::AddressSpace) -> R) -> Option<R> {
    with(|s| s.processes.get(&pid).and_then(|p| p.aspace.as_ref()).map(f))
}

/// Personality of the current process (Native for kernel threads).
pub fn current_personality() -> Personality {
    with(|s| {
        let pid = s.threads[&s.current].pid;
        s.processes
            .get(&pid)
            .map(|p| p.personality)
            .unwrap_or(Personality::Native)
    })
}

/// Name of the current process or kernel thread.
pub fn current_name() -> String {
    with(|s| {
        let t = &s.threads[&s.current];
        s.processes
            .get(&t.pid)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| t.name.clone())
    })
}

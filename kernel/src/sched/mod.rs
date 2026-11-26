//! Threads, processes and the preemptive scheduler.
//!
//! - Round robin within three priorities (High, Normal, Idle). Every CPU
//!   has its own idle thread, which runs only when nothing else is ready
//!   for that CPU.
//! - A thread runs for one timer tick (10 ms) before it is preempted.
//! - The kernel runs under the big kernel lock (`smp`): one CPU at a time
//!   is inside the kernel, user code runs on all CPUs. The lock is held
//!   across a thread switch and its nesting depth is saved per thread.
//! - All threads of a process run on one CPU, chosen when the process
//!   starts (the least loaded one). A process's memory is therefore only
//!   ever in one CPU's TLB, and unmapping user pages needs no
//!   interprocessor TLB shootdown (D43). Kernel threads run on any CPU.
//! - Dead threads are reaped by the next thread after the switch, because a
//!   thread cannot free the stack it is running on; a thread still running
//!   on another CPU is left until it has switched away.

mod process;
mod wait;

#[cfg_attr(not(feature = "ktest"), allow(unused_imports))]
pub use process::{
    FLAT_DATA_BASE, Personality, ProcessInfo, USER_STACK_PAGES, USER_STACK_TOP, current_cred,
    current_cwd, fd_get, fd_install, fd_remove, fd_set, kill, parent_of, reserve_mmap_range,
    set_cred, set_cwd, spawn_flat, spawn_user, uid_of, wait_pid,
};
pub use wait::WaitQueue;

use crate::arch::cpu;
use crate::arch::x86_64::{context, fpu, gdt, percpu};
use crate::mm::kstack::{self, KernelStack};
use crate::mm::paging;
use crate::smp::{self, MAX_CPUS};
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
    /// The only CPU this thread may run on (None: any).
    affinity: Option<usize>,
    /// The CPU running it right now.
    on_cpu: Option<usize>,
    /// The CPU it ran on last.
    last_cpu: usize,
    /// Big kernel lock depth to restore when it runs again.
    bkl_depth: u64,
}

/// One CPU's view of the scheduler.
#[derive(Clone, Copy)]
struct Cpu {
    current: Tid,
    idle: Tid,
    slice_left: u32,
    online: bool,
}

struct Sched {
    threads: BTreeMap<Tid, Box<Thread>>,
    ready: [VecDeque<Tid>; 3],
    cpus: Vec<Cpu>,
    dead: Vec<Tid>,
    processes: BTreeMap<Pid, Process>,
    next_tid: Tid,
    next_pid: Pid,
    /// Files of exited processes, closed outside the scheduler lock
    /// (closing a pipe end wakes threads, which takes the lock).
    closing: Vec<crate::fs::FdTable>,
    /// Address spaces of collected processes whose last threads still
    /// exist (killed while running on another CPU).
    orphan_spaces: Vec<(Vec<Tid>, paging::AddressSpace)>,
}

impl Sched {
    /// The thread running on this CPU.
    fn cur(&self) -> Tid {
        self.cpus[percpu::index()].current
    }

    fn is_idle_thread(&self, tid: Tid) -> bool {
        self.cpus.iter().any(|c| c.online && c.idle == tid)
    }

    /// Takes the first ready thread that may run on `cpu`.
    fn pick(&mut self, cpu: usize) -> Option<Tid> {
        for q in 0..self.ready.len() {
            let pos = self.ready[q].iter().position(|t| {
                self.threads
                    .get(t)
                    .is_some_and(|th| th.affinity.is_none_or(|a| a == cpu))
            });
            if let Some(i) = pos {
                return self.ready[q].remove(i);
            }
        }
        None
    }

    fn has_ready_for(&self, cpu: usize) -> bool {
        self.ready.iter().flatten().any(|t| {
            self.threads
                .get(t)
                .is_some_and(|th| th.affinity.is_none_or(|a| a == cpu))
        })
    }

    /// The online CPU with the fewest living processes (for a new one).
    fn least_loaded_cpu(&self) -> usize {
        let mut load = [0usize; MAX_CPUS];
        for p in self.processes.values() {
            if p.state == process::ProcState::Alive {
                load[p.cpu.min(MAX_CPUS - 1)] += 1;
            }
        }
        (0..self.cpus.len())
            .filter(|&c| self.cpus[c].online)
            .min_by_key(|&c| (load[c], c))
            .unwrap_or(0)
    }
}

static SCHED: SpinLock<Option<Sched>> = SpinLock::new(None);
static NEED_RESCHED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static RUNNING: AtomicBool = AtomicBool::new(false);

fn with<R>(f: impl FnOnce(&mut Sched) -> R) -> R {
    let mut g = SCHED.lock();
    f(g.as_mut().expect("scheduler not initialised"))
}

fn request_resched(cpu: usize) {
    NEED_RESCHED[cpu].store(true, Ordering::Release);
    smp::poke(cpu);
}

/// True once `init` has run.
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

fn thread(
    tid: Tid,
    name: &str,
    pid: Pid,
    priority: Priority,
    stack: KernelStack,
    affinity: Option<usize>,
) -> Box<Thread> {
    Box::new(Thread {
        tid,
        pid,
        name: String::from(name),
        state: State::Ready,
        priority,
        rsp: 0,
        stack,
        fx: Box::new(fpu::initial()),
        wake_at: None,
        user_entry: None,
        ticks: 0,
        affinity,
        on_cpu: None,
        last_cpu: affinity.unwrap_or(0),
        bkl_depth: 1,
    })
}

/// Turns the current boot code into thread 1 ("kmain") running on
/// `main_stack`, takes the big kernel lock and creates the boot CPU's
/// idle thread. Interrupts must be disabled.
pub fn init(main_stack: KernelStack) {
    smp::bkl_enter();
    let mut kmain = thread(1, "kmain", KERNEL_PID, Priority::Normal, main_stack, None);
    kmain.state = State::Running;
    kmain.on_cpu = Some(0);
    let mut s = Sched {
        threads: BTreeMap::new(),
        ready: [VecDeque::new(), VecDeque::new(), VecDeque::new()],
        cpus: alloc::vec![Cpu {
            current: 1,
            idle: 0,
            slice_left: TIME_SLICE,
            online: true,
        }],
        dead: Vec::new(),
        processes: BTreeMap::new(),
        next_tid: 2,
        next_pid: 1,
        closing: Vec::new(),
        orphan_spaces: Vec::new(),
    };
    s.threads.insert(1, kmain);
    gdt::set_rsp0(main_stack.top);
    percpu::set_kernel_rsp(main_stack.top);
    *SCHED.lock() = Some(s);
    let idle = with(|s| {
        new_thread(
            s,
            "idle0",
            KERNEL_PID,
            Priority::Idle,
            idle_loop,
            0,
            None,
            Some(0),
        )
    })
    .expect("cannot create the idle thread");
    with(|s| {
        // The idle thread is picked only when every queue is empty.
        s.cpus[0].idle = idle;
        for q in s.ready.iter_mut() {
            q.retain(|&t| t != idle);
        }
    });
    RUNNING.store(true, Ordering::Release);
}

/// Registers application processor `index`, whose current code (on
/// `stack`) becomes its idle thread. Called with the big kernel lock held.
pub fn add_cpu(index: usize, stack: KernelStack) {
    with(|s| {
        let tid = s.next_tid;
        s.next_tid += 1;
        let mut t = thread(
            tid,
            &alloc::format!("idle{}", index),
            KERNEL_PID,
            Priority::Idle,
            stack,
            Some(index),
        );
        t.state = State::Running;
        t.on_cpu = Some(index);
        s.threads.insert(tid, t);
        while s.cpus.len() <= index {
            s.cpus.push(Cpu {
                current: 0,
                idle: 0,
                slice_left: TIME_SLICE,
                online: false,
            });
        }
        s.cpus[index] = Cpu {
            current: tid,
            idle: tid,
            slice_left: TIME_SLICE,
            online: true,
        };
    });
    gdt::set_rsp0(stack.top);
    percpu::set_kernel_rsp(stack.top);
}

/// Runs this CPU's idle loop (the end of an AP's start-up).
pub fn run_idle() -> ! {
    idle_loop(0);
    unreachable!()
}

extern "C" fn idle_loop(_: u64) -> u64 {
    loop {
        // Idle CPUs must not hold the big kernel lock while they sleep;
        // an interrupt takes it again.
        cpu::disable_interrupts();
        smp::bkl_release_all();
        cpu::enable_interrupts_and_halt();
    }
}

#[allow(clippy::too_many_arguments)]
fn new_thread(
    s: &mut Sched,
    name: &str,
    pid: Pid,
    priority: Priority,
    entry: extern "C" fn(u64) -> u64,
    arg: u64,
    user_entry: Option<(u64, u64)>,
    affinity: Option<usize>,
) -> Option<Tid> {
    let stack = kstack::alloc(KERNEL_STACK_PAGES)?;
    let tid = s.next_tid;
    s.next_tid += 1;
    let mut t = thread(tid, name, pid, priority, stack, affinity);
    // SAFETY: the stack was just allocated and is unused.
    t.rsp = unsafe { context::prepare_stack(stack.top, entry, arg) };
    t.user_entry = user_entry;
    s.threads.insert(tid, t);
    make_ready_new(s, tid, priority);
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
    with(|s| new_thread(s, name, KERNEL_PID, priority, entry, arg, None, None))
}

/// Starts a kernel thread that runs only on CPU `cpu`.
pub fn spawn_kernel_on(
    name: &str,
    entry: extern "C" fn(u64) -> u64,
    arg: u64,
    priority: Priority,
    cpu: usize,
) -> Option<Tid> {
    with(|s| new_thread(s, name, KERNEL_PID, priority, entry, arg, None, Some(cpu)))
}

/// Entry of every user thread: jump to its user entry point.
extern "C" fn user_thread_start(_: u64) -> u64 {
    let (rip, rsp) = with(|s| s.threads[&s.cur()].user_entry).expect("user thread without entry");
    cpu::disable_interrupts();
    smp::bkl_exit();
    // SAFETY: the scheduler activated this thread's address space and set
    // TSS.rsp0 / the per-CPU kernel stack to this thread's stack.
    unsafe { context::enter_user(rip, rsp) }
}

pub fn current_tid() -> Tid {
    with(|s| s.cur())
}

pub fn current_pid() -> Pid {
    with(|s| s.threads[&s.cur()].pid)
}

/// Picks the next thread for this CPU and switches to it. Interrupts must
/// be disabled and the big kernel lock held.
fn schedule() {
    assert!(
        smp::bkl_held() || !percpu::ready(),
        "thread switch without the big kernel lock"
    );
    let me = percpu::index();
    let switch = {
        let mut g = SCHED.lock();
        let s = g.as_mut().expect("scheduler not initialised");
        let cur = s.cpus[me].current;
        let idle = s.cpus[me].idle;
        {
            let t = s.threads.get_mut(&cur).unwrap();
            if t.state == State::Running {
                t.state = State::Ready;
                if cur != idle {
                    s.ready[t.priority as usize].push_back(cur);
                }
            }
        }
        let next = s.pick(me).unwrap_or(idle);
        s.cpus[me].slice_left = TIME_SLICE;
        if next == cur {
            s.threads.get_mut(&cur).unwrap().state = State::Running;
            None
        } else {
            let old_pid = s.threads[&cur].pid;
            let new_pid = s.threads[&next].pid;
            {
                let old = s.threads.get_mut(&cur).unwrap();
                fpu::save(&mut old.fx);
                old.bkl_depth = smp::bkl_depth();
                old.on_cpu = None;
            }
            let n = s.threads.get_mut(&next).unwrap();
            n.state = State::Running;
            n.on_cpu = Some(me);
            n.last_cpu = me;
            fpu::restore(&n.fx);
            smp::set_bkl_depth(n.bkl_depth);
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
            s.cpus[me].current = next;
            let old_rsp: *mut u64 = &mut s.threads.get_mut(&cur).unwrap().rsp;
            Some((old_rsp, new_rsp))
        }
    };
    if let Some((old, new)) = switch {
        // SAFETY: interrupts are disabled; both stacks are valid kernel
        // stacks; the Box holding `old` stays alive while the thread
        // exists, and nothing reaps a thread before it has switched away
        // (the big kernel lock is held until then).
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
    let mut closing = Vec::new();
    with(|s| {
        closing = core::mem::take(&mut s.closing);
        let current = s.cur();
        let threads = &s.threads;
        let (gone, keep): (Vec<Tid>, Vec<Tid>) = s
            .dead
            .iter()
            .partition(|&&t| t != current && threads.get(&t).is_none_or(|th| th.on_cpu.is_none()));
        s.dead = keep;
        for tid in gone {
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
        let threads = &s.threads;
        let (free, wait): (Vec<_>, Vec<_>) = core::mem::take(&mut s.orphan_spaces)
            .into_iter()
            .partition(|(tids, _)| tids.iter().all(|t| !threads.contains_key(t)));
        s.orphan_spaces = wait;
        spaces.extend(free.into_iter().map(|(_, a)| a));
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
    drop(closing);
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
        let cur = s.cur();
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

/// Called before returning to user mode: a thread killed while it was
/// running (on this or another CPU) leaves now instead.
pub fn leave_if_killed() {
    if !is_running() {
        return;
    }
    let dead = with(|s| {
        s.threads
            .get(&s.cur())
            .is_some_and(|t| t.state == State::Dead)
    });
    if dead {
        schedule();
        unreachable!("a dead thread was scheduled again");
    }
}

fn mark_dead(s: &mut Sched, tid: Tid) {
    if let Some(t) = s.threads.get_mut(&tid) {
        if t.state == State::Dead {
            return;
        }
        t.state = State::Dead;
        t.wake_at = None;
        let running_on = t.on_cpu;
        for q in s.ready.iter_mut() {
            q.retain(|&x| x != tid);
        }
        s.dead.push(tid);
        // Running on another CPU: make that CPU switch away soon.
        if let Some(c) = running_on
            && c != percpu::index()
        {
            request_resched(c);
        }
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
            let cur = s.cur();
            let t = s.threads.get_mut(&cur).unwrap();
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
        let cur = s.cur();
        let t = s.threads.get_mut(&cur).unwrap();
        t.state = State::Blocked;
        t.wake_at = deadline;
    });
    schedule();
}

/// A thread became ready: tell a CPU that should run it.
fn make_ready_new(s: &mut Sched, tid: Tid, priority: Priority) {
    s.ready[priority as usize].push_back(tid);
    let affinity = s.threads[&tid].affinity;
    let me = percpu::index();
    let candidates: Vec<usize> = match affinity {
        Some(c) => alloc::vec![c],
        None => (0..s.cpus.len()).collect(),
    };
    // An idle CPU first, else one running something less important.
    let idle = candidates
        .iter()
        .copied()
        .find(|&c| c < s.cpus.len() && s.cpus[c].online && s.cpus[c].current == s.cpus[c].idle);
    let target = idle.or_else(|| {
        candidates.iter().copied().find(|&c| {
            c < s.cpus.len()
                && s.cpus[c].online
                && s.threads
                    .get(&s.cpus[c].current)
                    .is_some_and(|t| priority < t.priority)
        })
    });
    if let Some(c) = target {
        if c == me {
            NEED_RESCHED[c].store(true, Ordering::Release);
        } else {
            request_resched(c);
        }
    }
}

fn make_ready(s: &mut Sched, tid: Tid) {
    if let Some(t) = s.threads.get_mut(&tid)
        && matches!(t.state, State::Blocked | State::Sleeping)
    {
        t.state = State::Ready;
        t.wake_at = None;
        let priority = t.priority;
        make_ready_new(s, tid, priority);
    }
}

/// Wakes a blocked or sleeping thread.
pub fn wake(tid: Tid) {
    with(|s| make_ready(s, tid));
}

/// Timer interrupt hook on every CPU: the boot CPU wakes sleepers; each
/// CPU accounts its own time slice.
pub fn on_tick(now: u64) {
    if !is_running() {
        return;
    }
    let me = percpu::index();
    let mut g = SCHED.lock();
    let Some(s) = g.as_mut() else { return };
    if me >= s.cpus.len() || !s.cpus[me].online {
        return;
    }
    if me == 0 {
        let due: Vec<Tid> = s
            .threads
            .values()
            .filter(|t| t.wake_at.is_some_and(|w| w <= now))
            .map(|t| t.tid)
            .collect();
        for tid in due {
            make_ready(s, tid);
        }
    }
    let cur = s.cpus[me].current;
    if let Some(t) = s.threads.get_mut(&cur) {
        t.ticks += 1;
    }
    if cur == s.cpus[me].idle {
        if s.has_ready_for(me) {
            NEED_RESCHED[me].store(true, Ordering::Release);
        }
    } else {
        s.cpus[me].slice_left = s.cpus[me].slice_left.saturating_sub(1);
        if s.cpus[me].slice_left == 0 {
            NEED_RESCHED[me].store(true, Ordering::Release);
        }
    }
}

/// Called after every hardware interrupt (after EOI): switches threads if
/// the timer or a wakeup asked for it. Interrupts are disabled here.
pub fn preempt_if_needed() {
    if is_running() && NEED_RESCHED[percpu::index()].swap(false, Ordering::AcqRel) {
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
    /// The CPU it runs on now, or ran on last.
    pub cpu: usize,
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
                cpu: t.on_cpu.unwrap_or(t.last_cpu),
            })
            .collect()
    })
}

/// Idle time per CPU in timer ticks (the ticks of its idle thread).
pub fn idle_ticks() -> Vec<u64> {
    with(|s| {
        s.cpus
            .iter()
            .map(|c| s.threads.get(&c.idle).map_or(0, |t| t.ticks))
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
        let pid = s.threads[&s.cur()].pid;
        s.processes
            .get(&pid)
            .map(|p| p.personality)
            .unwrap_or(Personality::Native)
    })
}

/// Name of the current process or kernel thread.
pub fn current_name() -> String {
    with(|s| {
        let t = &s.threads[&s.cur()];
        s.processes
            .get(&t.pid)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| t.name.clone())
    })
}

/// True if `tid` is one of the per-CPU idle threads.
pub fn is_idle(tid: Tid) -> bool {
    with(|s| s.is_idle_thread(tid))
}

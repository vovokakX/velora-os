//! Multiprocessing: starting the other CPUs, the big kernel lock and
//! interprocessor interrupts.
//!
//! The kernel itself runs on one CPU at a time: every entry into the
//! kernel (system call, interrupt, exception) takes the big kernel lock
//! and every return to user mode gives it back (D43). Programs run on all
//! CPUs in parallel. All kernel code written for one CPU stays correct,
//! because it never runs concurrently with other kernel code.
//!
//! Start-up: Limine parks the application processors (APs) and starts
//! one when its `goto_address` is written. Their first stacks and the
//! page tables they run on are bootloader memory, which the kernel frees
//! early, so [`start_aps`] runs first: each AP switches to the kernel's
//! page tables and its own stack at once and then waits. [`release_aps`]
//! lets them set up their descriptor tables, APIC and idle thread.

use crate::arch::x86_64::{apic, cpu, irq, percpu};
use crate::boot::limine::{self, MpInfo};
use crate::mm::kstack::{self, KernelStack};
use alloc::boxed::Box;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

pub use percpu::MAX_CPUS;

/// Interrupt that makes a CPU look at its run queue again.
pub const VECTOR_RESCHEDULE: u8 = 0xe0;
/// Wakes CPUs that sleep while they wait for the big kernel lock. It is
/// the only vector above the priority they sleep with, and it is handled
/// without the lock.
pub const VECTOR_BKL_WAKE: u8 = 0xf1;
/// Priority class a waiting CPU sleeps with: 0xE0 and below are held.
const WAIT_PRIORITY_CLASS: u8 = 0xe;
/// Spins before a waiting CPU sleeps.
const SPINS_BEFORE_SLEEP: u32 = 2000;

// --- the big kernel lock ------------------------------------------------------
//
// A ticket lock (CPUs get it in the order they asked), recursive per CPU.

const NO_OWNER: u64 = u64::MAX;

// Test builds keep the last BKL operations of every CPU and report them
// if a CPU waits for the lock for seconds (a deadlock).
#[cfg(feature = "ktest")]
mod trace {
    use super::*;
    pub const N: usize = 16;
    /// (tsc, operation, depth before, owner before, caller line)
    pub static RING: [[[AtomicU64; 5]; N]; MAX_CPUS] =
        [const { [const { [const { AtomicU64::new(0) }; 5] }; N] }; MAX_CPUS];
    pub static POS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

    #[track_caller]
    pub fn record(cpu: usize, op: u64, depth: u64) {
        let i = (POS[cpu].fetch_add(1, Ordering::Relaxed) as usize) % N;
        let e = &RING[cpu][i];
        e[0].store(cpu::rdtsc(), Ordering::Relaxed);
        e[1].store(op, Ordering::Relaxed);
        e[2].store(depth, Ordering::Relaxed);
        e[3].store(OWNER.load(Ordering::Relaxed), Ordering::Relaxed);
        e[4].store(
            core::panic::Location::caller().line() as u64,
            Ordering::Relaxed,
        );
    }

    static DUMPING: AtomicBool = AtomicBool::new(false);
    /// Set to ask the CPU holding the lock (through an NMI) where it is.
    pub static WHERE_REQUEST: AtomicBool = AtomicBool::new(false);
    pub static WHERE_RIP: AtomicU64 = AtomicU64::new(0);
    pub static WHERE_RBP: AtomicU64 = AtomicU64::new(0);
    pub static WHERE_FLAGS: AtomicU64 = AtomicU64::new(0);

    /// Called from the NMI handler: true if this NMI was a "where are
    /// you" request, which is answered here.
    pub fn answer_where(rip: u64, rbp: u64, flags: u64) -> bool {
        if !WHERE_REQUEST.swap(false, Ordering::SeqCst) {
            return false;
        }
        WHERE_RBP.store(rbp, Ordering::SeqCst);
        WHERE_FLAGS.store(flags, Ordering::SeqCst);
        WHERE_RIP.store(rip, Ordering::SeqCst);
        true
    }
    pub static WAITING: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(u64::MAX) }; MAX_CPUS];

    pub fn dump() -> ! {
        if DUMPING.swap(true, Ordering::SeqCst) {
            cpu::halt_forever();
        }
        let owner = OWNER.load(Ordering::Relaxed);
        if let Some(id) = apic_id(owner as usize) {
            WHERE_REQUEST.store(true, Ordering::SeqCst);
            apic::send_nmi(id);
            let end = cpu::rdtsc() + crate::time::tsc_hz() / 10;
            while WHERE_RIP.load(Ordering::SeqCst) == 0 && cpu::rdtsc() < end {
                core::hint::spin_loop();
            }
        }
        crate::console::emergency(|w| {
            let _ = writeln!(
                w,
                "owner CPU was at RIP {:#x} RFLAGS {:#x}",
                WHERE_RIP.load(Ordering::SeqCst),
                WHERE_FLAGS.load(Ordering::SeqCst)
            );
            let _ = crate::panic::write_backtrace(
                w,
                WHERE_RIP.load(Ordering::SeqCst),
                WHERE_RBP.load(Ordering::SeqCst),
            );
            let _ = writeln!(
                w,
                "
BKL deadlock: owner {:#x} next {} serving {}",
                OWNER.load(Ordering::Relaxed),
                NEXT_TICKET.load(Ordering::Relaxed),
                SERVING.load(Ordering::Relaxed)
            );
            for c in 0..online() {
                let pos = POS[c].load(Ordering::Relaxed) as usize;
                let _ = writeln!(
                    w,
                    "CPU {} (waits for ticket {:#x}, depth {}):",
                    c,
                    WAITING[c].load(Ordering::Relaxed),
                    0
                );
                for k in 0..N {
                    let e = &RING[c][(pos + k) % N];
                    let _ = writeln!(
                        w,
                        "  tsc {} op {} depth {} owner {:#x} line {}",
                        e[0].load(Ordering::Relaxed),
                        e[1].load(Ordering::Relaxed),
                        e[2].load(Ordering::Relaxed),
                        e[3].load(Ordering::Relaxed),
                        e[4].load(Ordering::Relaxed)
                    );
                }
            }
        });
        cpu::halt_forever()
    }
}

#[cfg(feature = "ktest")]
#[track_caller]
fn trace(op: u64, depth: u64) {
    if percpu::ready() {
        trace::record(percpu::index(), op, depth);
    }
}

#[cfg(not(feature = "ktest"))]
fn trace(_op: u64, _depth: u64) {}

#[cfg(feature = "ktest")]
pub use trace::answer_where;
static OWNER: AtomicU64 = AtomicU64::new(NO_OWNER);
static NEXT_TICKET: AtomicU64 = AtomicU64::new(0);
static SERVING: AtomicU64 = AtomicU64::new(0);
/// CPUs sleeping in `bkl_enter` (bit per CPU index).
static SLEEPERS: AtomicU64 = AtomicU64::new(0);

/// Takes the big kernel lock (or nests deeper if this CPU holds it).
/// Interrupts must be disabled.
pub fn bkl_enter() {
    if !percpu::ready() {
        return;
    }
    let pc = percpu::this();
    if OWNER.load(Ordering::Relaxed) == pc.index {
        trace(1, pc.bkl_depth);
        pc.bkl_depth += 1;
        return;
    }
    trace(2, pc.bkl_depth);
    let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "ktest")]
    trace::WAITING[pc.index as usize].store(ticket, Ordering::Relaxed);
    #[cfg(feature = "ktest")]
    let start = cpu::rdtsc();
    let mut spins = 0u32;
    while SERVING.load(Ordering::Acquire) != ticket {
        core::hint::spin_loop();
        spins += 1;
        // A long wait (the owner polls a disk, say): sleep instead of
        // spinning, so the CPU (and, in a virtual machine, the host's
        // processor) is free. `unlock` wakes sleepers with an IPI.
        if spins >= SPINS_BEFORE_SLEEP && pc.index < 64 && online() > 1 {
            spins = 0;
            let bit = 1u64 << pc.index;
            SLEEPERS.fetch_or(bit, Ordering::SeqCst);
            if SERVING.load(Ordering::SeqCst) != ticket {
                apic::set_task_priority(WAIT_PRIORITY_CLASS);
                // `sti; hlt` is atomic: a wake-up IPI sent after the check
                // above still ends the halt.
                cpu::enable_interrupts_and_halt();
                cpu::disable_interrupts();
                apic::set_task_priority(0);
            }
            SLEEPERS.fetch_and(!bit, Ordering::SeqCst);
        }
        #[cfg(feature = "ktest")]
        if crate::time::tsc_hz() > 0 && cpu::rdtsc() - start > 30 * crate::time::tsc_hz() {
            trace::dump();
        }
    }
    OWNER.store(pc.index, Ordering::Relaxed);
    #[cfg(feature = "ktest")]
    trace::WAITING[pc.index as usize].store(u64::MAX, Ordering::Relaxed);
    pc.bkl_depth = 1;
}

fn unlock() {
    OWNER.store(NO_OWNER, Ordering::Relaxed);
    SERVING.fetch_add(1, Ordering::SeqCst);
    let sleepers = SLEEPERS.load(Ordering::SeqCst);
    if sleepers != 0 {
        for i in 0..64 {
            if sleepers & (1 << i) != 0
                && let Some(id) = apic_id(i)
            {
                apic::send_ipi(id, VECTOR_BKL_WAKE);
            }
        }
    }
}

/// Mismatched `bkl_exit` calls seen (should stay 0; checked by a test).
static BKL_MISMATCH: AtomicU64 = AtomicU64::new(0);

pub fn bkl_mismatches() -> u64 {
    BKL_MISMATCH.load(Ordering::Relaxed)
}

/// Leaves one level of the big kernel lock; the last level releases it.
/// A call on a CPU that does not hold the lock changes nothing (releasing
/// twice would break the ticket order for every CPU) and is reported.
#[track_caller]
pub fn bkl_exit() {
    if !percpu::ready() {
        return;
    }
    let pc = percpu::this();
    if pc.bkl_depth == 0 || OWNER.load(Ordering::Relaxed) != pc.index {
        if BKL_MISMATCH.fetch_add(1, Ordering::Relaxed) == 0 {
            crate::kwarn!(
                "big kernel lock released without being held (CPU {}, depth {}) at {}",
                pc.index,
                pc.bkl_depth,
                core::panic::Location::caller()
            );
        }
        pc.bkl_depth = 0;
        return;
    }
    trace(3, pc.bkl_depth);
    pc.bkl_depth -= 1;
    if pc.bkl_depth == 0 {
        unlock();
    }
}

/// Releases the lock completely (idle CPUs and waits for other CPUs).
/// Returns the depth to give back to [`bkl_reacquire`].
pub fn bkl_release_all() -> u64 {
    if !bkl_held() {
        return 0;
    }
    let pc = percpu::this();
    let d = pc.bkl_depth;
    trace(4, d);
    pc.bkl_depth = 0;
    unlock();
    d
}

pub fn bkl_reacquire(depth: u64) {
    if depth > 0 {
        bkl_enter();
        percpu::this().bkl_depth = depth;
    }
}

/// Lets CPUs waiting for the lock in first. Only for points where the
/// kernel may be preempted anyway (an interrupt that arrived with
/// interrupts enabled), so other kernel code may run before this CPU
/// continues — as it could on one CPU after a thread switch.
pub fn bkl_yield_if_contended() {
    if bkl_held() && NEXT_TICKET.load(Ordering::Relaxed) > SERVING.load(Ordering::Relaxed) + 1 {
        let d = bkl_release_all();
        bkl_reacquire(d);
    }
}

/// Depth of the lock on this CPU (saved and restored per thread by the
/// scheduler, which switches threads while holding the lock).
pub fn bkl_depth() -> u64 {
    if percpu::ready() {
        percpu::this().bkl_depth
    } else {
        0
    }
}

#[track_caller]
pub fn set_bkl_depth(depth: u64) {
    if percpu::ready() {
        trace(5, depth);
        percpu::this().bkl_depth = depth;
    }
}

/// Prints the big kernel lock state (test builds, from a watchdog).
#[cfg(feature = "ktest")]
pub fn debug_dump(w: &mut dyn core::fmt::Write) {
    let _ = writeln!(
        w,
        "bkl: owner {:#x} next {} serving {}",
        OWNER.load(Ordering::Relaxed),
        NEXT_TICKET.load(Ordering::Relaxed),
        SERVING.load(Ordering::Relaxed)
    );
    for c in 0..online() {
        let _ = writeln!(
            w,
            "cpu {}: waits for ticket {:#x}; last operations (op depth owner line):",
            c,
            trace::WAITING[c].load(Ordering::Relaxed)
        );
        let pos = trace::POS[c].load(Ordering::Relaxed) as usize;
        for k in 0..trace::N {
            let e = &trace::RING[c][(pos + k) % trace::N];
            let _ = writeln!(
                w,
                "  {} {} {:#x} {}",
                e[1].load(Ordering::Relaxed),
                e[2].load(Ordering::Relaxed),
                e[3].load(Ordering::Relaxed),
                e[4].load(Ordering::Relaxed)
            );
        }
    }
}

/// True if this CPU holds the lock.
pub fn bkl_held() -> bool {
    percpu::ready() && OWNER.load(Ordering::Relaxed) == percpu::this().index
}

// --- CPUs -----------------------------------------------------------------------

static ONLINE: AtomicUsize = AtomicUsize::new(1);
static PARKED: AtomicUsize = AtomicUsize::new(0);
static GO: AtomicBool = AtomicBool::new(false);
/// TSC of each AP right after it saw `GO` (to check the TSCs agree).
static AP_TSC_AT_GO: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Largest TSC difference between CPUs found at start-up, in cycles.
static TSC_SKEW: AtomicU64 = AtomicU64::new(0);

/// TSC difference between CPUs measured at start-up (cycles; 0 when
/// every AP's TSC was inside the boot CPU's window).
pub fn tsc_skew() -> u64 {
    TSC_SKEW.load(Ordering::Relaxed)
}
static APIC_IDS: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(u32::MAX) }; MAX_CPUS];
/// CPUs the firmware reported, including ones beyond `MAX_CPUS`.
static REPORTED: AtomicUsize = AtomicUsize::new(1);

/// Control registers of the boot CPU, copied to every AP.
static BSP_EFER: AtomicU64 = AtomicU64::new(0);
static BSP_PAT: AtomicU64 = AtomicU64::new(0);
static BSP_CR4: AtomicU64 = AtomicU64::new(0);

const MSR_PAT: u32 = 0x277;

struct ApBoot {
    index: usize,
    stack: KernelStack,
}

/// What an AP needs before it reaches the kernel's page tables: these
/// live in the kernel image, which the bootloader's tables map too (the
/// heap and the kernel stacks are not mapped there).
static AP_CR3: AtomicU64 = AtomicU64::new(0);
static AP_STACK_TOP: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// The `ApBoot` of each AP (heap), read once it runs on the kernel tables.
static AP_BOOT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// CPUs running the kernel (the boot CPU counts).
pub fn online() -> usize {
    ONLINE.load(Ordering::SeqCst)
}

/// CPUs the firmware reported.
pub fn reported() -> usize {
    REPORTED.load(Ordering::SeqCst)
}

pub fn this_cpu() -> usize {
    percpu::index()
}

/// Local APIC id of CPU `index`.
pub fn apic_id(index: usize) -> Option<u32> {
    match APIC_IDS.get(index)?.load(Ordering::Relaxed) {
        u32::MAX => None,
        id => Some(id),
    }
}

/// Makes CPU `index` run its scheduler soon (no-op for this CPU).
pub fn poke(index: usize) {
    if index == this_cpu() {
        return;
    }
    if let Some(id) = apic_id(index) {
        apic::send_ipi(id, VECTOR_RESCHEDULE);
    }
}

/// Stops the other CPUs (on a panic): they get an NMI and halt.
pub fn stop_others() {
    if online() > 1 {
        apic::send_nmi_to_others();
    }
}

fn read_cr4() -> u64 {
    let v: u64;
    // SAFETY: reading CR4 has no side effects.
    unsafe { asm!("mov {}, cr4", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// Starts the APs into a waiting loop on the kernel's page tables.
/// Must run before bootloader memory is released. Returns how many wait.
pub fn start_aps() -> usize {
    let Some(resp) = limine::MP.response() else {
        return 0;
    };
    // SAFETY: reading MSRs and CR3 of the boot CPU.
    unsafe {
        BSP_EFER.store(cpu::rdmsr(cpu::MSR_EFER), Ordering::SeqCst);
        BSP_PAT.store(cpu::rdmsr(MSR_PAT), Ordering::SeqCst);
    }
    BSP_CR4.store(read_cr4(), Ordering::SeqCst);
    AP_CR3.store(cpu::read_cr3(), Ordering::SeqCst);
    APIC_IDS[0].store(resp.bsp_lapic_id, Ordering::SeqCst);
    REPORTED.store(resp.cpus().len().max(1), Ordering::SeqCst);
    let mut next = 1;
    for info in resp.cpus() {
        if info.lapic_id == resp.bsp_lapic_id {
            continue;
        }
        if next >= MAX_CPUS {
            break;
        }
        let Some(stack) = kstack::alloc(8) else {
            break;
        };
        let boot = Box::leak(Box::new(ApBoot { index: next, stack }));
        AP_BOOT[next].store(boot as *const ApBoot as u64, Ordering::SeqCst);
        AP_STACK_TOP[next].store(stack.top, Ordering::SeqCst);
        APIC_IDS[next].store(info.lapic_id, Ordering::SeqCst);
        info.start(ap_entry, next as u64);
        next += 1;
    }
    let want = next - 1;
    // The TSC is not calibrated yet; a few hundred million spins are
    // seconds on any CPU.
    let mut spins = 0u64;
    while PARKED.load(Ordering::SeqCst) < want && spins < 500_000_000 {
        core::hint::spin_loop();
        spins += 1;
    }
    PARKED.load(Ordering::SeqCst)
}

/// First code on an AP, still on the bootloader's stack and page tables.
unsafe extern "C" fn ap_entry(info: *const MpInfo) -> ! {
    // SAFETY: Limine passes this CPU's info; `extra_argument` is the CPU
    // index written by `start_aps`. Only kernel-image statics are read
    // before CR3 is switched.
    unsafe {
        let index = (*info).extra_argument.load(Ordering::SeqCst);
        let top = AP_STACK_TOP[index as usize % MAX_CPUS].load(Ordering::SeqCst);
        // The kernel's page tables use the NX bit, which is a reserved
        // bit (a fault) unless EFER.NXE is set: set it before loading
        // them. PAT gives the write-combining type the framebuffer uses.
        const EFER_NXE: u64 = 1 << 11;
        let efer = cpu::rdmsr(cpu::MSR_EFER);
        cpu::wrmsr(
            cpu::MSR_EFER,
            efer | (BSP_EFER.load(Ordering::SeqCst) & EFER_NXE),
        );
        cpu::wrmsr(MSR_PAT, BSP_PAT.load(Ordering::SeqCst));
        asm!(
            "mov cr3, {cr3}",
            "mov rsp, {top}",
            "xor ebp, ebp",
            "call {main}",
            "ud2",
            cr3 = in(reg) AP_CR3.load(Ordering::SeqCst),
            top = in(reg) top,
            main = sym ap_main,
            in("rdi") index,
            options(noreturn)
        )
    }
}

extern "C" fn ap_main(index: u64) -> ! {
    // SAFETY: written by `start_aps` before this CPU was started; the
    // allocation is leaked and lives forever.
    let b = unsafe { &*(AP_BOOT[index as usize].load(Ordering::SeqCst) as *const ApBoot) };
    PARKED.fetch_add(1, Ordering::SeqCst);
    while !GO.load(Ordering::SeqCst) {
        core::hint::spin_loop();
    }
    AP_TSC_AT_GO[b.index].store(cpu::rdtsc(), Ordering::SeqCst);
    // The boot CPU's CR4 features (EFER and PAT were copied in
    // `ap_entry`).
    // SAFETY: values read from the boot CPU, which runs the same code.
    unsafe {
        let cr4 = read_cr4() | BSP_CR4.load(Ordering::SeqCst);
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }
    crate::arch::init_ap(b.index);
    apic::init_ap(crate::time::TICK_HZ);
    cpu::disable_interrupts();
    bkl_enter();
    crate::sched::add_cpu(b.index, b.stack);
    ONLINE.fetch_add(1, Ordering::SeqCst);
    crate::sched::run_idle()
}

fn on_reschedule(_: &mut crate::arch::x86_64::idt::InterruptFrame) {
    // Nothing to do: the scheduler runs on the way out of the interrupt.
}

/// Lets the parked APs join. The scheduler and the interrupt set-up must
/// be ready. Returns the number of CPUs online.
pub fn release_aps() -> usize {
    irq::set_handler(VECTOR_RESCHEDULE, on_reschedule);
    let parked = PARKED.load(Ordering::SeqCst);
    if parked == 0 {
        return 1;
    }
    // The APs need the big kernel lock to join.
    let depth = bkl_release_all();
    let before = cpu::rdtsc();
    GO.store(true, Ordering::SeqCst);
    let deadline = cpu::rdtsc() + crate::time::tsc_hz();
    while online() < parked + 1 && cpu::rdtsc() < deadline {
        core::hint::spin_loop();
    }
    let after = cpu::rdtsc();
    bkl_reacquire(depth);
    // Each AP read its TSC between `before` and `after` of the boot CPU,
    // unless the TSCs differ.
    for (i, slot) in AP_TSC_AT_GO.iter().enumerate().take(online()).skip(1) {
        let t = slot.load(Ordering::SeqCst);
        let off = if t < before {
            before - t
        } else {
            t.saturating_sub(after)
        };
        if off > TSC_SKEW.load(Ordering::Relaxed) {
            TSC_SKEW.store(off, Ordering::Relaxed);
        }
        if off > 0 {
            crate::kwarn!(
                "CPU {}: its TSC differs from the boot CPU's by at least {} us",
                i,
                crate::time::tsc_to_ns(off) / 1000
            );
        }
    }
    online()
}

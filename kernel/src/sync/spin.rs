use crate::arch::cpu;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Spinlock that also disables interrupts while held, so an interrupt
/// handler on the same CPU can never deadlock on a lock its thread holds.
pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: access to `data` is serialised by `locked`.
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        let irq = cpu::irq_save();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinLockGuard { lock: self, irq }
    }

    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        let irq = cpu::irq_save();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinLockGuard { lock: self, irq })
        } else {
            cpu::irq_restore(irq);
            None
        }
    }

    /// Releases the lock without a guard.
    ///
    /// # Safety
    /// Only for the panic path, where the holder will never run again.
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    irq: bool,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: we hold the lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: we hold the lock exclusively.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        cpu::irq_restore(self.irq);
    }
}

/// A cell for data that is written once during single-threaded early boot
/// (GDT, IDT, TSS) and must live at a fixed address.
#[repr(transparent)]
pub struct RacyCell<T>(UnsafeCell<T>);

// SAFETY: users promise to only mutate during single-threaded init.
unsafe impl<T> Sync for RacyCell<T> {}

impl<T> RacyCell<T> {
    pub const fn new(v: T) -> Self {
        Self(UnsafeCell::new(v))
    }

    pub fn get(&self) -> *mut T {
        self.0.get()
    }
}

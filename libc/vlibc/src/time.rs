//! Time since boot and sleeping.

use crate::sys::{syscall1, syscall2};
use crate::{Errno, check};
use core::time::Duration;
use velora_abi::{Timespec, clock, nr};

/// Time since boot (never goes backwards).
pub fn monotonic() -> Result<Duration, Errno> {
    let mut ts = Timespec::default();
    // SAFETY: `ts` is a valid, writable Timespec.
    check(unsafe {
        syscall2(
            nr::CLOCK_GETTIME,
            clock::MONOTONIC,
            &mut ts as *mut Timespec as u64,
        )
    })?;
    Ok(Duration::new(ts.seconds, ts.nanoseconds as u32))
}

/// Wall-clock time since 1970-01-01 UTC (from the RTC). `EAGAIN` if the
/// system does not know the time.
pub fn realtime() -> Result<Duration, Errno> {
    let mut ts = Timespec::default();
    // SAFETY: `ts` is a valid, writable Timespec.
    check(unsafe {
        syscall2(
            nr::CLOCK_GETTIME,
            clock::REALTIME,
            &mut ts as *mut Timespec as u64,
        )
    })?;
    Ok(Duration::new(ts.seconds, ts.nanoseconds as u32))
}

/// Sleeps at least `ms` milliseconds.
pub fn sleep_ms(ms: u64) {
    // SAFETY: no pointers.
    unsafe { syscall1(nr::SLEEP_MS, ms) };
}

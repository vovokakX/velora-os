//! Kernel log: the last 64 KiB of console output, kept in a static ring
//! buffer (no heap, so it also works during a panic). Read through
//! `/dev/kmsg` (`dmesg`) and saved in the panic log.

use crate::sync::SpinLock;

pub const SIZE: usize = 64 * 1024;

struct Ring {
    buf: [u8; SIZE],
    /// Total bytes ever written; the ring holds the last `min(total, SIZE)`.
    total: u64,
}

static LOG: SpinLock<Ring> = SpinLock::new(Ring {
    buf: [0; SIZE],
    total: 0,
});

pub fn append(s: &str) {
    let mut r = LOG.lock();
    for &b in s.as_bytes() {
        let at = (r.total % SIZE as u64) as usize;
        r.buf[at] = b;
        r.total += 1;
    }
}

/// Bytes currently kept.
pub fn len() -> usize {
    (LOG.lock().total as usize).min(SIZE)
}

/// Copies kept bytes starting at `offset` (0 = oldest kept byte) into
/// `out`; returns how many were copied.
pub fn read(offset: usize, out: &mut [u8]) -> usize {
    let r = LOG.lock();
    let kept = (r.total as usize).min(SIZE);
    if offset >= kept {
        return 0;
    }
    let start = r.total as usize - kept + offset;
    let n = out.len().min(kept - offset);
    for (i, o) in out[..n].iter_mut().enumerate() {
        *o = r.buf[(start + i) % SIZE];
    }
    n
}

/// Copies the newest bytes that fit into `out`, with ANSI escape
/// sequences removed (for files read outside a terminal). Returns the
/// length written.
pub fn newest_plain(out: &mut [u8]) -> usize {
    let r = LOG.lock();
    let kept = (r.total as usize).min(SIZE);
    let first = r.total as usize - kept;
    // Two passes: count the plain bytes, then copy the newest of them.
    let walk = |f: &mut dyn FnMut(u8)| {
        let mut esc = 0u8; // 0 = text, 1 = after ESC, 2 = inside CSI
        for i in 0..kept {
            let b = r.buf[(first + i) % SIZE];
            match (esc, b) {
                (0, 0x1b) => esc = 1,
                (1, b'[') => esc = 2,
                (1, _) => esc = 0,
                (2, 0x40..=0x7e) => esc = 0,
                (2, _) => {}
                (_, b'\r') => {}
                _ => f(b),
            }
        }
    };
    let mut total = 0usize;
    walk(&mut |_| total += 1);
    let mut skip = total.saturating_sub(out.len());
    let mut n = 0;
    walk(&mut |b| {
        if skip > 0 {
            skip -= 1;
        } else {
            out[n] = b;
            n += 1;
        }
    });
    n
}

/// # Safety
/// Only for the panic path, where the holder will never run again.
pub unsafe fn force_unlock() {
    // SAFETY: as documented for the caller.
    unsafe { LOG.force_unlock() };
}

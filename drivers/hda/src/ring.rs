//! The cyclic playback buffer. The controller reads it by DMA in a loop
//! and reports the byte it reached (the link position). The driver writes
//! samples ahead of that position and clears what was played to silence,
//! so the loop plays silence once the samples run out.

/// Bytes kept between the position and new samples after the buffer ran
/// empty (10 ms at 48 kHz stereo 16-bit): the controller has already
/// fetched a little ahead of the position it reports.
pub const LEAD: usize = 1920;

/// Positions are counted in bytes from the start, without wrapping.
#[derive(Clone, Debug)]
pub struct Ring {
    size: usize,
    written: u64,
    played: u64,
    pos: usize,
}

/// Parts of the buffer: (offset, length), at most two (wrapping).
pub type Spans = [(usize, usize); 2];

impl Ring {
    pub fn new(size: usize) -> Ring {
        Ring {
            size,
            written: LEAD as u64,
            played: 0,
            pos: 0,
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Bytes written and not played yet.
    pub fn pending(&self) -> usize {
        self.written.saturating_sub(self.played) as usize
    }

    /// Bytes that can be written now.
    pub fn free(&self) -> usize {
        self.size - self.pending().min(self.size)
    }

    /// The parts of the buffer that `start..start + len` (counted without
    /// wrapping) covers.
    fn spans(&self, start: u64, len: usize) -> Spans {
        let off = (start % self.size as u64) as usize;
        let first = len.min(self.size - off);
        [(off, first), (0, len - first)]
    }

    /// The controller reached `lpib`: counts what was played and returns
    /// the parts to clear to silence.
    pub fn advance(&mut self, lpib: usize) -> Spans {
        let lpib = lpib % self.size;
        let delta = (lpib + self.size - self.pos) % self.size;
        self.pos = lpib;
        let from = self.played;
        self.played += delta as u64;
        if self.written <= self.played {
            // Ran empty: new samples go a little ahead of the position.
            self.written = self.played + LEAD as u64;
        }
        self.spans(from, delta)
    }

    /// Takes up to `n` bytes (cut to whole frames and to [`Ring::free`])
    /// and returns where to copy them.
    pub fn reserve(&mut self, n: usize) -> (usize, Spans) {
        let n = n.min(self.free()) / crate::FRAME * crate::FRAME;
        let s = self.spans(self.written, n);
        self.written += n as u64;
        (n, s)
    }

    /// Back to the start (the stream was stopped and its position reset).
    pub fn reset(&mut self) {
        *self = Ring::new(self.size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_written_ahead_and_played_parts_cleared() {
        let mut r = Ring::new(16384);
        // Empty: everything but the lead is free; the first samples go
        // after the lead.
        assert_eq!(r.free(), 16384 - LEAD);
        let (n, s) = r.reserve(10_001);
        assert_eq!(n, 10_000);
        assert_eq!(s, [(LEAD, 10_000), (0, 0)]);
        assert_eq!(r.pending(), LEAD + 10_000);
        // The controller played 6000 bytes: they are cleared.
        assert_eq!(r.advance(6000), [(0, 6000), (0, 0)]);
        assert_eq!(r.pending(), LEAD + 4000);
        // Writing wraps around the end of the buffer.
        let (n, s) = r.reserve(10_000);
        assert_eq!(n, 10_000);
        assert_eq!(
            s,
            [
                (LEAD + 10_000, 16384 - LEAD - 10_000),
                (0, 10_000 - (16384 - LEAD - 10_000))
            ]
        );
        // Full: nothing more fits until something is played.
        assert_eq!(r.reserve(1000).0, 16384 - (LEAD + 14_000));
        assert_eq!(r.reserve(1000).0, 0);
        // Played past the end of the buffer: the position wrapped.
        let spans = r.advance(2000);
        assert_eq!(spans, [(6000, 10384), (0, 2000)]);
    }

    #[test]
    fn running_empty_puts_new_samples_after_the_position() {
        let mut r = Ring::new(16384);
        r.reserve(4000);
        // The controller played everything and more (silence).
        r.advance(9000);
        assert_eq!(r.pending(), LEAD);
        let (_, s) = r.reserve(400);
        assert_eq!(s[0].0, 9000 + LEAD);
    }
}

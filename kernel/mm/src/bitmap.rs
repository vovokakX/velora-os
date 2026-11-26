//! Bitmap allocator for physical page frames. One bit per frame:
//! 1 = in use (or not RAM), 0 = free.

#[derive(Debug, PartialEq, Eq)]
pub enum FreeError {
    /// The frame was not allocated (double free or bad address).
    NotAllocated(usize),
    OutOfRange(usize),
}

pub struct FrameBitmap<'a> {
    words: &'a mut [u64],
    frames: usize,
    free: usize,
    /// Word index where the next search starts (next-fit).
    hint: usize,
}

impl<'a> FrameBitmap<'a> {
    /// Words needed to track `frames` frames.
    pub const fn words_for(frames: usize) -> usize {
        frames.div_ceil(64)
    }

    /// Creates a bitmap with every frame marked as used.
    pub fn new(words: &'a mut [u64], frames: usize) -> Self {
        assert!(
            words.len() >= Self::words_for(frames),
            "bitmap storage too small"
        );
        words.fill(u64::MAX);
        Self {
            words,
            frames,
            free: 0,
            hint: 0,
        }
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn free_frames(&self) -> usize {
        self.free
    }

    fn is_set(&self, f: usize) -> bool {
        self.words[f / 64] & (1 << (f % 64)) != 0
    }

    fn set(&mut self, f: usize) {
        self.words[f / 64] |= 1 << (f % 64);
    }

    fn clear(&mut self, f: usize) {
        self.words[f / 64] &= !(1 << (f % 64));
    }

    /// Marks `[start, start + count)` as free. Frames past the end are ignored.
    pub fn mark_free(&mut self, start: usize, count: usize) {
        for f in start..(start + count).min(self.frames) {
            if self.is_set(f) {
                self.clear(f);
                self.free += 1;
            }
        }
    }

    /// Marks `[start, start + count)` as used. Frames past the end are ignored.
    pub fn mark_used(&mut self, start: usize, count: usize) {
        for f in start..(start + count).min(self.frames) {
            if !self.is_set(f) {
                self.set(f);
                self.free -= 1;
            }
        }
    }

    pub fn is_free(&self, f: usize) -> bool {
        f < self.frames && !self.is_set(f)
    }

    /// Allocates one frame.
    pub fn alloc(&mut self) -> Option<usize> {
        if self.free == 0 {
            return None;
        }
        let n = self.words.len();
        for i in 0..n {
            let w = (self.hint + i) % n;
            let word = self.words[w];
            if word != u64::MAX {
                let bit = (!word).trailing_zeros() as usize;
                let f = w * 64 + bit;
                if f >= self.frames {
                    continue;
                }
                self.set(f);
                self.free -= 1;
                self.hint = w;
                return Some(f);
            }
        }
        None
    }

    /// Allocates `count` contiguous frames whose first frame index is a
    /// multiple of `align` (in frames). Used for DMA buffers and huge pages.
    pub fn alloc_contiguous(&mut self, count: usize, align: usize) -> Option<usize> {
        if count == 0 || count > self.free {
            return None;
        }
        let align = align.max(1);
        let mut start = 0;
        while start + count <= self.frames {
            match (start..start + count).find(|&f| self.is_set(f)) {
                Some(used) => start = (used + 1).next_multiple_of(align),
                None => {
                    for f in start..start + count {
                        self.set(f);
                    }
                    self.free -= count;
                    return Some(start);
                }
            }
        }
        None
    }

    pub fn free(&mut self, f: usize) -> Result<(), FreeError> {
        if f >= self.frames {
            return Err(FreeError::OutOfRange(f));
        }
        if !self.is_set(f) {
            return Err(FreeError::NotAllocated(f));
        }
        self.clear(f);
        self.free += 1;
        if f / 64 < self.hint {
            self.hint = f / 64;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(frames: usize) -> (Vec<u64>, usize) {
        (vec![0; FrameBitmap::words_for(frames)], frames)
    }

    #[test]
    fn starts_fully_used() {
        let (mut w, n) = make(100);
        let mut b = FrameBitmap::new(&mut w, n);
        assert_eq!(b.free_frames(), 0);
        assert_eq!(b.alloc(), None);
    }

    #[test]
    fn alloc_and_free() {
        let (mut w, n) = make(200);
        let mut b = FrameBitmap::new(&mut w, n);
        b.mark_free(10, 5);
        assert_eq!(b.free_frames(), 5);
        let got: Vec<usize> = (0..5).map(|_| b.alloc().unwrap()).collect();
        assert_eq!(got, vec![10, 11, 12, 13, 14]);
        assert_eq!(b.alloc(), None);
        b.free(12).unwrap();
        assert_eq!(b.alloc(), Some(12));
    }

    #[test]
    fn double_free_and_range_are_errors() {
        let (mut w, n) = make(64);
        let mut b = FrameBitmap::new(&mut w, n);
        b.mark_free(0, 64);
        assert_eq!(b.free(3), Err(FreeError::NotAllocated(3)));
        assert_eq!(b.free(64), Err(FreeError::OutOfRange(64)));
        let f = b.alloc().unwrap();
        b.free(f).unwrap();
        assert_eq!(b.free(f), Err(FreeError::NotAllocated(f)));
    }

    #[test]
    fn marks_are_idempotent_and_clipped() {
        let (mut w, n) = make(70);
        let mut b = FrameBitmap::new(&mut w, n);
        b.mark_free(60, 100);
        assert_eq!(b.free_frames(), 10);
        b.mark_free(60, 5);
        assert_eq!(b.free_frames(), 10);
        b.mark_used(65, 2);
        b.mark_used(65, 2);
        assert_eq!(b.free_frames(), 8);
        // Bits past `frames` in the last word are never handed out.
        let mut all = Vec::new();
        while let Some(f) = b.alloc() {
            all.push(f);
        }
        assert!(all.iter().all(|&f| f < 70));
        assert_eq!(all.len(), 8);
    }

    #[test]
    fn contiguous_respects_alignment_and_holes() {
        let (mut w, n) = make(1024);
        let mut b = FrameBitmap::new(&mut w, n);
        b.mark_free(0, 1024);
        b.mark_used(5, 1); // hole
        let a = b.alloc_contiguous(8, 8).unwrap();
        assert_eq!(a, 8);
        let c = b.alloc_contiguous(512, 512).unwrap();
        assert_eq!(c, 512);
        assert_eq!(b.alloc_contiguous(600, 1), None);
        assert_eq!(b.free_frames(), 1024 - 1 - 8 - 512);
        for f in a..a + 8 {
            assert!(!b.is_free(f));
        }
    }

    #[test]
    fn every_frame_is_handed_out_once() {
        let (mut w, n) = make(1000);
        let mut b = FrameBitmap::new(&mut w, n);
        b.mark_free(0, 1000);
        let mut seen = vec![false; 1000];
        for _ in 0..1000 {
            let f = b.alloc().unwrap();
            assert!(!seen[f]);
            seen[f] = true;
        }
        assert_eq!(b.alloc(), None);
        for f in (0..1000).step_by(3) {
            b.free(f).unwrap();
        }
        assert_eq!(b.free_frames(), 334);
    }
}

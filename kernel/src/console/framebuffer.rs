//! Linear framebuffer access. This is also the `Display` abstraction the
//! graphical shell (Lumi) will build on later.

use crate::boot::limine::RawFramebuffer;

/// A 32-bit-per-pixel linear framebuffer.
pub struct Framebuffer {
    base: *mut u8,
    pub width: usize,
    pub height: usize,
    /// Bytes per scan line.
    pub pitch: usize,
    r_shift: u8,
    g_shift: u8,
    b_shift: u8,
}

// SAFETY: the framebuffer is only touched through the console lock.
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    /// Wraps the bootloader framebuffer. Returns `None` for formats the
    /// console does not draw (anything other than 32 bpp RGB).
    pub fn from_limine(fb: &RawFramebuffer) -> Option<Self> {
        if fb.bpp != 32 || fb.memory_model != 1 || fb.address.is_null() {
            return None;
        }
        Some(Self {
            base: fb.address,
            width: fb.width as usize,
            height: fb.height as usize,
            pitch: fb.pitch as usize,
            r_shift: fb.red_mask_shift,
            g_shift: fb.green_mask_shift,
            b_shift: fb.blue_mask_shift,
        })
    }

    /// Framebuffer over plain memory, in the usual XRGB8888 layout.
    ///
    /// # Safety
    /// `base` must point to `pitch * height` writable bytes that outlive
    /// the returned value.
    pub unsafe fn from_raw(base: *mut u8, width: usize, height: usize, pitch: usize) -> Self {
        Self {
            base,
            width,
            height,
            pitch,
            r_shift: 16,
            g_shift: 8,
            b_shift: 0,
        }
    }

    /// Converts 0xRRGGBB into the native pixel value.
    #[inline]
    pub fn native(&self, rgb: u32) -> u32 {
        let r = (rgb >> 16) & 0xff;
        let g = (rgb >> 8) & 0xff;
        let b = rgb & 0xff;
        (r << self.r_shift) | (g << self.g_shift) | (b << self.b_shift)
    }

    #[inline]
    fn row_ptr(&self, y: usize) -> *mut u32 {
        // SAFETY: callers keep y < height.
        unsafe { self.base.add(y * self.pitch).cast::<u32>() }
    }

    #[inline]
    pub fn put_native(&mut self, x: usize, y: usize, px: u32) {
        if x < self.width && y < self.height {
            // SAFETY: bounds checked above.
            unsafe { self.row_ptr(y).add(x).write_volatile(px) }
        }
    }

    pub fn get_native(&self, x: usize, y: usize) -> Option<u32> {
        if x < self.width && y < self.height {
            // SAFETY: bounds checked above.
            Some(unsafe { self.row_ptr(y).add(x).read_volatile() })
        } else {
            None
        }
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, rgb: u32) {
        let px = self.native(rgb);
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for yy in y..y_end {
            let row = self.row_ptr(yy);
            for xx in x..x_end {
                // SAFETY: clipped to the framebuffer above.
                unsafe { row.add(xx).write_volatile(px) }
            }
        }
    }

    /// Draws an 8-pixel-wide glyph with explicit foreground and background.
    pub fn draw_glyph(&mut self, x: usize, y: usize, rows: &[u8; 16], fg: u32, bg: u32) {
        if x + 8 > self.width || y + 16 > self.height {
            return;
        }
        let fg = self.native(fg);
        let bg = self.native(bg);
        for (dy, bits) in rows.iter().enumerate() {
            let row = self.row_ptr(y + dy);
            for dx in 0..8 {
                let px = if bits & (0x80 >> dx) != 0 { fg } else { bg };
                // SAFETY: the whole 8x16 cell is inside the framebuffer.
                unsafe { row.add(x + dx).write_volatile(px) }
            }
        }
    }

    /// Moves the image up by `lines` pixel rows and clears the bottom part.
    pub fn scroll_up(&mut self, lines: usize, rgb: u32) {
        if lines >= self.height {
            self.fill_rect(0, 0, self.width, self.height, rgb);
            return;
        }
        let bytes = (self.height - lines) * self.pitch;
        // SAFETY: both ranges are inside the framebuffer; `copy` handles overlap.
        unsafe { core::ptr::copy(self.base.add(lines * self.pitch), self.base, bytes) }
        self.fill_rect(0, self.height - lines, self.width, lines, rgb);
    }
}

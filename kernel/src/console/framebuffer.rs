//! Linear framebuffer access. This is also the `Display` abstraction the
//! graphical shell (Lumi) will build on later.

use crate::boot::limine::RawFramebuffer;
use alloc::boxed::Box;
use alloc::vec;

/// A 32-bit-per-pixel linear framebuffer.
///
/// With a back buffer enabled, every pixel is also kept in ordinary RAM.
/// Reads and scrolling then use RAM only; video memory is only written.
/// Reading video memory is very slow on real hardware.
pub struct Framebuffer {
    base: *mut u8,
    back: Option<Box<[u32]>>,
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
            back: None,
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
            back: None,
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

    /// Converts a native pixel value back into 0xRRGGBB.
    #[inline]
    pub fn rgb(&self, native: u32) -> u32 {
        let r = (native >> self.r_shift) & 0xff;
        let g = (native >> self.g_shift) & 0xff;
        let b = (native >> self.b_shift) & 0xff;
        (r << 16) | (g << 8) | b
    }

    #[inline]
    fn row_ptr(&self, y: usize) -> *mut u32 {
        // SAFETY: callers keep y < height.
        unsafe { self.base.add(y * self.pitch).cast::<u32>() }
    }

    /// Allocates the back buffer and fills it from the current screen.
    /// Needs the kernel heap.
    pub fn enable_back_buffer(&mut self) {
        if self.back.is_some() {
            return;
        }
        let mut back = vec![0u32; self.width * self.height].into_boxed_slice();
        // One bulk copy per row: much faster than per-pixel volatile reads.
        for y in 0..self.height {
            // SAFETY: row `y` is inside both buffers.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.row_ptr(y),
                    back[y * self.width..].as_mut_ptr(),
                    self.width,
                )
            }
        }
        self.back = Some(back);
    }

    pub fn has_back_buffer(&self) -> bool {
        self.back.is_some()
    }

    #[inline]
    fn write_px(&mut self, x: usize, y: usize, px: u32) {
        if let Some(b) = self.back.as_mut() {
            b[y * self.width + x] = px;
        }
        // SAFETY: callers keep (x, y) inside the framebuffer.
        unsafe { self.row_ptr(y).add(x).write_volatile(px) }
    }

    #[inline]
    pub fn put_native(&mut self, x: usize, y: usize, px: u32) {
        if x < self.width && y < self.height {
            self.write_px(x, y, px);
        }
    }

    pub fn get_native(&self, x: usize, y: usize) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(match &self.back {
            Some(b) => b[y * self.width + x],
            // SAFETY: bounds checked above.
            None => unsafe { self.row_ptr(y).add(x).read_volatile() },
        })
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, rgb: u32) {
        let px = self.native(rgb);
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for yy in y..y_end {
            for xx in x..x_end {
                self.write_px(xx, yy, px);
            }
        }
    }

    /// Draws an 8x16 glyph enlarged `scale` times, with explicit
    /// foreground and background.
    pub fn draw_glyph(
        &mut self,
        x: usize,
        y: usize,
        rows: &[u8; 16],
        fg: u32,
        bg: u32,
        scale: usize,
    ) {
        if x + 8 * scale > self.width || y + 16 * scale > self.height {
            return;
        }
        let fg = self.native(fg);
        let bg = self.native(bg);
        for (dy, bits) in rows.iter().enumerate() {
            for dx in 0..8 {
                let px = if bits & (0x80 >> dx) != 0 { fg } else { bg };
                for sy in 0..scale {
                    for sx in 0..scale {
                        self.write_px(x + dx * scale + sx, y + dy * scale + sy, px);
                    }
                }
            }
        }
    }

    /// Inverts the colours of a rectangle (used for the text cursor;
    /// inverting twice restores the image).
    pub fn invert_rect(&mut self, x: usize, y: usize, w: usize, h: usize) {
        let mask = self.native(0xff_ffff);
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for yy in y..y_end {
            for xx in x..x_end {
                if let Some(px) = self.get_native(xx, yy) {
                    self.write_px(xx, yy, px ^ mask);
                }
            }
        }
    }

    /// Moves the image up by `lines` pixel rows and clears the bottom part.
    pub fn scroll_up(&mut self, lines: usize, rgb: u32) {
        if lines >= self.height {
            self.fill_rect(0, 0, self.width, self.height, rgb);
            return;
        }
        let keep = self.height - lines;
        match self.back.as_mut() {
            Some(b) => {
                let w = self.width;
                b.copy_within(lines * w.., 0);
                // Present the moved rows: video memory is only written.
                for y in 0..keep {
                    // SAFETY: row `y` is inside both buffers.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            b[y * w..].as_ptr(),
                            self.base.add(y * self.pitch).cast::<u32>(),
                            w,
                        )
                    }
                }
            }
            None => {
                // SAFETY: both ranges are inside the framebuffer; `copy`
                // handles the overlap.
                unsafe {
                    core::ptr::copy(
                        self.base.add(lines * self.pitch),
                        self.base,
                        keep * self.pitch,
                    )
                }
            }
        }
        self.fill_rect(0, keep, self.width, lines, rgb);
    }
}

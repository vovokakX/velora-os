//! The real screen and input devices for programs on Velora (feature
//! `velora`): the frame buffer mapped with `display`, `/dev/keyboard` and
//! `/dev/pointer` (mice and tablets; `/dev/mouse` on older kernels).
//!
//! The program draws into [`Screen::canvas`] (the back buffer) and calls
//! [`Screen::present`] with the part that may have changed. A copy of what
//! video memory shows is kept, and only the pixels that differ from it
//! are written, row span by row span: video memory is slow to write, and
//! a frame shows whole (double buffering) without the cost of copying the
//! whole screen. The mouse pointer is drawn on the way to video memory and
//! never into the canvas, so moving it costs two small copies.

use crate::canvas::Canvas;
use crate::geom::Rect;
use crate::ui::{Event, Key};
use alloc::vec::Vec;
use velora_abi::{DisplayInfo, key, pointer};
use velora_gfx::image::Image;
use vlibc::fs::File;
use vlibc::{Errno, display};

pub struct Screen {
    fb: *mut u8,
    pub info: DisplayInfo,
    pub canvas: Canvas,
    keyboard: Option<File>,
    mouse: Option<File>,
    /// `/dev/pointer` (8-byte packets) rather than `/dev/mouse`.
    packets8: bool,
    /// What video memory shows (without the pointer).
    front: Vec<u32>,
    front_valid: bool,
    pub pointer_x: i32,
    pub pointer_y: i32,
    /// Pointer speed: pixels per mouse count, times 256.
    pub speed: i32,
    buttons: u8,
    pointer: Option<(Image, i32, i32)>,
    /// Where the pointer is on the screen now.
    shown: Option<Rect>,
    native: bool,
}

impl Screen {
    /// Takes the screen. Fails without a frame buffer or if another
    /// program owns it.
    pub fn open() -> Result<Screen, Errno> {
        let info = display::info()?;
        let fb = display::map()?;
        let (w, h) = (info.width as usize, info.height as usize);
        let native = info.red_shift == 16 && info.green_shift == 8 && info.blue_shift == 0;
        let (mouse, packets8) = match File::open("/dev/pointer") {
            Ok(f) => (Some(f), true),
            Err(_) => (File::open("/dev/mouse").ok(), false),
        };
        Ok(Screen {
            fb,
            info,
            canvas: Canvas::new(w, h),
            keyboard: File::open("/dev/keyboard").ok(),
            mouse,
            packets8,
            front: alloc::vec![0; w * h],
            front_valid: false,
            pointer_x: w as i32 / 2,
            pointer_y: h as i32 / 2,
            speed: 256,
            buttons: 0,
            pointer: None,
            shown: None,
            native,
        })
    }

    pub fn width(&self) -> i32 {
        self.info.width as i32
    }

    pub fn height(&self) -> i32 {
        self.info.height as i32
    }

    /// Sets the pointer picture and its hot spot (None hides it).
    pub fn set_pointer(&mut self, img: Option<(&Image, i32, i32)>) {
        self.pointer = img.map(|(i, x, y)| {
            (
                Image {
                    width: i.width,
                    height: i.height,
                    data: i.data.clone(),
                },
                x,
                y,
            )
        });
        self.present(Rect::default());
    }

    fn pointer_rect(&self) -> Option<Rect> {
        self.pointer.as_ref().map(|(img, hx, hy)| {
            Rect::new(
                self.pointer_x - hx,
                self.pointer_y - hy,
                img.width as i32,
                img.height as i32,
            )
        })
    }

    /// Reads the input devices without waiting.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        if let Some(k) = self.keyboard.as_mut() {
            while let Ok(n) = k.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for p in buf[..n].chunks_exact(key::PACKET) {
                    let code = u16::from_le_bytes([p[0], p[1]]);
                    let flags = p[2];
                    let ch = u32::from_le_bytes([p[4], p[5], p[6], p[7]]);
                    if flags & key::PRESSED == 0 {
                        continue;
                    }
                    let k = match code {
                        key::ESCAPE => Key::Escape,
                        key::TAB => Key::Tab,
                        key::ENTER => Key::Enter,
                        key::BACKSPACE => Key::Backspace,
                        key::DELETE => Key::Delete,
                        key::LEFT => Key::Left,
                        key::RIGHT => Key::Right,
                        key::UP => Key::Up,
                        key::DOWN => Key::Down,
                        key::HOME => Key::Home,
                        key::END => Key::End,
                        key::PAGE_UP => Key::PageUp,
                        key::PAGE_DOWN => Key::PageDown,
                        key::SPACE => Key::Space,
                        key::CHAR => match char::from_u32(ch) {
                            Some(c) => Key::Char(c),
                            None => continue,
                        },
                        _ => continue,
                    };
                    out.push(Event::Key {
                        key: k,
                        shift: flags & key::SHIFT != 0,
                    });
                }
            }
        }
        // Pointer packets as (buttons, dx, dy, wheel, absolute position).
        let mut packets: Vec<(u8, i32, i32, i32, Option<(i32, i32)>)> = Vec::new();
        if let Some(mut m) = self.mouse.take() {
            while let Ok(n) = m.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if self.packets8 {
                    for p in buf[..n].chunks_exact(pointer::PACKET) {
                        let x = i16::from_le_bytes([p[4], p[5]]) as i32;
                        let y = i16::from_le_bytes([p[6], p[7]]) as i32;
                        let abs = (p[0] & pointer::ABSOLUTE != 0).then_some((x, y));
                        let (dx, dy) = if abs.is_some() { (0, 0) } else { (x, y) };
                        packets.push((p[1], dx, dy, p[2] as i8 as i32, abs));
                    }
                } else {
                    for p in buf[..n].chunks_exact(4) {
                        packets.push((
                            p[0],
                            p[1] as i8 as i32,
                            p[2] as i8 as i32,
                            p[3] as i8 as i32,
                            None,
                        ));
                    }
                }
            }
            self.mouse = Some(m);
        }
        let (mut dx, mut dy) = (0i32, 0i32);
        let mut moved = false;
        for (buttons, pdx, pdy, wheel, abs) in packets {
            if let Some((ax, ay)) = abs {
                let (w, h) = (self.width() as i64, self.height() as i64);
                let x = (ax as i64 * (w - 1) / pointer::ABS_MAX as i64) as i32;
                let y = (ay as i64 * (h - 1) / pointer::ABS_MAX as i64) as i32;
                if (x, y) != (self.pointer_x, self.pointer_y) {
                    self.pointer_x = x;
                    self.pointer_y = y;
                    moved = true;
                }
            } else {
                dx += pdx;
                dy += pdy;
            }
            // The devices count the wheel away from the user; the event
            // counts towards the user (down the list).
            if wheel != 0 {
                self.flush_motion(&mut dx, &mut dy, &mut moved, &mut out);
                out.push(Event::Wheel(-wheel));
            }
            let left = buttons & 1;
            if left != self.buttons & 1 {
                // Movement before the button change counts first.
                self.flush_motion(&mut dx, &mut dy, &mut moved, &mut out);
                out.push(Event::MouseButton { down: left != 0 });
            }
            self.buttons = buttons;
        }
        self.flush_motion(&mut dx, &mut dy, &mut moved, &mut out);
        out
    }

    /// Applies the movement gathered so far and reports the position.
    fn flush_motion(&mut self, dx: &mut i32, dy: &mut i32, moved: &mut bool, out: &mut Vec<Event>) {
        if *dx != 0 || *dy != 0 {
            self.pointer_x = (self.pointer_x + *dx * self.speed / 256).clamp(0, self.width() - 1);
            self.pointer_y = (self.pointer_y + *dy * self.speed / 256).clamp(0, self.height() - 1);
            *dx = 0;
            *dy = 0;
            *moved = true;
        }
        if *moved {
            *moved = false;
            out.push(Event::MouseMove {
                x: self.pointer_x,
                y: self.pointer_y,
            });
            self.present(Rect::default());
        }
    }

    /// Shows `dirty` of the canvas (only the pixels that changed are
    /// written) and redraws the pointer.
    pub fn present(&mut self, dirty: Rect) {
        let bounds = self.canvas.bounds();
        let now = self.pointer_rect();
        // Where the pointer was: back to the picture under it.
        if let Some(old) = self.shown {
            let old = old.intersect(bounds);
            if !old.is_empty() {
                self.copy(old, true);
            }
        }
        let area = dirty.intersect(bounds);
        if !area.is_empty() {
            self.copy(area, !self.front_valid);
            if area == bounds {
                self.front_valid = true;
            }
        }
        if let Some(r) = now {
            self.draw_pointer(r.intersect(bounds));
        }
        self.shown = now;
    }

    /// Copies the whole canvas to the screen.
    pub fn present_all(&mut self) {
        let b = self.canvas.bounds();
        self.present(b);
    }

    fn row(&self, y: i32) -> *mut u32 {
        // SAFETY: callers keep y on the screen.
        unsafe {
            self.fb
                .add(y as usize * self.info.pitch as usize)
                .cast::<u32>()
        }
    }

    fn convert(&self, p: u32) -> u32 {
        if self.native {
            p
        } else {
            ((p >> 16) & 0xff) << self.info.red_shift
                | ((p >> 8) & 0xff) << self.info.green_shift
                | (p & 0xff) << self.info.blue_shift
        }
    }

    /// Copies `r` to video memory: all of it if `force`, else only the
    /// span of each row that differs from what is shown.
    fn copy(&mut self, r: Rect, force: bool) {
        let w = self.canvas.width;
        for y in r.y..r.bottom() {
            let row = y as usize * w;
            let (mut a, mut b) = (row + r.x as usize, row + r.right() as usize);
            if !force {
                let cur = &self.canvas.pixels[a..b];
                let old = &self.front[a..b];
                let Some(first) = cur.iter().zip(old).position(|(p, q)| p != q) else {
                    continue;
                };
                let last = cur.iter().zip(old).rposition(|(p, q)| p != q).unwrap();
                b = a + last + 1;
                a += first;
            }
            self.front[a..b].copy_from_slice(&self.canvas.pixels[a..b]);
            let x0 = (a - row) as i32;
            let src = &self.canvas.pixels[a..b];
            let dst = self.row(y);
            if self.native {
                // SAFETY: the row segment is inside the frame buffer.
                unsafe {
                    core::ptr::copy_nonoverlapping(src.as_ptr(), dst.add(x0 as usize), src.len())
                }
            } else {
                for (i, p) in src.iter().enumerate() {
                    // SAFETY: as above.
                    unsafe { dst.add(x0 as usize + i).write_volatile(self.convert(*p)) }
                }
            }
        }
    }

    fn draw_pointer(&mut self, r: Rect) {
        let Some((img, _, _)) = self.pointer.as_ref() else {
            return;
        };
        let pr = self.pointer_rect().unwrap();
        let w = self.canvas.width;
        for y in r.y..r.bottom() {
            let dst = self.row(y);
            for x in r.x..r.right() {
                let si = ((y - pr.y) as usize * img.width + (x - pr.x) as usize) * 4;
                let s = &img.data[si..si + 4];
                let d = self.canvas.pixels[y as usize * w + x as usize];
                let a = s[3] as u32;
                let inv = 255 - a;
                let ch =
                    |sh: u32, sv: u8| (sv as u32 + (((d >> sh) & 0xff) * inv + 127) / 255).min(255);
                let p = ch(16, s[0]) << 16 | ch(8, s[1]) << 8 | ch(0, s[2]);
                // SAFETY: (x, y) is on the screen.
                unsafe { dst.add(x as usize).write_volatile(self.convert(p)) }
            }
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = display::release();
    }
}

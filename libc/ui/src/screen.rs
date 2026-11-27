//! The real screen and input devices for programs on Velora (feature
//! `velora`): the frame buffer mapped with `display`, `/dev/keyboard` and
//! `/dev/mouse`.
//!
//! The program draws into [`Screen::canvas`] (the back buffer) and calls
//! [`Screen::present`] with the part that changed; only that part is
//! copied to video memory. The mouse pointer is drawn on the way to video
//! memory and never into the canvas, so moving it costs two small copies.

use crate::canvas::Canvas;
use crate::geom::Rect;
use crate::ui::{Event, Key};
use alloc::vec::Vec;
use velora_abi::{DisplayInfo, key};
use velora_gfx::image::Image;
use vlibc::fs::File;
use vlibc::{Errno, display};

pub struct Screen {
    fb: *mut u8,
    pub info: DisplayInfo,
    pub canvas: Canvas,
    keyboard: Option<File>,
    mouse: Option<File>,
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
        Ok(Screen {
            fb,
            info,
            canvas: Canvas::new(w, h),
            keyboard: File::open("/dev/keyboard").ok(),
            mouse: File::open("/dev/mouse").ok(),
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
        let (mut dx, mut dy) = (0i32, 0i32);
        if let Some(mut m) = self.mouse.take() {
            while let Ok(n) = m.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for p in buf[..n].chunks_exact(4) {
                    dx += p[1] as i8 as i32;
                    dy += p[2] as i8 as i32;
                    // /dev/mouse: positive is away from the user; the
                    // event counts towards the user (down the list).
                    let wheel = -(p[3] as i8 as i32);
                    if wheel != 0 {
                        out.push(Event::Wheel(wheel));
                    }
                    let left = p[0] & 1;
                    if left != self.buttons & 1 {
                        // Movement before the button change counts first.
                        if dx != 0 || dy != 0 {
                            self.move_pointer(dx, dy, &mut out);
                            dx = 0;
                            dy = 0;
                        }
                        out.push(Event::MouseButton { down: left != 0 });
                    }
                    self.buttons = p[0];
                }
            }
            self.mouse = Some(m);
        }
        if dx != 0 || dy != 0 {
            self.move_pointer(dx, dy, &mut out);
        }
        out
    }

    fn move_pointer(&mut self, dx: i32, dy: i32, out: &mut Vec<Event>) {
        self.pointer_x = (self.pointer_x + dx * self.speed / 256).clamp(0, self.width() - 1);
        self.pointer_y = (self.pointer_y + dy * self.speed / 256).clamp(0, self.height() - 1);
        out.push(Event::MouseMove {
            x: self.pointer_x,
            y: self.pointer_y,
        });
        self.present(Rect::default());
    }

    /// Copies `dirty` of the canvas to the screen and redraws the pointer.
    pub fn present(&mut self, dirty: Rect) {
        let bounds = self.canvas.bounds();
        let now = self.pointer_rect();
        // Rows to copy plainly: the change and where the pointer was.
        let mut area = dirty.intersect(bounds);
        if let Some(old) = self.shown {
            area = area.union(old.intersect(bounds));
        }
        if !area.is_empty() {
            self.copy(area);
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

    fn copy(&mut self, r: Rect) {
        let w = self.canvas.width;
        for y in r.y..r.bottom() {
            let src = &self.canvas.pixels
                [y as usize * w + r.x as usize..y as usize * w + r.right() as usize];
            let dst = self.row(y);
            if self.native {
                // SAFETY: the row segment is inside the frame buffer.
                unsafe {
                    core::ptr::copy_nonoverlapping(src.as_ptr(), dst.add(r.x as usize), src.len())
                }
            } else {
                for (i, p) in src.iter().enumerate() {
                    // SAFETY: as above.
                    unsafe { dst.add(r.x as usize + i).write_volatile(self.convert(*p)) }
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

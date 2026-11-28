//! PS/2 mouse packets: 3 bytes (buttons and movement), or 4 with a wheel
//! (IntelliMouse mode).

/// One decoded packet, in screen directions (y grows downwards, the
/// wheel is positive when turned towards the user).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// Bit 0 left, bit 1 right, bit 2 middle.
    pub buttons: u8,
    pub dx: i8,
    pub dy: i8,
    pub wheel: i8,
}

/// Assembles packets byte by byte.
pub struct Packets {
    len: usize,
    buf: [u8; 4],
    n: usize,
}

impl Packets {
    /// `wheel`: the mouse answered the IntelliMouse sequence (id 3).
    pub const fn new(wheel: bool) -> Self {
        Packets {
            len: if wheel { 4 } else { 3 },
            buf: [0; 4],
            n: 0,
        }
    }

    /// Adds a byte; returns a finished packet.
    pub fn feed(&mut self, b: u8) -> Option<Motion> {
        // The first byte always has bit 3 set; resynchronise otherwise.
        if self.n == 0 && b & 0x08 == 0 {
            return None;
        }
        self.buf[self.n] = b;
        self.n += 1;
        if self.n < self.len {
            return None;
        }
        self.n = 0;
        let [flags, x, y, z] = self.buf;
        if flags & 0xc0 != 0 {
            return None; // overflow: the movement is unknown
        }
        let dx = x as i32 - if flags & 0x10 != 0 { 256 } else { 0 };
        let dy = y as i32 - if flags & 0x20 != 0 { 256 } else { 0 };
        let wheel = if self.len == 4 {
            // 4-bit two's complement.
            let v = (z & 0x0f) as i32;
            if v >= 8 { v - 16 } else { v }
        } else {
            0
        };
        Some(Motion {
            buttons: flags & 7,
            dx: dx.clamp(-127, 127) as i8,
            // PS/2 counts up as positive.
            dy: (-dy).clamp(-127, 127) as i8,
            wheel: wheel as i8,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets() {
        let mut p = Packets::new(false);
        // Left button, right 5, up 3.
        assert_eq!(p.feed(0x09), None);
        assert_eq!(p.feed(5), None);
        assert_eq!(
            p.feed(3),
            Some(Motion {
                buttons: 1,
                dx: 5,
                dy: -3,
                wheel: 0
            })
        );
        // Left 2 and down 4 (sign bits), no buttons.
        let m = [0x38, 0xfe, 0xfc].iter().filter_map(|&b| p.feed(b)).next();
        assert_eq!(m.map(|m| (m.dx, m.dy, m.buttons)), Some((-2, 4, 0)));
        // A byte without bit 3 at the start is skipped (resync).
        assert_eq!(p.feed(0x00), None);
        let m = [0x08, 1, 1].iter().filter_map(|&b| p.feed(b)).next();
        assert_eq!(m.map(|m| m.dx), Some(1));
        // Overflow packets are dropped.
        let m = [0x48, 1, 1].iter().filter_map(|&b| p.feed(b)).next();
        assert_eq!(m, None);
        // Wheel mice send a fourth byte.
        let mut w = Packets::new(true);
        let m = [0x08, 0, 0, 0x0f].iter().filter_map(|&b| w.feed(b)).next();
        assert_eq!(m.map(|m| m.wheel), Some(-1));
    }
}

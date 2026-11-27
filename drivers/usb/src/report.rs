//! HID report descriptors (HID 1.11, section 6.2.2) for pointing devices
//! in report protocol: tablets (absolute X and Y, such as QEMU's
//! `usb-tablet`) and mice with more than the boot report.
//!
//! Only what a pointer needs is understood: the X, Y and wheel fields of
//! the Generic Desktop page, the first three buttons, report IDs and
//! padding. The first report that has both X and Y is used.

use alloc::vec::Vec;

/// Position of a value in a report: bit offset (after the report ID),
/// size in bits and its logical range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub offset: u32,
    pub size: u32,
    pub min: i32,
    pub max: i32,
    /// Relative (a movement) rather than absolute (a position).
    pub relative: bool,
}

impl Field {
    /// Reads the field from report data (without the report ID byte).
    pub fn read(&self, data: &[u8]) -> Option<i32> {
        if self.size == 0 || self.size > 32 {
            return None;
        }
        let mut v: u64 = 0;
        for i in 0..self.size {
            let bit = self.offset + i;
            let byte = *data.get((bit / 8) as usize)?;
            if (byte >> (bit % 8)) & 1 != 0 {
                v |= 1 << i;
            }
        }
        // Signed when the logical minimum is negative.
        if self.min < 0 && self.size < 32 && (v >> (self.size - 1)) & 1 != 0 {
            v |= !0u64 << self.size;
        }
        Some(v as i64 as i32)
    }
}

/// The layout of a pointer's reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pointer {
    pub report_id: Option<u8>,
    pub x: Field,
    pub y: Field,
    pub wheel: Option<Field>,
    /// Buttons 1..=3 (left, right, middle).
    pub buttons: [Option<Field>; 3],
    /// Bytes in a report, with the report ID byte.
    pub bytes: usize,
}

/// One report read with a [`Pointer`] layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerReport {
    /// Bit 0 left, 1 right, 2 middle.
    pub buttons: u8,
    /// A position from 0 to [`ABS_MAX`] when absolute, else a movement.
    pub x: i32,
    pub y: i32,
    pub absolute: bool,
    pub wheel: i8,
}

/// Absolute positions are scaled to 0..=ABS_MAX.
pub const ABS_MAX: i32 = 32767;

impl Pointer {
    pub fn absolute(&self) -> bool {
        !self.x.relative && !self.y.relative
    }

    /// Decodes a report (as it arrived, with the report ID if the device
    /// uses them). None for reports of another ID or too short.
    pub fn parse(&self, r: &[u8]) -> Option<PointerReport> {
        let data = match self.report_id {
            Some(id) => {
                if r.first() != Some(&id) {
                    return None;
                }
                &r[1..]
            }
            None => r,
        };
        let mut buttons = 0;
        for (i, b) in self.buttons.iter().enumerate() {
            if let Some(f) = b
                && f.read(data)? != 0
            {
                buttons |= 1 << i;
            }
        }
        let wheel = match self.wheel {
            Some(f) => f.read(data)?.clamp(-127, 127) as i8,
            None => 0,
        };
        let absolute = self.absolute();
        let scale = |f: &Field, v: i32| {
            if !absolute {
                return v;
            }
            let span = (f.max as i64 - f.min as i64).max(1);
            ((v as i64 - f.min as i64).clamp(0, span) * ABS_MAX as i64 / span) as i32
        };
        Some(PointerReport {
            buttons,
            x: scale(&self.x, self.x.read(data)?),
            y: scale(&self.y, self.y.read(data)?),
            absolute,
            wheel,
        })
    }
}

#[derive(Clone, Copy, Default)]
struct Globals {
    page: u32,
    min: i32,
    max: i32,
    size: u32,
    count: u32,
    id: u8,
}

/// A field found while reading the descriptor.
struct Found {
    id: u8,
    page: u32,
    usage: u32,
    field: Field,
}

fn value(data: &[u8]) -> u32 {
    data.iter()
        .enumerate()
        .fold(0u32, |v, (i, &b)| v | (b as u32) << (8 * i))
}

fn signed(data: &[u8]) -> i32 {
    match data.len() {
        1 => data[0] as i8 as i32,
        2 => i16::from_le_bytes([data[0], data[1]]) as i32,
        4 => i32::from_le_bytes([data[0], data[1], data[2], data[3]]),
        _ => 0,
    }
}

/// Finds the pointer in a report descriptor.
pub fn parse_pointer(desc: &[u8]) -> Option<Pointer> {
    let mut g = Globals::default();
    let mut stack: Vec<Globals> = Vec::new();
    let mut usages: Vec<u32> = Vec::new();
    let (mut umin, mut umax) = (None::<u32>, None::<u32>);
    // Bit offsets of the input reports by ID.
    let mut offsets = [0u32; 256];
    let mut found: Vec<Found> = Vec::new();
    let mut ids_used = false;
    let mut i = 0;
    while i < desc.len() {
        let prefix = desc[i];
        if prefix == 0xfe {
            // A long item: size, tag, data.
            let len = *desc.get(i + 1)? as usize;
            i += 3 + len;
            continue;
        }
        let size = match prefix & 3 {
            3 => 4,
            n => n as usize,
        };
        let data = desc.get(i + 1..i + 1 + size)?;
        i += 1 + size;
        let kind = (prefix >> 2) & 3;
        let tag = prefix >> 4;
        let v = value(data);
        match (kind, tag) {
            // Main items: input.
            (0, 8) => {
                let constant = v & 1 != 0;
                let relative = v & 4 != 0;
                let id = g.id as usize;
                for n in 0..g.count {
                    let usage = if !usages.is_empty() {
                        usages[(n as usize).min(usages.len() - 1)]
                    } else if let Some(lo) = umin {
                        let u = lo + n;
                        if umax.is_some_and(|hi| u > hi) { 0 } else { u }
                    } else {
                        0
                    };
                    // A usage may carry its page in the top 16 bits.
                    let (page, usage) = if usage > 0xffff {
                        (usage >> 16, usage & 0xffff)
                    } else {
                        (g.page, usage)
                    };
                    if !constant && usage != 0 {
                        found.push(Found {
                            id: g.id,
                            page,
                            usage,
                            field: Field {
                                offset: offsets[id] + n * g.size,
                                size: g.size,
                                min: g.min,
                                max: g.max,
                                relative,
                            },
                        });
                    }
                }
                offsets[id] += g.size * g.count;
                usages.clear();
                umin = None;
                umax = None;
            }
            // Output, feature, collection, end of collection.
            (0, 9) | (0, 10) | (0, 11) | (0, 12) => {
                usages.clear();
                umin = None;
                umax = None;
            }
            // Global items.
            (1, 0) => g.page = v,
            (1, 1) => g.min = signed(data),
            (1, 2) => {
                g.max = signed(data);
                // An unsigned maximum such as 0x7fff written in two bytes
                // reads as positive; one such as 0xff in one byte with a
                // minimum of 0 is unsigned too.
                if g.max < 0 && g.min >= 0 {
                    g.max = v as i32;
                }
            }
            (1, 7) => g.size = v,
            (1, 8) => {
                g.id = v as u8;
                ids_used = true;
            }
            (1, 9) => g.count = v,
            (1, 10) => stack.push(g),
            (1, 11) => g = stack.pop()?,
            // Local items.
            (2, 0) => usages.push(v),
            (2, 1) => umin = Some(v),
            (2, 2) => umax = Some(v),
            _ => {}
        }
    }
    // The first report with X and Y.
    let id = found
        .iter()
        .find(|f| f.page == 1 && f.usage == 0x30)
        .map(|f| f.id)?;
    let get = |page: u32, usage: u32| {
        found
            .iter()
            .find(|f| f.id == id && f.page == page && f.usage == usage)
            .map(|f| f.field)
    };
    let x = get(1, 0x30)?;
    let y = get(1, 0x31)?;
    Some(Pointer {
        report_id: ids_used.then_some(id),
        x,
        y,
        wheel: get(1, 0x38),
        buttons: [get(9, 1), get(9, 2), get(9, 3)],
        bytes: (offsets[id as usize].div_ceil(8)) as usize + ids_used as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QEMU's usb-tablet (hw/usb/dev-hid.c).
    const QEMU_TABLET: [u8; 74] = [
        0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29,
        0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05,
        0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00, 0x26, 0xff, 0x7f, 0x35, 0x00,
        0x46, 0xff, 0x7f, 0x75, 0x10, 0x95, 0x02, 0x81, 0x02, 0x05, 0x01, 0x09, 0x38, 0x15, 0x81,
        0x25, 0x7f, 0x35, 0x00, 0x45, 0x00, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0xc0, 0xc0,
    ];

    #[test]
    fn qemu_tablet() {
        let p = parse_pointer(&QEMU_TABLET).expect("pointer");
        assert!(p.absolute());
        assert_eq!(p.report_id, None);
        assert_eq!(p.bytes, 6);
        assert_eq!(p.x.offset, 8);
        assert_eq!(p.y.offset, 24);
        assert_eq!(p.x.max, 0x7fff);
        let r = p.parse(&[0x01, 0xff, 0x3f, 0x00, 0x40, 0xff]).unwrap();
        assert_eq!(r.buttons, 1);
        assert_eq!(r.x, 0x3fff);
        assert_eq!(r.y, 0x4000);
        assert_eq!(r.wheel, -1);
        assert!(r.absolute);
        assert_eq!(p.parse(&[0, 0, 0]), None);
    }

    /// A mouse with a report ID, 5 buttons, 12-bit relative X/Y and a
    /// wheel, as many gaming mice describe themselves.
    #[test]
    fn mouse_with_report_id() {
        let d = [
            0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x02, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09,
            0x19, 0x01, 0x29, 0x05, 0x15, 0x00, 0x25, 0x01, 0x95, 0x05, 0x75, 0x01, 0x81, 0x02,
            0x95, 0x01, 0x75, 0x03, 0x81, 0x01, 0x05, 0x01, 0x16, 0x01, 0xf8, 0x26, 0xff, 0x07,
            0x75, 0x0c, 0x95, 0x02, 0x09, 0x30, 0x09, 0x31, 0x81, 0x06, 0x15, 0x81, 0x25, 0x7f,
            0x75, 0x08, 0x95, 0x01, 0x09, 0x38, 0x81, 0x06, 0xc0, 0xc0,
        ];
        let p = parse_pointer(&d).expect("pointer");
        assert!(!p.absolute());
        assert_eq!(p.report_id, Some(2));
        assert_eq!(p.bytes, 1 + 1 + 3 + 1);
        // Right button, x = -2 (0xffe), y = 3, wheel 1.
        let r = p.parse(&[2, 0x02, 0xfe, 0x3f, 0x00, 0x01]).unwrap();
        assert_eq!(r.buttons, 2);
        assert_eq!((r.x, r.y, r.wheel), (-2, 3, 1));
        assert_eq!(p.parse(&[1, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn keyboards_are_not_pointers() {
        let kbd = [
            0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00,
            0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0xc0,
        ];
        assert_eq!(parse_pointer(&kbd), None);
        assert_eq!(parse_pointer(&[0x05]), None);
    }
}

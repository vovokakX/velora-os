//! Standard requests and descriptors (USB 2.0 chapter 9).

use alloc::vec::Vec;

pub const DESC_DEVICE: u8 = 1;
pub const DESC_CONFIG: u8 = 2;
pub const DESC_INTERFACE: u8 = 4;
pub const DESC_ENDPOINT: u8 = 5;

pub const REQ_GET_DESCRIPTOR: u8 = 6;
pub const REQ_SET_CONFIGURATION: u8 = 9;

pub const CLASS_HID: u8 = 3;
pub const CLASS_MASS_STORAGE: u8 = 8;
pub const CLASS_HUB: u8 = 9;

/// The 8-byte setup packet of a control transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl Setup {
    /// GET_DESCRIPTOR of `kind`, number `index`, `length` bytes.
    pub fn get_descriptor(kind: u8, index: u8, length: u16) -> Self {
        Self {
            request_type: 0x80,
            request: REQ_GET_DESCRIPTOR,
            value: (kind as u16) << 8 | index as u16,
            index: 0,
            length,
        }
    }

    pub fn set_configuration(value: u8) -> Self {
        Self {
            request_type: 0x00,
            request: REQ_SET_CONFIGURATION,
            value: value as u16,
            index: 0,
            length: 0,
        }
    }

    /// A class request to an interface (HID SET_PROTOCOL, SET_IDLE, ...).
    pub fn class_interface(
        request: u8,
        value: u16,
        interface: u8,
        length: u16,
        in_dir: bool,
    ) -> Self {
        Self {
            request_type: if in_dir { 0xa1 } else { 0x21 },
            request,
            value,
            index: interface as u16,
            length,
        }
    }

    /// Data stage direction: device to host.
    pub fn is_in(&self) -> bool {
        self.request_type & 0x80 != 0
    }

    /// The packet as it goes on the wire (little endian).
    pub fn to_u64(self) -> u64 {
        self.request_type as u64
            | (self.request as u64) << 8
            | (self.value as u64) << 16
            | (self.index as u64) << 32
            | (self.length as u64) << 48
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    TooShort,
    BadDescriptor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DeviceDescriptor {
    pub usb: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub max_packet0: u8,
    pub vendor: u16,
    pub product: u16,
    pub configurations: u8,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

/// Parses a device descriptor; the first 8 bytes are enough for
/// `max_packet0`.
pub fn parse_device(b: &[u8]) -> Result<DeviceDescriptor, Error> {
    if b.len() < 8 {
        return Err(Error::TooShort);
    }
    if b[1] != DESC_DEVICE || b[0] < 18 && b.len() >= 18 {
        return Err(Error::BadDescriptor);
    }
    let mut d = DeviceDescriptor {
        usb: u16_at(b, 2),
        class: b[4],
        subclass: b[5],
        protocol: b[6],
        max_packet0: b[7],
        ..Default::default()
    };
    if b.len() >= 18 {
        d.vendor = u16_at(b, 8);
        d.product = u16_at(b, 10);
        d.configurations = b[17];
    }
    Ok(d)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// Number and direction (bit 7: IN).
    pub address: u8,
    /// Bits 0-1: 0 control, 1 isochronous, 2 bulk, 3 interrupt.
    pub attributes: u8,
    pub max_packet: u16,
    pub interval: u8,
}

impl Endpoint {
    pub fn number(&self) -> u8 {
        self.address & 0x0f
    }

    pub fn is_in(&self) -> bool {
        self.address & 0x80 != 0
    }

    pub fn is_bulk(&self) -> bool {
        self.attributes & 3 == 2
    }

    pub fn is_interrupt(&self) -> bool {
        self.attributes & 3 == 3
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub number: u8,
    pub alternate: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: Vec<Endpoint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub total_length: u16,
    pub interfaces: Vec<Interface>,
}

/// Parses a whole configuration descriptor (with its interfaces and
/// endpoints). Only alternate setting 0 of each interface is kept.
pub fn parse_configuration(b: &[u8]) -> Result<Configuration, Error> {
    if b.len() < 9 {
        return Err(Error::TooShort);
    }
    if b[1] != DESC_CONFIG {
        return Err(Error::BadDescriptor);
    }
    let total = (u16_at(b, 2) as usize).min(b.len());
    let mut c = Configuration {
        value: b[5],
        total_length: u16_at(b, 2),
        interfaces: Vec::new(),
    };
    let mut i = b[0] as usize;
    let mut in_alt0 = false;
    while i + 2 <= total {
        let len = b[i] as usize;
        if len < 2 || i + len > total {
            return Err(Error::BadDescriptor);
        }
        let d = &b[i..i + len];
        match d[1] {
            DESC_INTERFACE if len >= 9 => {
                in_alt0 = d[3] == 0;
                if in_alt0 {
                    c.interfaces.push(Interface {
                        number: d[2],
                        alternate: d[3],
                        class: d[5],
                        subclass: d[6],
                        protocol: d[7],
                        endpoints: Vec::new(),
                    });
                }
            }
            DESC_ENDPOINT if len >= 7 => {
                if in_alt0 && let Some(iface) = c.interfaces.last_mut() {
                    iface.endpoints.push(Endpoint {
                        address: d[2],
                        attributes: d[3],
                        max_packet: u16_at(d, 4) & 0x7ff,
                        interval: d[6],
                    });
                }
            }
            _ => {}
        }
        i += len;
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_packets() {
        let s = Setup::get_descriptor(DESC_DEVICE, 0, 18);
        assert_eq!(s.to_u64(), 0x0012_0000_0100_0680);
        assert!(s.is_in());
        let s = Setup::set_configuration(1);
        assert_eq!(s.to_u64(), 0x0000_0000_0001_0900);
        assert!(!s.is_in());
        let s = Setup::class_interface(0x0b, 0, 1, 0, false);
        assert_eq!(s.to_u64(), 0x0000_0001_0000_0b21);
    }

    #[test]
    fn device_descriptor() {
        // QEMU usb-kbd.
        let b = [
            18, 1, 0x00, 0x02, 0, 0, 0, 8, 0x27, 0x06, 0x01, 0x00, 0, 0, 1, 4, 11, 1,
        ];
        let d = parse_device(&b).unwrap();
        assert_eq!(
            (d.usb, d.max_packet0, d.vendor, d.product),
            (0x200, 8, 0x0627, 1)
        );
        assert_eq!(d.configurations, 1);
        assert_eq!(parse_device(&b[..8]).unwrap().max_packet0, 8);
        assert_eq!(parse_device(&b[..4]), Err(Error::TooShort));
    }

    #[test]
    fn configuration_with_two_interfaces() {
        let mut b = alloc::vec![9, 2, 0, 0, 2, 1, 0, 0xa0, 50];
        // Mass storage interface with bulk IN and OUT.
        b.extend_from_slice(&[9, 4, 0, 0, 2, 8, 6, 0x50, 0]);
        b.extend_from_slice(&[7, 5, 0x81, 2, 0x00, 0x02, 0]);
        b.extend_from_slice(&[7, 5, 0x02, 2, 0x00, 0x02, 0]);
        // An alternate setting that must be ignored.
        b.extend_from_slice(&[9, 4, 0, 1, 1, 8, 6, 0x50, 0]);
        b.extend_from_slice(&[7, 5, 0x83, 2, 0x40, 0x00, 0]);
        // HID keyboard with an interrupt IN endpoint, plus a HID class
        // descriptor in between.
        b.extend_from_slice(&[9, 4, 1, 0, 1, 3, 1, 1, 0]);
        b.extend_from_slice(&[9, 0x21, 0x11, 1, 0, 1, 0x22, 63, 0]);
        b.extend_from_slice(&[7, 5, 0x84, 3, 8, 0, 10]);
        let total = b.len() as u16;
        b[2..4].copy_from_slice(&total.to_le_bytes());
        let c = parse_configuration(&b).unwrap();
        assert_eq!(c.value, 1);
        assert_eq!(c.interfaces.len(), 2);
        let msc = &c.interfaces[0];
        assert_eq!((msc.class, msc.subclass, msc.protocol), (8, 6, 0x50));
        assert_eq!(msc.endpoints.len(), 2);
        assert!(msc.endpoints[0].is_in() && msc.endpoints[0].is_bulk());
        assert_eq!(msc.endpoints[1].number(), 2);
        assert_eq!(msc.endpoints[1].max_packet, 512);
        let kbd = &c.interfaces[1];
        assert_eq!(kbd.number, 1);
        assert!(kbd.endpoints[0].is_interrupt());
        assert_eq!(kbd.endpoints[0].interval, 10);
        // A descriptor running past the end is rejected.
        let mut bad = b.clone();
        bad[9] = 200;
        assert_eq!(parse_configuration(&bad), Err(Error::BadDescriptor));
    }
}

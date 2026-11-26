//! PCI bus enumeration, independent of how configuration space is reached
//! (ECAM or I/O ports). The kernel implements [`ConfigAccess`]; tests use a
//! simulated bus.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Address {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl core::fmt::Display for Address {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:02x}:{:02x}.{}", self.bus, self.device, self.function)
    }
}

/// Access to configuration space. Offsets are 4-byte aligned.
pub trait ConfigAccess {
    fn read32(&self, addr: Address, offset: u16) -> u32;
    fn write32(&self, addr: Address, offset: u16, value: u32);

    fn read16(&self, addr: Address, offset: u16) -> u16 {
        (self.read32(addr, offset & !3) >> ((offset & 2) * 8)) as u16
    }

    fn read8(&self, addr: Address, offset: u16) -> u8 {
        (self.read32(addr, offset & !3) >> ((offset & 3) * 8)) as u8
    }

    fn write16(&self, addr: Address, offset: u16, value: u16) {
        let shift = (offset & 2) * 8;
        let old = self.read32(addr, offset & !3);
        let new = (old & !(0xffff << shift)) | ((value as u32) << shift);
        self.write32(addr, offset & !3, new);
    }
}

pub const COMMAND: u16 = 0x04;
pub const CMD_IO: u16 = 1 << 0;
pub const CMD_MEMORY: u16 = 1 << 1;
pub const CMD_BUS_MASTER: u16 = 1 << 2;
pub const CMD_INTX_DISABLE: u16 = 1 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    None,
    Memory {
        base: u64,
        size: u64,
        prefetchable: bool,
        is64: bool,
    },
    Io {
        port: u32,
        size: u32,
    },
}

impl Bar {
    pub fn memory(&self) -> Option<(u64, u64)> {
        match *self {
            Bar::Memory { base, size, .. } => Some((base, size)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub addr: Address,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    pub header_type: u8,
    pub bars: [Bar; 6],
}

impl Device {
    pub fn is(&self, class: u8, subclass: u8, prog_if: u8) -> bool {
        self.class == class && self.subclass == subclass && self.prog_if == prog_if
    }

    pub fn class_name(&self) -> &'static str {
        class_name(self.class, self.subclass, self.prog_if)
    }
}

pub fn class_name(class: u8, subclass: u8, prog_if: u8) -> &'static str {
    match (class, subclass, prog_if) {
        (0x01, 0x00, _) => "SCSI storage",
        (0x01, 0x01, _) => "IDE controller",
        (0x01, 0x06, 0x01) => "SATA controller (AHCI)",
        (0x01, 0x08, 0x02) => "NVMe controller",
        (0x01, _, _) => "storage controller",
        (0x02, 0x00, _) => "Ethernet controller",
        (0x02, _, _) => "network controller",
        (0x03, _, _) => "display controller",
        (0x04, _, _) => "multimedia controller",
        (0x06, 0x00, _) => "host bridge",
        (0x06, 0x01, _) => "ISA bridge",
        (0x06, 0x04, _) => "PCI-to-PCI bridge",
        (0x06, _, _) => "bridge",
        (0x0c, 0x03, 0x30) => "USB controller (xHCI)",
        (0x0c, 0x03, _) => "USB controller",
        (0x0c, 0x05, _) => "SMBus controller",
        (0x0c, _, _) => "serial bus controller",
        (0xff, _, _) => "unassigned class",
        (0x00, _, _) => "unclassified device",
        _ => "other device",
    }
}

fn exists<A: ConfigAccess>(a: &A, addr: Address) -> bool {
    a.read16(addr, 0x00) != 0xffff
}

/// Sizes the BARs of a type-0 header (6 BARs) or a bridge (2 BARs).
/// Decoding is switched off while sizing and restored afterwards.
pub fn read_bars<A: ConfigAccess>(a: &A, addr: Address, header_type: u8) -> [Bar; 6] {
    let count = match header_type & 0x7f {
        0 => 6,
        1 => 2,
        _ => 0,
    };
    let mut bars = [Bar::None; 6];
    let cmd = a.read16(addr, COMMAND);
    a.write16(addr, COMMAND, cmd & !(CMD_IO | CMD_MEMORY));
    let mut i = 0;
    while i < count {
        let off = 0x10 + 4 * i as u16;
        let orig = a.read32(addr, off);
        a.write32(addr, off, 0xffff_ffff);
        let mask = a.read32(addr, off);
        a.write32(addr, off, orig);
        if orig & 1 == 1 {
            let size_mask = mask & !0x3;
            if size_mask != 0 {
                let size = (!size_mask | 0xffff_0000).wrapping_add(1) & 0xffff;
                bars[i] = Bar::Io {
                    port: orig & !0x3,
                    size: if size == 0 { 0x1_0000 } else { size },
                };
            }
            i += 1;
            continue;
        }
        let kind = (orig >> 1) & 0x3;
        let prefetchable = orig & 0x8 != 0;
        if kind == 2 && i + 1 < count {
            let off_hi = off + 4;
            let orig_hi = a.read32(addr, off_hi);
            a.write32(addr, off_hi, 0xffff_ffff);
            let mask_hi = a.read32(addr, off_hi);
            a.write32(addr, off_hi, orig_hi);
            let full_mask = ((mask_hi as u64) << 32) | (mask & !0xf) as u64;
            if full_mask != 0 {
                bars[i] = Bar::Memory {
                    base: ((orig_hi as u64) << 32) | (orig & !0xf) as u64,
                    size: (!full_mask).wrapping_add(1),
                    prefetchable,
                    is64: true,
                };
            }
            i += 2;
        } else {
            let size_mask = mask & !0xf;
            if size_mask != 0 {
                bars[i] = Bar::Memory {
                    base: (orig & !0xf) as u64,
                    size: (!size_mask).wrapping_add(1) as u64,
                    prefetchable,
                    is64: false,
                };
            }
            i += 1;
        }
    }
    a.write16(addr, COMMAND, cmd);
    bars
}

fn read_device<A: ConfigAccess>(a: &A, addr: Address) -> Device {
    let id = a.read32(addr, 0x00);
    let class = a.read32(addr, 0x08);
    let header_type = a.read8(addr, 0x0e);
    Device {
        addr,
        vendor: id as u16,
        device: (id >> 16) as u16,
        revision: class as u8,
        prog_if: (class >> 8) as u8,
        subclass: (class >> 16) as u8,
        class: (class >> 24) as u8,
        header_type,
        bars: read_bars(a, addr, header_type),
    }
}

fn scan_bus<A: ConfigAccess>(a: &A, bus: u8, out: &mut Vec<Device>, depth: u32) {
    if depth > 32 {
        return; // misconfigured bridges must not loop forever
    }
    for device in 0..32 {
        let base = Address {
            bus,
            device,
            function: 0,
        };
        if !exists(a, base) {
            continue;
        }
        let multi = a.read8(base, 0x0e) & 0x80 != 0;
        let functions = if multi { 8 } else { 1 };
        for function in 0..functions {
            let addr = Address {
                bus,
                device,
                function,
            };
            if !exists(a, addr) {
                continue;
            }
            let d = read_device(a, addr);
            let is_bridge = d.class == 0x06 && d.subclass == 0x04 && d.header_type & 0x7f == 1;
            out.push(d);
            if is_bridge {
                let secondary = a.read8(addr, 0x19);
                if secondary > bus {
                    scan_bus(a, secondary, out, depth + 1);
                }
            }
        }
    }
}

/// Finds every function reachable from bus 0 (following bridges).
pub fn enumerate<A: ConfigAccess>(a: &A) -> Vec<Device> {
    let mut out = Vec::new();
    scan_bus(a, 0, &mut out, 0);
    out
}

/// Capability list: (capability id, offset in config space).
pub fn capabilities<A: ConfigAccess>(a: &A, addr: Address) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    if a.read16(addr, 0x06) & (1 << 4) == 0 {
        return out;
    }
    let mut off = a.read8(addr, 0x34) & !3;
    let mut guard = 0;
    while off >= 0x40 && guard < 48 {
        let id = a.read8(addr, off as u16);
        out.push((id, off));
        off = a.read8(addr, off as u16 + 1) & !3;
        guard += 1;
    }
    out
}

/// Turns on memory decoding and bus mastering (DMA) and turns off legacy
/// INTx interrupts (drivers poll or use MSI).
pub fn enable_device<A: ConfigAccess>(a: &A, addr: Address) {
    let cmd = a.read16(addr, COMMAND);
    a.write16(
        addr,
        COMMAND,
        cmd | CMD_MEMORY | CMD_BUS_MASTER | CMD_INTX_DISABLE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// A simulated bus: config space words plus read-only masks for BARs.
    #[derive(Default)]
    struct FakeBus {
        space: RefCell<BTreeMap<Address, [u32; 64]>>,
        bar_masks: BTreeMap<(Address, u16), u32>,
    }

    impl FakeBus {
        fn add(
            &mut self,
            addr: Address,
            vendor: u16,
            device: u16,
            class: u32,
            header: u8,
        ) -> &mut [u32; 64] {
            let mut s = [0u32; 64];
            s[0] = (device as u32) << 16 | vendor as u32;
            s[2] = class;
            s[3] = (header as u32) << 16;
            self.space.get_mut().insert(addr, s);
            self.space.get_mut().get_mut(&addr).unwrap()
        }
    }

    impl ConfigAccess for FakeBus {
        fn read32(&self, addr: Address, offset: u16) -> u32 {
            self.space
                .borrow()
                .get(&addr)
                .map(|s| s[offset as usize / 4])
                .unwrap_or(0xffff_ffff)
        }
        fn write32(&self, addr: Address, offset: u16, value: u32) {
            let mask = self.bar_masks.get(&(addr, offset)).copied();
            if let Some(s) = self.space.borrow_mut().get_mut(&addr) {
                let i = offset as usize / 4;
                let is_bar = (0x10..0x28).contains(&offset);
                s[i] = match mask {
                    // Hardware ignores writes to the size bits and type bits.
                    Some(m) if value == 0xffff_ffff => (s[i] & !m) | m | (s[i] & 0xf),
                    // Unimplemented BARs read as zero whatever is written.
                    None if is_bar => 0,
                    _ => value,
                };
            }
        }
    }

    fn a(bus: u8, device: u8, function: u8) -> Address {
        Address {
            bus,
            device,
            function,
        }
    }

    #[test]
    fn enumerates_functions_and_bridges() {
        let mut bus = FakeBus::default();
        bus.add(a(0, 0, 0), 0x8086, 0x29c0, 0x0600_0000, 0x80); // host bridge, multi-function
        bus.add(a(0, 0, 1), 0x8086, 0x1111, 0x0c05_0000, 0);
        bus.add(a(0, 31, 2), 0x8086, 0x2922, 0x0106_0102, 0); // AHCI (not multi-function: fn 2 only via fn 0)
        bus.add(a(0, 3, 0), 0x1b36, 0x0001, 0x0604_0000, 1); // bridge to bus 1
        bus.space.get_mut().get_mut(&a(0, 3, 0)).unwrap()[6] = 0x0001_0100; // secondary bus 1
        bus.add(a(1, 0, 0), 0x1b36, 0x0010, 0x0108_0200, 0); // NVMe behind the bridge
        let devs = enumerate(&bus);
        let found: Vec<Address> = devs.iter().map(|d| d.addr).collect();
        assert_eq!(found, vec![a(0, 0, 0), a(0, 0, 1), a(0, 3, 0), a(1, 0, 0)]);
        let nvme = devs.iter().find(|d| d.addr == a(1, 0, 0)).unwrap();
        assert!(nvme.is(0x01, 0x08, 0x02));
        assert_eq!(nvme.class_name(), "NVMe controller");
    }

    #[test]
    fn sizes_32_and_64_bit_bars() {
        let mut bus = FakeBus::default();
        let d = a(0, 4, 0);
        let s = bus.add(d, 0x1af4, 0x1042, 0x0100_0000, 0);
        s[1] = (CMD_MEMORY | CMD_IO) as u32;
        s[4] = 0xfebd_0000; // BAR0: 32-bit memory, 4 KiB
        s[5] = 0xc001; // BAR1: I/O, 64 ports
        s[8] = 0xfe00_000c; // BAR4: 64-bit prefetchable memory, 16 KiB
        s[9] = 0x0000_0001;
        bus.bar_masks.insert((d, 0x10), 0xffff_f000);
        bus.bar_masks.insert((d, 0x14), 0xffff_ffc0);
        bus.bar_masks.insert((d, 0x20), 0xffff_c000);
        bus.bar_masks.insert((d, 0x24), 0xffff_ffff);
        let bars = read_bars(&bus, d, 0);
        assert_eq!(
            bars[0],
            Bar::Memory {
                base: 0xfebd_0000,
                size: 0x1000,
                prefetchable: false,
                is64: false
            }
        );
        assert_eq!(
            bars[1],
            Bar::Io {
                port: 0xc000,
                size: 0x40
            }
        );
        assert_eq!(
            bars[4],
            Bar::Memory {
                base: 0x1_fe00_0000,
                size: 0x4000,
                prefetchable: true,
                is64: true
            }
        );
        assert_eq!(bars[5], Bar::None);
        assert_eq!(bars[2], Bar::None);
        // Original values and the command register are restored.
        let s = bus.space.borrow()[&d];
        assert_eq!(s[4], 0xfebd_0000);
        assert_eq!(s[1] as u16, CMD_MEMORY | CMD_IO);
    }

    #[test]
    fn capability_list() {
        let mut bus = FakeBus::default();
        let d = a(0, 5, 0);
        let s = bus.add(d, 0x1af4, 0x1042, 0x0100_0000, 0);
        s[1] = 1 << 20; // status: capability list present
        s[0x34 / 4] = 0x40;
        s[0x40 / 4] = 0x0000_5009; // vendor cap, next 0x50
        s[0x50 / 4] = 0x0000_0011; // MSI-X, end
        assert_eq!(capabilities(&bus, d), vec![(0x09, 0x40), (0x11, 0x50)]);
    }

    #[test]
    fn capability_loop_is_bounded() {
        let mut bus = FakeBus::default();
        let d = a(0, 6, 0);
        let s = bus.add(d, 1, 2, 0, 0);
        s[1] = 1 << 20;
        s[0x34 / 4] = 0x40;
        s[0x40 / 4] = 0x0000_4009; // points at itself
        assert_eq!(capabilities(&bus, d).len(), 48);
    }

    #[test]
    fn enable_sets_bus_master() {
        let mut bus = FakeBus::default();
        let d = a(0, 7, 0);
        bus.add(d, 1, 2, 0, 0);
        enable_device(&bus, d);
        let cmd = bus.read16(d, COMMAND);
        assert_eq!(
            cmd & (CMD_MEMORY | CMD_BUS_MASTER),
            CMD_MEMORY | CMD_BUS_MASTER
        );
    }
}

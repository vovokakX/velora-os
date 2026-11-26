//! ACPI table parsing.
//!
//! The kernel maps the tables and hands byte slices to these functions, so
//! all parsing and validation is tested on the host with synthetic tables.
//! Only what the kernel uses is parsed: RSDP, RSDT/XSDT, MADT, MCFG,
//! FADT and HPET.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    BadSignature,
    BadChecksum,
    TooShort,
    BadLength,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn checksum_ok(b: &[u8]) -> bool {
    b.iter().fold(0u8, |a, &x| a.wrapping_add(x)) == 0
}

/// Root System Description Pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rsdp {
    pub revision: u8,
    pub rsdt: u32,
    /// Only for revision 2 and later.
    pub xsdt: Option<u64>,
}

pub const RSDP_V1_LEN: usize = 20;
pub const RSDP_V2_LEN: usize = 36;

/// Parses an RSDP. `b` must hold at least 20 bytes (36 for ACPI 2.0+).
pub fn parse_rsdp(b: &[u8]) -> Result<Rsdp, Error> {
    if b.len() < RSDP_V1_LEN {
        return Err(Error::TooShort);
    }
    if &b[..8] != b"RSD PTR " {
        return Err(Error::BadSignature);
    }
    if !checksum_ok(&b[..RSDP_V1_LEN]) {
        return Err(Error::BadChecksum);
    }
    let revision = b[15];
    let rsdt = u32_at(b, 16);
    if revision < 2 {
        return Ok(Rsdp {
            revision,
            rsdt,
            xsdt: None,
        });
    }
    if b.len() < RSDP_V2_LEN {
        return Err(Error::TooShort);
    }
    let len = u32_at(b, 20) as usize;
    if len < RSDP_V2_LEN || len > b.len() {
        return Err(Error::BadLength);
    }
    if !checksum_ok(&b[..len]) {
        return Err(Error::BadChecksum);
    }
    let xsdt = u64_at(b, 24);
    Ok(Rsdp {
        revision,
        rsdt,
        xsdt: if xsdt != 0 { Some(xsdt) } else { None },
    })
}

pub const HEADER_LEN: usize = 36;

/// System description table header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub oem_id: [u8; 6],
}

/// Reads the header only (to learn the table length before mapping it).
pub fn parse_header(b: &[u8]) -> Result<SdtHeader, Error> {
    if b.len() < HEADER_LEN {
        return Err(Error::TooShort);
    }
    Ok(SdtHeader {
        signature: b[..4].try_into().unwrap(),
        length: u32_at(b, 4),
        revision: b[8],
        oem_id: b[10..16].try_into().unwrap(),
    })
}

/// Validates a whole table: length and checksum. Returns the header.
pub fn validate_table(b: &[u8]) -> Result<SdtHeader, Error> {
    let h = parse_header(b)?;
    let len = h.length as usize;
    if len < HEADER_LEN || len > b.len() {
        return Err(Error::BadLength);
    }
    if !checksum_ok(&b[..len]) {
        return Err(Error::BadChecksum);
    }
    Ok(h)
}

/// Physical addresses listed in an RSDT (32-bit) or XSDT (64-bit).
pub fn root_entries(table: &[u8]) -> Result<Vec<u64>, Error> {
    let h = validate_table(table)?;
    let wide = match &h.signature {
        b"XSDT" => true,
        b"RSDT" => false,
        _ => return Err(Error::BadSignature),
    };
    let body = &table[HEADER_LEN..h.length as usize];
    let step = if wide { 8 } else { 4 };
    Ok(body
        .chunks_exact(step)
        .map(|c| {
            if wide {
                u64_at(c, 0)
            } else {
                u32_at(c, 0) as u64
            }
        })
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalApic {
    pub processor_uid: u32,
    pub apic_id: u32,
    /// Usable now, or can be brought online later.
    pub enabled: bool,
    pub online_capable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoApic {
    pub id: u8,
    pub address: u32,
    pub gsi_base: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    /// Conforms to the bus (ISA: active high).
    BusDefault,
    ActiveHigh,
    ActiveLow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Conforms to the bus (ISA: edge).
    BusDefault,
    Edge,
    Level,
}

fn polarity(flags: u16) -> Polarity {
    match flags & 0b11 {
        1 => Polarity::ActiveHigh,
        3 => Polarity::ActiveLow,
        _ => Polarity::BusDefault,
    }
}

fn trigger(flags: u16) -> Trigger {
    match (flags >> 2) & 0b11 {
        1 => Trigger::Edge,
        3 => Trigger::Level,
        _ => Trigger::BusDefault,
    }
}

/// An ISA IRQ routed to a different global system interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptOverride {
    pub bus: u8,
    pub source_irq: u8,
    pub gsi: u32,
    pub polarity: Polarity,
    pub trigger: Trigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LapicNmi {
    /// 0xff (or 0xffffffff for x2APIC) means all processors.
    pub processor_uid: u32,
    pub lint: u8,
    pub polarity: Polarity,
    pub trigger: Trigger,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Madt {
    pub lapic_address: u64,
    /// The system also has dual 8259 PICs.
    pub has_8259: bool,
    pub lapics: Vec<LocalApic>,
    pub ioapics: Vec<IoApic>,
    pub overrides: Vec<InterruptOverride>,
    pub nmis: Vec<LapicNmi>,
}

impl Madt {
    /// GSI, polarity and trigger for an ISA IRQ, applying overrides.
    pub fn isa_irq(&self, irq: u8) -> (u32, Polarity, Trigger) {
        match self
            .overrides
            .iter()
            .find(|o| o.bus == 0 && o.source_irq == irq)
        {
            Some(o) => (o.gsi, o.polarity, o.trigger),
            None => (irq as u32, Polarity::BusDefault, Trigger::BusDefault),
        }
    }

    /// The I/O APIC that serves `gsi`, if any. `max_entries` gives the
    /// number of redirection entries per I/O APIC (read from the chip).
    pub fn ioapic_for(&self, gsi: u32, max_entries: impl Fn(&IoApic) -> u32) -> Option<IoApic> {
        self.ioapics
            .iter()
            .copied()
            .find(|io| gsi >= io.gsi_base && gsi < io.gsi_base + max_entries(io))
    }
}

/// Parses the MADT ("APIC" table). Unknown entry types are skipped.
pub fn parse_madt(table: &[u8]) -> Result<Madt, Error> {
    let h = validate_table(table)?;
    if &h.signature != b"APIC" {
        return Err(Error::BadSignature);
    }
    let len = h.length as usize;
    if len < HEADER_LEN + 8 {
        return Err(Error::BadLength);
    }
    let mut m = Madt {
        lapic_address: u32_at(table, 36) as u64,
        has_8259: u32_at(table, 40) & 1 != 0,
        ..Default::default()
    };
    let mut off = HEADER_LEN + 8;
    while off + 2 <= len {
        let kind = table[off];
        let elen = table[off + 1] as usize;
        if elen < 2 || off + elen > len {
            return Err(Error::BadLength);
        }
        let e = &table[off..off + elen];
        match (kind, elen) {
            (0, 8..) => {
                let flags = u32_at(e, 4);
                m.lapics.push(LocalApic {
                    processor_uid: e[2] as u32,
                    apic_id: e[3] as u32,
                    enabled: flags & 1 != 0,
                    online_capable: flags & 2 != 0,
                });
            }
            (1, 12..) => m.ioapics.push(IoApic {
                id: e[2],
                address: u32_at(e, 4),
                gsi_base: u32_at(e, 8),
            }),
            (2, 10..) => {
                let flags = u16_at(e, 8);
                m.overrides.push(InterruptOverride {
                    bus: e[2],
                    source_irq: e[3],
                    gsi: u32_at(e, 4),
                    polarity: polarity(flags),
                    trigger: trigger(flags),
                });
            }
            (4, 6..) => {
                let flags = u16_at(e, 3);
                m.nmis.push(LapicNmi {
                    processor_uid: e[2] as u32,
                    lint: e[5],
                    polarity: polarity(flags),
                    trigger: trigger(flags),
                });
            }
            (5, 12..) => m.lapic_address = u64_at(e, 4),
            (9, 16..) => {
                let flags = u32_at(e, 8);
                m.lapics.push(LocalApic {
                    processor_uid: u32_at(e, 12),
                    apic_id: u32_at(e, 4),
                    enabled: flags & 1 != 0,
                    online_capable: flags & 2 != 0,
                });
            }
            _ => {}
        }
        off += elen;
    }
    Ok(m)
}

/// One PCI Express ECAM region from the MCFG table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EcamRegion {
    pub base: u64,
    pub segment: u16,
    pub start_bus: u8,
    pub end_bus: u8,
}

impl EcamRegion {
    /// Physical address of the configuration space of a function.
    pub fn address(&self, bus: u8, device: u8, function: u8) -> Option<u64> {
        if bus < self.start_bus || bus > self.end_bus || device > 31 || function > 7 {
            return None;
        }
        let b = (bus - self.start_bus) as u64;
        Some(self.base + (b << 20) + ((device as u64) << 15) + ((function as u64) << 12))
    }
}

/// Parses the MCFG table (PCI Express memory-mapped configuration).
pub fn parse_mcfg(table: &[u8]) -> Result<Vec<EcamRegion>, Error> {
    let h = validate_table(table)?;
    if &h.signature != b"MCFG" {
        return Err(Error::BadSignature);
    }
    let len = h.length as usize;
    if len < HEADER_LEN + 8 || !(len - HEADER_LEN - 8).is_multiple_of(16) {
        return Err(Error::BadLength);
    }
    Ok(table[HEADER_LEN + 8..len]
        .as_chunks::<16>()
        .0
        .iter()
        .map(|e| EcamRegion {
            base: u64_at(e, 0),
            segment: u16_at(e, 8),
            start_bus: e[10],
            end_bus: e[11],
        })
        .collect())
}

/// The few FADT fields the kernel uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fadt {
    /// CMOS register holding the century, 0 if not provided.
    pub century_register: u8,
    /// IA-PC boot architecture flags (bit 1: 8042 present).
    pub iapc_boot_flags: u16,
    /// Physical address of the DSDT (X_DSDT when present), 0 if none.
    pub dsdt: u64,
    /// PM1a/PM1b control register I/O ports (0 = absent). The extended
    /// (X_) addresses are used when they are I/O ports.
    pub pm1a_cnt: u32,
    pub pm1b_cnt: u32,
    /// PM1a/PM1b event blocks (status, then enable) and their length.
    pub pm1a_evt: u32,
    pub pm1b_evt: u32,
    pub pm1_evt_len: u8,
    /// General-purpose event blocks (status, then enable) and lengths.
    pub gpe0_blk: u32,
    pub gpe0_len: u8,
    pub gpe1_blk: u32,
    pub gpe1_len: u8,
    /// ISA interrupt of the SCI.
    pub sci_int: u16,
    /// Port and value that switch the firmware to ACPI mode (0 = already
    /// in ACPI mode or not needed).
    pub smi_cmd: u32,
    pub acpi_enable: u8,
    /// Hardware-reduced ACPI: sleep control and status registers
    /// (space id, address); space 0xff = absent.
    pub sleep_control: (u8, u64),
    pub sleep_status: (u8, u64),
    /// Fixed feature flags (bit 10: reset register supported).
    pub flags: u32,
    /// Reset register (generic address) and the value to write.
    pub reset_space: u8,
    pub reset_address: u64,
    pub reset_value: u8,
    /// ACPI PM timer I/O port (0 = none) and whether it counts 32 bits
    /// (otherwise 24). It runs at [`PM_TIMER_HZ`].
    pub pm_timer_port: u32,
    pub pm_timer_32bit: bool,
}

/// Frequency of the ACPI PM timer.
pub const PM_TIMER_HZ: u64 = 3_579_545;

/// The HPET description table: where the timer block is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hpet {
    /// Physical address of the register block (system memory).
    pub address: u64,
    pub number: u8,
    /// Minimum tick in periodic mode, in counter ticks.
    pub min_tick: u16,
}

/// Parses the HPET table. Only timer blocks in system memory are accepted.
pub fn parse_hpet(table: &[u8]) -> Result<Hpet, Error> {
    let h = validate_table(table)?;
    if &h.signature != b"HPET" {
        return Err(Error::BadSignature);
    }
    if (h.length as usize) < 56 {
        return Err(Error::TooShort);
    }
    // Generic address structure at 40: space id, widths, then the address.
    if table[40] != 0 {
        return Err(Error::BadLength);
    }
    let address = u64_at(table, 44);
    if address == 0 {
        return Err(Error::BadLength);
    }
    Ok(Hpet {
        address,
        number: table[52],
        min_tick: u16_at(table, 53),
    })
}

impl Fadt {
    /// The power button is a control method (or absent), not the fixed
    /// PWRBTN event.
    pub const PWR_BUTTON_IS_METHOD: u32 = 1 << 4;
    pub const RESET_REG_SUPPORTED: u32 = 1 << 10;
    pub const HW_REDUCED_ACPI: u32 = 1 << 20;
}

/// Generic address structure space ids.
pub const SPACE_MEMORY: u8 = 0;
pub const SPACE_IO: u8 = 1;
pub const SPACE_PCI: u8 = 2;

/// Sleep type values of the \_S5 (soft off) state, found in the DSDT
/// without an AML interpreter: the usual `Name (_S5, Package {a, b, ...})`
/// encoding is decoded directly. Returns (SLP_TYPa, SLP_TYPb).
pub fn find_s5(aml: &[u8]) -> Option<(u8, u8)> {
    // Every place where "_S5_" is followed by a package; other uses of the
    // name (a reference inside a method, say) are skipped.
    let mut from = 0;
    while let Some(off) = aml[from..].windows(4).position(|w| w == b"_S5_") {
        let pos = from + off;
        from = pos + 1;
        if let Some(v) = s5_package(aml, pos + 4) {
            return Some(v);
        }
    }
    None
}

/// Decodes `Package (n) { SLP_TYPa, SLP_TYPb, ... }` at `i`.
fn s5_package(aml: &[u8], mut i: usize) -> Option<(u8, u8)> {
    if *aml.get(i)? != 0x12 {
        return None; // PackageOp
    }
    i += 1;
    let lead = *aml.get(i)?;
    i += 1 + (lead >> 6) as usize; // PkgLength: 1 + extra bytes
    let count = *aml.get(i)?;
    i += 1;
    if count < 1 {
        return None;
    }
    let a = aml_integer(aml, &mut i)?;
    // Some tables give only SLP_TYPa; SLP_TYPb is then unused.
    let b = if count >= 2 {
        aml_integer(aml, &mut i)?
    } else {
        0
    };
    Some(((a & 7) as u8, (b & 7) as u8))
}

/// An AML integer data object: ZeroOp, OneOp, OnesOp or a Byte, Word,
/// DWord or QWord constant.
fn aml_integer(aml: &[u8], i: &mut usize) -> Option<u64> {
    let op = *aml.get(*i)?;
    *i += 1;
    let mut read = |n: usize| -> Option<u64> {
        let bytes = aml.get(*i..*i + n)?;
        *i += n;
        Some(bytes.iter().rev().fold(0u64, |v, &b| (v << 8) | b as u64))
    };
    match op {
        0x00 => Some(0),
        0x01 => Some(1),
        0xff => Some(u64::MAX),
        0x0a => read(1),
        0x0b => read(2),
        0x0c => read(4),
        0x0e => read(8),
        _ => None,
    }
}

pub fn parse_fadt(table: &[u8]) -> Result<Fadt, Error> {
    let h = validate_table(table)?;
    if &h.signature != b"FACP" {
        return Err(Error::BadSignature);
    }
    let len = h.length as usize;
    let u32_if = |o: usize| if len >= o + 4 { u32_at(table, o) } else { 0 };
    let x_dsdt = if len >= 148 { u64_at(table, 140) } else { 0 };
    let u8_if = |o: usize| if len > o { table[o] } else { 0 };
    // An extended (X_) block wins when it is an I/O port; else the
    // 32-bit field.
    let x_io = |gas: usize, legacy: usize| -> u32 {
        if len >= gas + 12 && table[gas] == SPACE_IO {
            let a = u64_at(table, gas + 4) as u32;
            if a != 0 {
                return a;
            }
        }
        u32_if(legacy)
    };
    let gas = |o: usize| -> (u8, u64) {
        if len >= o + 12 && u64_at(table, o + 4) != 0 {
            (table[o], u64_at(table, o + 4))
        } else {
            (0xff, 0)
        }
    };
    Ok(Fadt {
        sci_int: if len >= 48 { u16_at(table, 46) } else { 0 },
        smi_cmd: u32_if(48),
        acpi_enable: u8_if(52),
        pm1a_evt: x_io(148, 56),
        pm1b_evt: x_io(160, 60),
        pm1_evt_len: u8_if(88),
        gpe0_blk: x_io(220, 80),
        gpe0_len: u8_if(92),
        gpe1_blk: x_io(232, 84),
        gpe1_len: u8_if(93),
        sleep_control: gas(244),
        sleep_status: gas(256),
        century_register: if len > 108 { table[108] } else { 0 },
        iapc_boot_flags: if len > 110 { u16_at(table, 109) } else { 0 },
        dsdt: if x_dsdt != 0 {
            x_dsdt
        } else {
            u32_if(40) as u64
        },
        pm1a_cnt: x_io(172, 64),
        pm1b_cnt: x_io(184, 68),
        flags: u32_if(112),
        reset_space: if len > 116 { table[116] } else { 0 },
        reset_address: if len >= 128 { u64_at(table, 120) } else { 0 },
        reset_value: if len > 128 { table[128] } else { 0 },
        pm_timer_port: {
            // X_PM_TMR_BLK (generic address at 208) wins when it is an
            // I/O port; otherwise PM_TMR_BLK at 76.
            let x = if len >= 220 && table[208] == 1 {
                u64_at(table, 212) as u32
            } else {
                0
            };
            if x != 0 { x } else { u32_if(76) }
        },
        pm_timer_32bit: u32_if(112) & (1 << 8) != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix_checksum(b: &mut [u8], at: usize, range: core::ops::Range<usize>) {
        b[at] = 0;
        let s = b[range].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        b[at] = 0u8.wrapping_sub(s);
    }

    fn table(sig: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; HEADER_LEN];
        t[..4].copy_from_slice(sig);
        t[8] = 1;
        t[10..16].copy_from_slice(b"VELORA");
        t.extend_from_slice(body);
        let len = t.len() as u32;
        t[4..8].copy_from_slice(&len.to_le_bytes());
        let n = t.len();
        fix_checksum(&mut t, 9, 0..n);
        t
    }

    fn rsdp(rev: u8, rsdt: u32, xsdt: u64) -> Vec<u8> {
        let mut b = vec![0u8; RSDP_V2_LEN];
        b[..8].copy_from_slice(b"RSD PTR ");
        b[15] = rev;
        b[16..20].copy_from_slice(&rsdt.to_le_bytes());
        b[20..24].copy_from_slice(&(RSDP_V2_LEN as u32).to_le_bytes());
        b[24..32].copy_from_slice(&xsdt.to_le_bytes());
        fix_checksum(&mut b, 8, 0..RSDP_V1_LEN);
        fix_checksum(&mut b, 32, 0..RSDP_V2_LEN);
        b
    }

    #[test]
    fn rsdp_v1_and_v2() {
        let v2 = rsdp(2, 0x1000, 0x2000);
        assert_eq!(
            parse_rsdp(&v2),
            Ok(Rsdp {
                revision: 2,
                rsdt: 0x1000,
                xsdt: Some(0x2000)
            })
        );
        let v1 = rsdp(0, 0x1000, 0);
        assert_eq!(
            parse_rsdp(&v1[..20]),
            Ok(Rsdp {
                revision: 0,
                rsdt: 0x1000,
                xsdt: None
            })
        );
    }

    #[test]
    fn rsdp_errors() {
        let mut b = rsdp(2, 1, 2);
        assert_eq!(parse_rsdp(&b[..10]), Err(Error::TooShort));
        b[30] ^= 1; // breaks only the extended checksum
        assert_eq!(parse_rsdp(&b), Err(Error::BadChecksum));
        let mut c = rsdp(2, 1, 2);
        c[0] = b'X';
        assert_eq!(parse_rsdp(&c), Err(Error::BadSignature));
    }

    #[test]
    fn xsdt_and_rsdt_entries() {
        let mut body = Vec::new();
        body.extend_from_slice(&0x1111_2222_3333u64.to_le_bytes());
        body.extend_from_slice(&0x4444u64.to_le_bytes());
        assert_eq!(
            root_entries(&table(b"XSDT", &body)).unwrap(),
            vec![0x1111_2222_3333, 0x4444]
        );
        let mut body = Vec::new();
        body.extend_from_slice(&0x5555u32.to_le_bytes());
        assert_eq!(root_entries(&table(b"RSDT", &body)).unwrap(), vec![0x5555]);
        assert_eq!(
            root_entries(&table(b"FACP", &body)),
            Err(Error::BadSignature)
        );
    }

    #[test]
    fn bad_table_checksum_and_length() {
        let mut t = table(b"XSDT", &[0; 8]);
        t[40] = 1;
        assert_eq!(validate_table(&t), Err(Error::BadChecksum));
        let t = table(b"XSDT", &[0; 8]);
        assert_eq!(validate_table(&t[..40]), Err(Error::BadLength));
    }

    fn qemu_like_madt() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0xfee0_0000u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes()); // PCAT_COMPAT
        body.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]); // LAPIC cpu 0
        body.extend_from_slice(&[0, 8, 1, 1, 0, 0, 0, 0]); // LAPIC cpu 1, disabled
        let mut io = vec![1, 12, 0, 0];
        io.extend_from_slice(&0xfec0_0000u32.to_le_bytes());
        io.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&io);
        let mut iso = vec![2, 10, 0, 0];
        iso.extend_from_slice(&2u32.to_le_bytes());
        iso.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&iso); // IRQ0 -> GSI2
        let mut iso9 = vec![2, 10, 0, 9];
        iso9.extend_from_slice(&9u32.to_le_bytes());
        iso9.extend_from_slice(&0x000du16.to_le_bytes()); // high, level
        body.extend_from_slice(&iso9);
        body.extend_from_slice(&[4, 6, 0xff, 0, 0, 1]); // NMI on LINT1, all CPUs
        body.extend_from_slice(&[0x7f, 3, 0]); // unknown entry type, skipped
        table(b"APIC", &body)
    }

    #[test]
    fn madt_entries() {
        let m = parse_madt(&qemu_like_madt()).unwrap();
        assert_eq!(m.lapic_address, 0xfee0_0000);
        assert!(m.has_8259);
        assert_eq!(m.lapics.len(), 2);
        assert!(m.lapics[0].enabled && !m.lapics[1].enabled);
        assert_eq!(
            m.ioapics,
            vec![IoApic {
                id: 0,
                address: 0xfec0_0000,
                gsi_base: 0
            }]
        );
        assert_eq!(m.isa_irq(0), (2, Polarity::BusDefault, Trigger::BusDefault));
        assert_eq!(m.isa_irq(9), (9, Polarity::ActiveHigh, Trigger::Level));
        assert_eq!(m.isa_irq(1), (1, Polarity::BusDefault, Trigger::BusDefault));
        assert_eq!(m.nmis[0].lint, 1);
        assert_eq!(m.ioapic_for(1, |_| 24).unwrap().address, 0xfec0_0000);
        assert!(m.ioapic_for(30, |_| 24).is_none());
    }

    #[test]
    fn madt_lapic_override_and_x2apic() {
        let mut body = Vec::new();
        body.extend_from_slice(&0xfee0_0000u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        let mut ov = vec![5, 12, 0, 0];
        ov.extend_from_slice(&0x1_fee0_0000u64.to_le_bytes());
        body.extend_from_slice(&ov);
        let mut x2 = vec![9, 16, 0, 0];
        x2.extend_from_slice(&300u32.to_le_bytes());
        x2.extend_from_slice(&1u32.to_le_bytes());
        x2.extend_from_slice(&7u32.to_le_bytes());
        body.extend_from_slice(&x2);
        let m = parse_madt(&table(b"APIC", &body)).unwrap();
        assert_eq!(m.lapic_address, 0x1_fee0_0000);
        assert_eq!(
            m.lapics[0],
            LocalApic {
                processor_uid: 7,
                apic_id: 300,
                enabled: true,
                online_capable: false
            }
        );
    }

    #[test]
    fn madt_with_broken_entry_is_rejected() {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&[0, 40, 0, 0]); // entry claims 40 bytes
        assert_eq!(parse_madt(&table(b"APIC", &body)), Err(Error::BadLength));
        assert_eq!(
            parse_madt(&table(b"FACP", &[0; 8])),
            Err(Error::BadSignature)
        );
    }
    #[test]
    fn mcfg_regions() {
        let mut body = vec![0u8; 8];
        body.extend_from_slice(&0xb000_0000u64.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&[0, 255, 0, 0, 0, 0]);
        let r = parse_mcfg(&table(b"MCFG", &body)).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].address(0, 0, 0), Some(0xb000_0000));
        assert_eq!(
            r[0].address(1, 2, 3),
            Some(0xb000_0000 + (1 << 20) + (2 << 15) + (3 << 12))
        );
        assert_eq!(r[0].address(0, 32, 0), None);
        let mut bad = vec![0u8; 8];
        bad.extend_from_slice(&[0; 10]);
        assert_eq!(parse_mcfg(&table(b"MCFG", &bad)), Err(Error::BadLength));
    }

    #[test]
    fn fadt_century() {
        let mut body = vec![0u8; 120 - HEADER_LEN];
        body[108 - HEADER_LEN] = 0x32;
        body[109 - HEADER_LEN] = 0x02;
        let f = parse_fadt(&table(b"FACP", &body)).unwrap();
        assert_eq!(f.century_register, 0x32);
        assert_eq!(f.iapc_boot_flags & 2, 2);
    }

    #[test]
    fn fadt_power_fields() {
        let mut body = vec![0u8; 244 - HEADER_LEN];
        let at = |o: usize| o - HEADER_LEN;
        body[at(40)..at(44)].copy_from_slice(&0x1234u32.to_le_bytes());
        body[at(64)..at(68)].copy_from_slice(&0x604u32.to_le_bytes());
        body[at(112)..at(116)].copy_from_slice(&(1u32 << 10).to_le_bytes());
        body[at(116)] = 1;
        body[at(120)..at(128)].copy_from_slice(&0xcf9u64.to_le_bytes());
        body[at(128)] = 6;
        let f = parse_fadt(&table(b"FACP", &body)).unwrap();
        assert_eq!((f.dsdt, f.pm1a_cnt, f.pm1b_cnt), (0x1234, 0x604, 0));
        assert_eq!(
            (f.reset_space, f.reset_address, f.reset_value),
            (1, 0xcf9, 6)
        );
        assert!(f.flags & Fadt::RESET_REG_SUPPORTED != 0);
        body[at(140)..at(148)].copy_from_slice(&0x5678u64.to_le_bytes());
        assert_eq!(parse_fadt(&table(b"FACP", &body)).unwrap().dsdt, 0x5678);
    }

    #[test]
    fn fadt_pm_timer() {
        let mut body = vec![0u8; 244 - HEADER_LEN];
        let at = |o: usize| o - HEADER_LEN;
        body[at(76)..at(80)].copy_from_slice(&0x608u32.to_le_bytes());
        body[at(112)..at(116)].copy_from_slice(&(1u32 << 8).to_le_bytes());
        let f = parse_fadt(&table(b"FACP", &body)).unwrap();
        assert_eq!((f.pm_timer_port, f.pm_timer_32bit), (0x608, true));
        // The extended block in I/O space takes precedence.
        body[at(208)] = 1;
        body[at(212)..at(220)].copy_from_slice(&0x1808u64.to_le_bytes());
        assert_eq!(
            parse_fadt(&table(b"FACP", &body)).unwrap().pm_timer_port,
            0x1808
        );
        // In memory space it is ignored.
        body[at(208)] = 0;
        assert_eq!(
            parse_fadt(&table(b"FACP", &body)).unwrap().pm_timer_port,
            0x608
        );
    }

    #[test]
    fn hpet_table() {
        let mut body = vec![0u8; 56 - HEADER_LEN];
        let at = |o: usize| o - HEADER_LEN;
        body[at(36)..at(40)].copy_from_slice(&0x8086a201u32.to_le_bytes());
        body[at(44)..at(52)].copy_from_slice(&0xfed0_0000u64.to_le_bytes());
        body[at(53)..at(55)].copy_from_slice(&128u16.to_le_bytes());
        let h = parse_hpet(&table(b"HPET", &body)).unwrap();
        assert_eq!(h.address, 0xfed0_0000);
        assert_eq!(h.min_tick, 128);
        body[at(40)] = 1; // I/O space: not supported
        assert!(parse_hpet(&table(b"HPET", &body)).is_err());
        assert_eq!(
            parse_hpet(&table(b"HPET", &body[..4])),
            Err(Error::TooShort)
        );
        assert_eq!(parse_hpet(&table(b"APIC", &body)), Err(Error::BadSignature));
    }

    #[test]
    fn fadt_events_and_acpi_mode() {
        let mut body = vec![0u8; 276 - HEADER_LEN];
        let at = |o: usize| o - HEADER_LEN;
        body[at(46)..at(48)].copy_from_slice(&9u16.to_le_bytes());
        body[at(48)..at(52)].copy_from_slice(&0xb2u32.to_le_bytes());
        body[at(52)] = 0xf1;
        body[at(56)..at(60)].copy_from_slice(&0x600u32.to_le_bytes());
        body[at(64)..at(68)].copy_from_slice(&0x604u32.to_le_bytes());
        body[at(80)..at(84)].copy_from_slice(&0x620u32.to_le_bytes());
        body[at(88)] = 4;
        body[at(92)] = 16;
        let f = parse_fadt(&table(b"FACP", &body)).unwrap();
        assert_eq!((f.sci_int, f.smi_cmd, f.acpi_enable), (9, 0xb2, 0xf1));
        assert_eq!((f.pm1a_evt, f.pm1_evt_len, f.pm1a_cnt), (0x600, 4, 0x604));
        assert_eq!((f.gpe0_blk, f.gpe0_len, f.gpe1_blk), (0x620, 16, 0));
        assert_eq!(f.sleep_control, (0xff, 0));
        // Extended I/O addresses replace the 32-bit ones.
        body[at(148)] = SPACE_IO;
        body[at(152)..at(160)].copy_from_slice(&0x1800u64.to_le_bytes());
        body[at(172)] = SPACE_IO;
        body[at(176)..at(184)].copy_from_slice(&0x1804u64.to_le_bytes());
        // Hardware-reduced sleep registers.
        body[at(244)] = SPACE_IO;
        body[at(248)..at(256)].copy_from_slice(&0x1810u64.to_le_bytes());
        let f = parse_fadt(&table(b"FACP", &body)).unwrap();
        assert_eq!((f.pm1a_evt, f.pm1a_cnt), (0x1800, 0x1804));
        assert_eq!(f.sleep_control, (SPACE_IO, 0x1810));
        // A short (ACPI 1.0) FADT has none of the extended fields.
        let f = parse_fadt(&table(b"FACP", &body[..at(116)])).unwrap();
        assert_eq!((f.pm1a_evt, f.pm1a_cnt), (0x600, 0x604));
    }

    #[test]
    fn s5_in_other_encodings() {
        // \_S5 with DWord and Word constants, SLP_TYPb missing.
        let aml = [
            0x08, b'\\', b'_', b'S', b'5', b'_', 0x12, 0x07, 0x01, 0x0c, 7, 0, 0, 0,
        ];
        assert_eq!(find_s5(&aml), Some((7, 0)));
        let aml = [
            0x08, b'_', b'S', b'5', b'_', 0x12, 0x07, 0x02, 0x0b, 5, 0, 0x0b, 5, 0,
        ];
        assert_eq!(find_s5(&aml), Some((5, 5)));
        // A reference to _S5_ in code comes first and is skipped.
        let mut aml = vec![0x70, b'_', b'S', b'5', b'_', 0x60];
        aml.extend_from_slice(&[
            0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x04, 0x0a, 3, 0x0a, 3,
        ]);
        assert_eq!(find_s5(&aml), Some((3, 3)));
        // Unknown element encodings are refused, truncated data too.
        assert_eq!(
            find_s5(&[b'_', b'S', b'5', b'_', 0x12, 0x05, 0x02, 0x5b, 1]),
            None
        );
        assert_eq!(
            find_s5(&[b'_', b'S', b'5', b'_', 0x12, 0x05, 0x02, 0x0c, 1]),
            None
        );
    }

    #[test]
    fn s5_package() {
        // Name (_S5, Package (4) { 0x05, Zero, Zero, Zero }) as iasl emits it.
        let aml = [
            0x10, 0x08, b'_', b'S', b'5', b'_', 0x12, 0x0a, 0x04, 0x0a, 0x05, 0x00, 0x00, 0x00,
        ];
        assert_eq!(find_s5(&aml), Some((5, 0)));
        // QEMU: Package { Zero, Zero, ... }
        let aml = [
            0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x04, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(find_s5(&aml), Some((0, 0)));
        assert_eq!(find_s5(b"no sleep state here"), None);
    }
}

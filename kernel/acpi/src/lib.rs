//! ACPI table parsing.
//!
//! The kernel maps the tables and hands byte slices to these functions, so
//! all parsing and validation is tested on the host with synthetic tables.
//! Only what the kernel uses is parsed: RSDP, RSDT/XSDT and the MADT.

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
}

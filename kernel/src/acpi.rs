//! Finding and reading ACPI tables. Parsing lives in `velora-acpi`.
//!
//! With Limine base revision 6 the RSDP address is virtual (HHDM) and all
//! ACPI tables are in memory map regions that the kernel's HHDM covers.

use crate::boot;
use crate::mm::phys_to_virt;
use crate::sync::RacyCell;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use velora_acpi::{self as parse, EcamRegion, Fadt, Hpet, Madt, Rsdp, SdtHeader};

pub struct Acpi {
    pub rsdp: Rsdp,
    /// (signature, physical address) of every table in the RSDT/XSDT.
    pub tables: Vec<([u8; 4], u64)>,
    pub madt: Madt,
    /// PCI Express configuration regions (empty: use I/O ports).
    pub mcfg: Vec<EcamRegion>,
    pub fadt: Fadt,
    pub hpet: Option<Hpet>,
}

static ACPI: RacyCell<Option<Acpi>> = RacyCell::new(None);
static READY: AtomicBool = AtomicBool::new(false);

/// A validated table as a byte slice in the HHDM.
fn table_at(phys: u64) -> Result<(&'static [u8], SdtHeader), &'static str> {
    let va = phys_to_virt(phys) as *const u8;
    // SAFETY: ACPI tables are mapped in the HHDM; the header is read first
    // to learn the length.
    let head = unsafe { core::slice::from_raw_parts(va, parse::HEADER_LEN) };
    let h = parse::parse_header(head).map_err(|_| "short ACPI table")?;
    if h.length as usize > 16 * 1024 * 1024 {
        return Err("ACPI table length is not plausible");
    }
    // SAFETY: as above, now with the full length from the header.
    let full = unsafe { core::slice::from_raw_parts(va, h.length as usize) };
    parse::validate_table(full).map_err(|_| "ACPI table checksum or length is wrong")?;
    Ok((full, h))
}

pub fn init() -> Result<&'static Acpi, &'static str> {
    let rsdp_va = boot::info()
        .rsdp
        .ok_or("no ACPI RSDP from the bootloader")?;
    // SAFETY: Limine reports the RSDP as an HHDM address; 36 bytes cover
    // both RSDP versions.
    let raw = unsafe { core::slice::from_raw_parts(rsdp_va as *const u8, parse::RSDP_V2_LEN) };
    let rsdp = parse::parse_rsdp(raw).map_err(|_| "invalid RSDP")?;
    let root_phys = rsdp.xsdt.unwrap_or(rsdp.rsdt as u64);
    let (root, _) = table_at(root_phys)?;
    let entries = parse::root_entries(root).map_err(|_| "invalid RSDT/XSDT")?;

    let mut tables = Vec::new();
    let mut madt = None;
    let mut mcfg = Vec::new();
    let mut fadt = Fadt::default();
    let mut hpet = None;
    for phys in entries {
        let Ok((bytes, h)) = table_at(phys) else {
            crate::kwarn!("acpi: skipping invalid table at {:#x}", phys);
            continue;
        };
        tables.push((h.signature, phys));
        match &h.signature {
            b"APIC" => madt = Some(parse::parse_madt(bytes).map_err(|_| "invalid MADT")?),
            b"MCFG" => mcfg = parse::parse_mcfg(bytes).unwrap_or_default(),
            b"FACP" => fadt = parse::parse_fadt(bytes).unwrap_or_default(),
            b"HPET" => hpet = parse::parse_hpet(bytes).ok(),
            _ => {}
        }
    }
    let madt = madt.ok_or("no MADT (APIC) table")?;
    // SAFETY: written once during single-threaded boot.
    unsafe {
        *ACPI.get() = Some(Acpi {
            rsdp,
            tables,
            madt,
            mcfg,
            fadt,
            hpet,
        })
    };
    READY.store(true, Ordering::SeqCst);
    Ok(get())
}

/// The DSDT as bytes, if the FADT points to a valid one.
pub fn dsdt() -> Option<&'static [u8]> {
    let a = get();
    if a.fadt.dsdt == 0 {
        return None;
    }
    table_at(a.fadt.dsdt).ok().map(|(b, _)| b)
}

pub fn get() -> &'static Acpi {
    assert!(READY.load(Ordering::SeqCst), "ACPI used before init");
    // SAFETY: read-only after init.
    unsafe { (*ACPI.get()).as_ref().unwrap() }
}

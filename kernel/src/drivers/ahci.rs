//! AHCI (SATA) driver for ATA disks. ATAPI devices (CD/DVD) are skipped.
//!
//! One command slot per port, completion is polled, data goes through a
//! 64 KiB bounce buffer. NOT IMPLEMENTED: BIOS/OS handoff (BOHC), needed on
//! some real machines whose firmware keeps using the controller.

use super::block::{self, BlockDevice, BlockError};
use super::dma::DmaBuffer;
use super::pci;
use crate::mm::mmio;
use crate::sync::SpinLock;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use velora_pci::Device;

const GHC: u64 = 0x04;
const PI: u64 = 0x0c;
const GHC_AE: u32 = 1 << 31;
const GHC_HR: u32 = 1 << 0;

const P_CLB: u64 = 0x00;
const P_CLBU: u64 = 0x04;
const P_FB: u64 = 0x08;
const P_FBU: u64 = 0x0c;
const P_IS: u64 = 0x10;
const P_IE: u64 = 0x14;
const P_CMD: u64 = 0x18;
const P_TFD: u64 = 0x20;
const P_SIG: u64 = 0x24;
const P_SSTS: u64 = 0x28;
const P_SERR: u64 = 0x30;
const P_CI: u64 = 0x38;

const CMD_ST: u32 = 1 << 0;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;
const TFD_ERR: u32 = 1 << 0;
const TFD_DRQ: u32 = 1 << 3;
const TFD_BSY: u32 = 1 << 7;
const IS_TFES: u32 = 1 << 30;

const SIG_ATA: u32 = 0x0000_0101;

const ATA_IDENTIFY: u8 = 0xec;
const ATA_READ_DMA_EXT: u8 = 0x25;
const ATA_WRITE_DMA_EXT: u8 = 0x35;
const ATA_FLUSH_EXT: u8 = 0xea;

const SECTOR: usize = 512;
const BOUNCE: usize = 64 * 1024;

// Per-port buffer layout.
const CLB_OFF: usize = 0;
const FB_OFF: usize = 1024;
const CTBA_OFF: usize = 2048;

struct Port {
    regs: u64,
    mem: DmaBuffer,
    data: DmaBuffer,
}

pub struct AhciDisk {
    name: String,
    model: String,
    sectors: u64,
    port: SpinLock<Port>,
}

impl Drop for Port {
    /// Stops the port before its DMA buffers are freed.
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl Port {
    fn r(&self, off: u64) -> u32 {
        // SAFETY: `regs` is this port's mapped register block.
        unsafe { mmio::read32(self.regs + off) }
    }

    fn w(&self, off: u64, v: u32) {
        // SAFETY: as in `r`.
        unsafe { mmio::write32(self.regs + off, v) }
    }

    fn stop(&self) -> Result<(), BlockError> {
        self.w(P_CMD, self.r(P_CMD) & !(CMD_ST | CMD_FRE));
        block::poll(500, || self.r(P_CMD) & (CMD_CR | CMD_FR) == 0)
    }

    fn start(&mut self) -> Result<(), BlockError> {
        self.stop()?;
        let base = self.mem.phys();
        self.w(P_CLB, (base + CLB_OFF as u64) as u32);
        self.w(P_CLBU, ((base + CLB_OFF as u64) >> 32) as u32);
        self.w(P_FB, (base + FB_OFF as u64) as u32);
        self.w(P_FBU, ((base + FB_OFF as u64) >> 32) as u32);
        self.w(P_SERR, 0xffff_ffff);
        self.w(P_IS, 0xffff_ffff);
        self.w(P_IE, 0); // polled
        self.w(P_CMD, self.r(P_CMD) | CMD_FRE);
        block::poll(1000, || self.r(P_TFD) & (TFD_BSY | TFD_DRQ) == 0)?;
        self.w(P_CMD, self.r(P_CMD) | CMD_ST);
        Ok(())
    }

    /// Issues one ATA command in slot 0. `len` bytes of `data` are used.
    fn command(
        &mut self,
        cmd: u8,
        lba: u64,
        sectors: u16,
        len: usize,
        write: bool,
    ) -> Result<(), BlockError> {
        let table = self.mem.phys() + CTBA_OFF as u64;
        let prdtl: u32 = if len > 0 { 1 } else { 0 };
        // Command header 0: FIS length 5 dwords, write flag, PRDT length.
        let flags = 5 | if write { 1 << 6 } else { 0 } | prdtl << 16;
        self.mem.write::<u32>(CLB_OFF, flags);
        self.mem.write::<u32>(CLB_OFF + 4, 0);
        self.mem.write::<u32>(CLB_OFF + 8, table as u32);
        self.mem.write::<u32>(CLB_OFF + 12, (table >> 32) as u32);
        // Command FIS (host to device register FIS).
        let mut fis = [0u8; 20];
        fis[0] = 0x27;
        fis[1] = 0x80; // command, not control
        fis[2] = cmd;
        fis[4] = lba as u8;
        fis[5] = (lba >> 8) as u8;
        fis[6] = (lba >> 16) as u8;
        fis[7] = 1 << 6; // LBA mode
        fis[8] = (lba >> 24) as u8;
        fis[9] = (lba >> 32) as u8;
        fis[10] = (lba >> 40) as u8;
        fis[12] = sectors as u8;
        fis[13] = (sectors >> 8) as u8;
        for (i, b) in fis.iter().enumerate() {
            self.mem.write::<u8>(CTBA_OFF + i, *b);
        }
        if len > 0 {
            let dba = self.data.phys();
            self.mem.write::<u32>(CTBA_OFF + 0x80, dba as u32);
            self.mem.write::<u32>(CTBA_OFF + 0x84, (dba >> 32) as u32);
            self.mem.write::<u32>(CTBA_OFF + 0x88, 0);
            self.mem.write::<u32>(CTBA_OFF + 0x8c, (len - 1) as u32);
        }
        block::poll(1000, || self.r(P_TFD) & (TFD_BSY | TFD_DRQ) == 0)?;
        self.w(P_IS, 0xffff_ffff);
        self.w(P_CI, 1);
        let r = block::poll(5000, || {
            self.r(P_CI) & 1 == 0 || self.r(P_IS) & IS_TFES != 0
        });
        if self.r(P_IS) & IS_TFES != 0 || self.r(P_TFD) & TFD_ERR != 0 {
            self.w(P_IS, 0xffff_ffff);
            return Err(BlockError::Device);
        }
        r
    }
}

fn ata_string(id: &[u8], from: usize, to: usize) -> String {
    let mut s = String::new();
    for pair in id[from..to].as_chunks::<2>().0 {
        s.push(pair[1] as char);
        s.push(pair[0] as char);
    }
    String::from(s.trim())
}

impl BlockDevice for AhciDisk {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        format!("AHCI SATA disk \"{}\"", self.model)
    }

    fn model(&self) -> String {
        self.model.clone()
    }

    fn kind(&self) -> u32 {
        velora_abi::disk_kind::SATA
    }

    fn sector_size(&self) -> usize {
        SECTOR
    }

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        block::check_request(self, lba, buf.len())?;
        let mut p = self.port.lock();
        for (i, chunk) in buf.chunks_mut(BOUNCE).enumerate() {
            let at = lba + (i * BOUNCE / SECTOR) as u64;
            let n = (chunk.len() / SECTOR) as u16;
            p.command(ATA_READ_DMA_EXT, at, n, chunk.len(), false)?;
            chunk.copy_from_slice(&p.data.as_slice()[..chunk.len()]);
        }
        Ok(())
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        block::check_request(self, lba, buf.len())?;
        let mut p = self.port.lock();
        for (i, chunk) in buf.chunks(BOUNCE).enumerate() {
            let at = lba + (i * BOUNCE / SECTOR) as u64;
            let n = (chunk.len() / SECTOR) as u16;
            p.data.as_mut_slice()[..chunk.len()].copy_from_slice(chunk);
            p.command(ATA_WRITE_DMA_EXT, at, n, chunk.len(), true)?;
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.port.lock().command(ATA_FLUSH_EXT, 0, 0, 0, false)
    }

    unsafe fn force_unlock(&self) {
        // SAFETY: the caller is the panic path.
        unsafe { self.port.force_unlock() }
    }
}

fn probe_controller(d: &Device, first_index: usize) -> Result<usize, &'static str> {
    let cfg = pci::config();
    velora_pci::enable_device(cfg, d.addr);
    let (abar, size) = d.bars[5].memory().ok_or("BAR5 (ABAR) missing")?;
    let base = mmio::map(abar, size.max(0x1100)).ok_or("cannot map ABAR")?;
    // SAFETY: ABAR is the controller's mapped register block.
    // Reset the whole HBA first. The firmware may have left ports running
    // (for example the CD port it booted from) with DMA pointing into
    // memory the kernel now owns.
    // SAFETY (register accesses below): ABAR is the controller's mapped
    // register block.
    unsafe {
        mmio::write32(base + GHC, mmio::read32(base + GHC) | GHC_AE);
        mmio::write32(base + GHC, mmio::read32(base + GHC) | GHC_HR);
    }
    block::poll(1000, || unsafe { mmio::read32(base + GHC) } & GHC_HR == 0)
        .map_err(|_| "HBA reset timed out")?;
    let pi = unsafe {
        mmio::write32(base + GHC, mmio::read32(base + GHC) | GHC_AE);
        mmio::read32(base + PI)
    };
    // Links renegotiate after the reset; give them time to come up.
    let all_up = || {
        (0..32)
            .filter(|p| pi & (1 << p) != 0)
            .all(|p| unsafe { mmio::read32(base + 0x100 + p as u64 * 0x80 + P_SSTS) } & 0xf == 3)
    };
    let _ = block::poll(50, all_up);
    let mut found = 0;
    for port in 0..32 {
        if pi & (1 << port) == 0 {
            continue;
        }
        let regs = base + 0x100 + port as u64 * 0x80;
        // SAFETY: an implemented port's register block.
        let ssts = unsafe { mmio::read32(regs + P_SSTS) };
        if ssts & 0xf != 3 || (ssts >> 8) & 0xf != 1 {
            continue; // no device
        }
        let (Some(mem), Some(data)) = (DmaBuffer::new(4096), DmaBuffer::new(BOUNCE)) else {
            return Err("out of memory");
        };
        let mut p = Port { regs, mem, data };
        if p.start().is_err() {
            crate::kwarn!("ahci: port {} did not start", port);
            continue;
        }
        // The signature arrives with the device's first FIS after the
        // port starts receiving.
        let _ = block::poll(100, || p.r(P_SIG) != 0xffff_ffff);
        if p.r(P_SIG) != SIG_ATA {
            continue; // ATAPI (CD/DVD) or unknown: `p` is stopped on drop
        }
        if p.command(ATA_IDENTIFY, 0, 0, 512, false).is_err() {
            crate::kwarn!("ahci: port {} did not answer IDENTIFY", port);
            continue;
        }
        let id = &p.data.as_slice()[..512];
        let lba48 = u64::from_le_bytes(id[200..208].try_into().unwrap());
        let lba28 = u32::from_le_bytes(id[120..124].try_into().unwrap()) as u64;
        let sectors = if lba48 != 0 { lba48 } else { lba28 };
        let model = ata_string(id, 54, 94);
        block::register(Arc::new(AhciDisk {
            name: format!("sd{}", first_index + found),
            model,
            sectors,
            port: SpinLock::new(p),
        }));
        found += 1;
    }
    Ok(found)
}

/// Finds every AHCI controller and registers its ATA disks.
pub fn probe_all() -> usize {
    let mut n = 0;
    for d in pci::devices().iter().filter(|d| d.is(0x01, 0x06, 0x01)) {
        match probe_controller(d, n) {
            Ok(k) => n += k,
            Err(e) => crate::kwarn!("ahci {}: {}", d.addr, e),
        }
    }
    n
}

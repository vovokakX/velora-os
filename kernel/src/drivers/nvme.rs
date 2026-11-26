//! NVMe driver: admin queue, one I/O queue pair, namespace 1.
//!
//! Completion is polled (interrupts disabled for the queues). Data goes
//! through a 64 KiB bounce buffer described by a PRP list.

use super::block::{self, BlockDevice, BlockError};
use super::dma::DmaBuffer;
use super::pci;
use crate::mm::mmio;
use crate::sync::SpinLock;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use velora_pci::Device;

const REG_CAP: u64 = 0x00;
const REG_CC: u64 = 0x14;
const REG_CSTS: u64 = 0x1c;
const REG_AQA: u64 = 0x24;
const REG_ASQ: u64 = 0x28;
const REG_ACQ: u64 = 0x30;
const DOORBELLS: u64 = 0x1000;

const CC_EN: u32 = 1;
const CC_IOSQES: u32 = 6 << 16;
const CC_IOCQES: u32 = 4 << 20;
const CSTS_RDY: u32 = 1;
const CSTS_CFS: u32 = 1 << 1;

const ADMIN_CREATE_SQ: u8 = 0x01;
const ADMIN_CREATE_CQ: u8 = 0x05;
const ADMIN_IDENTIFY: u8 = 0x06;
const IO_FLUSH: u8 = 0x00;
const IO_WRITE: u8 = 0x01;
const IO_READ: u8 = 0x02;

const QUEUE_LEN: u16 = 16;
const BOUNCE: usize = 64 * 1024;
const NSID: u32 = 1;

struct QueuePair {
    sq: DmaBuffer,
    cq: DmaBuffer,
    sq_tail: u16,
    cq_head: u16,
    phase: bool,
    sq_doorbell: u64,
    cq_doorbell: u64,
    next_cid: u16,
}

impl QueuePair {
    fn new(qid: u16, regs: u64, stride: u64) -> Option<Self> {
        Some(Self {
            sq: DmaBuffer::new(QUEUE_LEN as usize * 64)?,
            cq: DmaBuffer::new(QUEUE_LEN as usize * 16)?,
            sq_tail: 0,
            cq_head: 0,
            phase: true,
            sq_doorbell: regs + DOORBELLS + (2 * qid as u64) * stride,
            cq_doorbell: regs + DOORBELLS + (2 * qid as u64 + 1) * stride,
            next_cid: 0,
        })
    }

    /// Submits a command (cdw0 opcode/cid filled in here) and waits for its
    /// completion. Returns completion dword 0.
    fn submit(&mut self, mut cmd: [u32; 16]) -> Result<u32, BlockError> {
        let cid = self.next_cid;
        self.next_cid = self.next_cid.wrapping_add(1);
        cmd[0] = (cmd[0] & 0xff) | (cid as u32) << 16;
        let slot = self.sq_tail as usize * 64;
        for (i, dw) in cmd.iter().enumerate() {
            self.sq.write::<u32>(slot + i * 4, *dw);
        }
        self.sq_tail = (self.sq_tail + 1) % QUEUE_LEN;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        // SAFETY: this queue's submission tail doorbell.
        unsafe { mmio::write32(self.sq_doorbell, self.sq_tail as u32) };

        let entry = self.cq_head as usize * 16;
        let phase = self.phase;
        let cq = &self.cq;
        block::poll(5000, || {
            (cq.read::<u32>(entry + 12) >> 16) & 1 == phase as u32
        })?;
        let dw0 = self.cq.read::<u32>(entry);
        let dw3 = self.cq.read::<u32>(entry + 12);
        self.cq_head += 1;
        if self.cq_head == QUEUE_LEN {
            self.cq_head = 0;
            self.phase = !self.phase;
        }
        // SAFETY: this queue's completion head doorbell.
        unsafe { mmio::write32(self.cq_doorbell, self.cq_head as u32) };
        let status = (dw3 >> 17) & 0x7fff;
        if (dw3 & 0xffff) as u16 != cid || status != 0 {
            return Err(BlockError::Device);
        }
        Ok(dw0)
    }
}

struct Io {
    queue: QueuePair,
    data: DmaBuffer,
    prp_list: DmaBuffer,
}

pub struct NvmeDisk {
    name: String,
    model: String,
    sectors: u64,
    sector_size: usize,
    io: SpinLock<Io>,
}

fn command(opcode: u8, nsid: u32) -> [u32; 16] {
    let mut c = [0u32; 16];
    c[0] = opcode as u32;
    c[1] = nsid;
    c
}

impl Io {
    /// PRP entries for `len` bytes of the bounce buffer.
    fn prps(&mut self, len: usize) -> (u64, u64) {
        let base = self.data.phys();
        let pages = len.div_ceil(4096);
        match pages {
            0 | 1 => (base, 0),
            2 => (base, base + 4096),
            _ => {
                for i in 1..pages {
                    self.prp_list
                        .write::<u64>((i - 1) * 8, base + i as u64 * 4096);
                }
                (base, self.prp_list.phys())
            }
        }
    }

    fn rw(&mut self, opcode: u8, lba: u64, blocks: u32, len: usize) -> Result<(), BlockError> {
        let (prp1, prp2) = self.prps(len);
        let mut c = command(opcode, NSID);
        c[6] = prp1 as u32;
        c[7] = (prp1 >> 32) as u32;
        c[8] = prp2 as u32;
        c[9] = (prp2 >> 32) as u32;
        c[10] = lba as u32;
        c[11] = (lba >> 32) as u32;
        c[12] = blocks - 1;
        self.queue.submit(c).map(|_| ())
    }
}

impl BlockDevice for NvmeDisk {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        format!("NVMe \"{}\"", self.model)
    }

    fn sector_size(&self) -> usize {
        self.sector_size
    }

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        block::check_request(self, lba, buf.len())?;
        let mut io = self.io.lock();
        for (i, chunk) in buf.chunks_mut(BOUNCE).enumerate() {
            let at = lba + (i * BOUNCE / self.sector_size) as u64;
            io.rw(
                IO_READ,
                at,
                (chunk.len() / self.sector_size) as u32,
                chunk.len(),
            )?;
            chunk.copy_from_slice(&io.data.as_slice()[..chunk.len()]);
        }
        Ok(())
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        block::check_request(self, lba, buf.len())?;
        let mut io = self.io.lock();
        for (i, chunk) in buf.chunks(BOUNCE).enumerate() {
            let at = lba + (i * BOUNCE / self.sector_size) as u64;
            io.data.as_mut_slice()[..chunk.len()].copy_from_slice(chunk);
            io.rw(
                IO_WRITE,
                at,
                (chunk.len() / self.sector_size) as u32,
                chunk.len(),
            )?;
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.io
            .lock()
            .queue
            .submit(command(IO_FLUSH, NSID))
            .map(|_| ())
    }
}

fn probe(d: &Device, index: usize) -> Result<NvmeDisk, &'static str> {
    let cfg = pci::config();
    velora_pci::enable_device(cfg, d.addr);
    let (bar, size) = d.bars[0].memory().ok_or("BAR0 missing")?;
    let regs = mmio::map(bar, size.clamp(0x2000, 0x4000)).ok_or("cannot map registers")?;
    let r32 = |off: u64| unsafe { mmio::read32(regs + off) };
    let w32 = |off: u64, v: u32| unsafe { mmio::write32(regs + off, v) };
    // SAFETY: BAR0 is the controller's mapped register block.
    let cap = unsafe { mmio::read64(regs + REG_CAP) };
    let stride = 4u64 << ((cap >> 32) & 0xf);
    let timeout_ms = ((cap >> 24) & 0xff).max(1) * 500;
    if (cap >> 37) & 1 == 0 {
        return Err("controller does not support the NVM command set");
    }

    w32(REG_CC, r32(REG_CC) & !CC_EN);
    block::poll(timeout_ms, || r32(REG_CSTS) & CSTS_RDY == 0)
        .map_err(|_| "controller did not stop")?;
    let mut admin = QueuePair::new(0, regs, stride).ok_or("out of memory")?;
    let q = (QUEUE_LEN - 1) as u32;
    w32(REG_AQA, q << 16 | q);
    // SAFETY: admin queue base registers.
    unsafe {
        mmio::write64(regs + REG_ASQ, admin.sq.phys());
        mmio::write64(regs + REG_ACQ, admin.cq.phys());
    }
    w32(REG_CC, CC_EN | CC_IOSQES | CC_IOCQES);
    block::poll(timeout_ms, || r32(REG_CSTS) & (CSTS_RDY | CSTS_CFS) != 0)
        .map_err(|_| "controller did not start")?;
    if r32(REG_CSTS) & CSTS_CFS != 0 {
        return Err("controller fatal status");
    }

    let mut ident = DmaBuffer::new(4096).ok_or("out of memory")?;
    let identify = |admin: &mut QueuePair, ident: &DmaBuffer, cns: u32, nsid: u32| {
        let mut c = command(ADMIN_IDENTIFY, nsid);
        c[6] = ident.phys() as u32;
        c[7] = (ident.phys() >> 32) as u32;
        c[10] = cns;
        admin.submit(c)
    };
    identify(&mut admin, &ident, 1, 0).map_err(|_| "identify controller failed")?;
    let model = String::from(
        core::str::from_utf8(&ident.as_slice()[24..64])
            .unwrap_or("")
            .trim(),
    );
    ident.as_mut_slice().fill(0);
    identify(&mut admin, &ident, 0, NSID).map_err(|_| "identify namespace failed")?;
    let sectors = ident.read::<u64>(0);
    let flbas = (ident.read::<u8>(26) & 0xf) as usize;
    let lbads = (ident.read::<u32>(128 + 4 * flbas) >> 16) & 0xff;
    if sectors == 0 || !(9..=12).contains(&lbads) {
        return Err("namespace 1 is missing or has an unsupported block size");
    }

    let io_queue = QueuePair::new(1, regs, stride).ok_or("out of memory")?;
    let mut c = command(ADMIN_CREATE_CQ, 0);
    c[6] = io_queue.cq.phys() as u32;
    c[7] = (io_queue.cq.phys() >> 32) as u32;
    c[10] = q << 16 | 1;
    c[11] = 1; // physically contiguous, no interrupts
    admin
        .submit(c)
        .map_err(|_| "create I/O completion queue failed")?;
    let mut c = command(ADMIN_CREATE_SQ, 0);
    c[6] = io_queue.sq.phys() as u32;
    c[7] = (io_queue.sq.phys() >> 32) as u32;
    c[10] = q << 16 | 1;
    c[11] = 1 << 16 | 1; // completion queue 1, physically contiguous
    admin
        .submit(c)
        .map_err(|_| "create I/O submission queue failed")?;
    // The admin queue pair stays allocated for the controller's lifetime.
    core::mem::forget(admin);

    Ok(NvmeDisk {
        name: format!("nvme{}n1", index),
        model,
        sectors,
        sector_size: 1 << lbads,
        io: SpinLock::new(Io {
            queue: io_queue,
            data: DmaBuffer::new(BOUNCE).ok_or("out of memory")?,
            prp_list: DmaBuffer::new(4096).ok_or("out of memory")?,
        }),
    })
}

/// Finds every NVMe controller and registers namespace 1 of each.
pub fn probe_all() -> usize {
    let mut n = 0;
    for d in pci::devices().iter().filter(|d| d.is(0x01, 0x08, 0x02)) {
        match probe(d, n) {
            Ok(disk) => {
                block::register(Arc::new(disk));
                n += 1;
            }
            Err(e) => crate::kwarn!("nvme {}: {}", d.addr, e),
        }
    }
    n
}

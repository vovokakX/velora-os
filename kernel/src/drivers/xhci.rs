//! xHCI USB host controller, with the devices Velora uses: boot protocol
//! keyboards and mice (HID) and flash drives (mass storage, bulk-only
//! transport, SCSI).
//!
//! - Commands and control/bulk transfers wait for their event by polling,
//!   like the disk drivers (D21).
//! - Keyboard and mouse reports: with MSI-X or MSI the interrupt handler
//!   reads the event ring, so a mouse is read at its own rate. Without
//!   them (or with `velora.xhci_poll` on the command line) the `usb`
//!   kernel thread reads it every timer tick (100 Hz). D51.
//! - Devices on root hub ports that are connected at boot are set up.
//! - Descriptor parsing, HID reports and SCSI commands are in
//!   `velora-usb` (tested on the host).
//!
//! NOT IMPLEMENTED: USB hubs (devices behind a hub are not seen), hot
//! plugging (devices connected after boot), keyboard LEDs, isochronous
//! transfers, legacy INTx interrupts (without MSI the controller is
//! polled).

use super::block::{self, BlockDevice, BlockError};
use super::dma::DmaBuffer;
use super::msix::{self, MsiX};
use super::pci;
use crate::arch::x86_64::{idt::InterruptFrame, irq};
use crate::mm::mmio;
use crate::sched::{self, Priority, WaitQueue};
use crate::sync::SpinLock;
use crate::{input, kok, kwarn, time};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use velora_input::{Keyboard as Decoder, Layout};
use velora_usb::desc::{self, Endpoint, Setup};
use velora_usb::{hid, msc, report};

// Capability registers.
const CAP_HCSPARAMS1: u64 = 0x04;
const CAP_HCSPARAMS2: u64 = 0x08;
const CAP_HCCPARAMS1: u64 = 0x10;
const CAP_DBOFF: u64 = 0x14;
const CAP_RTSOFF: u64 = 0x18;
// Operational registers.
const OP_USBCMD: u64 = 0x00;
const OP_USBSTS: u64 = 0x04;
const OP_CRCR: u64 = 0x18;
const OP_DCBAAP: u64 = 0x30;
const OP_CONFIG: u64 = 0x38;
const OP_PORTS: u64 = 0x400;
const CMD_RUN: u32 = 1;
const CMD_RESET: u32 = 1 << 1;
const CMD_INTERRUPTS: u32 = 1 << 2;
const STS_HALTED: u32 = 1;
const STS_EVENT_INTERRUPT: u32 = 1 << 3;
const STS_NOT_READY: u32 = 1 << 11;
// PORTSC bits.
const PORT_CONNECTED: u32 = 1;
const PORT_ENABLED: u32 = 1 << 1;
const PORT_RESET: u32 = 1 << 4;
const PORT_POWER: u32 = 1 << 9;
const PORT_CHANGE_BITS: u32 = 0x00fe_0000;
const PORT_RESET_CHANGE: u32 = 1 << 21;
// TRB types.
const TRB_NORMAL: u32 = 1;
const TRB_SETUP: u32 = 2;
const TRB_DATA: u32 = 3;
const TRB_STATUS: u32 = 4;
const TRB_LINK: u32 = 6;
const TRB_ENABLE_SLOT: u32 = 9;
const TRB_ADDRESS_DEVICE: u32 = 11;
const TRB_CONFIGURE_EP: u32 = 12;
const TRB_EVALUATE_CTX: u32 = 13;
const TRB_TRANSFER_EVENT: u32 = 32;
const TRB_COMMAND_DONE: u32 = 33;
// TRB control bits.
const TRB_CYCLE: u32 = 1;
const TRB_TOGGLE: u32 = 1 << 1;
const TRB_CHAIN: u32 = 1 << 4;
const TRB_IOC: u32 = 1 << 5;
const TRB_IDT: u32 = 1 << 6;
const TRB_DIR_IN: u32 = 1 << 16;
const CC_SUCCESS: u32 = 1;
const CC_SHORT_PACKET: u32 = 13;
// Endpoint types in the endpoint context.
const EP_CONTROL: u32 = 4;
const EP_BULK_OUT: u32 = 2;
const EP_BULK_IN: u32 = 6;
const EP_INTERRUPT_IN: u32 = 7;

// Interrupter 0 registers (offsets in its register set).
const IR_MANAGEMENT: u64 = 0x00;
const IR_MODERATION: u64 = 0x04;
const IMAN_PENDING: u32 = 1;
const IMAN_ENABLE: u32 = 1 << 1;
/// At most one interrupt a millisecond (in units of 250 ns).
const MODERATION_250NS: u32 = 4000;

const RING_TRBS: usize = 256;
const TIMEOUT_MS: u64 = 2000;

#[derive(Debug)]
enum UsbError {
    Timeout,
    Completion(u32),
    NoMemory,
    Descriptor,
}

/// A producer ring (command or transfer ring) of 256 TRBs, the last one a
/// link back to the start.
struct Ring {
    mem: DmaBuffer,
    enq: usize,
    cycle: bool,
}

impl Ring {
    fn new() -> Option<Self> {
        Some(Self {
            mem: DmaBuffer::new(RING_TRBS * 16)?,
            enq: 0,
            cycle: true,
        })
    }

    fn phys(&self) -> u64 {
        self.mem.phys()
    }

    /// Writes a TRB (the cycle bit is added) and returns its address.
    fn push(&mut self, trb: [u32; 4]) -> u64 {
        let at = self.enq;
        let cyc = if self.cycle { TRB_CYCLE } else { 0 };
        self.mem.write(at * 16, trb[0]);
        self.mem.write(at * 16 + 4, trb[1]);
        self.mem.write(at * 16 + 8, trb[2]);
        // The control word goes last: it hands the TRB to the controller.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.mem.write(at * 16 + 12, (trb[3] & !TRB_CYCLE) | cyc);
        let addr = self.phys() + (at * 16) as u64;
        self.enq += 1;
        if self.enq == RING_TRBS - 1 {
            let link = self.phys();
            self.mem.write((RING_TRBS - 1) * 16, link as u32);
            self.mem
                .write((RING_TRBS - 1) * 16 + 4, (link >> 32) as u32);
            self.mem.write((RING_TRBS - 1) * 16 + 8, 0u32);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            self.mem
                .write((RING_TRBS - 1) * 16 + 12, TRB_LINK << 10 | TRB_TOGGLE | cyc);
            self.enq = 0;
            self.cycle = !self.cycle;
        }
        addr
    }
}

#[derive(Clone, Copy)]
struct Event {
    trb: [u32; 4],
}

impl Event {
    fn kind(&self) -> u32 {
        (self.trb[3] >> 10) & 0x3f
    }
    fn code(&self) -> u32 {
        self.trb[2] >> 24
    }
    fn slot(&self) -> u8 {
        (self.trb[3] >> 24) as u8
    }
    fn endpoint(&self) -> u8 {
        ((self.trb[3] >> 16) & 0x1f) as u8
    }
    fn pointer(&self) -> u64 {
        self.trb[0] as u64 | (self.trb[1] as u64) << 32
    }
    /// Bytes not transferred (transfer events).
    fn residue(&self) -> usize {
        (self.trb[2] & 0xff_ffff) as usize
    }
}

/// What a configured device is used for.
enum Function {
    Keyboard {
        dci: u8,
        ring: Ring,
        reports: Reports,
        report: hid::Keyboard,
        decoder: Decoder,
    },
    Mouse {
        dci: u8,
        ring: Ring,
        reports: Reports,
    },
    /// A pointer in report protocol (a tablet such as QEMU's usb-tablet,
    /// or a mouse that is not a boot device), read with its report
    /// descriptor.
    Pointer {
        dci: u8,
        ring: Ring,
        reports: Reports,
        format: report::Pointer,
    },
    Storage,
}

/// Transfers queued at once on a keyboard or mouse endpoint.
const REPORTS_QUEUED: usize = 8;

/// Report buffers of an interrupt IN endpoint. Several transfers stay
/// queued, so the controller can deliver more than one report before the
/// driver looks at the event ring again (a polled controller is read
/// once a tick, and a device may report more often).
struct Reports {
    bufs: Vec<DmaBuffer>,
    /// Queued transfers, oldest first: TRB address and buffer.
    queued: Vec<(u64, usize)>,
    len: u32,
}

impl Reports {
    /// Buffers for reports of `len` bytes, all queued on `ring`.
    fn new(ring: &mut Ring, len: u32) -> Option<Self> {
        let mut r = Reports {
            bufs: Vec::new(),
            queued: Vec::new(),
            len,
        };
        for i in 0..REPORTS_QUEUED {
            r.bufs.push(DmaBuffer::new(64)?);
            r.queue(ring, i);
        }
        Some(r)
    }

    fn queue(&mut self, ring: &mut Ring, i: usize) {
        let p = self.bufs[i].phys();
        let trb = ring.push([
            p as u32,
            (p >> 32) as u32,
            self.len,
            TRB_NORMAL << 10 | TRB_IOC | 1 << 2,
        ]);
        self.queued.push((trb, i));
    }

    /// The buffer of the transfer at TRB `trb`, which has completed.
    fn complete(&mut self, trb: u64) -> Option<usize> {
        let at = self.queued.iter().position(|&(t, _)| t == trb)?;
        Some(self.queued.remove(at).1)
    }

    /// Copies the start of buffer `i` into `out`.
    fn read(&self, i: usize, out: &mut [u8]) {
        for (k, b) in out.iter_mut().enumerate() {
            *b = self.bufs[i].read::<u8>(k);
        }
    }
}

struct Slot {
    id: u8,
    ep0: Ring,
    /// Transfer rings of the other endpoints, by device context index.
    rings: Vec<(u8, Ring)>,
    function: Option<Function>,
}

struct Controller {
    cap: u64,
    op: u64,
    rt: u64,
    db: u64,
    ports: u8,
    /// Context size: 32 or 64 bytes.
    csz: usize,
    cmd: Ring,
    events: DmaBuffer,
    ev_deq: usize,
    ev_cycle: bool,
    _erst: DmaBuffer,
    dcbaa: DmaBuffer,
    contexts: Vec<DmaBuffer>,
    _scratch: Vec<DmaBuffer>,
    slots: Vec<Slot>,
    irq: Irq,
}

/// How the controller tells the `usb` thread about events.
enum Irq {
    MsiX(MsiX),
    Msi,
    Polled,
}

impl Irq {
    fn name(&self) -> &'static str {
        match self {
            Irq::MsiX(_) => "MSI-X",
            Irq::Msi => "MSI",
            Irq::Polled => "polled",
        }
    }
}

fn r32(addr: u64) -> u32 {
    // SAFETY: `addr` is inside the controller's mapped register block.
    unsafe { mmio::read32(addr) }
}

fn w32(addr: u64, v: u32) {
    // SAFETY: as above.
    unsafe { mmio::write32(addr, v) }
}

fn w64(addr: u64, v: u64) {
    w32(addr, v as u32);
    w32(addr + 4, (v >> 32) as u32);
}

fn wait(timeout_ms: u64, mut done: impl FnMut() -> bool) -> Result<(), UsbError> {
    block::poll(timeout_ms, &mut done).map_err(|_| UsbError::Timeout)
}

impl Controller {
    fn portsc(&self, port: u8) -> u64 {
        self.op + OP_PORTS + 0x10 * (port as u64 - 1)
    }

    fn ring_doorbell(&self, slot: u8, target: u8) {
        w32(self.db + 4 * slot as u64, target as u32);
    }

    /// The next event, if the controller has written one.
    fn next_event(&mut self) -> Option<Event> {
        let off = self.ev_deq * 16;
        let ctrl: u32 = self.events.read(off + 12);
        if (ctrl & TRB_CYCLE != 0) != self.ev_cycle {
            return None;
        }
        let trb = [
            self.events.read(off),
            self.events.read(off + 4),
            self.events.read(off + 8),
            ctrl,
        ];
        self.ev_deq += 1;
        if self.ev_deq == RING_TRBS {
            self.ev_deq = 0;
            self.ev_cycle = !self.ev_cycle;
        }
        // Tell the controller how far we got (and clear Event Handler Busy).
        w64(
            self.rt + 0x20 + 0x18,
            (self.events.phys() + (self.ev_deq * 16) as u64) | 1 << 3,
        );
        Some(Event { trb })
    }

    /// Waits for an event accepted by `want`; keyboard and mouse reports
    /// that arrive meanwhile are handled.
    fn wait_event(&mut self, mut want: impl FnMut(&Event) -> bool) -> Result<Event, UsbError> {
        let deadline = crate::arch::cpu::rdtsc() + time::tsc_hz() * TIMEOUT_MS / 1000;
        loop {
            while let Some(e) = self.next_event() {
                if want(&e) {
                    return Ok(e);
                }
                self.handle_async(&e);
            }
            if crate::arch::cpu::rdtsc() > deadline {
                return Err(UsbError::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    fn command(&mut self, trb: [u32; 4]) -> Result<Event, UsbError> {
        let addr = self.cmd.push(trb);
        self.ring_doorbell(0, 0);
        let e = self.wait_event(|e| e.kind() == TRB_COMMAND_DONE && e.pointer() == addr)?;
        if e.code() != CC_SUCCESS {
            return Err(UsbError::Completion(e.code()));
        }
        Ok(e)
    }

    fn slot_mut(&mut self, id: u8) -> &mut Slot {
        self.slots
            .iter_mut()
            .find(|s| s.id == id)
            .expect("unknown slot")
    }

    /// A control transfer on endpoint 0. Returns the bytes transferred.
    fn control(
        &mut self,
        slot: u8,
        setup: Setup,
        data: Option<&mut DmaBuffer>,
    ) -> Result<usize, UsbError> {
        let len = setup.length as u32;
        let input = setup.is_in();
        let trt = match (len, input) {
            (0, _) => 0,
            (_, false) => 2,
            (_, true) => 3,
        };
        let packet = setup.to_u64();
        let data_phys = data.as_ref().map(|d| d.phys()).unwrap_or(0);
        let ring = &mut self.slot_mut(slot).ep0;
        ring.push([
            packet as u32,
            (packet >> 32) as u32,
            8,
            TRB_SETUP << 10 | TRB_IDT | trt << 16,
        ]);
        if len > 0 {
            ring.push([
                data_phys as u32,
                (data_phys >> 32) as u32,
                len,
                TRB_DATA << 10 | if input { TRB_DIR_IN } else { 0 },
            ]);
        }
        let status_in = len == 0 || !input;
        let status = ring.push([
            0,
            0,
            0,
            TRB_STATUS << 10 | TRB_IOC | if status_in { TRB_DIR_IN } else { 0 },
        ]);
        self.ring_doorbell(slot, 1);
        let mut residue = 0;
        let e = self.wait_event(|e| {
            e.kind() == TRB_TRANSFER_EVENT && e.slot() == slot && e.endpoint() == 1 && {
                if e.code() == CC_SHORT_PACKET {
                    residue = e.residue();
                }
                e.pointer() == status || e.code() != CC_SUCCESS && e.code() != CC_SHORT_PACKET
            }
        })?;
        if e.code() != CC_SUCCESS && e.code() != CC_SHORT_PACKET {
            return Err(UsbError::Completion(e.code()));
        }
        Ok((len as usize).saturating_sub(residue))
    }

    /// A bulk transfer of `len` bytes at physical `phys` on endpoint
    /// context `dci`; waits for it. Returns the bytes transferred.
    fn bulk(&mut self, slot: u8, dci: u8, phys: u64, len: usize) -> Result<usize, UsbError> {
        let s = self.slot_mut(slot);
        let ring = &mut s
            .rings
            .iter_mut()
            .find(|(d, _)| *d == dci)
            .expect("endpoint")
            .1;
        // One TRB per page, so no TRB crosses a 64 KiB boundary.
        let mut off = 0usize;
        let last = loop {
            let a = phys + off as u64;
            let in_page = (4096 - (a & 4095) as usize).min(len - off);
            let end = off + in_page >= len;
            let ctl = TRB_NORMAL << 10 | if end { TRB_IOC } else { TRB_CHAIN } | 1 << 2;
            let at = ring.push([a as u32, (a >> 32) as u32, in_page as u32, ctl]);
            off += in_page;
            if end {
                break at;
            }
        };
        self.ring_doorbell(slot, dci);
        let mut residue = 0;
        let e = self.wait_event(|e| {
            e.kind() == TRB_TRANSFER_EVENT && e.slot() == slot && e.endpoint() == dci && {
                if e.code() == CC_SHORT_PACKET {
                    residue += e.residue();
                }
                e.pointer() == last || e.code() != CC_SUCCESS && e.code() != CC_SHORT_PACKET
            }
        })?;
        if e.code() != CC_SUCCESS && e.code() != CC_SHORT_PACKET {
            return Err(UsbError::Completion(e.code()));
        }
        Ok(len.saturating_sub(residue))
    }

    /// A keyboard or mouse report arrived (or another unexpected event).
    /// Its buffer is read and queued again.
    fn handle_async(&mut self, e: &Event) {
        if e.kind() != TRB_TRANSFER_EVENT {
            return;
        }
        let (slot, ep) = (e.slot(), e.endpoint());
        let Some(s) = self.slots.iter_mut().find(|s| s.id == slot) else {
            return;
        };
        let ok = e.code() == CC_SUCCESS || e.code() == CC_SHORT_PACKET;
        let (dci, ring, reports) = match &mut s.function {
            Some(
                Function::Keyboard {
                    dci, ring, reports, ..
                }
                | Function::Mouse { dci, ring, reports }
                | Function::Pointer {
                    dci, ring, reports, ..
                },
            ) if *dci == ep => (*dci, ring, reports),
            _ => return,
        };
        let Some(i) = reports.complete(e.pointer()) else {
            return;
        };
        let mut r = [0u8; 64];
        reports.read(i, &mut r[..reports.len as usize]);
        reports.queue(ring, i);
        if ok {
            match &mut s.function {
                Some(Function::Keyboard {
                    report, decoder, ..
                }) => {
                    let r: [u8; 8] = core::array::from_fn(|k| r[k]);
                    for byte in report.report(&r) {
                        let (ev, _) = decoder.feed(byte);
                        if let Some(ev) = ev {
                            input::push(ev);
                        }
                    }
                }
                Some(Function::Mouse { .. }) => {
                    let r: [u8; 4] = core::array::from_fn(|k| r[k]);
                    if let Some(m) = hid::parse_mouse(&r) {
                        input::push_mouse(input::MouseEvent {
                            buttons: m.buttons,
                            dx: m.dx,
                            dy: m.dy,
                            wheel: m.wheel,
                            abs: None,
                        });
                    }
                }
                Some(Function::Pointer { format, .. }) => {
                    let len = format.bytes.min(64);
                    if let Some(p) = format.parse(&r[..len]) {
                        let rel = |v: i32| v.clamp(-127, 127) as i8;
                        input::push_mouse(input::MouseEvent {
                            buttons: p.buttons,
                            dx: if p.absolute { 0 } else { rel(p.x) },
                            dy: if p.absolute { 0 } else { rel(p.y) },
                            wheel: p.wheel,
                            abs: p.absolute.then_some((p.x as u16, p.y as u16)),
                        });
                    }
                }
                _ => {}
            }
        }
        self.ring_doorbell(slot, dci);
    }

    // --- device set-up ----------------------------------------------------

    fn input_context(&self) -> Option<DmaBuffer> {
        DmaBuffer::new(33 * self.csz)
    }

    /// Offset of context `index` (0 = slot, 1.. = endpoints) in an input
    /// context (which starts with the input control context).
    fn ictx(&self, index: usize) -> usize {
        (index + 1) * self.csz
    }

    fn ep_context(
        &self,
        ctx: &mut DmaBuffer,
        dci: u8,
        ep_type: u32,
        max_packet: u16,
        ring: u64,
        interval: u8,
    ) {
        let o = self.ictx(dci as usize);
        ctx.write(o, (interval as u32) << 16);
        ctx.write(o + 4, 3 << 1 | ep_type << 3 | (max_packet as u32) << 16);
        ctx.write(o + 8, (ring as u32) | 1);
        ctx.write(o + 12, (ring >> 32) as u32);
        ctx.write(
            o + 16,
            if ep_type == EP_CONTROL {
                8u32
            } else {
                max_packet as u32
            },
        );
    }

    /// Resets a root port. Returns its speed (1 full, 2 low, 3 high, 4+
    /// super) once it is enabled.
    fn reset_port(&self, port: u8) -> Option<u32> {
        let reg = self.portsc(port);
        let sc = r32(reg);
        if sc & PORT_CONNECTED == 0 {
            return None;
        }
        if sc & PORT_ENABLED == 0 {
            // Keep power, clear the change bits, never write 1 to "enabled".
            w32(
                reg,
                (sc & !(PORT_ENABLED | PORT_CHANGE_BITS)) | PORT_POWER | PORT_RESET,
            );
            wait(500, || r32(reg) & PORT_RESET_CHANGE != 0).ok()?;
            let sc = r32(reg);
            w32(
                reg,
                (sc & !(PORT_ENABLED | PORT_CHANGE_BITS)) | PORT_RESET_CHANGE,
            );
            time::delay_us(20_000);
        }
        let sc = r32(reg);
        (sc & PORT_ENABLED != 0).then_some((sc >> 10) & 0xf)
    }

    /// Enables a slot and gives the device on `port` an address.
    fn address(&mut self, port: u8, speed: u32) -> Result<u8, UsbError> {
        let e = self.command([0, 0, 0, TRB_ENABLE_SLOT << 10])?;
        let id = e.slot();
        let out = DmaBuffer::new(32 * self.csz).ok_or(UsbError::NoMemory)?;
        self.dcbaa.write(id as usize * 8, out.phys());
        self.contexts.push(out);
        let ep0 = Ring::new().ok_or(UsbError::NoMemory)?;
        let mps: u16 = match speed {
            2 | 1 => 8,
            3 => 64,
            _ => 512,
        };
        let mut ctx = self.input_context().ok_or(UsbError::NoMemory)?;
        ctx.write(4, 0b11u32); // add slot and endpoint 0
        let s = self.ictx(0);
        ctx.write(s, speed << 20 | 1 << 27);
        ctx.write(s + 4, (port as u32) << 16);
        self.ep_context(&mut ctx, 1, EP_CONTROL, mps, ep0.phys(), 0);
        self.slots.push(Slot {
            id,
            ep0,
            rings: Vec::new(),
            function: None,
        });
        let p = ctx.phys();
        self.command([
            p as u32,
            (p >> 32) as u32,
            0,
            TRB_ADDRESS_DEVICE << 10 | (id as u32) << 24,
        ])?;
        // Full-speed devices may have a larger endpoint 0 than 8 bytes.
        let mut buf = DmaBuffer::new(4096).ok_or(UsbError::NoMemory)?;
        self.control(
            id,
            Setup::get_descriptor(desc::DESC_DEVICE, 0, 8),
            Some(&mut buf),
        )?;
        let real = buf.read::<u8>(7) as u16;
        if real != 0 && real != mps && speed < 3 {
            let mut ctx = self.input_context().ok_or(UsbError::NoMemory)?;
            ctx.write(4, 0b10u32);
            let ring = self.slot_mut(id).ep0.phys();
            self.ep_context(&mut ctx, 1, EP_CONTROL, real, ring, 0);
            let p = ctx.phys();
            self.command([
                p as u32,
                (p >> 32) as u32,
                0,
                TRB_EVALUATE_CTX << 10 | (id as u32) << 24,
            ])?;
        }
        Ok(id)
    }

    /// Configures endpoints `eps` (with their xHCI types) of a slot.
    fn configure(
        &mut self,
        slot: u8,
        port: u8,
        speed: u32,
        eps: &[(Endpoint, u32)],
    ) -> Result<(), UsbError> {
        let mut ctx = self.input_context().ok_or(UsbError::NoMemory)?;
        let mut add = 1u32;
        let mut max_dci = 1;
        let mut rings = Vec::new();
        for (ep, ty) in eps {
            let dci = ep.number() * 2 + ep.is_in() as u8;
            let ring = Ring::new().ok_or(UsbError::NoMemory)?;
            let interval = if ep.is_interrupt() {
                if speed >= 3 {
                    ep.interval.clamp(1, 16) - 1
                } else {
                    // Frames of 1 ms, as a power of two of 125 us units.
                    let us125 = (ep.interval.max(1) as u32) * 8;
                    (31 - us125.leading_zeros()).clamp(3, 10) as u8
                }
            } else {
                0
            };
            self.ep_context(&mut ctx, dci, *ty, ep.max_packet, ring.phys(), interval);
            add |= 1 << dci;
            max_dci = max_dci.max(dci);
            rings.push((dci, ring));
        }
        ctx.write(4, add);
        let s = self.ictx(0);
        ctx.write(s, speed << 20 | (max_dci as u32) << 27);
        ctx.write(s + 4, (port as u32) << 16);
        let p = ctx.phys();
        self.command([
            p as u32,
            (p >> 32) as u32,
            0,
            TRB_CONFIGURE_EP << 10 | (slot as u32) << 24,
        ])?;
        self.slot_mut(slot).rings.extend(rings);
        Ok(())
    }
}

/// The controllers (shared by the `usb` thread and the storage devices).
static CONTROLLERS: SpinLock<Vec<Arc<SpinLock<Controller>>>> = SpinLock::new(Vec::new());
static WAKE: WaitQueue = WaitQueue::new();

/// Controllers that can have an interrupt vector of their own.
const MAX_IRQ_CONTROLLERS: usize = 4;
/// Operational and interrupter register addresses of each controller
/// with an interrupt, by vector (0 when unused): the interrupt handler
/// must not take the controller lock.
static IRQ_OP: [AtomicU64; MAX_IRQ_CONTROLLERS] =
    [const { AtomicU64::new(0) }; MAX_IRQ_CONTROLLERS];
static IRQ_IR: [AtomicU64; MAX_IRQ_CONTROLLERS] =
    [const { AtomicU64::new(0) }; MAX_IRQ_CONTROLLERS];
/// An interrupt came since the `usb` thread last looked at the rings.
static PENDING: AtomicBool = AtomicBool::new(false);
static INTERRUPTS: AtomicU64 = AtomicU64::new(0);

fn on_interrupt(frame: &mut InterruptFrame) {
    let i = (frame.vector as u8).wrapping_sub(irq::VECTOR_XHCI) as usize;
    if i < MAX_IRQ_CONTROLLERS {
        let op = IRQ_OP[i].load(Ordering::Acquire);
        let ir = IRQ_IR[i].load(Ordering::Acquire);
        if op != 0 {
            // Both bits are cleared by writing 1.
            w32(op + OP_USBSTS, STS_EVENT_INTERRUPT);
            w32(ir + IR_MANAGEMENT, IMAN_ENABLE | IMAN_PENDING);
        }
    }
    INTERRUPTS.fetch_add(1, Ordering::Relaxed);
    // The reports are handled here, on the interrupted thread: waking the
    // `usb` thread would switch threads (and address spaces, flushing the
    // TLB of a program that draws) for every report. The controller lock
    // turns interrupts off while held, so on this CPU it is free; another
    // CPU may hold it for storage I/O, which handles reports itself.
    let ctl = CONTROLLERS.lock().get(i).cloned();
    match ctl.as_ref().map(|c| c.try_lock()) {
        Some(Some(mut c)) => {
            while let Some(e) = c.next_event() {
                c.handle_async(&e);
            }
        }
        // Still being set up: its waits read the event ring.
        None => {}
        // Busy: the `usb` thread looks again shortly.
        Some(None) => {
            PENDING.store(true, Ordering::Release);
            if sched::is_running() {
                WAKE.wake_all();
            }
        }
    }
}

/// Interrupts taken from xHCI controllers since boot.
pub fn interrupt_count() -> u64 {
    INTERRUPTS.load(Ordering::Relaxed)
}

/// How each controller signals events: "MSI-X", "MSI" or "polled".
pub fn irq_modes() -> Vec<&'static str> {
    CONTROLLERS
        .lock()
        .iter()
        .map(|c| c.lock().irq.name())
        .collect()
}

/// Gives controller `index` an interrupt: MSI-X, else MSI, else none.
fn setup_irq(d: &velora_pci::Device, index: usize, op: u64, ir: u64) -> Irq {
    if index >= MAX_IRQ_CONTROLLERS || crate::boot::cmdline_has("velora.xhci_poll") {
        return Irq::Polled;
    }
    let vector = irq::VECTOR_XHCI + index as u8;
    IRQ_OP[index].store(op, Ordering::Release);
    IRQ_IR[index].store(ir, Ordering::Release);
    irq::set_handler(vector, on_interrupt);
    if let Ok(m) = MsiX::enable(d)
        && m.route(0, vector).is_ok()
    {
        return Irq::MsiX(m);
    }
    if msix::enable_msi(d, vector) {
        return Irq::Msi;
    }
    irq::clear_handler(vector);
    IRQ_OP[index].store(0, Ordering::Release);
    IRQ_IR[index].store(0, Ordering::Release);
    Irq::Polled
}

fn start(d: &velora_pci::Device, index: usize) -> Result<Controller, &'static str> {
    let cfg = pci::config();
    velora_pci::enable_device(cfg, d.addr);
    let (bar, size) = d.bars[0].memory().ok_or("BAR0 missing")?;
    let cap = mmio::map(bar, size.clamp(0x1000, 0x10000)).ok_or("cannot map registers")?;
    let caplen = r32(cap) & 0xff;
    let op = cap + caplen as u64;
    let hcs1 = r32(cap + CAP_HCSPARAMS1);
    let hcs2 = r32(cap + CAP_HCSPARAMS2);
    let hcc1 = r32(cap + CAP_HCCPARAMS1);
    let rt = cap + (r32(cap + CAP_RTSOFF) & !0x1f) as u64;
    let db = cap + (r32(cap + CAP_DBOFF) & !0x3) as u64;
    let max_slots = (hcs1 & 0xff) as usize;
    let ports = (hcs1 >> 24) as u8;
    let csz = if hcc1 & (1 << 2) != 0 { 64 } else { 32 };

    // Take the controller from the firmware (USB legacy support).
    let mut xecp = ((hcc1 >> 16) & 0xffff) as u64 * 4;
    while xecp != 0 {
        let at = cap + xecp;
        let v = r32(at);
        if v & 0xff == 1 {
            w32(at, v | 1 << 24);
            if wait(1000, || r32(at) & (1 << 16) == 0).is_err() {
                kwarn!("xhci: the firmware did not release the controller");
            }
            // No more SMIs for USB events.
            w32(at + 4, r32(at + 4) & 0xffff_0000 | 0xe000_0000);
        }
        let next = (v >> 8) & 0xff;
        xecp = if next == 0 { 0 } else { xecp + next as u64 * 4 };
    }

    // Stop and reset.
    w32(op + OP_USBCMD, r32(op + OP_USBCMD) & !CMD_RUN);
    wait(100, || r32(op + OP_USBSTS) & STS_HALTED != 0).map_err(|_| "controller did not stop")?;
    w32(op + OP_USBCMD, CMD_RESET);
    wait(1000, || {
        r32(op + OP_USBCMD) & CMD_RESET == 0 && r32(op + OP_USBSTS) & STS_NOT_READY == 0
    })
    .map_err(|_| "controller did not reset")?;

    let mut dcbaa = DmaBuffer::new((max_slots + 1) * 8).ok_or("out of memory")?;
    // Scratchpad buffers the controller asks for.
    let scratch_n = (((hcs2 >> 21) & 0x1f) << 5 | (hcs2 >> 27)) as usize;
    let mut scratch = Vec::new();
    if scratch_n > 0 {
        let mut table = DmaBuffer::new(scratch_n * 8).ok_or("out of memory")?;
        for i in 0..scratch_n {
            let page = DmaBuffer::new(4096).ok_or("out of memory")?;
            table.write(i * 8, page.phys());
            scratch.push(page);
        }
        dcbaa.write(0, table.phys());
        scratch.push(table);
    }
    let cmd = Ring::new().ok_or("out of memory")?;
    let events = DmaBuffer::new(RING_TRBS * 16).ok_or("out of memory")?;
    let mut erst = DmaBuffer::new(64).ok_or("out of memory")?;
    erst.write(0, events.phys());
    erst.write(8, RING_TRBS as u32);

    w32(op + OP_CONFIG, max_slots as u32);
    w64(op + OP_DCBAAP, dcbaa.phys());
    w64(op + OP_CRCR, cmd.phys() | 1);
    let ir = rt + 0x20;
    w32(ir + 0x08, 1); // ERSTSZ
    w64(ir + 0x18, events.phys());
    w64(ir + 0x10, erst.phys());
    let irq = setup_irq(d, index, op, ir);
    if matches!(irq, Irq::Polled) {
        w32(ir + IR_MANAGEMENT, 0);
        w32(op + OP_USBCMD, CMD_RUN);
    } else {
        w32(ir + IR_MODERATION, MODERATION_250NS);
        w32(ir + IR_MANAGEMENT, IMAN_ENABLE | IMAN_PENDING);
        w32(op + OP_USBCMD, CMD_RUN | CMD_INTERRUPTS);
    }
    wait(100, || r32(op + OP_USBSTS) & STS_HALTED == 0).map_err(|_| "controller did not start")?;

    Ok(Controller {
        cap,
        op,
        rt,
        db,
        ports,
        csz,
        cmd,
        events,
        ev_deq: 0,
        ev_cycle: true,
        _erst: erst,
        dcbaa,
        contexts: Vec::new(),
        _scratch: scratch,
        slots: Vec::new(),
        irq,
    })
}

/// A USB flash drive as a block device.
struct UsbDisk {
    name: String,
    model: String,
    ctl: Arc<SpinLock<Controller>>,
    slot: u8,
    out_dci: u8,
    in_dci: u8,
    sectors: u64,
    sector_size: usize,
    io: SpinLock<DiskIo>,
}

struct DiskIo {
    tag: u32,
    cmd: DmaBuffer,
    data: DmaBuffer,
}

const BOUNCE: usize = 64 * 1024;

impl UsbDisk {
    /// One SCSI command through the bulk-only transport.
    fn scsi(
        &self,
        io: &mut DiskIo,
        cdb: &[u8],
        len: usize,
        input: bool,
    ) -> Result<usize, BlockError> {
        io.tag = io.tag.wrapping_add(1);
        let tag = io.tag;
        let mut c = self.ctl.lock();
        let w = msc::cbw(tag, len as u32, input, 0, cdb);
        io.cmd.as_mut_slice()[..msc::CBW_LEN].copy_from_slice(&w);
        let fail = |_| BlockError::Device;
        c.bulk(self.slot, self.out_dci, io.cmd.phys(), msc::CBW_LEN)
            .map_err(fail)?;
        let mut done = 0;
        if len > 0 {
            let dci = if input { self.in_dci } else { self.out_dci };
            done = c.bulk(self.slot, dci, io.data.phys(), len).map_err(fail)?;
        }
        let status = io.cmd.phys() + 512;
        c.bulk(self.slot, self.in_dci, status, msc::CSW_LEN)
            .map_err(fail)?;
        let csw: [u8; msc::CSW_LEN] = core::array::from_fn(|i| io.cmd.read::<u8>(512 + i));
        msc::parse_csw(&csw, tag).map_err(|_| BlockError::Device)?;
        Ok(done)
    }
}

impl BlockDevice for UsbDisk {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> String {
        format!("USB mass storage, {}", self.model)
    }
    fn model(&self) -> String {
        self.model.clone()
    }
    fn kind(&self) -> u32 {
        velora_abi::disk_kind::USB
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
        let per = BOUNCE / self.sector_size;
        for (i, chunk) in buf.chunks_mut(per * self.sector_size).enumerate() {
            let n = chunk.len() / self.sector_size;
            let at = lba + (i * per) as u64;
            let cdb = msc::read10(at as u32, n as u16);
            self.scsi(&mut io, &cdb, chunk.len(), true)?;
            chunk.copy_from_slice(&io.data.as_slice()[..chunk.len()]);
        }
        Ok(())
    }
    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        block::check_request(self, lba, buf.len())?;
        let mut io = self.io.lock();
        let per = BOUNCE / self.sector_size;
        for (i, chunk) in buf.chunks(per * self.sector_size).enumerate() {
            let n = chunk.len() / self.sector_size;
            let at = lba + (i * per) as u64;
            io.data.as_mut_slice()[..chunk.len()].copy_from_slice(chunk);
            let cdb = msc::write10(at as u32, n as u16);
            self.scsi(&mut io, &cdb, chunk.len(), false)?;
        }
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> {
        let mut io = self.io.lock();
        // Many flash drives do not implement it; a failure is not fatal.
        let _ = self.scsi(&mut io, &msc::sync_cache(), 0, false);
        Ok(())
    }
    unsafe fn force_unlock(&self) {
        // SAFETY: the caller is the panic path.
        unsafe {
            self.io.force_unlock();
            self.ctl.force_unlock();
        }
    }
}

/// Sets up the device on `port`. Returns a description for the log.
fn setup_port(
    ctl: &Arc<SpinLock<Controller>>,
    port: u8,
    disks: &mut usize,
) -> Result<String, UsbError> {
    let mut c = ctl.lock();
    let Some(speed) = c.reset_port(port) else {
        return Err(UsbError::Timeout);
    };
    let slot = c.address(port, speed)?;
    let mut buf = DmaBuffer::new(4096).ok_or(UsbError::NoMemory)?;
    c.control(
        slot,
        Setup::get_descriptor(desc::DESC_DEVICE, 0, 18),
        Some(&mut buf),
    )?;
    let dev = desc::parse_device(&buf.as_slice()[..18]).map_err(|_| UsbError::Descriptor)?;
    c.control(
        slot,
        Setup::get_descriptor(desc::DESC_CONFIG, 0, 9),
        Some(&mut buf),
    )?;
    let total = (buf.read::<u16>(2) as usize).clamp(9, 4096);
    let got = c.control(
        slot,
        Setup::get_descriptor(desc::DESC_CONFIG, 0, total as u16),
        Some(&mut buf),
    )?;
    let config =
        desc::parse_configuration(&buf.as_slice()[..got]).map_err(|_| UsbError::Descriptor)?;
    let speed_name = match speed {
        1 => "full speed",
        2 => "low speed",
        3 => "high speed",
        _ => "SuperSpeed",
    };
    let id = format!("{:04x}:{:04x}, {}", dev.vendor, dev.product, speed_name);

    for iface in &config.interfaces {
        let int_in = iface
            .endpoints
            .iter()
            .find(|e| e.is_interrupt() && e.is_in());
        if iface.class == desc::CLASS_HID {
            let Some(&ep) = int_in else { continue };
            let boot = iface.subclass == hid::SUBCLASS_BOOT;
            let keyboard = boot && iface.protocol == hid::PROTOCOL_KEYBOARD;
            let boot_mouse = boot && iface.protocol == hid::PROTOCOL_MOUSE;
            c.control(slot, Setup::set_configuration(config.value), None)?;
            // Other pointers (tablets above all) describe their reports;
            // a boot mouse keeps the boot protocol unless it is absolute.
            let mut format = None;
            if !keyboard {
                let mut d = DmaBuffer::new(512).ok_or(UsbError::NoMemory)?;
                let req = Setup::get_interface_descriptor(0x22, iface.number, 512);
                if let Ok(n) = c.control(slot, req, Some(&mut d)) {
                    let bytes: Vec<u8> = (0..n.min(512)).map(|i| d.read::<u8>(i)).collect();
                    format = report::parse_pointer(&bytes)
                        .filter(|p| (p.absolute() || !boot_mouse) && p.bytes <= 64);
                }
            }
            if !keyboard && !boot_mouse && format.is_none() {
                continue;
            }
            // Boot protocol for keyboards and boot mice, report protocol
            // for the others; reports only on change.
            let _ = c.control(
                slot,
                Setup::class_interface(
                    hid::REQ_SET_PROTOCOL,
                    format.is_some() as u16,
                    iface.number,
                    0,
                    false,
                ),
                None,
            );
            let _ = c.control(
                slot,
                Setup::class_interface(hid::REQ_SET_IDLE, 0, iface.number, 0, false),
                None,
            );
            c.configure(slot, port, speed, &[(ep, EP_INTERRUPT_IN)])?;
            let dci = ep.number() * 2 + 1;
            let s = c.slot_mut(slot);
            let i = s.rings.iter().position(|(d, _)| *d == dci).expect("ring");
            let (_, mut ring) = s.rings.remove(i);
            let len = match &format {
                Some(f) => f.bytes as u32,
                None if keyboard => 8,
                None => 4,
            };
            let reports = Reports::new(&mut ring, len).ok_or(UsbError::NoMemory)?;
            let what = match &format {
                Some(f) if f.absolute() => "tablet",
                _ if keyboard => "keyboard",
                _ => "mouse",
            };
            s.function = Some(if keyboard {
                Function::Keyboard {
                    dci,
                    ring,
                    reports,
                    report: hid::Keyboard::new(),
                    decoder: Decoder::new(Layout::Us, Layout::Russian),
                }
            } else if let Some(format) = format {
                Function::Pointer {
                    dci,
                    ring,
                    reports,
                    format,
                }
            } else {
                Function::Mouse { dci, ring, reports }
            });
            c.ring_doorbell(slot, dci);
            return Ok(format!("{} ({}) on port {}", what, id, port));
        }
        if iface.class == desc::CLASS_MASS_STORAGE
            && iface.subclass == msc::SUBCLASS_SCSI
            && iface.protocol == msc::PROTOCOL_BOT
        {
            let bin = iface.endpoints.iter().find(|e| e.is_bulk() && e.is_in());
            let bout = iface.endpoints.iter().find(|e| e.is_bulk() && !e.is_in());
            let (Some(&bin), Some(&bout)) = (bin, bout) else {
                continue;
            };
            c.control(slot, Setup::set_configuration(config.value), None)?;
            c.configure(slot, port, speed, &[(bin, EP_BULK_IN), (bout, EP_BULK_OUT)])?;
            c.slot_mut(slot).function = Some(Function::Storage);
            drop(c);
            let mut disk = UsbDisk {
                name: format!("usb{}", *disks),
                model: String::new(),
                ctl: ctl.clone(),
                slot,
                out_dci: bout.number() * 2,
                in_dci: bin.number() * 2 + 1,
                sectors: 0,
                sector_size: 512,
                io: SpinLock::new(DiskIo {
                    tag: 0,
                    cmd: DmaBuffer::new(4096).ok_or(UsbError::NoMemory)?,
                    data: DmaBuffer::new(BOUNCE).ok_or(UsbError::NoMemory)?,
                }),
            };
            let mut io = disk.io.lock();
            if disk.scsi(&mut io, &msc::inquiry(), 36, true).is_ok()
                && let Some((v, p)) = msc::parse_inquiry(&io.data.as_slice()[..36])
            {
                disk.model = format!("{} {}", v, p);
            }
            // A drive may need a moment after power-up.
            let mut ready = false;
            for _ in 0..20 {
                if disk
                    .scsi(&mut io, &msc::test_unit_ready(), 0, false)
                    .is_ok()
                {
                    ready = true;
                    break;
                }
                let _ = disk.scsi(&mut io, &msc::request_sense(), 18, true);
                time::delay_us(50_000);
            }
            if !ready {
                return Err(UsbError::Timeout);
            }
            disk.scsi(&mut io, &msc::read_capacity(), 8, true)
                .map_err(|_| UsbError::Descriptor)?;
            let (sectors, size) =
                msc::parse_capacity(&io.data.as_slice()[..8]).ok_or(UsbError::Descriptor)?;
            if !matches!(size, 512 | 1024 | 2048 | 4096) {
                return Err(UsbError::Descriptor);
            }
            drop(io);
            disk.sectors = sectors;
            disk.sector_size = size as usize;
            let what = format!("storage {} ({}) on port {}", disk.name, id, port);
            block::register(Arc::new(disk));
            *disks += 1;
            return Ok(what);
        }
    }
    Err(UsbError::Descriptor)
}

/// Reads the event rings of polled controllers every tick. With
/// interrupts the handler reads them, and the thread only takes over when
/// the handler found a controller busy: it runs on CPU 0, which takes the
/// interrupts, and checks `PENDING` with interrupts off before it blocks,
/// so no wake-up is missed. A controller that was busy is looked at again
/// on the next tick.
extern "C" fn usb_thread(_: u64) -> u64 {
    let ctls = CONTROLLERS.lock().clone();
    let polled = ctls.iter().any(|c| matches!(c.lock().irq, Irq::Polled));
    loop {
        PENDING.store(false, Ordering::Release);
        let mut busy = false;
        for ctl in &ctls {
            // Storage I/O may hold the controller (it handles reports
            // that arrive meanwhile); try again next tick.
            if let Some(mut c) = ctl.try_lock() {
                while let Some(e) = c.next_event() {
                    c.handle_async(&e);
                }
            } else {
                busy = true;
            }
        }
        crate::arch::cpu::disable_interrupts();
        if !PENDING.load(Ordering::Acquire) {
            let ticks = if polled || busy { 1 } else { time::TICK_HZ };
            WAKE.wait(Some(time::ticks() + ticks));
        }
        crate::arch::cpu::enable_interrupts();
    }
}

/// Finds the xHCI controllers and sets up the devices on their ports.
/// Storage devices are registered as block devices. Returns the number
/// of USB devices in use.
pub fn probe_all() -> usize {
    if crate::boot::cmdline_has("velora.nousb") {
        return 0;
    }
    let mut devices = 0;
    let mut disks = 0;
    for d in pci::devices().iter().filter(|d| d.is(0x0c, 0x03, 0x30)) {
        let index = CONTROLLERS.lock().len();
        let ctl = match start(d, index) {
            Ok(c) => Arc::new(SpinLock::new(c)),
            Err(e) => {
                kwarn!("xhci at {}: {}", d.addr, e);
                continue;
            }
        };
        let (ports, mode) = {
            let c = ctl.lock();
            (c.ports, c.irq.name())
        };
        kok!("xhci at {}: {} ports, {}", d.addr, ports, mode);
        // Give connected devices time to report themselves.
        time::delay_us(50_000);
        for port in 1..=ports {
            let connected = {
                let c = ctl.lock();
                r32(c.portsc(port)) & PORT_CONNECTED != 0
            };
            if !connected {
                continue;
            }
            match setup_port(&ctl, port, &mut disks) {
                Ok(what) => {
                    kok!("usb: {}", what);
                    devices += 1;
                }
                Err(e) => kwarn!("usb: device on port {} not used: {:?}", port, e),
            }
        }
        CONTROLLERS.lock().push(ctl);
    }
    devices
}

/// Keyboards and mice in use.
pub fn hid_devices() -> (usize, usize) {
    let (mut k, mut m) = (0, 0);
    for ctl in CONTROLLERS.lock().iter() {
        for s in &ctl.lock().slots {
            match s.function {
                Some(Function::Keyboard { .. }) => k += 1,
                Some(Function::Mouse { .. } | Function::Pointer { .. }) => m += 1,
                _ => {}
            }
        }
    }
    (k, m)
}

/// Starts the thread that reads keyboard and mouse reports.
pub fn start_thread() {
    let ctls = CONTROLLERS.lock().clone();
    if ctls.is_empty() {
        return;
    }
    // Interrupts go to the CPU that set them up (CPU 0, see `usb_thread`).
    let irqs = ctls.iter().all(|c| !matches!(c.lock().irq, Irq::Polled));
    let _ = irqs;
    let tid = sched::spawn_kernel("usb", usb_thread, 0, Priority::High);
    tid.expect("cannot start the usb thread");
}

//! xHCI USB host controller, with the devices Velora uses: boot protocol
//! keyboards and mice (HID) and flash drives (mass storage, bulk-only
//! transport, SCSI).
//!
//! - Polling, like the disk drivers (D21): commands and control/bulk
//!   transfers wait for their event; the `usb` kernel thread looks at the
//!   event ring every tick for keyboard and mouse reports.
//! - Devices on root hub ports that are connected at boot are set up.
//! - Descriptor parsing, HID reports and SCSI commands are in
//!   `velora-usb` (tested on the host).
//!
//! NOT IMPLEMENTED: USB hubs (devices behind a hub are not seen), hot
//! plugging (devices connected after boot), keyboard LEDs, isochronous
//! transfers, interrupts (MSI) instead of polling.

use super::block::{self, BlockDevice, BlockError};
use super::dma::DmaBuffer;
use super::pci;
use crate::mm::mmio;
use crate::sched::{self, Priority, WaitQueue};
use crate::sync::SpinLock;
use crate::{input, kok, kwarn, time};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use velora_input::{Keyboard as Decoder, Layout};
use velora_usb::desc::{self, Endpoint, Setup};
use velora_usb::{hid, msc};

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
const STS_HALTED: u32 = 1;
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
        buf: DmaBuffer,
        report: hid::Keyboard,
        decoder: Decoder,
    },
    Mouse {
        dci: u8,
        ring: Ring,
        buf: DmaBuffer,
    },
    Storage,
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

    /// Queues a read of one report on an interrupt IN endpoint.
    fn queue_report(ring: &mut Ring, buf: &DmaBuffer, len: u32) {
        let p = buf.phys();
        ring.push([
            p as u32,
            (p >> 32) as u32,
            len,
            TRB_NORMAL << 10 | TRB_IOC | 1 << 2,
        ]);
    }

    /// A keyboard or mouse report arrived (or another unexpected event).
    fn handle_async(&mut self, e: &Event) {
        if e.kind() != TRB_TRANSFER_EVENT {
            return;
        }
        let (slot, ep) = (e.slot(), e.endpoint());
        let Some(s) = self.slots.iter_mut().find(|s| s.id == slot) else {
            return;
        };
        let ok = e.code() == CC_SUCCESS || e.code() == CC_SHORT_PACKET;
        let mut ring_bell = None;
        match &mut s.function {
            Some(Function::Keyboard {
                dci,
                ring,
                buf,
                report,
                decoder,
            }) if *dci == ep => {
                if ok {
                    let r: [u8; 8] = core::array::from_fn(|i| buf.read::<u8>(i));
                    for byte in report.report(&r) {
                        let (ev, _) = decoder.feed(byte);
                        if let Some(ev) = ev {
                            input::push(ev);
                        }
                    }
                }
                Self::queue_report(ring, buf, 8);
                ring_bell = Some(*dci);
            }
            Some(Function::Mouse { dci, ring, buf }) if *dci == ep => {
                if ok {
                    let r: [u8; 4] = core::array::from_fn(|i| buf.read::<u8>(i));
                    if let Some(m) = hid::parse_mouse(&r) {
                        input::push_mouse(input::MouseEvent {
                            buttons: m.buttons,
                            dx: m.dx,
                            dy: m.dy,
                            wheel: m.wheel,
                        });
                    }
                }
                Self::queue_report(ring, buf, 4);
                ring_bell = Some(*dci);
            }
            _ => {}
        }
        if let Some(dci) = ring_bell {
            self.ring_doorbell(slot, dci);
        }
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

fn start(d: &velora_pci::Device) -> Result<Controller, &'static str> {
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
    w32(ir, 0); // no interrupts: polled
    w32(op + OP_USBCMD, CMD_RUN);
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
        if iface.class == desc::CLASS_HID && iface.subclass == hid::SUBCLASS_BOOT {
            let Some(&ep) = int_in else { continue };
            let keyboard = iface.protocol == hid::PROTOCOL_KEYBOARD;
            if !keyboard && iface.protocol != hid::PROTOCOL_MOUSE {
                continue;
            }
            c.control(slot, Setup::set_configuration(config.value), None)?;
            // Boot protocol, reports only on change.
            let _ = c.control(
                slot,
                Setup::class_interface(hid::REQ_SET_PROTOCOL, 0, iface.number, 0, false),
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
            let buf = DmaBuffer::new(64).ok_or(UsbError::NoMemory)?;
            Controller::queue_report(&mut ring, &buf, if keyboard { 8 } else { 4 });
            s.function = Some(if keyboard {
                Function::Keyboard {
                    dci,
                    ring,
                    buf,
                    report: hid::Keyboard::new(),
                    decoder: Decoder::new(Layout::Us, Layout::Russian),
                }
            } else {
                Function::Mouse { dci, ring, buf }
            });
            c.ring_doorbell(slot, dci);
            return Ok(format!(
                "{} ({}) on port {}",
                if keyboard { "keyboard" } else { "mouse" },
                id,
                port
            ));
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

extern "C" fn usb_thread(_: u64) -> u64 {
    loop {
        let ctls = CONTROLLERS.lock().clone();
        for ctl in ctls {
            // Storage I/O may hold the controller; try again next tick.
            if let Some(mut c) = ctl.try_lock() {
                while let Some(e) = c.next_event() {
                    c.handle_async(&e);
                }
            }
        }
        crate::arch::cpu::disable_interrupts();
        WAKE.wait(Some(time::ticks() + 1));
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
        let ctl = match start(d) {
            Ok(c) => Arc::new(SpinLock::new(c)),
            Err(e) => {
                kwarn!("xhci at {}: {}", d.addr, e);
                continue;
            }
        };
        let ports = ctl.lock().ports;
        kok!("xhci at {}: {} ports", d.addr, ports);
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
                Some(Function::Mouse { .. }) => m += 1,
                _ => {}
            }
        }
    }
    (k, m)
}

/// Starts the thread that reads keyboard and mouse reports.
pub fn start_thread() {
    if !CONTROLLERS.lock().is_empty() {
        sched::spawn_kernel("usb", usb_thread, 0, Priority::High)
            .expect("cannot start the usb thread");
    }
}

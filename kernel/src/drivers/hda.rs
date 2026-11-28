//! Intel High Definition Audio controllers (PCI class 04:03): playback
//! of 48 kHz 16-bit stereo through the first codec that has an output.
//!
//! The codec is driven through the CORB/RIRB rings (polled). The first
//! output stream reads a 256 KiB cyclic buffer (4 BDL entries, 1.36 s);
//! the `audio` thread follows the link position every tick while sound
//! plays, clears what was played to silence and stops the stream after
//! 2 s of silence. Programs write samples to `/dev/audio` (as much as
//! fits; the rest of the write is refused with a short count) and control
//! the volume, mute and output through `/dev/mixer`
//! ([`velora_hda::mixer`]). No interrupts are used.
//!
//! Tested in QEMU with `-device intel-hda -device hda-duplex`. Codecs of
//! real machines (Realtek, Conexant, IDT) are found through the same
//! widget graph, but were not tested on hardware: jack detection and
//! switching to headphones when they are plugged in are NOT IMPLEMENTED
//! (the output is chosen in `/dev/mixer`).

use super::dma::DmaBuffer;
use super::pci;
use crate::arch::x86_64::cpu;
use crate::mm::mmio;
use crate::sched::{self, Priority, WaitQueue};
use crate::sync::SpinLock;
use crate::time;
use alloc::string::String;
use alloc::vec::Vec;
use velora_hda::codec::{self, Codec, Graph, Path};
use velora_hda::mixer::{self, Command, State};
use velora_hda::ring::Ring;

const GCAP: u64 = 0x00;
const GCTL: u64 = 0x08;
const STATESTS: u64 = 0x0e;
const INTCTL: u64 = 0x20;
const CORBLBASE: u64 = 0x40;
const CORBUBASE: u64 = 0x44;
const CORBWP: u64 = 0x48;
const CORBRP: u64 = 0x4a;
const CORBCTL: u64 = 0x4c;
const CORBSIZE: u64 = 0x4e;
const RIRBLBASE: u64 = 0x50;
const RIRBUBASE: u64 = 0x54;
const RIRBWP: u64 = 0x58;
const RINTCNT: u64 = 0x5a;
const RIRBCTL: u64 = 0x5c;
const RIRBSTS: u64 = 0x5d;
const RIRBSIZE: u64 = 0x5e;
// Stream descriptor registers (from its base).
const SD_CTL: u64 = 0x00;
const SD_STS: u64 = 0x03;
const SD_LPIB: u64 = 0x04;
const SD_CBL: u64 = 0x08;
const SD_LVI: u64 = 0x0c;
const SD_FMT: u64 = 0x12;
const SD_BDPL: u64 = 0x18;
const SD_BDPU: u64 = 0x1c;

const CTL_SRST: u8 = 1 << 0;
const CTL_RUN: u8 = 1 << 1;

const BUFFER: usize = 256 * 1024;
const BDL_ENTRIES: usize = 4;
/// Stream tag of the output stream (1..=15).
const TAG: u8 = 1;
/// Silence after which the stream stops (ms).
const IDLE_STOP_MS: u64 = 2000;

struct Hda {
    regs: u64,
    corb: DmaBuffer,
    rirb: DmaBuffer,
    corb_entries: u16,
    rirb_entries: u16,
    rirb_read: u16,
    /// Base of the output stream descriptor.
    sd: u64,
    buf: DmaBuffer,
    _bdl: DmaBuffer,
    ring: Ring,
    running: bool,
    last_sound: u64,
    graph: Graph,
    paths: Vec<Path>,
    state: State,
    /// The path has no amplifier: samples are scaled ([`codec::soft_gain`]).
    soft: bool,
    name: String,
}

// SAFETY: the registers and buffers belong to this driver only.
unsafe impl Send for Hda {}

static HDA: SpinLock<Option<Hda>> = SpinLock::new(None);
/// Wakes the audio thread (sound was written).
static WAKE: WaitQueue = WaitQueue::new();

impl Hda {
    fn r8(&self, r: u64) -> u8 {
        // SAFETY: a register of the mapped BAR0.
        unsafe { mmio::read8(self.regs + r) }
    }
    fn r16(&self, r: u64) -> u16 {
        // SAFETY: as above.
        unsafe { mmio::read16(self.regs + r) }
    }
    fn r32(&self, r: u64) -> u32 {
        // SAFETY: as above.
        unsafe { mmio::read32(self.regs + r) }
    }
    fn w8(&self, r: u64, v: u8) {
        // SAFETY: as above.
        unsafe { mmio::write8(self.regs + r, v) }
    }
    fn w16(&self, r: u64, v: u16) {
        // SAFETY: as above.
        unsafe { mmio::write16(self.regs + r, v) }
    }
    fn w32(&self, r: u64, v: u32) {
        // SAFETY: as above.
        unsafe { mmio::write32(self.regs + r, v) }
    }

    /// Waits up to `ms` for `f` to hold.
    fn wait(&self, ms: u64, f: impl Fn(&Self) -> bool) -> bool {
        for _ in 0..ms * 10 {
            if f(self) {
                return true;
            }
            time::delay_us(100);
        }
        f(self)
    }

    /// Resets the controller and starts the command rings.
    fn reset(&mut self) -> Result<(), &'static str> {
        // Stop the rings, then enter and leave reset.
        self.w8(CORBCTL, 0);
        self.w8(RIRBCTL, 0);
        self.w32(GCTL, self.r32(GCTL) & !1);
        if !self.wait(100, |h| h.r32(GCTL) & 1 == 0) {
            return Err("does not enter reset");
        }
        time::delay_us(100);
        self.w32(GCTL, self.r32(GCTL) | 1);
        if !self.wait(100, |h| h.r32(GCTL) & 1 == 1) {
            return Err("does not leave reset");
        }
        // Codecs announce themselves within 521 us of the reset.
        time::delay_us(1000);
        self.w32(INTCTL, 0);
        let pick = |caps: u8| -> (u8, u16) {
            if caps & 0x40 != 0 {
                (2, 256)
            } else if caps & 0x20 != 0 {
                (1, 16)
            } else {
                (0, 2)
            }
        };
        let (cs, ce) = pick(self.r8(CORBSIZE) >> 4 << 4);
        self.w8(CORBSIZE, cs);
        self.corb_entries = ce;
        let corb = self.corb.phys();
        self.w32(CORBLBASE, corb as u32);
        self.w32(CORBUBASE, (corb >> 32) as u32);
        self.w16(CORBWP, 0);
        // Reset the read pointer: set bit 15, wait for it, clear it.
        self.w16(CORBRP, 1 << 15);
        self.wait(10, |h| h.r16(CORBRP) & (1 << 15) != 0);
        self.w16(CORBRP, 0);
        self.wait(10, |h| h.r16(CORBRP) & (1 << 15) == 0);
        let (rs, re) = pick(self.r8(RIRBSIZE) >> 4 << 4);
        self.w8(RIRBSIZE, rs);
        self.rirb_entries = re;
        let rirb = self.rirb.phys();
        self.w32(RIRBLBASE, rirb as u32);
        self.w32(RIRBUBASE, (rirb >> 32) as u32);
        self.w16(RIRBWP, 1 << 15);
        self.w16(RINTCNT, 0xff);
        self.rirb_read = 0;
        self.w8(CORBCTL, 1 << 1);
        self.w8(RIRBCTL, 1 << 1);
        Ok(())
    }

    /// The output stream: reset, buffer, BDL, format; not running.
    fn prepare_stream(&mut self) {
        let sd = self.sd;
        self.w8(sd + SD_CTL, 0);
        self.wait(10, |h| h.r8(sd + SD_CTL) & CTL_RUN == 0);
        self.w8(sd + SD_CTL, CTL_SRST);
        self.wait(10, |h| h.r8(sd + SD_CTL) & CTL_SRST != 0);
        self.w8(sd + SD_CTL, 0);
        self.wait(10, |h| h.r8(sd + SD_CTL) & CTL_SRST == 0);
        self.w8(sd + SD_STS, 0x1c);
        self.w32(sd + SD_CBL, BUFFER as u32);
        self.w16(sd + SD_LVI, BDL_ENTRIES as u16 - 1);
        self.w16(sd + SD_FMT, velora_hda::FORMAT_48K_16_STEREO);
        let bdl = self._bdl.phys();
        self.w32(sd + SD_BDPL, bdl as u32);
        self.w32(sd + SD_BDPU, (bdl >> 32) as u32);
        // Stream tag in bits 23:20 of the control register (byte 2).
        self.w8(sd + SD_CTL + 2, TAG << 4);
        self.buf.as_mut_slice().fill(0);
        self.ring.reset();
        self.running = false;
    }

    fn start(&mut self) {
        if self.running {
            return;
        }
        self.prepare_stream();
        self.w8(self.sd + SD_CTL, CTL_RUN);
        self.running = true;
        self.last_sound = time::uptime_ms();
    }

    fn stop(&mut self) {
        self.w8(self.sd + SD_CTL, 0);
        self.running = false;
    }

    /// Follows the link position and clears what was played.
    fn follow(&mut self) {
        if !self.running {
            return;
        }
        let lpib = self.r32(self.sd + SD_LPIB) as usize;
        for (off, len) in self.ring.advance(lpib) {
            self.buf.as_mut_slice()[off..off + len].fill(0);
        }
        let now = time::uptime_ms();
        if self.ring.pending() > velora_hda::ring::LEAD {
            self.last_sound = now;
        } else if now - self.last_sound > IDLE_STOP_MS {
            self.stop();
        }
    }

    /// Copies samples into the buffer; returns the bytes taken.
    fn write(&mut self, data: &[u8]) -> usize {
        self.start();
        self.follow();
        let (n, spans) = self.ring.reserve(data.len());
        let gain = if self.soft {
            codec::soft_gain(self.state.volume, self.state.mute)
        } else {
            65536
        };
        let mut src = 0;
        let buf = self.buf.as_mut_slice();
        for (off, len) in spans {
            let dst = &mut buf[off..off + len];
            if gain >= 65536 {
                dst.copy_from_slice(&data[src..src + len]);
            } else {
                let (d2, _) = dst.as_chunks_mut::<2>();
                let (s2, _) = data[src..src + len].as_chunks::<2>();
                for (d, s) in d2.iter_mut().zip(s2) {
                    let v = (i16::from_le_bytes(*s) as i32 * gain as i32) >> 16;
                    *d = (v as i16).to_le_bytes();
                }
            }
            src += len;
        }
        if n > 0 {
            self.last_sound = time::uptime_ms();
        }
        n
    }

    /// Sets the chosen path up and its volume.
    fn route(&mut self) {
        let Some(path) = self.paths.get(self.state.output).cloned() else {
            return;
        };
        let g = self.graph.clone();
        g.set_up(self, &path, TAG, velora_hda::FORMAT_48K_16_STEREO);
        self.apply_volume();
    }

    fn apply_volume(&mut self) {
        let Some(path) = self.paths.get(self.state.output).cloned() else {
            return;
        };
        let g = self.graph.clone();
        let (v, m) = (self.state.volume, self.state.mute);
        self.soft = !g.set_volume(self, &path, v, m);
    }
}

impl Codec for Hda {
    fn cmd(&mut self, verb: u32) -> Option<u32> {
        let wp = (self.r16(CORBWP) & 0xff).wrapping_add(1) % self.corb_entries;
        self.corb.write(wp as usize * 4, verb);
        self.w16(CORBWP, wp);
        let cad = (verb >> 28) as u64;
        for _ in 0..2000 {
            let rwp = self.r16(RIRBWP) & 0xff;
            // The controller stops taking commands after the response
            // count until the response flag is cleared.
            self.w8(RIRBSTS, 0x05);
            while self.rirb_read != rwp {
                self.rirb_read = (self.rirb_read + 1) % self.rirb_entries;
                let i = self.rirb_read as usize * 8;
                let resp: u32 = self.rirb.read(i);
                let ext: u32 = self.rirb.read(i + 4);
                // Unsolicited responses (jack events) are skipped.
                if ext & 0x10 == 0 && (ext & 0xf) as u64 == cad {
                    return Some(resp);
                }
            }
            time::delay_us(10);
        }
        None
    }
}

/// Finds and starts the first HDA controller with an output; returns
/// whether one works.
pub fn probe() -> bool {
    for d in pci::devices()
        .iter()
        .filter(|d| d.class == 0x04 && d.subclass == 0x03)
    {
        match start(d) {
            Ok(h) => {
                crate::kok!(
                    "audio: {} codec {:08x}, outputs: {}",
                    h.name,
                    h.graph.vendor,
                    h.state
                        .devices
                        .iter()
                        .map(|j| j.key())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                *HDA.lock() = Some(h);
                sched::spawn_kernel("audio", audio_thread, 0, Priority::High)
                    .expect("cannot start the audio thread");
                return true;
            }
            Err(e) => crate::kwarn!("audio at {}: {}", d.addr, e),
        }
    }
    false
}

fn start(d: &velora_pci::Device) -> Result<Hda, &'static str> {
    velora_pci::enable_device(pci::config(), d.addr);
    let (bar, size) = d.bars[0].memory().ok_or("BAR0 missing")?;
    let regs = mmio::map(bar, size.max(0x4000)).ok_or("cannot map registers")?;
    let oom = "out of memory";
    let mut h = Hda {
        regs,
        corb: DmaBuffer::new(1024).ok_or(oom)?,
        rirb: DmaBuffer::new(2048).ok_or(oom)?,
        corb_entries: 2,
        rirb_entries: 2,
        rirb_read: 0,
        sd: 0,
        buf: DmaBuffer::new(BUFFER).ok_or(oom)?,
        _bdl: DmaBuffer::new(BDL_ENTRIES * 16).ok_or(oom)?,
        ring: Ring::new(BUFFER),
        running: false,
        last_sound: 0,
        graph: Graph {
            cad: 0,
            afg: 0,
            vendor: 0,
            widgets: Vec::new(),
        },
        paths: Vec::new(),
        state: State::default(),
        soft: false,
        name: alloc::format!("{:04x}:{:04x}", d.vendor, d.device),
    };
    let gcap = h.r16(GCAP);
    let (oss, iss) = ((gcap >> 12) & 0xf, (gcap >> 8) & 0xf);
    if oss == 0 {
        return Err("no output streams");
    }
    h.sd = 0x80 + iss as u64 * 0x20;
    h.reset()?;
    let codecs = h.r16(STATESTS);
    let part = BUFFER / BDL_ENTRIES;
    for i in 0..BDL_ENTRIES {
        let addr = h.buf.phys() + (i * part) as u64;
        h._bdl.write(i * 16, addr);
        h._bdl.write(i * 16 + 8, part as u32);
        h._bdl.write(i * 16 + 12, 0u32);
    }
    for cad in (0..15).filter(|c| codecs & (1 << c) != 0) {
        let Some(g) = Graph::read(&mut h, cad) else {
            continue;
        };
        let paths = g.output_paths();
        if paths.is_empty() {
            continue;
        }
        h.state.devices = paths.iter().map(|p| p.jack).collect();
        h.graph = g;
        h.paths = paths;
        h.prepare_stream();
        h.route();
        return Ok(h);
    }
    Err(if codecs == 0 {
        "no codecs"
    } else {
        "no codec with an output"
    })
}

extern "C" fn audio_thread(_: u64) -> u64 {
    loop {
        let running = HDA
            .lock()
            .as_mut()
            .map(|h| {
                h.follow();
                h.running
            })
            .unwrap_or(false);
        cpu::disable_interrupts();
        // While sound plays: every tick; otherwise until a write.
        WAKE.wait(running.then(|| time::ticks() + 1));
        cpu::enable_interrupts();
    }
}

pub fn present() -> bool {
    HDA.lock().is_some()
}

/// Writes samples (48 kHz, 16-bit little-endian, stereo); returns the
/// bytes taken (0: the buffer is full), `None` without a sound card.
pub fn write(data: &[u8]) -> Option<usize> {
    let n = HDA.lock().as_mut().map(|h| h.write(data));
    WAKE.wake_all();
    n
}

/// The text of `/dev/mixer`.
pub fn mixer_text() -> String {
    HDA.lock()
        .as_ref()
        .map(|h| h.state.to_text())
        .unwrap_or_else(|| State::default().to_text())
}

/// Applies `/dev/mixer` commands; false if the text is not valid or
/// there is no sound card.
pub fn mixer_write(text: &str) -> bool {
    let Some(cmds) = mixer::commands(text) else {
        return false;
    };
    let mut g = HDA.lock();
    let Some(h) = g.as_mut() else {
        return false;
    };
    let mut ok = true;
    for c in cmds {
        let old = h.state.output;
        ok &= h.state.apply(c);
        match c {
            Command::Output(_) if h.state.output != old => {
                // Mute the old path's pin and route the new one.
                let pin = h.paths[old].pin();
                let cad = h.graph.cad;
                let _ = h.cmd(codec::verb12(cad, pin, codec::SET_PIN_CONTROL, 0));
                h.route();
            }
            _ => h.apply_volume(),
        }
    }
    ok
}

/// For the kernel tests: the stream is running and its position moved.
#[cfg(feature = "ktest")]
pub fn position() -> Option<(bool, u32)> {
    HDA.lock()
        .as_ref()
        .map(|h| (h.running, h.r32(h.sd + SD_LPIB)))
}

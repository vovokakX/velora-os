//! TCP (RFC 793 / 9293), the parts needed for reliable byte streams:
//! three-way handshake, data with acknowledgements and the peer's window,
//! retransmission (go-back-N with exponential back-off), FIN in both
//! directions, RST. Out-of-order segments are dropped (the peer resends
//! them); there is no congestion control.

use crate::wire::{Ipv4, TcpSegment, tcp, tcp_flags as f};
use crate::{Handle, NetError};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpState {
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    Closed,
}

const RX_CAP: usize = 64 * 1024;
const TX_CAP: usize = 64 * 1024;
const OUR_MSS: u16 = 1460;
const DEFAULT_MSS: usize = 536;
const RTO_START: u64 = 500;
const RTO_MAX: u64 = 8000;
const MAX_RETRIES: u32 = 8;
/// Fewer tries for the SYN: about 15 s until "timed out".
const MAX_SYN_RETRIES: u32 = 5;
const TIME_WAIT_MS: u64 = 2000;
/// Handles for connections not yet accepted.
const INTERNAL: Handle = 1 << 31;

fn lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}
fn le(a: u32, b: u32) -> bool {
    a == b || lt(a, b)
}

struct Conn {
    state: TcpState,
    local: (Ipv4, u16),
    remote: (Ipv4, u16),
    snd_una: u32,
    snd_nxt: u32,
    snd_wnd: u32,
    iss: u32,
    mss: usize,
    tx: VecDeque<u8>,
    /// Sequence number of `tx[0]`.
    tx_seq: u32,
    fin_queued: bool,
    fin_sent: bool,
    fin_seq: u32,
    rcv_nxt: u32,
    rx: VecDeque<u8>,
    peer_fin: bool,
    rto: u64,
    deadline: Option<u64>,
    retries: u32,
    /// Duplicate ACKs in a row (three trigger a retransmission).
    dup_acks: u32,
    error: Option<NetError>,
    parent: Option<Handle>,
    released: bool,
    time_wait_until: u64,
}

struct Listener {
    port: u16,
    ready: VecDeque<Handle>,
}

pub struct Tcp {
    /// Current time in ms, kept up to date by the stack.
    pub now: u64,
    conns: BTreeMap<Handle, Conn>,
    listeners: BTreeMap<Handle, Listener>,
    out: Vec<(Ipv4, Vec<u8>)>,
    rng: u64,
    next_internal: Handle,
}

impl Tcp {
    pub fn new(seed: u64) -> Self {
        Self {
            now: 0,
            conns: BTreeMap::new(),
            listeners: BTreeMap::new(),
            out: Vec::new(),
            rng: seed | 1,
            next_internal: INTERNAL,
        }
    }

    pub fn random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    pub fn take_output(&mut self) -> Vec<(Ipv4, Vec<u8>)> {
        core::mem::take(&mut self.out)
    }

    pub fn port_in_use(&self, p: u16) -> bool {
        self.listeners.values().any(|l| l.port == p)
            || self
                .conns
                .values()
                .any(|c| c.local.1 == p && c.state != TcpState::Closed)
    }

    fn emit(out: &mut Vec<(Ipv4, Vec<u8>)>, c: &Conn, seq: u32, flags: u8, data: &[u8]) {
        let window = (RX_CAP - c.rx.len()).min(65535) as u16;
        let ack = if flags & f::ACK != 0 { c.rcv_nxt } else { 0 };
        let mss = (flags & f::SYN != 0).then_some(OUR_MSS);
        let seg = tcp(
            c.local.0, c.local.1, c.remote.0, c.remote.1, seq, ack, flags, window, mss, data,
        );
        out.push((c.remote.0, seg));
    }

    fn reset_for(&mut self, local: Ipv4, src: Ipv4, seg: &TcpSegment) {
        if seg.flags & f::RST != 0 {
            return;
        }
        let (seq, ack, flags) = if seg.flags & f::ACK != 0 {
            (seg.ack, 0, f::RST)
        } else {
            let len = seg.data.len() as u32 + (seg.flags & (f::SYN | f::FIN) != 0) as u32;
            (0, seg.seq.wrapping_add(len), f::RST | f::ACK)
        };
        let s = tcp(
            local,
            seg.dport,
            src,
            seg.sport,
            seq,
            ack,
            flags,
            0,
            None,
            &[],
        );
        self.out.push((src, s));
    }

    fn new_conn(&mut self, local: (Ipv4, u16), remote: (Ipv4, u16), state: TcpState) -> Conn {
        let iss = self.random() as u32;
        Conn {
            state,
            local,
            remote,
            snd_una: iss,
            snd_nxt: iss.wrapping_add(1),
            snd_wnd: 0,
            iss,
            mss: DEFAULT_MSS,
            tx: VecDeque::new(),
            tx_seq: iss.wrapping_add(1),
            fin_queued: false,
            fin_sent: false,
            fin_seq: 0,
            rcv_nxt: 0,
            rx: VecDeque::new(),
            peer_fin: false,
            rto: RTO_START,
            deadline: None,
            retries: 0,
            dup_acks: 0,
            error: None,
            parent: None,
            released: false,
            time_wait_until: 0,
        }
    }

    pub fn connect(
        &mut self,
        h: Handle,
        local_ip: Ipv4,
        local_port: u16,
        dst: Ipv4,
        port: u16,
        now: u64,
    ) {
        let mut c = self.new_conn((local_ip, local_port), (dst, port), TcpState::SynSent);
        c.deadline = Some(now + c.rto);
        Self::emit(&mut self.out, &c, c.iss, f::SYN, &[]);
        self.conns.insert(h, c);
    }

    pub fn listen(&mut self, h: Handle, port: u16) {
        self.listeners.insert(
            h,
            Listener {
                port,
                ready: VecDeque::new(),
            },
        );
    }

    pub fn accept(
        &mut self,
        listener: Handle,
        mut alloc: impl FnMut() -> Handle,
    ) -> Result<Option<Handle>, NetError> {
        let l = self.listeners.get_mut(&listener).ok_or(NetError::Invalid)?;
        let Some(internal) = l.ready.pop_front() else {
            return Ok(None);
        };
        let mut c = self.conns.remove(&internal).ok_or(NetError::Invalid)?;
        c.parent = None;
        let h = alloc();
        self.conns.insert(h, c);
        Ok(Some(h))
    }

    pub fn state(&self, h: Handle) -> Option<TcpState> {
        if self.listeners.contains_key(&h) {
            return Some(TcpState::Listen);
        }
        self.conns.get(&h).map(|c| c.state)
    }

    pub fn error(&self, h: Handle) -> Option<NetError> {
        self.conns.get(&h).and_then(|c| c.error)
    }

    pub fn fail_unreachable(&mut self, unreachable: impl Fn(Ipv4) -> bool) {
        for c in self.conns.values_mut() {
            if c.state == TcpState::SynSent && unreachable(c.remote.0) {
                c.state = TcpState::Closed;
                c.error = Some(NetError::NoRoute);
                c.deadline = None;
            }
        }
    }

    pub fn send(&mut self, h: Handle, data: &[u8]) -> Result<usize, NetError> {
        let c = self.conns.get_mut(&h).ok_or(NetError::Invalid)?;
        if let Some(e) = c.error {
            return Err(e);
        }
        match c.state {
            TcpState::Established | TcpState::CloseWait if !c.fin_queued => {}
            TcpState::SynSent | TcpState::SynReceived => return Err(NetError::WouldBlock),
            _ => return Err(NetError::Closed),
        }
        let n = data.len().min(TX_CAP - c.tx.len());
        if n == 0 {
            return Err(NetError::WouldBlock);
        }
        c.tx.extend(&data[..n]);
        Self::output(&mut self.out, c, self.now);
        Ok(n)
    }

    pub fn recv(&mut self, h: Handle, buf: &mut [u8]) -> Result<usize, NetError> {
        let c = self.conns.get_mut(&h).ok_or(NetError::Invalid)?;
        if !c.rx.is_empty() {
            let was_full = RX_CAP - c.rx.len() < c.mss;
            let n = buf.len().min(c.rx.len());
            for (d, s) in buf.iter_mut().zip(c.rx.drain(..n)) {
                *d = s;
            }
            if was_full && c.state != TcpState::Closed {
                // Tell the peer the window opened again.
                Self::emit(&mut self.out, c, c.snd_nxt, f::ACK, &[]);
            }
            return Ok(n);
        }
        if let Some(e) = c.error {
            return Err(e);
        }
        if c.peer_fin || c.state == TcpState::Closed {
            return Ok(0);
        }
        Err(NetError::WouldBlock)
    }

    /// Ends the sending side (FIN after the queued data); receiving goes on.
    pub fn shutdown(&mut self, h: Handle) -> Result<(), NetError> {
        let c = self.conns.get_mut(&h).ok_or(NetError::Invalid)?;
        match c.state {
            TcpState::SynReceived | TcpState::Established | TcpState::CloseWait => {
                c.fin_queued = true;
                Self::output(&mut self.out, c, self.now);
                Ok(())
            }
            _ => Err(NetError::NotConnected),
        }
    }

    pub fn close(&mut self, h: Handle) {
        if let Some(l) = self.listeners.remove(&h) {
            for internal in l.ready {
                self.close(internal);
            }
            let orphans: Vec<Handle> = self
                .conns
                .iter()
                .filter(|(_, c)| c.parent == Some(h))
                .map(|(&k, _)| k)
                .collect();
            for k in orphans {
                self.conns.remove(&k);
            }
            return;
        }
        let Some(c) = self.conns.get_mut(&h) else {
            return;
        };
        c.released = true;
        match c.state {
            TcpState::SynSent | TcpState::Closed | TcpState::Listen => {
                self.conns.remove(&h);
            }
            TcpState::SynReceived | TcpState::Established | TcpState::CloseWait => {
                c.fin_queued = true;
                Self::output(&mut self.out, c, self.now);
            }
            _ => {}
        }
    }

    /// Sends what the window allows, then a FIN once the data is out.
    fn output(out: &mut Vec<(Ipv4, Vec<u8>)>, c: &mut Conn, now: u64) {
        if !matches!(
            c.state,
            TcpState::Established
                | TcpState::CloseWait
                | TcpState::FinWait1
                | TcpState::LastAck
                | TcpState::Closing
        ) {
            return;
        }
        let mut sent = false;
        loop {
            let off = c.snd_nxt.wrapping_sub(c.tx_seq) as usize;
            if off >= c.tx.len() {
                break;
            }
            let wnd_end = c.snd_una.wrapping_add(c.snd_wnd);
            if !lt(c.snd_nxt, wnd_end) {
                break;
            }
            let room = wnd_end.wrapping_sub(c.snd_nxt) as usize;
            let n = c.mss.min(c.tx.len() - off).min(room);
            let (a, b) = c.tx.as_slices();
            let data: Vec<u8> = a.iter().chain(b).skip(off).take(n).copied().collect();
            Self::emit(out, c, c.snd_nxt, f::ACK | f::PSH, &data);
            c.snd_nxt = c.snd_nxt.wrapping_add(n as u32);
            sent = true;
        }
        let all_sent = c.snd_nxt == c.tx_seq.wrapping_add(c.tx.len() as u32);
        if c.fin_queued && !c.fin_sent && all_sent {
            c.fin_seq = c.snd_nxt;
            Self::emit(out, c, c.snd_nxt, f::FIN | f::ACK, &[]);
            c.snd_nxt = c.snd_nxt.wrapping_add(1);
            c.fin_sent = true;
            c.state = match c.state {
                TcpState::Established => TcpState::FinWait1,
                TcpState::CloseWait => TcpState::LastAck,
                s => s,
            };
            sent = true;
        }
        if sent && c.deadline.is_none() {
            c.deadline = Some(now + c.rto);
        }
    }

    pub fn receive(&mut self, src: Ipv4, me: Ipv4, seg: &TcpSegment, now: u64) {
        let key = self
            .conns
            .iter()
            .find(|(_, c)| {
                c.local.1 == seg.dport
                    && c.remote == (src, seg.sport)
                    && c.state != TcpState::Closed
            })
            .map(|(&h, _)| h);
        let Some(h) = key else {
            let listener = self
                .listeners
                .iter()
                .find(|(_, l)| l.port == seg.dport)
                .map(|(&h, _)| h);
            match listener {
                Some(l) if seg.flags & f::SYN != 0 && seg.flags & (f::ACK | f::RST) == 0 => {
                    let mut c =
                        self.new_conn((me, seg.dport), (src, seg.sport), TcpState::SynReceived);
                    c.rcv_nxt = seg.seq.wrapping_add(1);
                    c.snd_wnd = seg.window as u32;
                    c.mss = seg
                        .mss
                        .map_or(DEFAULT_MSS, |m| m as usize)
                        .clamp(64, OUR_MSS as usize);
                    c.parent = Some(l);
                    c.deadline = Some(now + c.rto);
                    Self::emit(&mut self.out, &c, c.iss, f::SYN | f::ACK, &[]);
                    let id = self.next_internal;
                    self.next_internal += 1;
                    self.conns.insert(id, c);
                }
                _ => self.reset_for(me, src, seg),
            }
            return;
        };
        let out = &mut self.out;
        let c = self.conns.get_mut(&h).unwrap();
        let has = |flag: u8| seg.flags & flag != 0;

        if c.state == TcpState::SynSent {
            if has(f::ACK) && seg.ack != c.snd_nxt {
                if !has(f::RST) {
                    let s = tcp(
                        c.local.0,
                        c.local.1,
                        src,
                        seg.sport,
                        seg.ack,
                        0,
                        f::RST,
                        0,
                        None,
                        &[],
                    );
                    out.push((src, s));
                }
                return;
            }
            if has(f::RST) {
                if has(f::ACK) {
                    c.state = TcpState::Closed;
                    c.error = Some(NetError::Refused);
                    c.deadline = None;
                }
                return;
            }
            if has(f::SYN) && has(f::ACK) {
                c.rcv_nxt = seg.seq.wrapping_add(1);
                c.snd_una = seg.ack;
                c.snd_wnd = seg.window as u32;
                c.mss = seg
                    .mss
                    .map_or(DEFAULT_MSS, |m| m as usize)
                    .clamp(64, OUR_MSS as usize);
                c.state = TcpState::Established;
                c.deadline = None;
                c.retries = 0;
                c.rto = RTO_START;
                Self::emit(out, c, c.snd_nxt, f::ACK, &[]);
                Self::output(out, c, now);
            }
            return;
        }

        if has(f::RST) {
            if seg.seq == c.rcv_nxt || c.state == TcpState::SynReceived {
                c.error = (c.state != TcpState::TimeWait).then_some(NetError::Reset);
                c.state = TcpState::Closed;
                c.deadline = None;
                if c.parent.is_some() || c.released {
                    self.conns.remove(&h);
                }
            }
            return;
        }
        if has(f::SYN) {
            if c.state == TcpState::SynReceived && seg.seq.wrapping_add(1) == c.rcv_nxt {
                Self::emit(out, c, c.iss, f::SYN | f::ACK, &[]);
            } else {
                Self::emit(out, c, c.snd_nxt, f::ACK, &[]);
            }
            return;
        }
        if !has(f::ACK) {
            return;
        }
        if c.state == TcpState::SynReceived {
            if seg.ack != c.snd_nxt {
                let s = tcp(
                    c.local.0,
                    c.local.1,
                    src,
                    seg.sport,
                    seg.ack,
                    0,
                    f::RST,
                    0,
                    None,
                    &[],
                );
                out.push((src, s));
                return;
            }
            c.state = TcpState::Established;
            c.snd_una = seg.ack;
            c.deadline = None;
            c.retries = 0;
            if let Some(l) = c.parent
                && let Some(listener) = self.listeners.get_mut(&l)
            {
                listener.ready.push_back(h);
            }
        }
        let c = self.conns.get_mut(&h).unwrap();

        // Acknowledgement of our data (and FIN).
        if lt(c.snd_una, seg.ack) && le(seg.ack, c.snd_nxt) {
            let data_end = c.tx_seq.wrapping_add(c.tx.len() as u32);
            let upto = if lt(seg.ack, data_end) {
                seg.ack
            } else {
                data_end
            };
            let n = upto.wrapping_sub(c.tx_seq) as usize;
            if lt(c.tx_seq, upto) {
                c.tx.drain(..n.min(c.tx.len()));
                c.tx_seq = upto;
            }
            c.snd_una = seg.ack;
            c.retries = 0;
            c.dup_acks = 0;
            c.rto = RTO_START;
            c.deadline = (c.snd_una != c.snd_nxt).then_some(now + c.rto);
            if c.fin_sent && seg.ack == c.fin_seq.wrapping_add(1) {
                c.state = match c.state {
                    TcpState::FinWait1 => TcpState::FinWait2,
                    TcpState::Closing => {
                        c.time_wait_until = now + TIME_WAIT_MS;
                        TcpState::TimeWait
                    }
                    TcpState::LastAck => TcpState::Closed,
                    s => s,
                };
            }
        } else if seg.ack == c.snd_una
            && c.snd_una != c.snd_nxt
            && seg.data.is_empty()
            && !has(f::FIN)
        {
            // The peer is missing data: after three duplicates, send again
            // from the first unacknowledged byte (fast retransmit).
            c.dup_acks += 1;
            if c.dup_acks == 3 {
                c.snd_nxt = c.snd_una;
                if c.fin_sent && le(c.snd_una, c.fin_seq) {
                    c.fin_sent = false;
                }
                c.deadline = Some(now + c.rto);
            }
        }
        if le(c.snd_una, seg.ack) {
            c.snd_wnd = seg.window as u32;
        }

        // Data in order is kept; anything else is answered with an ACK.
        let mut ack_needed = false;
        let mut accepted_all = true;
        if !seg.data.is_empty() {
            ack_needed = true;
            let can_receive = matches!(
                c.state,
                TcpState::Established | TcpState::FinWait1 | TcpState::FinWait2
            );
            if seg.seq == c.rcv_nxt && can_receive {
                let n = seg.data.len().min(RX_CAP - c.rx.len());
                c.rx.extend(&seg.data[..n]);
                c.rcv_nxt = c.rcv_nxt.wrapping_add(n as u32);
                accepted_all = n == seg.data.len();
            } else {
                accepted_all = false;
            }
        }
        if has(f::FIN)
            && accepted_all
            && seg.seq.wrapping_add(seg.data.len() as u32) == c.rcv_nxt
            && !c.peer_fin
        {
            c.rcv_nxt = c.rcv_nxt.wrapping_add(1);
            c.peer_fin = true;
            ack_needed = true;
            c.state = match c.state {
                TcpState::Established => TcpState::CloseWait,
                TcpState::FinWait1 => TcpState::Closing,
                TcpState::FinWait2 => {
                    c.time_wait_until = now + TIME_WAIT_MS;
                    TcpState::TimeWait
                }
                s => s,
            };
        }
        if ack_needed {
            Self::emit(out, c, c.snd_nxt, f::ACK, &[]);
        }
        Self::output(out, c, now);
        if c.state == TcpState::Closed && c.released {
            self.conns.remove(&h);
        }
    }

    /// Retransmission and TIME-WAIT timers.
    pub fn poll(&mut self, now: u64) {
        let mut remove = Vec::new();
        for (&h, c) in self.conns.iter_mut() {
            if c.state == TcpState::TimeWait && now >= c.time_wait_until {
                c.state = TcpState::Closed;
            }
            if c.state == TcpState::Closed {
                if c.released || c.parent.is_some() {
                    remove.push(h);
                }
                continue;
            }
            let Some(d) = c.deadline else { continue };
            if now < d {
                continue;
            }
            c.retries += 1;
            let limit = if c.state == TcpState::SynSent {
                MAX_SYN_RETRIES
            } else {
                MAX_RETRIES
            };
            if c.retries > limit {
                c.state = TcpState::Closed;
                c.error = Some(NetError::TimedOut);
                c.deadline = None;
                continue;
            }
            c.rto = (c.rto * 2).min(RTO_MAX);
            c.deadline = Some(now + c.rto);
            match c.state {
                TcpState::SynSent => Self::emit(&mut self.out, c, c.iss, f::SYN, &[]),
                TcpState::SynReceived => Self::emit(&mut self.out, c, c.iss, f::SYN | f::ACK, &[]),
                _ => {
                    // Go back to the first unacknowledged byte.
                    c.snd_nxt = c.snd_una;
                    if c.fin_sent && le(c.snd_una, c.fin_seq) {
                        c.fin_sent = false;
                    }
                    let saved = c.deadline;
                    Self::output(&mut self.out, c, now);
                    c.deadline = saved;
                }
            }
        }
        for h in remove {
            self.conns.remove(&h);
        }
    }
}

//! Networking: the virtio-net device, the `velora-net` stack and sockets.
//!
//! The `net` kernel thread moves frames between the device and the stack
//! and runs the stack's timers. It wakes on the device interrupt and at
//! least every timer tick. System calls work on the stack under the same
//! lock and wait on [`ACTIVITY`] when they have to block.

use crate::arch::x86_64::cpu;
use crate::drivers::{self, NetDevice};
use crate::sched::{self, Priority, WaitQueue};
use crate::sync::SpinLock;
use crate::{kok, time};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use velora_net::{Handle, Ipv4, NetError, Stack, TcpState};

struct Net {
    dev: alloc::boxed::Box<dyn NetDevice>,
    stack: Stack,
    sockets: BTreeMap<u32, Socket>,
    next_socket: u32,
    announced: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Udp,
    Tcp,
}

/// A socket as the file descriptor sees it.
struct Socket {
    kind: Kind,
    /// Stack handle once bound, connected or listening.
    handle: Option<Handle>,
    port: u16,
    peer: Option<(Ipv4, u16)>,
    listening: bool,
}

static NET: SpinLock<Option<Net>> = SpinLock::new(None);
/// Wakes the network thread (interrupts, new frames to send).
static WAKE: WaitQueue = WaitQueue::new();
/// Wakes system calls waiting for the network.
static ACTIVITY: WaitQueue = WaitQueue::new();

/// Called from the device interrupt.
pub fn wake() {
    WAKE.wake_all();
}

fn with<R>(f: impl FnOnce(&mut Net) -> R) -> Option<R> {
    NET.lock().as_mut().map(f)
}

fn now() -> u64 {
    time::uptime_ms()
}

/// Sends the frames the stack produced.
fn pump(n: &mut Net) {
    while let Some(f) = n.stack.take_frame() {
        if !n.dev.send(&f) {
            break; // transmit ring full: the frame is lost, TCP resends
        }
    }
}

fn process(n: &mut Net) {
    let t = now();
    while let Some(frame) = n.dev.receive() {
        n.stack.receive(&frame, t);
    }
    n.stack.poll(t);
    pump(n);
    if !n.announced
        && let Some(c) = n.stack.config()
    {
        n.announced = true;
        let bits = c.netmask.0.count_ones();
        kok!(
            "net0: {}/{} via {} (DHCP), DNS {}",
            c.ip,
            bits,
            c.gateway,
            c.dns
        );
    }
}

extern "C" fn net_thread(_: u64) -> u64 {
    loop {
        with(process);
        ACTIVITY.wake_all();
        cpu::disable_interrupts();
        WAKE.wait(Some(time::ticks() + 1));
        cpu::enable_interrupts();
    }
}

/// Finds the network card, starts DHCP and the network thread.
pub fn init() {
    let Some(dev) = drivers::probe_network() else {
        return;
    };
    let mac = dev.mac();
    let mut stack = Stack::new(mac, cpu::rdtsc());
    stack.start_dhcp(now());
    let mut n = Net {
        dev,
        stack,
        sockets: BTreeMap::new(),
        next_socket: 1,
        announced: false,
    };
    pump(&mut n);
    let model = alloc::string::String::from(n.dev.name());
    *NET.lock() = Some(n);
    kok!(
        "net0: {}, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, asking DHCP for an address",
        model,
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );
    sched::spawn_kernel("net", net_thread, 0, Priority::High).expect("cannot start the net thread");
}

pub fn present() -> bool {
    NET.lock().is_some()
}

/// Interface information for `netinfo`: (MAC, config, rx, tx).
pub fn info() -> Option<([u8; 6], Option<velora_net::Config>, u64, u64)> {
    with(|n| {
        let c = n.stack.counters;
        (n.stack.mac(), n.stack.config(), c.rx_frames, c.tx_frames)
    })
}

/// Waits until `f` returns Some, the deadline (ms) passes (`None`), or
/// forever when `deadline` is None.
fn block_until<R>(deadline: Option<u64>, mut f: impl FnMut(&mut Net) -> Option<R>) -> Option<R> {
    let was = cpu::irq_save();
    let r = loop {
        cpu::disable_interrupts();
        match with(|n| {
            let r = f(n);
            pump(n);
            r
        }) {
            Some(Some(r)) => break Some(r),
            None => break None,
            Some(None) => {}
        }
        if deadline.is_some_and(|d| now() >= d) {
            break None;
        }
        WAKE.wake_all();
        ACTIVITY.wait(Some(time::ticks() + 1));
    };
    cpu::irq_restore(was);
    r
}

pub fn errno(e: NetError) -> i64 {
    use velora_abi::errno::*;
    match e {
        NetError::NoRoute => ENETUNREACH,
        NetError::Refused => ECONNREFUSED,
        NetError::TimedOut => ETIMEDOUT,
        NetError::Reset => ECONNRESET,
        NetError::InUse => EADDRINUSE,
        NetError::NotConnected => ENOTCONN,
        NetError::Closed => EPIPE,
        NetError::Invalid => EINVAL,
        NetError::WouldBlock => EAGAIN,
    }
}

// --- sockets -----------------------------------------------------------------

pub fn socket(kind: Kind) -> Result<u32, i64> {
    with(|n| {
        let id = n.next_socket;
        n.next_socket += 1;
        n.sockets.insert(
            id,
            Socket {
                kind,
                handle: None,
                port: 0,
                peer: None,
                listening: false,
            },
        );
        id
    })
    .ok_or(-velora_abi::errno::ENETDOWN)
}

pub fn close(id: u32) {
    with(|n| {
        if let Some(s) = n.sockets.remove(&id)
            && let Some(h) = s.handle
        {
            match s.kind {
                Kind::Udp => n.stack.udp_close(h),
                Kind::Tcp => n.stack.tcp_close(h),
            }
        }
        pump(n);
    });
}

fn sock<R>(id: u32, f: impl FnOnce(&mut Net, &mut Socket) -> Result<R, i64>) -> Result<R, i64> {
    with(|n| {
        let mut s = n.sockets.remove(&id).ok_or(-velora_abi::errno::EBADF)?;
        let r = f(n, &mut s);
        n.sockets.insert(id, s);
        r
    })
    .unwrap_or(Err(-velora_abi::errno::ENETDOWN))
}

pub fn bind(id: u32, port: u16) -> Result<(), i64> {
    sock(id, |n, s| {
        if s.handle.is_some() || s.port != 0 {
            return Err(-velora_abi::errno::EINVAL);
        }
        match s.kind {
            Kind::Udp => s.handle = Some(n.stack.udp_bind(port).map_err(|e| -errno(e))?),
            Kind::Tcp => s.port = port,
        }
        s.port = port;
        Ok(())
    })
}

pub fn shutdown(id: u32) -> Result<(), i64> {
    sock(id, |n, s| {
        let h = s.handle.filter(|_| s.kind == Kind::Tcp && !s.listening);
        let h = h.ok_or(-velora_abi::errno::ENOTCONN)?;
        let r = n.stack.tcp_shutdown(h).map_err(|e| -errno(e));
        pump(n);
        r
    })
}

pub fn listen(id: u32) -> Result<(), i64> {
    sock(id, |n, s| {
        if s.kind != Kind::Tcp || s.port == 0 || s.handle.is_some() {
            return Err(-velora_abi::errno::EINVAL);
        }
        s.handle = Some(n.stack.tcp_listen(s.port).map_err(|e| -errno(e))?);
        s.listening = true;
        Ok(())
    })
}

pub fn connect(id: u32, ip: Ipv4, port: u16) -> Result<(), i64> {
    let h = sock(id, |n, s| match s.kind {
        Kind::Udp => {
            if s.handle.is_none() {
                s.handle = Some(n.stack.udp_bind(0).map_err(|e| -errno(e))?);
            }
            s.peer = Some((ip, port));
            Ok(None)
        }
        Kind::Tcp => {
            if s.handle.is_some() {
                return Err(-velora_abi::errno::EISCONN);
            }
            let h = n.stack.tcp_connect(ip, port).map_err(|e| -errno(e))?;
            s.handle = Some(h);
            s.peer = Some((ip, port));
            Ok(Some(h))
        }
    })?;
    let Some(h) = h else { return Ok(()) };
    block_until(None, |n| match n.stack.tcp_state(h) {
        Some(TcpState::SynSent) => None,
        Some(TcpState::Established) | Some(TcpState::CloseWait) => Some(Ok(())),
        _ => Some(Err(-errno(
            n.stack.tcp_error(h).unwrap_or(NetError::Refused),
        ))),
    })
    .unwrap_or(Err(-velora_abi::errno::ENETDOWN))
}

pub fn accept(id: u32) -> Result<u32, i64> {
    let lh = sock(id, |_, s| {
        if !s.listening {
            return Err(-velora_abi::errno::EINVAL);
        }
        Ok(s.handle.unwrap())
    })?;
    block_until(None, |n| match n.stack.tcp_accept(lh) {
        Ok(Some(h)) => {
            let nid = n.next_socket;
            n.next_socket += 1;
            n.sockets.insert(
                nid,
                Socket {
                    kind: Kind::Tcp,
                    handle: Some(h),
                    port: 0,
                    peer: None,
                    listening: false,
                },
            );
            Some(Ok(nid))
        }
        Ok(None) => None,
        Err(e) => Some(Err(-errno(e))),
    })
    .unwrap_or(Err(-velora_abi::errno::ENETDOWN))
}

pub fn send(id: u32, data: &[u8], to: Option<(Ipv4, u16)>) -> Result<usize, i64> {
    let (kind, h, peer) = sock(id, |_, s| Ok((s.kind, s.handle, s.peer)))?;
    match kind {
        Kind::Udp => {
            let dest = to.or(peer).ok_or(-velora_abi::errno::EDESTADDRREQ)?;
            let h = match h {
                Some(h) => h,
                None => sock(id, |n, s| {
                    let h = n.stack.udp_bind(0).map_err(|e| -errno(e))?;
                    s.handle = Some(h);
                    Ok(h)
                })?,
            };
            with(|n| {
                let r = n
                    .stack
                    .udp_send(h, dest.0, dest.1, data)
                    .map_err(|e| -errno(e));
                pump(n);
                r
            })
            .unwrap_or(Err(-velora_abi::errno::ENETDOWN))
        }
        Kind::Tcp => {
            let h = h.ok_or(-velora_abi::errno::ENOTCONN)?;
            let mut done = 0;
            while done < data.len() {
                let r = block_until(None, |n| match n.stack.tcp_send(h, &data[done..]) {
                    Ok(k) => Some(Ok(k)),
                    Err(NetError::WouldBlock) => None,
                    Err(e) => Some(Err(e)),
                });
                match r {
                    Some(Ok(k)) => done += k,
                    Some(Err(e)) if done == 0 => return Err(-errno(e)),
                    _ => break,
                }
            }
            Ok(done)
        }
    }
}

/// Receives data; for UDP also the sender.
/// Received bytes and, for UDP, the sender.
pub type Received = (Vec<u8>, Option<(Ipv4, u16)>);

/// Waits for data (at most `timeout_ms` if not 0).
pub fn recv(id: u32, max: usize, timeout_ms: u64) -> Result<Received, i64> {
    let (kind, h) = sock(id, |_, s| Ok((s.kind, s.handle)))?;
    let h = h.ok_or(-velora_abi::errno::ENOTCONN)?;
    let deadline = (timeout_ms > 0).then(|| now() + timeout_ms);
    let r = block_until(deadline, |n| match kind {
        Kind::Udp => match n.stack.udp_recv(h) {
            Ok(Some((ip, port, mut data))) => {
                data.truncate(max);
                Some(Ok((data, Some((ip, port)))))
            }
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        },
        Kind::Tcp => {
            let mut buf = alloc::vec![0u8; max.min(64 * 1024)];
            match n.stack.tcp_recv(h, &mut buf) {
                Ok(k) => {
                    buf.truncate(k);
                    Some(Ok((buf, None)))
                }
                Err(NetError::WouldBlock) => None,
                Err(e) => Some(Err(e)),
            }
        }
    });
    match r {
        Some(Ok(v)) => Ok(v),
        Some(Err(e)) => Err(-errno(e)),
        None if deadline.is_some() && present() => Err(-velora_abi::errno::ETIMEDOUT),
        None => Err(-velora_abi::errno::ENETDOWN),
    }
}

/// Sends an echo request and waits for the reply. Returns the round trip
/// time in microseconds.
pub fn ping(dst: Ipv4, seq: u16, timeout_ms: u64) -> Result<u64, i64> {
    let id = sched::current_pid() as u16;
    let start = cpu::rdtsc();
    with(|n| {
        let r = n.stack.ping(dst, id, seq, b"velora ping 0123456789abcdef");
        pump(n);
        r
    })
    .ok_or(-velora_abi::errno::ENETDOWN)?
    .map_err(|e| -errno(e))?;
    let deadline = now() + timeout_ms;
    block_until(Some(deadline), |n| {
        n.stack.take_echo_reply(dst, id, seq).then_some(())
    })
    .map(|_| time::tsc_to_ns(cpu::rdtsc() - start) / 1000)
    .ok_or(-velora_abi::errno::ETIMEDOUT)
}

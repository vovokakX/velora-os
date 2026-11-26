//! Networking: IPv4 addresses, TCP and UDP sockets, interface information
//! and ping. There is no name resolution (DNS) yet: addresses are numeric.

use crate::sys::{syscall1, syscall2, syscall3, syscall4, syscall6};
use crate::{Errno, check};
use alloc::string::String;
use alloc::vec::Vec;
pub use velora_abi::NetInfo;
use velora_abi::{SockAddr, nr, sock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4(pub u32);

impl Ipv4 {
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Ipv4(u32::from_be_bytes([a, b, c, d]))
    }

    pub fn parse(s: &str) -> Option<Ipv4> {
        let mut out = [0u8; 4];
        let mut parts = s.split('.');
        for o in &mut out {
            let p = parts.next()?;
            if p.is_empty() || p.len() > 3 || !p.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            *o = p.parse().ok()?;
        }
        parts
            .next()
            .is_none()
            .then_some(Ipv4(u32::from_be_bytes(out)))
    }
}

impl core::fmt::Display for Ipv4 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let o = self.0.to_be_bytes();
        write!(f, "{}.{}.{}.{}", o[0], o[1], o[2], o[3])
    }
}

fn socket(kind: u64) -> Result<u64, Errno> {
    // SAFETY: no pointers.
    check(unsafe { syscall1(nr::SOCKET, kind) })
}

/// A TCP connection; closed (with FIN) when dropped.
pub struct TcpStream {
    fd: u64,
}

impl TcpStream {
    /// Connects and waits for the handshake.
    pub fn connect(ip: Ipv4, port: u16) -> Result<TcpStream, Errno> {
        let fd = socket(sock::TCP)?;
        let s = TcpStream { fd };
        // SAFETY: no pointers.
        check(unsafe { syscall3(nr::CONNECT, fd, ip.0 as u64, port as u64) })?;
        Ok(s)
    }

    pub fn fd(&self) -> u64 {
        self.fd
    }

    /// Received bytes; 0 when the peer has closed its side.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, Errno> {
        crate::io::read(self.fd, buf)
    }

    pub fn write_all(&mut self, data: &[u8]) -> Result<(), Errno> {
        crate::io::write_all(self.fd, data)
    }

    /// Ends our sending side (the peer sees the end of input); reading
    /// goes on.
    pub fn shutdown_write(&mut self) -> Result<(), Errno> {
        // SAFETY: no pointers.
        check(unsafe { syscall1(nr::SHUTDOWN, self.fd) }).map(|_| ())
    }

    /// Reads until the peer closes.
    pub fn read_to_end(&mut self) -> Result<Vec<u8>, Errno> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = self.read(&mut buf)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
        }
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        let _ = crate::fs::close(self.fd);
    }
}

/// A listening TCP socket.
pub struct TcpListener {
    fd: u64,
}

impl TcpListener {
    pub fn bind(port: u16) -> Result<TcpListener, Errno> {
        let fd = socket(sock::TCP)?;
        let l = TcpListener { fd };
        // SAFETY: no pointers.
        check(unsafe { syscall2(nr::BIND, fd, port as u64) })?;
        // SAFETY: no pointers.
        check(unsafe { syscall1(nr::LISTEN, fd) })?;
        Ok(l)
    }

    /// Waits for the next connection.
    pub fn accept(&self) -> Result<TcpStream, Errno> {
        // SAFETY: no pointers.
        let fd = check(unsafe { syscall1(nr::ACCEPT, self.fd) })?;
        Ok(TcpStream { fd })
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        let _ = crate::fs::close(self.fd);
    }
}

pub struct UdpSocket {
    fd: u64,
}

impl UdpSocket {
    /// A UDP socket on `port` (0 = any free port).
    pub fn bind(port: u16) -> Result<UdpSocket, Errno> {
        let fd = socket(sock::UDP)?;
        let s = UdpSocket { fd };
        // SAFETY: no pointers.
        check(unsafe { syscall2(nr::BIND, fd, port as u64) })?;
        Ok(s)
    }

    pub fn send_to(&self, data: &[u8], ip: Ipv4, port: u16) -> Result<usize, Errno> {
        // SAFETY: the buffer is valid for its length.
        check(unsafe {
            syscall6(
                nr::SENDTO,
                self.fd,
                data.as_ptr() as u64,
                data.len() as u64,
                ip.0 as u64,
                port as u64,
                0,
            )
        })
        .map(|n| n as usize)
    }

    /// Waits for a datagram; returns its length and sender.
    pub fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, Ipv4, u16), Errno> {
        let mut a = SockAddr::default();
        // SAFETY: the buffer and `a` are writable.
        let n = check(unsafe {
            syscall4(
                nr::RECVFROM,
                self.fd,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                &mut a as *mut SockAddr as u64,
            )
        })?;
        Ok((n as usize, Ipv4(a.ip), a.port))
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        let _ = crate::fs::close(self.fd);
    }
}

/// The network interface (flags bit 0: a card exists, bit 1: configured).
pub fn netinfo() -> Result<NetInfo, Errno> {
    let mut i = NetInfo::default();
    // SAFETY: `i` is writable and has the kernel's layout.
    check(unsafe { syscall1(nr::NETINFO, &mut i as *mut NetInfo as u64) })?;
    Ok(i)
}

/// Sends one echo request and waits; returns the round trip in µs.
pub fn ping(ip: Ipv4, seq: u16, timeout_ms: u64) -> Result<u64, Errno> {
    // SAFETY: no pointers.
    check(unsafe { syscall3(nr::PING, ip.0 as u64, seq as u64, timeout_ms) })
}

/// "aa:bb:cc:dd:ee:ff".
pub fn format_mac(m: &[u8; 6]) -> String {
    alloc::format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        m[0],
        m[1],
        m[2],
        m[3],
        m[4],
        m[5]
    )
}

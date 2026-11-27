//! Socket system calls (stage 10). Sockets are file descriptors; `read`
//! and `write` work on connected ones.

use super::uaccess;
use crate::fs::{FileRef, Handle, OpenFile};
use crate::sync::SpinLock;
use crate::{net, sched};
use alloc::sync::Arc;
use velora_abi::{NetInfo, SockAddr, errno, sock};
use velora_net::Ipv4;

fn as_bytes<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: plain `repr(C)` ABI data.
    unsafe { core::slice::from_raw_parts((v as *const T).cast::<u8>(), size_of::<T>()) }
}

fn install(id: u32) -> i64 {
    let f: FileRef = Arc::new(SpinLock::new(OpenFile {
        handle: Handle::Socket { id },
        offset: 0,
        readable: true,
        writable: true,
        append: false,
    }));
    match sched::fd_install(f) {
        Some(fd) => fd as i64,
        None => -errno::EMFILE,
    }
}

/// The socket id behind descriptor `fd`.
fn socket_of(fd: u64) -> Result<u32, i64> {
    let f = sched::fd_get(fd).ok_or(-errno::EBADF)?;
    match f.lock().handle {
        Handle::Socket { id } => Ok(id),
        _ => Err(-errno::ENOTSOCK),
    }
}

fn port(p: u64) -> Result<u16, i64> {
    u16::try_from(p).map_err(|_| -errno::EINVAL)
}

pub fn socket(kind: u64) -> i64 {
    let k = match kind {
        sock::UDP => net::Kind::Udp,
        sock::TCP => net::Kind::Tcp,
        _ => return -errno::EINVAL,
    };
    match net::socket(k) {
        Ok(id) => install(id),
        Err(e) => e,
    }
}

pub fn bind(fd: u64, p: u64) -> i64 {
    match socket_of(fd).and_then(|id| net::bind(id, port(p)?)) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub fn listen(fd: u64) -> i64 {
    match socket_of(fd).and_then(net::listen) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub fn connect(fd: u64, ip: u64, p: u64) -> i64 {
    let r = socket_of(fd).and_then(|id| net::connect(id, Ipv4(ip as u32), port(p)?));
    match r {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub fn shutdown(fd: u64) -> i64 {
    match socket_of(fd).and_then(net::shutdown) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub fn accept(fd: u64) -> i64 {
    match socket_of(fd).and_then(net::accept) {
        Ok(id) => install(id),
        Err(e) => e,
    }
}

pub fn sendto(id: u32, buf: u64, len: u64, ip: u64, p: u64) -> i64 {
    let Some(data) = uaccess::read_slice(buf, len.min(64 * 1024)) else {
        return -errno::EFAULT;
    };
    let to = if ip == 0 {
        None
    } else {
        match port(p) {
            Ok(p) => Some((Ipv4(ip as u32), p)),
            Err(e) => return e,
        }
    };
    // The slice points into user memory, which stays mapped while the
    // call blocks.
    let data = alloc::vec::Vec::from(data);
    match net::send(id, &data, to) {
        Ok(n) => n as i64,
        Err(e) => e,
    }
}

pub fn sendto_fd(fd: u64, buf: u64, len: u64, ip: u64, p: u64) -> i64 {
    match socket_of(fd) {
        Ok(id) => sendto(id, buf, len, ip, p),
        Err(e) => e,
    }
}

pub fn recvfrom(id: u32, buf: u64, len: u64, from: u64, timeout_ms: u64) -> i64 {
    match net::recv(id, len.min(64 * 1024) as usize, timeout_ms) {
        Ok((data, src)) => {
            if !uaccess::write(buf, &data) {
                return -errno::EFAULT;
            }
            if from != 0
                && let Some((ip, p)) = src
            {
                let a = SockAddr {
                    ip: ip.0,
                    port: p,
                    _pad: 0,
                };
                if !uaccess::write(from, as_bytes(&a)) {
                    return -errno::EFAULT;
                }
            }
            data.len() as i64
        }
        Err(e) => e,
    }
}

pub fn recvfrom_fd(fd: u64, buf: u64, len: u64, from: u64, timeout_ms: u64) -> i64 {
    match socket_of(fd) {
        Ok(id) => recvfrom(id, buf, len, from, timeout_ms),
        Err(e) => e,
    }
}

pub fn netinfo(out: u64) -> i64 {
    let mut info = NetInfo::default();
    if let Some((mac, cfg, rx, tx)) = net::info() {
        info.flags = 1;
        info.mac = mac;
        info.rx_frames = rx;
        info.tx_frames = tx;
        if let Some(c) = cfg {
            info.flags |= 2;
            info.ip = c.ip.0;
            info.netmask = c.netmask.0;
            info.gateway = c.gateway.0;
            info.dns = c.dns.0;
        }
    }
    if uaccess::write(out, as_bytes(&info)) {
        0
    } else {
        -errno::EFAULT
    }
}

pub fn ping(ip: u64, seq: u64, timeout_ms: u64) -> i64 {
    match net::ping(Ipv4(ip as u32), seq as u16, timeout_ms.clamp(1, 60_000)) {
        Ok(us) => us as i64,
        Err(e) => e,
    }
}

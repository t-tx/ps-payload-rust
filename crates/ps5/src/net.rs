//! L2 networking — safe TCP/UDP sockets over the BSD-socket FFI in
//! [`ps5_sys::net`], modeled on `std::net`.
//!
//! Addresses use [`core::net`] types. IPv4 is fully supported; passing an IPv6
//! address currently returns [`Error::InvalidInput`] (building `sockaddr_in6` is
//! a TODO). DNS resolution is available via [`lookup_host`].

use crate::error::{Error, Result};
use crate::util::{cstr, cvt_i32, cvt_ssize};
use alloc::format;
use alloc::vec::Vec;
use core::ffi::{c_int, c_void};
use core::mem;
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use ps5_sys::net as sys;

/// How to shut down a connection (mirrors `std::net::Shutdown`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shutdown {
    Read,
    Write,
    Both,
}

// --- address helpers -------------------------------------------------------

/// Require an IPv4 address (until IPv6 `sockaddr` building is implemented).
fn require_v4(addr: SocketAddr) -> Result<SocketAddrV4> {
    match addr {
        SocketAddr::V4(v4) => Ok(v4),
        SocketAddr::V6(_) => Err(Error::InvalidInput("IPv6 is not yet supported")),
    }
}

/// Build a `sockaddr_in` (network byte order) from a [`SocketAddrV4`].
fn to_sockaddr_in(addr: &SocketAddrV4) -> sys::sockaddr_in {
    // SAFETY: sockaddr_in is plain-old-data; zeroing then filling is valid.
    let mut sa: sys::sockaddr_in = unsafe { mem::zeroed() };
    sa.sin_len = mem::size_of::<sys::sockaddr_in>() as u8;
    sa.sin_family = sys::AF_INET as _;
    sa.sin_port = addr.port().to_be();
    sa.sin_addr.s_addr = u32::from(*addr.ip()).to_be();
    sa
}

/// Convert a populated `sockaddr_in` back into a [`SocketAddrV4`].
fn from_sockaddr_in(sa: &sys::sockaddr_in) -> SocketAddrV4 {
    let ip = Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr));
    SocketAddrV4::new(ip, u16::from_be(sa.sin_port))
}

const SOCKADDR_IN_LEN: sys::socklen_t = mem::size_of::<sys::sockaddr_in>() as sys::socklen_t;

// --- low-level fd helpers --------------------------------------------------

fn new_socket(ty: c_int) -> Result<c_int> {
    // SAFETY: a plain socket() syscall.
    cvt_i32(unsafe { sys::socket(sys::AF_INET as c_int, ty, 0) })
}

fn close_fd(fd: c_int) {
    // SAFETY: fd is owned by the caller; ignore close errors at drop time.
    unsafe {
        sys::close(fd);
    }
}

fn set_nonblocking_fd(fd: c_int, nonblocking: bool) -> Result<()> {
    // SAFETY: fcntl on an owned fd; F_GETFL takes no extra arg, F_SETFL takes one.
    let flags = cvt_i32(unsafe { sys::fcntl(fd, sys::F_GETFL as c_int) })?;
    let new = if nonblocking {
        flags | sys::O_NONBLOCK as c_int
    } else {
        flags & !(sys::O_NONBLOCK as c_int)
    };
    cvt_i32(unsafe { sys::fcntl(fd, sys::F_SETFL as c_int, new) })?;
    Ok(())
}

fn setsockopt_int(fd: c_int, level: c_int, name: c_int, value: c_int) -> Result<()> {
    // SAFETY: value outlives the call; len matches a c_int.
    cvt_i32(unsafe {
        sys::setsockopt(
            fd,
            level,
            name,
            &value as *const c_int as *const c_void,
            mem::size_of::<c_int>() as sys::socklen_t,
        )
    })?;
    Ok(())
}

fn recv_fd(fd: c_int, buf: &mut [u8]) -> Result<usize> {
    // SAFETY: buf is valid for buf.len() bytes.
    cvt_ssize(unsafe { sys::recv(fd, buf.as_mut_ptr() as *mut c_void, buf.len(), 0) })
}

fn send_fd(fd: c_int, buf: &[u8]) -> Result<usize> {
    // SAFETY: buf is valid for buf.len() bytes.
    cvt_ssize(unsafe { sys::send(fd, buf.as_ptr() as *const c_void, buf.len(), 0) })
}

// --- TcpStream -------------------------------------------------------------

/// A connected TCP stream (owns its fd; closed on drop).
pub struct TcpStream {
    fd: c_int,
}

impl TcpStream {
    /// Open a TCP connection to `addr`.
    pub fn connect(addr: SocketAddr) -> Result<TcpStream> {
        let v4 = require_v4(addr)?;
        let sa = to_sockaddr_in(&v4);
        let stream = TcpStream {
            fd: new_socket(sys::SOCK_STREAM as c_int)?,
        };
        // SAFETY: sa is a valid sockaddr_in for SOCKADDR_IN_LEN bytes.
        cvt_i32(unsafe {
            sys::connect(
                stream.fd,
                &sa as *const sys::sockaddr_in as *const sys::sockaddr,
                SOCKADDR_IN_LEN,
            )
        })?;
        Ok(stream)
    }

    /// Read into `buf`, returning the number of bytes read (0 = EOF).
    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        recv_fd(self.fd, buf)
    }

    /// Write `buf`, returning the number of bytes accepted.
    pub fn write(&self, buf: &[u8]) -> Result<usize> {
        send_fd(self.fd, buf)
    }

    /// Write the entire buffer, retrying short writes and `EINTR`.
    pub fn write_all(&self, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.write(buf) {
                Ok(0) => return Err(Error::last_os()),
                Ok(n) => buf = &buf[n..],
                Err(e) if e.errno().is_some_and(|n| n.interrupted()) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Set `TCP_NODELAY` (disable Nagle's algorithm).
    pub fn set_nodelay(&self, nodelay: bool) -> Result<()> {
        setsockopt_int(
            self.fd,
            sys::IPPROTO_TCP as c_int,
            sys::TCP_NODELAY as c_int,
            nodelay as c_int,
        )
    }

    /// Put the socket into (non)blocking mode.
    pub fn set_nonblocking(&self, nonblocking: bool) -> Result<()> {
        set_nonblocking_fd(self.fd, nonblocking)
    }

    /// Shut down the read, write, or both halves of the connection.
    pub fn shutdown(&self, how: Shutdown) -> Result<()> {
        let how = match how {
            Shutdown::Read => sys::SHUT_RD,
            Shutdown::Write => sys::SHUT_WR,
            Shutdown::Both => sys::SHUT_RDWR,
        } as c_int;
        cvt_i32(unsafe { sys::shutdown(self.fd, how) })?;
        Ok(())
    }

    /// The underlying file descriptor.
    pub fn as_raw_fd(&self) -> i32 {
        self.fd
    }

    /// Adopt an existing fd. The caller transfers ownership.
    ///
    /// # Safety
    /// `fd` must be an open socket not owned elsewhere.
    pub unsafe fn from_raw_fd(fd: i32) -> TcpStream {
        TcpStream { fd }
    }

    /// Consume the stream and return its fd without closing it.
    pub fn into_raw_fd(self) -> i32 {
        let fd = self.fd;
        mem::forget(self);
        fd
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        close_fd(self.fd);
    }
}

// --- TcpListener -----------------------------------------------------------

/// A TCP socket listening for connections (owns its fd; closed on drop).
pub struct TcpListener {
    fd: c_int,
}

impl TcpListener {
    /// Bind to `addr` and start listening (backlog 128, `SO_REUSEADDR` set).
    pub fn bind(addr: SocketAddr) -> Result<TcpListener> {
        let v4 = require_v4(addr)?;
        let sa = to_sockaddr_in(&v4);
        let listener = TcpListener {
            fd: new_socket(sys::SOCK_STREAM as c_int)?,
        };
        setsockopt_int(
            listener.fd,
            sys::SOL_SOCKET as c_int,
            sys::SO_REUSEADDR as c_int,
            1,
        )?;
        // SAFETY: sa is a valid sockaddr_in.
        cvt_i32(unsafe {
            sys::bind(
                listener.fd,
                &sa as *const sys::sockaddr_in as *const sys::sockaddr,
                SOCKADDR_IN_LEN,
            )
        })?;
        cvt_i32(unsafe { sys::listen(listener.fd, 128) })?;
        Ok(listener)
    }

    /// Accept one connection, returning the stream and the peer's address.
    pub fn accept(&self) -> Result<(TcpStream, SocketAddr)> {
        let mut sa: sys::sockaddr_in = unsafe { mem::zeroed() };
        let mut len = SOCKADDR_IN_LEN;
        // SAFETY: sa/len are valid out-params sized for a sockaddr_in.
        let fd = cvt_i32(unsafe {
            sys::accept(
                self.fd,
                &mut sa as *mut sys::sockaddr_in as *mut sys::sockaddr,
                &mut len,
            )
        })?;
        let peer = SocketAddr::V4(from_sockaddr_in(&sa));
        Ok((TcpStream { fd }, peer))
    }

    /// An iterator that yields an incoming connection per `next()`.
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }

    /// Put the listening socket into (non)blocking mode.
    pub fn set_nonblocking(&self, nonblocking: bool) -> Result<()> {
        set_nonblocking_fd(self.fd, nonblocking)
    }

    pub fn as_raw_fd(&self) -> i32 {
        self.fd
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        close_fd(self.fd);
    }
}

/// Iterator over incoming connections, created by [`TcpListener::incoming`].
pub struct Incoming<'a> {
    listener: &'a TcpListener,
}

impl Iterator for Incoming<'_> {
    type Item = Result<TcpStream>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.listener.accept().map(|(stream, _)| stream))
    }
}

// --- UdpSocket -------------------------------------------------------------

/// A UDP socket (owns its fd; closed on drop).
pub struct UdpSocket {
    fd: c_int,
}

impl UdpSocket {
    /// Bind a UDP socket to `addr`.
    pub fn bind(addr: SocketAddr) -> Result<UdpSocket> {
        let v4 = require_v4(addr)?;
        let sa = to_sockaddr_in(&v4);
        let sock = UdpSocket {
            fd: new_socket(sys::SOCK_DGRAM as c_int)?,
        };
        // SAFETY: sa is a valid sockaddr_in.
        cvt_i32(unsafe {
            sys::bind(
                sock.fd,
                &sa as *const sys::sockaddr_in as *const sys::sockaddr,
                SOCKADDR_IN_LEN,
            )
        })?;
        Ok(sock)
    }

    /// Send `buf` to `addr`, returning the number of bytes sent.
    pub fn send_to(&self, buf: &[u8], addr: SocketAddr) -> Result<usize> {
        let v4 = require_v4(addr)?;
        let sa = to_sockaddr_in(&v4);
        // SAFETY: buf valid for buf.len(); sa valid for SOCKADDR_IN_LEN.
        cvt_ssize(unsafe {
            sys::sendto(
                self.fd,
                buf.as_ptr() as *const c_void,
                buf.len(),
                0,
                &sa as *const sys::sockaddr_in as *const sys::sockaddr,
                SOCKADDR_IN_LEN,
            )
        })
    }

    /// Receive a datagram, returning the byte count and the sender's address.
    pub fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        let mut sa: sys::sockaddr_in = unsafe { mem::zeroed() };
        let mut len = SOCKADDR_IN_LEN;
        // SAFETY: buf valid for buf.len(); sa/len valid out-params.
        let n = cvt_ssize(unsafe {
            sys::recvfrom(
                self.fd,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                0,
                &mut sa as *mut sys::sockaddr_in as *mut sys::sockaddr,
                &mut len,
            )
        })?;
        Ok((n, SocketAddr::V4(from_sockaddr_in(&sa))))
    }

    /// Set the default peer for [`send`](Self::send)/[`recv`](Self::recv).
    pub fn connect(&self, addr: SocketAddr) -> Result<()> {
        let v4 = require_v4(addr)?;
        let sa = to_sockaddr_in(&v4);
        // SAFETY: sa is a valid sockaddr_in.
        cvt_i32(unsafe {
            sys::connect(
                self.fd,
                &sa as *const sys::sockaddr_in as *const sys::sockaddr,
                SOCKADDR_IN_LEN,
            )
        })?;
        Ok(())
    }

    /// Send to the connected peer.
    pub fn send(&self, buf: &[u8]) -> Result<usize> {
        send_fd(self.fd, buf)
    }

    /// Receive from the connected peer.
    pub fn recv(&self, buf: &mut [u8]) -> Result<usize> {
        recv_fd(self.fd, buf)
    }

    /// Put the socket into (non)blocking mode.
    pub fn set_nonblocking(&self, nonblocking: bool) -> Result<()> {
        set_nonblocking_fd(self.fd, nonblocking)
    }

    pub fn as_raw_fd(&self) -> i32 {
        self.fd
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        close_fd(self.fd);
    }
}

// --- DNS -------------------------------------------------------------------

/// Resolve `host`:`port` to a list of socket addresses via `getaddrinfo`
/// (IPv4 results only, for now).
pub fn lookup_host(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let chost = cstr(host)?;
    let service = cstr(&format!("{port}"))?;

    let mut hints: sys::addrinfo = unsafe { mem::zeroed() };
    hints.ai_family = sys::AF_INET as c_int;
    hints.ai_socktype = sys::SOCK_STREAM as c_int;

    let mut res: *mut sys::addrinfo = core::ptr::null_mut();
    // SAFETY: chost/service are valid C strings; hints/res are valid pointers.
    let rc = unsafe { sys::getaddrinfo(chost.as_ptr(), service.as_ptr(), &hints, &mut res) };
    if rc != 0 {
        return Err(Error::gai(rc));
    }

    let mut out = Vec::new();
    let mut cur = res;
    while !cur.is_null() {
        // SAFETY: cur points to a valid addrinfo from getaddrinfo.
        let ai = unsafe { &*cur };
        if ai.ai_family == sys::AF_INET as c_int && !ai.ai_addr.is_null() {
            // SAFETY: an AF_INET ai_addr points to a sockaddr_in.
            let sin = unsafe { &*(ai.ai_addr as *const sys::sockaddr_in) };
            out.push(SocketAddr::V4(from_sockaddr_in(sin)));
        }
        cur = ai.ai_next;
    }
    // SAFETY: res was allocated by getaddrinfo and not freed elsewhere.
    unsafe { sys::freeaddrinfo(res) };
    Ok(out)
}

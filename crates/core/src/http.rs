//! A minimal HTTP/1.1 server, thread-per-connection, over [`ps5::net`].
//!
//! There is no Sony HTTP-server library, so the protocol is implemented here in
//! plain Rust on top of [`ps5::net::TcpListener`]. Each accepted connection is
//! handled on its own thread (spawned via [`ps5::thread::Builder`] with a small,
//! bounded stack), reads one request, runs the handler, writes the response, and
//! closes — `Connection: close`, no keep-alive (kept deliberately simple).
//!
//! ```ignore
//! use ps5_core::http::{Server, Response};
//! use core::net::{SocketAddr, SocketAddrV4, Ipv4Addr};
//!
//! let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 8080));
//! Server::new().serve(addr, |req| {
//!     match req.path() {
//!         "/" => Response::ok().text("hello from the PS5"),
//!         _ => Response::new(404).text("not found"),
//!     }
//! })?;
//! ```
//!
//! Scope/limits: HTTP/1.1 only; one request per connection; `Content-Length`
//! bodies only (no chunked request bodies); concurrency is unbounded (one thread
//! per live connection) — see the notes on [`Server`].

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::net::SocketAddr;

use ps5::net::{Shutdown, TcpListener, TcpStream};
use ps5::{thread, Error, Result};

/// Default per-connection thread stack (bytes). Small but ample for parsing a
/// request and running a typical handler; cap on memory per live connection.
const DEFAULT_STACK_SIZE: usize = 128 * 1024;
/// Default cap on the request head (request line + headers), in bytes.
const DEFAULT_MAX_HEADER_BYTES: usize = 64 * 1024;
/// Default cap on the request body, in bytes.
const DEFAULT_MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Read granularity when pulling bytes off the socket.
const READ_CHUNK: usize = 4096;

/// Size limits applied while parsing a request.
#[derive(Clone, Copy, Debug)]
struct Limits {
    max_header_bytes: usize,
    max_body_bytes: usize,
}

// ============================================================================
// Request
// ============================================================================

/// A parsed HTTP/1.1 request.
#[derive(Clone, Debug)]
pub struct Request {
    /// Request method, uppercased as received (e.g. `"GET"`).
    pub method: String,
    /// Raw request target, including any query string (e.g. `"/a?b=1"`).
    pub target: String,
    /// HTTP version token (e.g. `"HTTP/1.1"`).
    pub version: String,
    /// Header fields, in received order, names as received.
    pub headers: Vec<(String, String)>,
    /// Request body (empty unless a `Content-Length` body was present).
    pub body: Vec<u8>,
}

impl Request {
    /// The path portion of the target, with any `?query` stripped.
    pub fn path(&self) -> &str {
        match self.target.split_once('?') {
            Some((path, _)) => path,
            None => &self.target,
        }
    }

    /// The raw query string (everything after the first `?`), if any.
    pub fn query(&self) -> Option<&str> {
        self.target.split_once('?').map(|(_, q)| q)
    }

    /// Look up a header value by case-insensitive name (first match).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

// ============================================================================
// Response
// ============================================================================

/// An HTTP response to send back to the client.
///
/// `Content-Length` and `Connection: close` are added automatically on encode;
/// any header you set with those names is ignored.
#[derive(Clone, Debug)]
pub struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    /// A response with the given status code, no headers, and an empty body.
    pub fn new(status: u16) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// A `200 OK` response.
    pub fn ok() -> Response {
        Response::new(200)
    }

    /// Set the status code.
    pub fn status(mut self, status: u16) -> Response {
        self.status = status;
        self
    }

    /// Append a header field.
    pub fn header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Set the raw body bytes (does not set a content type).
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Response {
        self.body = body.into();
        self
    }

    /// Set a `text/plain; charset=utf-8` body.
    pub fn text(self, body: &str) -> Response {
        self.with_content_type("text/plain; charset=utf-8")
            .body(body.as_bytes().to_vec())
    }

    /// Set a `text/html; charset=utf-8` body.
    pub fn html(self, body: &str) -> Response {
        self.with_content_type("text/html; charset=utf-8")
            .body(body.as_bytes().to_vec())
    }

    /// Set an `application/json` body (caller supplies the JSON text).
    pub fn json(self, body: &str) -> Response {
        self.with_content_type("application/json")
            .body(body.as_bytes().to_vec())
    }

    fn with_content_type(self, ct: &str) -> Response {
        self.header("Content-Type", ct)
    }

    /// Serialize the response to bytes ready to write to the socket.
    fn encode(&self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\n",
            self.status,
            reason_phrase(self.status)
        );
        for (k, v) in &self.headers {
            // We emit Content-Length / Connection ourselves; skip duplicates.
            if k.eq_ignore_ascii_case("content-length") || k.eq_ignore_ascii_case("connection") {
                continue;
            }
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        head.push_str("Connection: close\r\n\r\n");

        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

/// Reason phrase for common status codes (defaults to `"OK"`).
fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

// ============================================================================
// Server
// ============================================================================

/// A thread-per-connection HTTP/1.1 server.
///
/// Build with [`Server::new`] (optionally tuning the per-connection stack and
/// size limits), then call [`serve`](Server::serve) with a handler. `serve`
/// loops forever, accepting connections and spawning a detached thread per
/// connection.
///
/// Concurrency is **unbounded** (one live thread per open connection); each
/// thread uses [`stack_size`](Server::stack_size) bytes of stack, so peak memory
/// is roughly `live_connections × stack_size`. For a control/status surface this
/// is fine; add a connection cap if you expect bursts of clients.
#[derive(Clone, Debug)]
pub struct Server {
    stack_size: usize,
    limits: Limits,
}

impl Default for Server {
    fn default() -> Self {
        Server::new()
    }
}

impl Server {
    /// A server with default settings (128 KiB per-connection stack, 64 KiB
    /// header cap, 8 MiB body cap).
    pub fn new() -> Server {
        Server {
            stack_size: DEFAULT_STACK_SIZE,
            limits: Limits {
                max_header_bytes: DEFAULT_MAX_HEADER_BYTES,
                max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            },
        }
    }

    /// Set the stack size (bytes) for each connection's handler thread.
    pub fn stack_size(mut self, bytes: usize) -> Server {
        self.stack_size = bytes;
        self
    }

    /// Set the maximum accepted request-head size (request line + headers).
    pub fn max_header_bytes(mut self, bytes: usize) -> Server {
        self.limits.max_header_bytes = bytes;
        self
    }

    /// Set the maximum accepted request-body size.
    pub fn max_body_bytes(mut self, bytes: usize) -> Server {
        self.limits.max_body_bytes = bytes;
        self
    }

    /// Bind `addr` and serve connections forever, dispatching each request to
    /// `handler`.
    ///
    /// `handler` is shared across all connection threads, so it must be `Send +
    /// Sync`. Per-connection errors (parse failures, dropped sockets) are handled
    /// locally — a malformed request gets a `400` and the connection closes —
    /// and never stop the accept loop. Only a failure of the listener itself
    /// returns an error.
    pub fn serve<H>(&self, addr: SocketAddr, handler: H) -> Result<()>
    where
        H: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        let listener = TcpListener::bind(addr)?;
        let handler = Arc::new(handler);

        loop {
            let stream = match listener.accept() {
                Ok((stream, _peer)) => stream,
                // A signal interrupted accept(); just try again.
                Err(e) if e.errno().is_some_and(|n| n.interrupted()) => continue,
                // The listening socket itself failed — propagate.
                Err(e) => return Err(e),
            };

            let handler = Arc::clone(&handler);
            let limits = self.limits;
            // Detached: we never join. If spawn fails (out of resources), the
            // closure — and with it `stream` — is dropped, closing the socket.
            let _ = thread::Builder::new()
                .stack_size(self.stack_size)
                .spawn(move || serve_connection(&stream, handler.as_ref(), limits));
        }
    }
}

/// Handle exactly one connection: read a request, run the handler, write the
/// response, then close. Errors are contained to this connection.
fn serve_connection<H>(stream: &TcpStream, handler: &H, limits: Limits)
where
    H: Fn(&Request) -> Response,
{
    let response = match read_request(stream, limits) {
        Ok(Some(req)) => handler(&req),
        // Client closed before sending anything — nothing to answer.
        Ok(None) => {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        // Malformed/oversized request: best-effort 400, then close.
        Err(_) => Response::new(400).text("Bad Request"),
    };

    let _ = write_response(stream, &response);
    let _ = stream.shutdown(Shutdown::Both);
}

/// Write a full response to the socket.
fn write_response(stream: &TcpStream, response: &Response) -> Result<()> {
    stream.write_all(&response.encode())
}

/// Read and parse one HTTP/1.1 request from `stream`.
///
/// Returns `Ok(None)` if the peer closed the connection before sending any
/// bytes, `Ok(Some(req))` on a complete request, or `Err` on a malformed or
/// oversized request (or a socket error).
fn read_request(stream: &TcpStream, limits: Limits) -> Result<Option<Request>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; READ_CHUNK];

    // 1. Read until the end-of-headers marker (CRLF CRLF).
    let head_end = loop {
        if let Some(pos) = find_double_crlf(&buf) {
            break pos;
        }
        if buf.len() > limits.max_header_bytes {
            return Err(Error::InvalidInput("request head exceeds limit"));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            // EOF before full headers.
            return if buf.is_empty() {
                Ok(None) // peer just closed; not an error
            } else {
                Err(Error::InvalidInput("connection closed mid-request"))
            };
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    // 2. Parse the request line + headers from the head.
    let head = core::str::from_utf8(&buf[..head_end])
        .map_err(|_| Error::InvalidInput("request head is not valid UTF-8"))?;
    let mut lines = head.split("\r\n");

    let request_line = lines
        .next()
        .ok_or(Error::InvalidInput("missing request line"))?;
    let mut parts = request_line.split(' ');
    let method = parts
        .next()
        .ok_or(Error::InvalidInput("missing method"))?
        .to_string();
    let target = parts
        .next()
        .ok_or(Error::InvalidInput("missing request target"))?
        .to_string();
    let version = parts
        .next()
        .ok_or(Error::InvalidInput("missing HTTP version"))?
        .to_string();

    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(Error::InvalidInput("malformed header line"))?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }

    // 3. Read the body, if a (sane) Content-Length says so.
    let content_length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > limits.max_body_bytes {
        return Err(Error::InvalidInput("request body exceeds limit"));
    }

    // Bytes already read past the head are the start of the body.
    let body_start = head_end + 4; // skip the CRLF CRLF
    let mut body: Vec<u8> = buf[body_start..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break; // short body; hand over what we got
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Ok(Some(Request {
        method,
        target,
        version,
        headers,
        body,
    }))
}

/// Find the byte offset of the first `\r\n\r\n` in `buf`, if present.
fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

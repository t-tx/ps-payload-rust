//! HTTP(S) client over the Sony `SceHttp2` / `SceSsl` / `SceNet` stack.
//!
//! This is a small, `std`-like client layered on the hand-written FFI in
//! [`ps5_sys::sce`]. A [`Client`] owns the full SCE init chain (Net pool → SSL
//! context → HTTP/2 library context → request template) and tears it down in
//! reverse on [`Drop`], so callers never have to manage the raw handles.
//!
//! ```ignore
//! let client = ps5::http::Client::new("my-payload/1.0")?;
//! let resp = client.get("http://192.168.1.1")?;
//! if resp.status() == 200 {
//!     // resp.into_body() is the raw response bytes
//! }
//! ```
//!
//! The init / teardown order and the pool-size constants mirror
//! `sdk/samples/http2_get/main.c` exactly.
//!
//! **Linkage:** requires `-lSceHttp2 -lSceSsl` (and the default-linked
//! `-lSceNet`), emitted by the `ps5-sys` cargo feature `http`.

use crate::error::{Error, Result};
use crate::util::cstr;
use alloc::vec::Vec;
use core::ffi::{c_int, c_void};

use ps5_sys::sce::{
    sceHttp2CreateRequestWithURL, sceHttp2CreateTemplate, sceHttp2DeleteRequest,
    sceHttp2DeleteTemplate, sceHttp2GetStatusCode, sceHttp2Init, sceHttp2ReadData,
    sceHttp2SendRequest, sceHttp2Term, sceNetInit, sceNetPoolCreate, sceNetPoolDestroy, sceSslInit,
    sceSslTerm,
};

/// Size of the `SceNet` memory pool, matching `sdk/samples/http2_get/main.c`.
const NET_POOL_SIZE: c_int = 32 * 1024;
/// Size of the `SceSsl` memory pool, matching the sample.
const SSL_POOL_SIZE: usize = 256 * 1024;
/// Size of the `SceHttp2` memory pool, matching the sample.
const HTTP2_POOL_SIZE: usize = 256 * 1024;
/// Maximum number of concurrent HTTP/2 connections, matching the sample.
const HTTP2_MAX_CONNECTIONS: c_int = 1;
/// Follow up to this many redirects automatically, matching the sample.
const HTTP2_AUTO_REDIRECT: c_int = 3;
/// Enable HTTP/2 (vs. HTTP/1.1), matching the sample.
const HTTP2_ENABLE: c_int = 1;
/// Read-loop chunk size for draining the response body.
const READ_CHUNK: usize = 0x1000;

/// A reusable HTTP(S) client backed by the SCE HTTP/2 stack.
///
/// Owns the full SCE init chain; the handles are released in reverse order on
/// [`Drop`]. Construct one with [`Client::new`] and issue requests with
/// [`Client::get`] / [`Client::request`].
pub struct Client {
    /// `SceNet` memory-pool id from `sceNetPoolCreate`.
    net_mem_id: c_int,
    /// `SceSsl` context id from `sceSslInit`.
    ssl_ctx_id: c_int,
    /// `SceHttp2` library-context id from `sceHttp2Init`.
    lib_ctx_id: c_int,
    /// Request-template id from `sceHttp2CreateTemplate`.
    tmpl_id: c_int,
}

impl Client {
    /// Initialize the SCE Net → SSL → HTTP/2 stack and create a request
    /// template tagged with `user_agent`.
    ///
    /// On any failure mid-init, every resource created so far is unwound (in
    /// reverse order) before returning the error, so no SCE handle leaks.
    pub fn new(user_agent: &str) -> Result<Client> {
        // CString must outlive the FFI call that borrows its pointer.
        let agent = cstr(user_agent)?;

        // SAFETY: no arguments; `sceNetInit` is idempotent and returns 0 on
        // success, a negative SCE code otherwise.
        let rc = unsafe { sceNetInit() };
        if rc != 0 {
            return Err(Error::sce("sceNetInit", rc));
        }

        // SAFETY: `c"..."`-style ptr is valid for the call; size/flags are
        // plain scalars. Returns a positive pool id or a negative code.
        let net_mem_id = unsafe { sceNetPoolCreate(c"ps5_http".as_ptr(), NET_POOL_SIZE, 0) };
        if net_mem_id < 0 {
            // Nothing to unwind for sceNetInit (it has no explicit teardown).
            return Err(Error::sce("sceNetPoolCreate", net_mem_id));
        }

        // SAFETY: plain scalar argument. Returns a positive ctx id or < 0.
        let ssl_ctx_id = unsafe { sceSslInit(SSL_POOL_SIZE) };
        if ssl_ctx_id < 0 {
            // SAFETY: `net_mem_id` is a live pool id we just created.
            unsafe { sceNetPoolDestroy(net_mem_id) };
            return Err(Error::sce("sceSslInit", ssl_ctx_id));
        }

        // SAFETY: `net_mem_id`/`ssl_ctx_id` are live ids; sizes are scalars.
        // Returns a positive library-context id or < 0.
        let lib_ctx_id = unsafe {
            sceHttp2Init(
                net_mem_id,
                ssl_ctx_id,
                HTTP2_POOL_SIZE,
                HTTP2_MAX_CONNECTIONS,
            )
        };
        if lib_ctx_id < 0 {
            // SAFETY: both ids are live; tear down in reverse creation order.
            unsafe {
                sceSslTerm(ssl_ctx_id);
                sceNetPoolDestroy(net_mem_id);
            }
            return Err(Error::sce("sceHttp2Init", lib_ctx_id));
        }

        // SAFETY: `lib_ctx_id` is live; `agent` outlives this call. Returns a
        // positive template id or < 0.
        let tmpl_id = unsafe {
            sceHttp2CreateTemplate(
                lib_ctx_id,
                agent.as_ptr(),
                HTTP2_AUTO_REDIRECT,
                HTTP2_ENABLE,
            )
        };
        if tmpl_id < 0 {
            // SAFETY: all three ids are live; tear down in reverse order.
            unsafe {
                sceHttp2Term(lib_ctx_id);
                sceSslTerm(ssl_ctx_id);
                sceNetPoolDestroy(net_mem_id);
            }
            return Err(Error::sce("sceHttp2CreateTemplate", tmpl_id));
        }

        Ok(Client {
            net_mem_id,
            ssl_ctx_id,
            lib_ctx_id,
            tmpl_id,
        })
    }

    /// Perform a `GET` request against `url` and return the full response.
    pub fn get(&self, url: &str) -> Result<Response> {
        self.request("GET", url, &[])
    }

    /// Perform an arbitrary `method` request against `url`, sending `body` as
    /// the request payload when it is non-empty.
    ///
    /// Reads the status code, then drains the entire response body into a
    /// [`Vec`]. The underlying SCE request is always deleted before returning,
    /// even on error.
    pub fn request(&self, method: &str, url: &str, body: &[u8]) -> Result<Response> {
        // Both CStrings must outlive the create call that borrows their ptrs.
        let method_c = cstr(method)?;
        let url_c = cstr(url)?;

        // SAFETY: `self.tmpl_id` is a live template; the two CStrings outlive
        // this call. Returns a positive request id or a negative SCE code.
        let req_id = unsafe {
            sceHttp2CreateRequestWithURL(
                self.tmpl_id,
                method_c.as_ptr(),
                url_c.as_ptr(),
                body.len() as u64,
            )
        };
        if req_id < 0 {
            return Err(Error::sce("sceHttp2CreateRequestWithURL", req_id));
        }

        // Run the request/read sequence, then delete the request regardless of
        // outcome so the handle never leaks.
        let result = self.run_request(req_id, body);

        // SAFETY: `req_id` is the live request we just created.
        let del = unsafe { sceHttp2DeleteRequest(req_id) };

        match result {
            Ok(resp) => {
                if del != 0 {
                    Err(Error::sce("sceHttp2DeleteRequest", del))
                } else {
                    Ok(resp)
                }
            }
            // Preserve the original (more meaningful) error over a delete error.
            Err(e) => Err(e),
        }
    }

    /// Send `req_id` (with optional `body`), read the status, and drain the
    /// body. Does **not** delete the request — the caller owns that.
    fn run_request(&self, req_id: c_int, body: &[u8]) -> Result<Response> {
        // Send the request, attaching the body only when present (the sample
        // passes NULL/0 for a bodyless GET).
        let (data, size): (*const c_void, usize) = if body.is_empty() {
            (core::ptr::null(), 0)
        } else {
            (body.as_ptr().cast(), body.len())
        };

        // SAFETY: `req_id` is live; `data`/`size` describe `body` (or null/0),
        // both valid for the duration of this call. Returns 0 on success.
        let rc = unsafe { sceHttp2SendRequest(req_id, data, size) };
        if rc != 0 {
            return Err(Error::sce("sceHttp2SendRequest", rc));
        }

        // SAFETY: `req_id` is live; `&mut status` is a valid, aligned out-ptr.
        let mut status: c_int = 0;
        let rc = unsafe { sceHttp2GetStatusCode(req_id, &mut status) };
        if rc != 0 {
            return Err(Error::sce("sceHttp2GetStatusCode", rc));
        }

        // Drain the body: positive => bytes read, 0 => EOF, negative => error.
        let mut body_buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; READ_CHUNK];
        loop {
            // SAFETY: `req_id` is live; `chunk` is a valid buffer of `len`
            // bytes that outlives the call. The returned count never exceeds
            // `len`, so the subsequent slice is in-bounds.
            let n = unsafe {
                sceHttp2ReadData(req_id, chunk.as_mut_ptr().cast::<c_void>(), chunk.len())
            };
            if n < 0 {
                return Err(Error::sce("sceHttp2ReadData", n));
            }
            if n == 0 {
                break;
            }
            body_buf.extend_from_slice(&chunk[..n as usize]);
        }

        Ok(Response {
            status: status as u16,
            body: body_buf,
        })
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Tear down in strict reverse order of creation. Each id was validated
        // (>= 0) at construction, so each call below operates on a live handle.
        // SAFETY: all four handles are live for the lifetime of the `Client`.
        unsafe {
            sceHttp2DeleteTemplate(self.tmpl_id);
            sceHttp2Term(self.lib_ctx_id);
            sceSslTerm(self.ssl_ctx_id);
            sceNetPoolDestroy(self.net_mem_id);
        }
    }
}

/// A completed HTTP response: the status code plus the full body bytes.
pub struct Response {
    /// The HTTP status code (e.g. `200`).
    pub status: u16,
    /// The raw response body.
    pub body: Vec<u8>,
}

impl Response {
    /// The HTTP status code.
    #[inline]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Consume the response and return its body bytes.
    #[inline]
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }
}

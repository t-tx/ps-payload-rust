//! Hand-written FFI for the Sony SCE HTTP/2 + TLS + Net stack.
//!
//! These libraries are **symbol-only**: the SDK exports the symbols (via
//! `sce_stubs/libSceHttp2.c`, `libSceSsl.c`, `libSceNet.c`) but ships **no
//! headers**, so bindgen cannot generate them. The prototypes below are the
//! known-good signatures used verbatim by `sdk/samples/http2_get/main.c` — the
//! only SDK-attested signatures for these symbols.
//!
//! Linkage (requested by `build.rs` under feature `http`): `-lSceHttp2 -lSceSsl`
//! (opt-in) plus `-lSceNet` (default-linked). Init order is **Net → Ssl →
//! Http2** and teardown reverses it; see `docs/SDK_API_INVENTORY.md` §4.
//!
//! Add further `sceHttp2*` / `sceSsl*` / `sceNet*` entry points here as L2 needs
//! them, resolving argument types from PS5 libdoc / community headers (there are
//! 55 `sceHttp2*`, 54 `sceSsl*` and 220 `sceNet*` symbols in total).

use core::ffi::{c_char, c_int, c_void};

unsafe extern "C" {
    // --- libSceNet (default-linked) -------------------------------------
    /// Initialize the SceNet library. Returns 0 on success.
    pub fn sceNetInit() -> c_int;
    /// Create a network memory pool; returns a positive memory-pool id or < 0.
    pub fn sceNetPoolCreate(name: *const c_char, size: c_int, flags: c_int) -> c_int;
    /// Destroy a memory pool previously created with [`sceNetPoolCreate`].
    pub fn sceNetPoolDestroy(memid: c_int) -> c_int;

    // --- libSceSsl (-lSceSsl) -------------------------------------------
    /// Initialize the SSL/TLS library; returns a positive ssl-context id or < 0.
    pub fn sceSslInit(pool_size: usize) -> c_int;
    /// Terminate the SSL/TLS context.
    pub fn sceSslTerm(ssl_ctx_id: c_int) -> c_int;

    // --- libSceHttp2 (-lSceHttp2) ---------------------------------------
    /// Initialize the HTTP/2 library; returns a positive library-context id or < 0.
    /// `net_mem_id` and `ssl_ctx_id` come from [`sceNetPoolCreate`]/[`sceSslInit`].
    pub fn sceHttp2Init(
        net_mem_id: c_int,
        ssl_ctx_id: c_int,
        pool_size: usize,
        max_connections: c_int,
    ) -> c_int;
    /// Terminate the HTTP/2 library context.
    pub fn sceHttp2Term(lib_ctx_id: c_int) -> c_int;

    /// Create a request template; returns a positive template id or < 0.
    pub fn sceHttp2CreateTemplate(
        lib_ctx_id: c_int,
        user_agent: *const c_char,
        auto_redirect: c_int,
        http2: c_int,
    ) -> c_int;
    /// Delete a template created with [`sceHttp2CreateTemplate`].
    pub fn sceHttp2DeleteTemplate(tmpl_id: c_int) -> c_int;

    /// Create a request bound to `url`; returns a positive request id or < 0.
    pub fn sceHttp2CreateRequestWithURL(
        tmpl_id: c_int,
        method: *const c_char,
        url: *const c_char,
        content_length: u64,
    ) -> c_int;
    /// Delete a request created with [`sceHttp2CreateRequestWithURL`].
    pub fn sceHttp2DeleteRequest(req_id: c_int) -> c_int;

    /// Send the request, optionally with a request body (`data`/`size`).
    pub fn sceHttp2SendRequest(req_id: c_int, data: *const c_void, size: usize) -> c_int;
    /// Read the HTTP status code into `*status_code`.
    pub fn sceHttp2GetStatusCode(req_id: c_int, status_code: *mut c_int) -> c_int;
    /// Read up to `size` bytes of the response body; returns bytes read or < 0.
    pub fn sceHttp2ReadData(req_id: c_int, data: *mut c_void, size: usize) -> c_int;
}

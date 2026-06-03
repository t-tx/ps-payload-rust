//! L2 — safe, idiomatic Rust wrappers over the ps5-payload-sdk FFI ([`ps5_sys`]).
//!
//! This is the second layer of the payload. It turns the raw, `unsafe` L1
//! bindings into ergonomic, memory-safe APIs (RAII handles, `Result`, slices,
//! `&str` paths), modeled loosely on `std`. Higher layers (L3 business core, L4
//! application) build on this and should not need `unsafe`.
//!
//! `no_std` + `alloc`: the payload has no `std`, but a global allocator (backed
//! by the SDK's `malloc`) is provided by the final binary (L4), so L2 may use
//! `alloc` collections (`Vec`, `String`, `Box`).
//!
//! ## Modules (each gated by a cargo feature, all on by default)
//! - [`fs`]     — files & directories (`File`, `read`, `write`, `read_dir`).
//! - [`net`]    — TCP/UDP sockets (`TcpStream`, `TcpListener`, `UdpSocket`).
//! - [`http`]   — HTTP(S) client over Sony `SceHttp2` (`Client`, `Response`).
//! - [`thread`] — threads & synchronization (`spawn`, `Mutex`, `Condvar`).
//!
//! Errors are reported with [`Error`]/[`Result`]; see [`error`].
#![no_std]

extern crate alloc;

pub mod error;
pub use error::{Errno, Error, Result, SceError};

pub(crate) mod util;

#[cfg(feature = "fs")]
pub mod fs;

#[cfg(feature = "net")]
pub mod net;

#[cfg(feature = "thread")]
pub mod thread;

#[cfg(feature = "http")]
pub mod http;

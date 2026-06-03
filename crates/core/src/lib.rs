//! L3 — business-core use cases, built on the safe L2 wrappers ([`ps5`]).
//!
//! This layer is `no_std` + `alloc` and contains **no `unsafe` and no FFI**: it
//! is plain Rust logic over the std-like APIs L2 provides (`ps5::net`,
//! `ps5::thread`, …), so in principle it ports to any platform offering those.
//!
//! ## Modules
//! - [`http`] — a minimal HTTP/1.1 **server** (`Server`, `Request`, `Response`),
//!   thread-per-connection over [`ps5::net`].
#![no_std]

extern crate alloc;

pub mod http;

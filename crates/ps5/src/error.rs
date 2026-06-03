//! The error type shared by every L2 wrapper module.
//!
//! Two failure modes are modeled:
//! - [`Errno`] — a POSIX `errno` from a failed `fs`/`net`/`thread` libc call,
//!   read via the SDK's `__error()` and rendered with `strerror`.
//! - [`SceError`] — a negative status code returned by a Sony SCE call
//!   (HTTP/TLS), which does NOT use `errno`.

use core::ffi::CStr;
use core::fmt;

/// A POSIX `errno` value captured after a failing libc call.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Errno(pub i32);

impl Errno {
    /// The current thread's `errno`, as set by the last failing libc call.
    ///
    /// Call this immediately after a `-1`/null return, before any other libc
    /// call can overwrite `errno`.
    #[inline]
    pub fn last() -> Self {
        // SAFETY: `__error()` returns a valid pointer to this thread's errno slot.
        Self(unsafe { *ps5_sys::__error() })
    }

    /// The raw `errno` integer.
    #[inline]
    pub fn code(self) -> i32 {
        self.0
    }

    /// `true` if the code indicates a non-blocking op would block
    /// (`EAGAIN`/`EWOULDBLOCK`).
    #[inline]
    pub fn would_block(self) -> bool {
        self.0 == ps5_sys::EAGAIN as i32 || self.0 == ps5_sys::EWOULDBLOCK as i32
    }

    /// `true` if the call was interrupted by a signal (`EINTR`) and may be retried.
    #[inline]
    pub fn interrupted(self) -> bool {
        self.0 == ps5_sys::EINTR as i32
    }
}

impl fmt::Display for Errno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: strerror returns a pointer to a (per-locale) C string valid for
        // the duration of this call; we only borrow it within this fmt scope.
        let ptr = unsafe { ps5_sys::strerror(self.0) };
        let msg = if ptr.is_null() {
            "unknown error"
        } else {
            unsafe { CStr::from_ptr(ptr) }
                .to_str()
                .unwrap_or("invalid error message")
        };
        write!(f, "{msg} (errno {})", self.0)
    }
}

impl fmt::Debug for Errno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Errno({self})")
    }
}

/// A failure from a Sony SCE library call (HTTP/TLS), which signals errors with
/// a negative return value rather than `errno`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SceError {
    /// The SCE function that failed (e.g. `"sceHttp2SendRequest"`).
    pub op: &'static str,
    /// The negative status code it returned.
    pub code: i32,
}

impl fmt::Display for SceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed (sce 0x{:08x})", self.op, self.code)
    }
}

impl fmt::Debug for SceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SceError {{ op: {:?}, code: 0x{:08x} }}",
            self.op, self.code
        )
    }
}

/// The unified L2 error.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A POSIX/libc failure (`fs`, `net`, `thread`).
    Os(Errno),
    /// A Sony SCE failure (`http`).
    Sce(SceError),
    /// A `getaddrinfo` failure (an `EAI_*` code — distinct from `errno`).
    Gai(i32),
    /// An argument could not be represented for FFI (e.g. an interior NUL in a path).
    InvalidInput(&'static str),
}

impl Error {
    /// Build an [`Error::Os`] from the current thread's `errno`.
    #[inline]
    pub fn last_os() -> Self {
        Error::Os(Errno::last())
    }

    /// Build an [`Error::Sce`] for a failed SCE `op` returning `code`.
    #[inline]
    pub fn sce(op: &'static str, code: i32) -> Self {
        Error::Sce(SceError { op, code })
    }

    /// Build an [`Error::Gai`] from a `getaddrinfo` `EAI_*` code.
    #[inline]
    pub fn gai(code: i32) -> Self {
        Error::Gai(code)
    }

    /// The underlying `errno`, if this is an OS error.
    pub fn errno(&self) -> Option<Errno> {
        match self {
            Error::Os(e) => Some(*e),
            _ => None,
        }
    }
}

impl From<Errno> for Error {
    #[inline]
    fn from(e: Errno) -> Self {
        Error::Os(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Os(e) => write!(f, "{e}"),
            Error::Sce(e) => write!(f, "{e}"),
            Error::Gai(c) => write!(f, "name resolution failed (EAI {c})"),
            Error::InvalidInput(m) => write!(f, "invalid input: {m}"),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Os(e) => write!(f, "Os({e:?})"),
            Error::Sce(e) => write!(f, "Sce({e:?})"),
            Error::Gai(c) => write!(f, "Gai({c})"),
            Error::InvalidInput(m) => write!(f, "InvalidInput({m:?})"),
        }
    }
}

/// Convenience alias used throughout L2.
pub type Result<T> = core::result::Result<T, Error>;

//! Small internal helpers shared by the L2 wrapper modules.

use crate::error::{Error, Result};
use alloc::ffi::CString;

/// Convert a Rust `&str` to an owned C string, erroring on an interior NUL.
#[allow(dead_code)]
pub(crate) fn cstr(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::InvalidInput("string contains an interior NUL byte"))
}

/// Map a libc `int` return where `-1` signals failure (`errno` is then read).
#[allow(dead_code)]
pub(crate) fn cvt_i32(ret: i32) -> Result<i32> {
    if ret == -1 {
        Err(Error::last_os())
    } else {
        Ok(ret)
    }
}

/// Map a libc `ssize_t` (isize) return: `-1` is failure, else the byte count.
#[allow(dead_code)]
pub(crate) fn cvt_ssize(ret: isize) -> Result<usize> {
    if ret == -1 {
        Err(Error::last_os())
    } else {
        Ok(ret as usize)
    }
}

/// Map a libc pointer return where a null pointer signals failure.
#[allow(dead_code)]
pub(crate) fn cvt_ptr<T>(ptr: *mut T) -> Result<*mut T> {
    if ptr.is_null() {
        Err(Error::last_os())
    } else {
        Ok(ptr)
    }
}

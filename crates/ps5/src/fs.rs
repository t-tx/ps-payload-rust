//! Files & directories — a `std::fs`-flavored safe layer over `ps5_sys::fs`.
//!
//! [`File`] is an RAII wrapper around a raw file descriptor that closes itself
//! on drop. [`OpenOptions`] is the usual builder. Free functions
//! ([`read`], [`write`], [`metadata`], [`read_dir`], …) cover the common
//! whole-path operations. Paths are `&str` and converted to C strings, so an
//! interior NUL yields [`Error::InvalidInput`].

use crate::error::{Errno, Error, Result};
use crate::util::{cstr, cvt_i32, cvt_ptr, cvt_ssize};
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_int, c_uint, c_void};

use ps5_sys::fs as sys;

/// Default permission bits for newly created files (`rw-r--r--`).
const DEFAULT_CREATE_MODE: sys::mode_t = 0o644;

/// Reference point for [`File::seek`].
///
/// Mirrors `std::io::SeekFrom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeekFrom {
    /// Seek to an absolute offset (bytes from the start).
    Start(u64),
    /// Seek relative to the end of the file (may be negative).
    End(i64),
    /// Seek relative to the current position (may be negative).
    Current(i64),
}

/// An open file backed by a raw OS file descriptor.
///
/// The descriptor is closed automatically when the `File` is dropped.
#[derive(Debug)]
pub struct File {
    fd: c_int,
}

impl File {
    /// Open a file in read-only mode (`O_RDONLY`).
    pub fn open(path: &str) -> Result<File> {
        OpenOptions::new().read(true).open(path)
    }

    /// Open a file for writing, creating it (mode `0o644`) and truncating any
    /// existing contents (`O_WRONLY | O_CREAT | O_TRUNC`).
    pub fn create(path: &str) -> Result<File> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
    }

    /// Start building an open request; see [`OpenOptions`].
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }

    /// Read up to `buf.len()` bytes into `buf`, returning the number read.
    ///
    /// A return of `0` indicates end of file.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        // SAFETY: `self.fd` is a live fd we own; `buf` is a valid writable
        // region of exactly `buf.len()` bytes.
        let ret = unsafe { sys::read(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len()) };
        cvt_ssize(ret)
    }

    /// Write up to `buf.len()` bytes from `buf`, returning the number written.
    pub fn write(&self, buf: &[u8]) -> Result<usize> {
        // SAFETY: `self.fd` is a live fd we own; `buf` is a valid readable
        // region of exactly `buf.len()` bytes.
        let ret = unsafe { sys::write(self.fd, buf.as_ptr() as *const c_void, buf.len()) };
        cvt_ssize(ret)
    }

    /// Write the entire buffer, looping until all bytes are flushed.
    ///
    /// A `write` returning `0` (no progress) is reported as
    /// [`Error::InvalidInput`] to avoid an infinite loop.
    pub fn write_all(&self, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() {
            let n = self.write(buf)?;
            if n == 0 {
                return Err(Error::InvalidInput(
                    "write returned zero (failed to write whole buffer)",
                ));
            }
            buf = &buf[n..];
        }
        Ok(())
    }

    /// Read the file to end of file, appending all bytes to `buf`.
    ///
    /// Returns the number of bytes appended.
    pub fn read_to_end(&self, buf: &mut Vec<u8>) -> Result<usize> {
        let start = buf.len();
        let mut chunk = [0u8; 8192];
        loop {
            let n = self.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Ok(buf.len() - start)
    }

    /// Reposition the file offset, returning the resulting absolute offset.
    pub fn seek(&self, pos: SeekFrom) -> Result<u64> {
        let (off, whence) = match pos {
            SeekFrom::Start(n) => (n as sys::off_t, sys::SEEK_SET as c_int),
            SeekFrom::End(n) => (n as sys::off_t, sys::SEEK_END as c_int),
            SeekFrom::Current(n) => (n as sys::off_t, sys::SEEK_CUR as c_int),
        };
        // SAFETY: `self.fd` is a live fd we own; `whence` is a valid SEEK_* code.
        // `lseek` returns `off_t` (i64), so we cannot use `cvt_*`: handle the
        // `-1` sentinel here.
        let ret = unsafe { sys::lseek(self.fd, off, whence) };
        if ret == -1 {
            Err(Error::last_os())
        } else {
            Ok(ret as u64)
        }
    }

    /// Flush all in-memory file data and metadata to the device (`fsync`).
    pub fn sync_all(&self) -> Result<()> {
        // SAFETY: `self.fd` is a live fd we own.
        cvt_i32(unsafe { sys::fsync(self.fd) }).map(|_| ())
    }

    /// Truncate or extend the file to exactly `size` bytes (`ftruncate`).
    pub fn set_len(&self, size: u64) -> Result<()> {
        // SAFETY: `self.fd` is a live fd we own.
        cvt_i32(unsafe { sys::ftruncate(self.fd, size as sys::off_t) }).map(|_| ())
    }

    /// Borrow the underlying raw file descriptor without affecting ownership.
    pub fn as_raw_fd(&self) -> i32 {
        self.fd
    }

    /// Adopt an existing raw file descriptor.
    ///
    /// # Safety
    /// `fd` must be a valid, open descriptor that is not owned elsewhere; the
    /// returned `File` takes ownership and will `close` it on drop.
    pub unsafe fn from_raw_fd(fd: i32) -> File {
        File { fd }
    }

    /// Consume the `File` and return the raw descriptor without closing it.
    ///
    /// The caller becomes responsible for closing the returned descriptor.
    pub fn into_raw_fd(self) -> i32 {
        let fd = self.fd;
        core::mem::forget(self);
        fd
    }
}

impl Drop for File {
    fn drop(&mut self) {
        // SAFETY: `self.fd` was a live fd we own; `close` is the correct
        // teardown. We ignore the result because `drop` cannot report errors.
        unsafe {
            sys::close(self.fd);
        }
    }
}

/// Options and flags configuring how a [`File`] is opened.
///
/// Mirrors `std::fs::OpenOptions`. Build with [`OpenOptions::new`] (or
/// [`File::options`]) and finish with [`OpenOptions::open`].
#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    /// A fresh set of options with every flag disabled.
    pub fn new() -> OpenOptions {
        OpenOptions::default()
    }

    /// Allow reads from the opened file.
    pub fn read(&mut self, read: bool) -> &mut OpenOptions {
        self.read = read;
        self
    }

    /// Allow writes to the opened file.
    pub fn write(&mut self, write: bool) -> &mut OpenOptions {
        self.write = write;
        self
    }

    /// Open in append mode; every write goes to the end of the file.
    pub fn append(&mut self, append: bool) -> &mut OpenOptions {
        self.append = append;
        self
    }

    /// Truncate the file to length 0 when opening it.
    pub fn truncate(&mut self, truncate: bool) -> &mut OpenOptions {
        self.truncate = truncate;
        self
    }

    /// Create the file if it does not already exist.
    pub fn create(&mut self, create: bool) -> &mut OpenOptions {
        self.create = create;
        self
    }

    /// Create a new file, failing if the path already exists.
    ///
    /// Implies `create` and ignores `truncate` (matching `std`).
    pub fn create_new(&mut self, create_new: bool) -> &mut OpenOptions {
        self.create_new = create_new;
        self
    }

    /// Compute the access-mode portion of the open flags (`O_RDONLY` etc.).
    fn access_flags(&self) -> Result<c_int> {
        let flags = match (self.read, self.write || self.append) {
            (true, false) => sys::O_RDONLY,
            (false, true) => sys::O_WRONLY,
            (true, true) => sys::O_RDWR,
            (false, false) => {
                return Err(Error::InvalidInput(
                    "OpenOptions: no access mode set (need read and/or write)",
                ))
            }
        };
        Ok(flags as c_int)
    }

    /// Open the file at `path` according to these options.
    pub fn open(&self, path: &str) -> Result<File> {
        let mut flags = self.access_flags()?;

        if self.append {
            flags |= sys::O_APPEND as c_int;
        }
        if self.create_new {
            flags |= (sys::O_CREAT | sys::O_EXCL) as c_int;
        } else {
            if self.create {
                flags |= sys::O_CREAT as c_int;
            }
            // `create_new` disables truncate, matching std semantics.
            if self.truncate {
                flags |= sys::O_TRUNC as c_int;
            }
        }

        let c_path = cstr(path)?;
        // C default argument promotion: a `mode_t` (u16) passed through `...`
        // is promoted to `c_uint`, which is how the kernel reads it back.
        let mode: c_uint = DEFAULT_CREATE_MODE as c_uint;

        // SAFETY: `c_path` is a valid NUL-terminated C string that outlives the
        // call. `open` is variadic; we always pass the `mode` argument, which is
        // only consulted by the kernel when `O_CREAT`/`O_EXCL` is present and is
        // otherwise ignored.
        let fd = unsafe { sys::open(c_path.as_ptr(), flags, mode) };
        let fd = cvt_i32(fd)?;
        Ok(File { fd })
    }
}

/// Metadata describing a filesystem object, obtained via [`metadata`].
#[derive(Clone, Copy)]
pub struct Metadata {
    stat: sys::stat,
}

impl Metadata {
    /// Size of the file in bytes.
    pub fn len(&self) -> u64 {
        self.stat.st_size as u64
    }

    /// `true` if the size is zero.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `true` if this is a directory.
    pub fn is_dir(&self) -> bool {
        (self.stat.st_mode as u32 & sys::S_IFMT) == sys::S_IFDIR
    }

    /// `true` if this is a regular file.
    pub fn is_file(&self) -> bool {
        (self.stat.st_mode as u32 & sys::S_IFMT) == sys::S_IFREG
    }

    /// The raw mode bits (file type and permissions).
    pub fn mode(&self) -> u32 {
        self.stat.st_mode as u32
    }
}

/// An entry yielded by [`ReadDir`].
#[derive(Clone, Debug)]
pub struct DirEntry {
    name: String,
}

impl DirEntry {
    /// The bare file name of this entry (no directory component).
    pub fn file_name(&self) -> &str {
        &self.name
    }
}

/// Iterator over the entries of a directory, returned by [`read_dir`].
///
/// The underlying directory stream is closed when the iterator is dropped.
/// The `.` and `..` entries are skipped, matching `std::fs::read_dir`.
pub struct ReadDir {
    dir: *mut sys::DIR,
}

impl Iterator for ReadDir {
    type Item = Result<DirEntry>;

    fn next(&mut self) -> Option<Result<DirEntry>> {
        loop {
            // `readdir` reports both "end of directory" and "error" with a null
            // return, distinguished via errno (left at 0 on a clean end). Clear
            // errno first so a stale value cannot be mistaken for a failure.
            //
            // SAFETY: `__error()` yields this thread's errno slot; `self.dir` is
            // a live `DIR*` we own. The returned `dirent` (when non-null) points
            // into storage owned by the stream, valid until the next call.
            let ent = unsafe {
                *ps5_sys::__error() = 0;
                sys::readdir(self.dir)
            };

            if ent.is_null() {
                let errno = Errno::last().code();
                if errno == 0 {
                    return None; // clean end of directory
                }
                return Some(Err(Error::Os(Errno(errno))));
            }

            // SAFETY: `ent` is non-null and points to a valid `dirent`.
            let name = unsafe { dirent_name(&*ent) };
            if name == "." || name == ".." {
                continue;
            }
            return Some(Ok(DirEntry { name }));
        }
    }
}

impl Drop for ReadDir {
    fn drop(&mut self) {
        // SAFETY: `self.dir` is a live `DIR*` we own; `closedir` is the correct
        // teardown and also frees the underlying fd. Result ignored in drop.
        unsafe {
            sys::closedir(self.dir);
        }
    }
}

/// Decode the `d_name` field of a `dirent` into an owned `String`, lossily for
/// any non-UTF-8 bytes.
///
/// # Safety
/// `ent` must reference a valid `dirent`.
unsafe fn dirent_name(ent: &sys::dirent) -> String {
    // `d_name` is a fixed `[c_char; 256]`; the real name is `d_namlen` bytes,
    // but we defensively clamp to the array bound and stop at an embedded NUL.
    let max = core::cmp::min(ent.d_namlen as usize, ent.d_name.len());
    // SAFETY: `d_name` is a valid array of `max <= 256` `c_char`; reading it as
    // `u8` is sound (same size/align) and stays within bounds.
    let bytes: &[u8] =
        unsafe { core::slice::from_raw_parts(ent.d_name.as_ptr() as *const u8, max) };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Read the entire contents of a file into a byte vector.
pub fn read(path: &str) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Read the entire contents of a file into a `String`.
///
/// Returns [`Error::InvalidInput`] if the contents are not valid UTF-8.
pub fn read_to_string(path: &str) -> Result<String> {
    let bytes = read(path)?;
    String::from_utf8(bytes).map_err(|_| Error::InvalidInput("file contents are not valid UTF-8"))
}

/// Create a file at `path` (truncating any existing one) and write `contents`.
pub fn write(path: &str, contents: &[u8]) -> Result<()> {
    let file = File::create(path)?;
    file.write_all(contents)
}

/// Remove a file from the filesystem (`unlink`).
pub fn remove_file(path: &str) -> Result<()> {
    let c_path = cstr(path)?;
    // SAFETY: `c_path` is a valid NUL-terminated C string outliving the call.
    cvt_i32(unsafe { sys::unlink(c_path.as_ptr()) }).map(|_| ())
}

/// Create a directory at `path` with mode `0o755` (`mkdir`).
pub fn create_dir(path: &str) -> Result<()> {
    let c_path = cstr(path)?;
    // SAFETY: `c_path` is a valid NUL-terminated C string outliving the call.
    cvt_i32(unsafe { sys::mkdir(c_path.as_ptr(), 0o755 as sys::mode_t) }).map(|_| ())
}

/// Remove an empty directory at `path` (`rmdir`).
pub fn remove_dir(path: &str) -> Result<()> {
    let c_path = cstr(path)?;
    // SAFETY: `c_path` is a valid NUL-terminated C string outliving the call.
    cvt_i32(unsafe { sys::rmdir(c_path.as_ptr()) }).map(|_| ())
}

/// Rename a file or directory, replacing `to` if it exists (`rename`).
pub fn rename(from: &str, to: &str) -> Result<()> {
    let c_from = cstr(from)?;
    let c_to = cstr(to)?;
    // SAFETY: both C strings are valid NUL-terminated and outlive the call.
    cvt_i32(unsafe { sys::rename(c_from.as_ptr(), c_to.as_ptr()) }).map(|_| ())
}

/// Test whether a path exists (`access(path, F_OK)`).
///
/// Returns `false` on any error (including permission and bad-path errors).
pub fn exists(path: &str) -> bool {
    let c_path = match cstr(path) {
        Ok(p) => p,
        Err(_) => return false,
    };
    // SAFETY: `c_path` is a valid NUL-terminated C string outliving the call.
    unsafe { sys::access(c_path.as_ptr(), sys::F_OK as c_int) == 0 }
}

/// Query metadata for the object at `path` (`stat`, following symlinks).
pub fn metadata(path: &str) -> Result<Metadata> {
    let c_path = cstr(path)?;
    let mut stat = core::mem::MaybeUninit::<sys::stat>::uninit();
    // SAFETY: `c_path` is valid and NUL-terminated; `stat.as_mut_ptr()` points
    // to writable, suitably-aligned storage for a `stat`. On success the kernel
    // fully initializes the struct, so `assume_init` is then sound.
    let ret = unsafe { sys::stat(c_path.as_ptr(), stat.as_mut_ptr()) };
    cvt_i32(ret)?;
    let stat = unsafe { stat.assume_init() };
    Ok(Metadata { stat })
}

/// Iterate over the entries of the directory at `path` (`opendir`).
pub fn read_dir(path: &str) -> Result<ReadDir> {
    let c_path = cstr(path)?;
    // SAFETY: `c_path` is a valid NUL-terminated C string outliving the call.
    let dir = unsafe { sys::opendir(c_path.as_ptr()) };
    let dir = cvt_ptr(dir)?;
    Ok(ReadDir { dir })
}

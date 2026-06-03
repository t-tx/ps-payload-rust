//! L2 threading & synchronization, modeled loosely on `std::thread` and
//! `std::sync`.
//!
//! This module wraps the raw pthread FFI from [`ps5_sys::thread`] into safe,
//! RAII-based abstractions:
//! - [`spawn`] / [`JoinHandle`] — start a thread running a closure and collect
//!   its result, mirroring [`std::thread::spawn`].
//! - [`Mutex`] / [`MutexGuard`] — a mutual-exclusion lock guarding some data,
//!   mirroring [`std::sync::Mutex`].
//! - [`Condvar`] — a condition variable paired with a [`Mutex`], mirroring
//!   [`std::sync::Condvar`].
//!
//! Unlike `std`, locking and waiting return [`Result`] because the underlying
//! pthread calls can in principle fail; there is no poisoning.
//!
//! ## Error mapping
//! The pthread functions used here report failure by *returning* an `errno`
//! value directly (`0` on success, a positive code on failure); they do **not**
//! set the thread-local `errno` and do not return `-1`. So results are mapped
//! with [`Error::Os`]`(`[`Errno`]`(rc))` rather than [`Error::last_os`].

use crate::error::{Errno, Error, Result};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::ptr;

use ps5_sys::thread::{
    pthread_attr_destroy, pthread_attr_init, pthread_attr_setstacksize, pthread_attr_t,
    pthread_cond_broadcast, pthread_cond_destroy, pthread_cond_init, pthread_cond_signal,
    pthread_cond_t, pthread_cond_wait, pthread_create, pthread_detach, pthread_join,
    pthread_mutex_destroy, pthread_mutex_init, pthread_mutex_lock, pthread_mutex_t,
    pthread_mutex_unlock, pthread_t,
};

/// Start routine type pthread expects.
type StartRoutine = unsafe extern "C" fn(arg: *mut c_void) -> *mut c_void;

/// Turn a pthread-style return code into a [`Result`].
///
/// The pthread API returns `0` on success and a positive `errno`-style code on
/// failure (without touching the thread-local `errno`), so we wrap the code
/// into an [`Error::Os`] directly.
#[inline]
fn cvt_pthread(rc: i32) -> Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(Error::Os(Errno(rc)))
    }
}

// ============================================================================
// spawn / JoinHandle
// ============================================================================

/// Result slot co-owned by the spawned thread and its [`JoinHandle`].
///
/// The thread writes the closure's value exactly once, then drops its `Arc`
/// reference. The joiner reads it after `pthread_join`. Whichever side drops the
/// **last** `Arc` reference runs this slot's destructor — so the value `T` is
/// always dropped, whether the handle is joined or merely dropped (no leak),
/// matching `std::thread`.
struct Shared<T> {
    slot: UnsafeCell<Option<T>>,
}

// SAFETY: access to `slot` is externally synchronized, never concurrent. There
// is a single writer (the thread, once, before it drops its `Arc` ref) and a
// single reader (the joiner, only after `pthread_join`, which happens-after that
// write); a dropped-without-join handle has no reader at all. So sharing
// `Arc<Shared<T>>` across the two threads is sound when `T: Send`.
unsafe impl<T: Send> Send for Shared<T> {}
unsafe impl<T: Send> Sync for Shared<T> {}

/// The heap packet handed to the new thread: the user closure plus this thread's
/// reference to the shared result slot.
struct Packet<F, T> {
    closure: F,
    shared: Arc<Shared<T>>,
}

/// The C entry point handed to `pthread_create`.
///
/// # Safety
/// `arg` must be exactly the `Box::into_raw(Box<Packet<F, T>>)` pointer passed
/// as the `pthread_create` user-data argument, and `F`/`T` must match the
/// `JoinHandle<T>` produced by the corresponding [`spawn`] call. The runtime
/// guarantees this because the only caller is [`spawn`], which monomorphizes
/// this function for the same `F`/`T` and passes the matching pointer.
extern "C" fn trampoline<F, T>(arg: *mut c_void) -> *mut c_void
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    // SAFETY: `arg` was produced by `Box::into_raw` of a `Box<Packet<F, T>>` in
    // `spawn`, with the same monomorphized `F`/`T`. Reconstructing the `Box`
    // takes back sole ownership of that allocation. This runs exactly once.
    let packet = unsafe { Box::from_raw(arg as *mut Packet<F, T>) };
    let Packet { closure, shared } = *packet;

    let value = closure();
    // SAFETY: this thread is the sole writer; the joiner only reads after
    // `pthread_join` (which happens-after this store), and a non-joining handle
    // never reads. So this single store is unsynchronized-but-exclusive.
    unsafe {
        *shared.slot.get() = Some(value);
    }

    // Drop this thread's reference to the slot. If the handle was already
    // dropped, this is the last reference, so the value is dropped right here;
    // otherwise the joiner takes it. Either way nothing leaks.
    drop(shared);
    ptr::null_mut()
}

/// An owned handle to a spawned thread, carrying the closure's return type `T`.
///
/// Mirrors [`std::thread::JoinHandle`]. Call [`join`](JoinHandle::join) to wait
/// for the thread and retrieve its value. If the handle is dropped without
/// joining, the thread is detached and runs to completion; its return value is
/// dropped (not leaked) when the thread finishes.
pub struct JoinHandle<T> {
    handle: pthread_t,
    /// Records whether the thread has been joined, so `Drop` knows whether to
    /// detach.
    joined: bool,
    /// This handle's reference to the shared result slot.
    shared: Arc<Shared<T>>,
}

// SAFETY: A `JoinHandle<T>` only exposes `T` by moving it out on `join` (which
// requires `T: Send`). The `pthread_t` is an opaque handle sound to move between
// threads, and `Arc<Shared<T>>` is `Send`/`Sync` for `T: Send`. So the handle is
// `Send`/`Sync` when `T: Send`.
unsafe impl<T: Send> Send for JoinHandle<T> {}
unsafe impl<T: Send> Sync for JoinHandle<T> {}

impl<T> JoinHandle<T> {
    /// Wait for the associated thread to finish and return the value its
    /// closure produced.
    ///
    /// Mirrors [`std::thread::JoinHandle::join`].
    pub fn join(mut self) -> Result<T> {
        // We collect the result through the shared slot, not pthread's return
        // value, so pass null for the `retval` out-pointer.
        // SAFETY: `self.handle` is a live, un-joined, un-detached pthread handle
        // from `spawn`. `pthread_join` blocks until the thread exits.
        let rc = unsafe { pthread_join(self.handle, ptr::null_mut()) };
        // Mark joined first so `Drop` does not also detach the handle.
        self.joined = true;
        cvt_pthread(rc)?;

        // `pthread_join` synchronizes with the thread's termination, so the
        // trampoline's store happens-before this read, and the thread has
        // already dropped its `Arc` ref (this handle now holds the only one).
        // SAFETY: sole reader after join; the slot was written by the trampoline.
        let value = unsafe { (*self.shared.slot.get()).take() };
        // `None` is unreachable for a handle from `spawn` (the trampoline always
        // stores `Some(..)`), but we surface it rather than panic.
        value.ok_or(Error::Os(Errno(ps5_sys::EINVAL as i32)))
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        if !self.joined {
            // Detach so the OS reclaims the thread's stack when it exits; we will
            // never join it. The result value is still dropped: when the thread
            // finishes it drops its `Arc` ref, and the last `Arc` ref (this one
            // or the thread's) runs `Shared`'s destructor.
            // SAFETY: `self.handle` is live and neither joined nor detached
            // (guarded by `self.joined`). Best-effort: errors ignored in drop.
            unsafe {
                let _ = pthread_detach(self.handle);
            }
        }
        // `self.shared` drops here automatically.
    }
}

/// Configures and spawns a thread, mirroring [`std::thread::Builder`].
///
/// The main reason to use this over [`spawn`] is [`stack_size`](Builder::stack_size):
/// the default per-thread stack is large and implementation-defined, so a server
/// that runs one thread per connection should cap it (e.g. 64–256 KiB) to keep
/// memory predictable.
#[derive(Clone, Debug, Default)]
pub struct Builder {
    stack_size: Option<usize>,
}

impl Builder {
    /// A builder with default thread attributes.
    pub fn new() -> Builder {
        Builder { stack_size: None }
    }

    /// Set the new thread's stack size, in bytes.
    ///
    /// Rounded up to a page and clamped to at least `PTHREAD_STACK_MIN` by the
    /// platform. If unset, the platform default is used.
    pub fn stack_size(mut self, size: usize) -> Builder {
        self.stack_size = Some(size);
        self
    }

    /// Spawn a thread running `f`, returning a [`JoinHandle`] for its result.
    ///
    /// # Errors
    /// Returns an [`Error::Os`] if the thread could not be created (e.g. the
    /// stack size is invalid or the process is out of resources).
    pub fn spawn<F, T>(self, f: F) -> Result<JoinHandle<T>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let shared = Arc::new(Shared {
            slot: UnsafeCell::new(None),
        });
        let packet = Box::new(Packet {
            closure: f,
            shared: Arc::clone(&shared),
        });
        // Hand the packet to the new thread; the trampoline reclaims it. On a
        // creation failure below we reclaim it here to avoid a leak.
        let raw = Box::into_raw(packet);

        let mut handle: pthread_t = ptr::null_mut();
        let rc = create_thread(
            &mut handle,
            self.stack_size,
            trampoline::<F, T>,
            raw as *mut c_void,
        );

        if rc != 0 {
            // The thread was never created, so the trampoline will not run.
            // Reclaim the packet (which also drops its `Arc` clone of `shared`);
            // our local `shared` then drops at end of scope. No leak.
            // SAFETY: `raw` came from `Box::into_raw` above and was never handed
            // to a running thread (creation failed).
            unsafe {
                drop(Box::from_raw(raw));
            }
            return Err(Error::Os(Errno(rc)));
        }

        Ok(JoinHandle {
            handle,
            joined: false,
            shared,
        })
    }
}

/// Create a thread via `pthread_create`, optionally with a custom stack size,
/// returning the pthread status code (`0` on success).
///
/// When `stack_size` is `Some`, a `pthread_attr_t` is initialized, configured,
/// and destroyed around the create call.
fn create_thread(
    handle: *mut pthread_t,
    stack_size: Option<usize>,
    start: StartRoutine,
    arg: *mut c_void,
) -> i32 {
    let Some(size) = stack_size else {
        // SAFETY: `handle` is a valid out-pointer; null attr = defaults; `start`
        // has the required signature; `arg` is the matching packet pointer.
        return unsafe { pthread_create(handle, ptr::null(), Some(start), arg) };
    };

    // SAFETY: `attr` is a fresh, owned `pthread_attr_t` slot; each call below
    // operates on it while it is live, and it is destroyed before returning.
    unsafe {
        let mut attr: pthread_attr_t = ptr::null_mut();
        let rc = pthread_attr_init(&mut attr);
        if rc != 0 {
            return rc;
        }
        let rc = pthread_attr_setstacksize(&mut attr, size);
        if rc != 0 {
            pthread_attr_destroy(&mut attr);
            return rc;
        }
        let rc = pthread_create(handle, &attr, Some(start), arg);
        pthread_attr_destroy(&mut attr);
        rc
    }
}

/// Spawn a new thread running `f`, returning a [`JoinHandle`] for its result.
///
/// Mirrors [`std::thread::spawn`]; equivalent to [`Builder::new().spawn(f)`](Builder::spawn).
/// The closure is moved onto the heap and the new thread takes ownership; its
/// return value is stored in a slot shared with the [`JoinHandle`], so
/// [`join`](JoinHandle::join) can collect it and dropping the handle without
/// joining still drops the value rather than leaking it. Use [`Builder`] to set
/// a custom stack size.
///
/// # Errors
/// Returns an [`Error::Os`] if the thread could not be created.
#[must_use = "dropping the JoinHandle detaches the thread; ignore deliberately if intended"]
pub fn spawn<F, T>(f: F) -> Result<JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new().spawn(f)
}

// ============================================================================
// Mutex / MutexGuard
// ============================================================================

/// A mutual-exclusion lock protecting some data of type `T`.
///
/// Mirrors [`std::sync::Mutex`], but without poisoning: [`lock`](Mutex::lock)
/// returns a [`Result`] reflecting the underlying pthread call. The data can
/// only be accessed through the [`MutexGuard`] returned by `lock`, which
/// releases the lock on drop.
pub struct Mutex<T> {
    inner: UnsafeCell<pthread_mutex_t>,
    data: UnsafeCell<T>,
}

// SAFETY: The `pthread_mutex_t` serializes access to `data`, so it is sound to
// share `&Mutex<T>` across threads as long as the guarded `T` may travel
// between threads (`T: Send`). Likewise it is sound to move the `Mutex` to
// another thread when `T: Send`.
unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// Create a new mutex guarding `data`.
    ///
    /// # Errors
    /// Returns an [`Error::Os`] if the underlying mutex could not be
    /// initialized.
    pub fn new(data: T) -> Result<Self> {
        let mutex = Mutex {
            inner: UnsafeCell::new(ptr::null_mut()),
            data: UnsafeCell::new(data),
        };
        // SAFETY: `inner` points to a freshly created, uninitialized
        // `pthread_mutex_t` slot we own exclusively; a null attr selects default
        // attributes. On success the slot holds an initialized handle.
        let rc = unsafe { pthread_mutex_init(mutex.inner.get(), ptr::null()) };
        cvt_pthread(rc)?;
        Ok(mutex)
    }

    /// Acquire the lock, blocking until it is available, and return a guard
    /// granting access to the protected data.
    ///
    /// Mirrors [`std::sync::Mutex::lock`].
    pub fn lock(&self) -> Result<MutexGuard<'_, T>> {
        // SAFETY: `inner` holds a mutex initialized in `new` and not yet
        // destroyed (destruction happens in `Drop`, which consumes `self`).
        let rc = unsafe { pthread_mutex_lock(self.inner.get()) };
        cvt_pthread(rc)?;
        Ok(MutexGuard {
            mutex: self,
            _not_send: PhantomData,
        })
    }
}

impl<T> Drop for Mutex<T> {
    fn drop(&mut self) {
        // SAFETY: `inner` was initialized in `new` and there are no outstanding
        // guards (a guard borrows `&self`, so none can outlive this `&mut self`
        // drop). Destroying it once here is correct; errors are ignored as a
        // best-effort cleanup.
        unsafe {
            let _ = pthread_mutex_destroy(self.inner.get());
        }
    }
}

/// An RAII guard granting exclusive access to the data behind a [`Mutex`].
///
/// Created by [`Mutex::lock`]. Dereferences to the protected `T` and releases
/// the lock when dropped. Mirrors [`std::sync::MutexGuard`].
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
    /// Makes the guard `!Send`: a pthread mutex must be unlocked by the same
    /// thread that locked it, so a guard must never cross a thread boundary
    /// (matching `std::sync::MutexGuard`). The manual `Sync` impl below is
    /// unaffected by this marker.
    _not_send: PhantomData<*const ()>,
}

// SAFETY: A held guard implies exclusive access to the data; sharing `&guard`
// (and thus `&T`) across threads is sound when `T: Sync`, matching `std`. The
// `*const ()` marker keeps the guard `!Send` (no manual `Send` impl exists).
unsafe impl<T: Sync> Sync for MutexGuard<'_, T> {}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: Holding this guard means the mutex is locked, granting
        // exclusive access to `data`, so a shared reference is sound.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: Holding this guard (by `&mut`) means the mutex is locked and
        // this is the only guard, granting exclusive access to `data`.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        // SAFETY: This guard proves the mutex is currently locked (by this
        // thread, from `Mutex::lock`); unlocking it exactly once on drop is
        // correct. Errors are ignored in `Drop`.
        unsafe {
            let _ = pthread_mutex_unlock(self.mutex.inner.get());
        }
    }
}

// ============================================================================
// Condvar
// ============================================================================

/// A condition variable, to be used together with a [`Mutex`].
///
/// Mirrors [`std::sync::Condvar`]. A thread [`wait`](Condvar::wait)s on the
/// condvar while holding a [`MutexGuard`], atomically releasing the lock and
/// blocking until another thread calls [`notify_one`](Condvar::notify_one) or
/// [`notify_all`](Condvar::notify_all); on wake the lock is reacquired and the
/// guard returned.
pub struct Condvar {
    inner: UnsafeCell<pthread_cond_t>,
}

// SAFETY: A condition variable is inherently a cross-thread primitive; the
// underlying pthread implementation performs its own internal synchronization,
// so sharing `&Condvar` and moving `Condvar` across threads is sound.
unsafe impl Send for Condvar {}
unsafe impl Sync for Condvar {}

impl Condvar {
    /// Create a new condition variable.
    ///
    /// # Errors
    /// Returns an [`Error::Os`] if the underlying condvar could not be
    /// initialized.
    pub fn new() -> Result<Self> {
        let cond = Condvar {
            inner: UnsafeCell::new(ptr::null_mut()),
        };
        // SAFETY: `inner` points to a freshly created, uninitialized
        // `pthread_cond_t` slot we own exclusively; a null attr selects default
        // attributes.
        let rc = unsafe { pthread_cond_init(cond.inner.get(), ptr::null()) };
        cvt_pthread(rc)?;
        Ok(cond)
    }

    /// Atomically release `guard`'s lock and block until notified, then
    /// reacquire the lock and return the guard.
    ///
    /// Mirrors [`std::sync::Condvar::wait`].
    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> Result<MutexGuard<'a, T>> {
        let mutex = guard.mutex;
        // The condvar takes over locking/unlocking of the mutex for the
        // duration of the wait, so forget our guard *without* running its `Drop`
        // (which would unlock). We reconstruct an equivalent guard afterwards.
        core::mem::forget(guard);

        // SAFETY: `self.inner` is an initialized condvar (from `new`), and
        // `mutex.inner` is an initialized mutex that the caller currently holds
        // locked (proven by the consumed guard). `pthread_cond_wait` atomically
        // unlocks the mutex and blocks; on return the mutex is locked again.
        let rc = unsafe { pthread_cond_wait(self.inner.get(), mutex.inner.get()) };

        if rc != 0 {
            // POSIX guarantees the calling thread re-holds the mutex on return
            // from `pthread_cond_wait`, including on error. Rebuild the guard so
            // its `Drop` releases the lock, then surface the error rather than
            // handing the guard to the caller.
            let _guard = MutexGuard {
                mutex,
                _not_send: PhantomData,
            };
            return Err(Error::Os(Errno(rc)));
        }

        // The mutex is locked again; reconstruct the guard to resume RAII.
        Ok(MutexGuard {
            mutex,
            _not_send: PhantomData,
        })
    }

    /// Wake one thread blocked in [`wait`](Condvar::wait), if any.
    ///
    /// Mirrors [`std::sync::Condvar::notify_one`]. Errors are ignored, matching
    /// the `std` signature.
    pub fn notify_one(&self) {
        // SAFETY: `inner` is an initialized condvar from `new`, not yet
        // destroyed (destruction consumes `self` in `Drop`).
        unsafe {
            let _ = pthread_cond_signal(self.inner.get());
        }
    }

    /// Wake all threads blocked in [`wait`](Condvar::wait).
    ///
    /// Mirrors [`std::sync::Condvar::notify_all`]. Errors are ignored, matching
    /// the `std` signature.
    pub fn notify_all(&self) {
        // SAFETY: `inner` is an initialized condvar from `new`, not yet
        // destroyed (destruction consumes `self` in `Drop`).
        unsafe {
            let _ = pthread_cond_broadcast(self.inner.get());
        }
    }
}

impl Drop for Condvar {
    fn drop(&mut self) {
        // SAFETY: `inner` was initialized in `new`; no thread can be waiting on
        // it here because waiting borrows `&self` and cannot outlive this
        // `&mut self` drop. Destroying it once is correct; errors are ignored.
        unsafe {
            let _ = pthread_cond_destroy(self.inner.get());
        }
    }
}

//! C ABI for the block-device framework.
//!
//! Every sister crate (qcow2 reader, partition probe, fs-* drivers) speaks
//! through the [`FsCoreDevice`] handle defined here, so consumers (Swift
//! FSKit modules, Go callers, C programs) only learn one device-handle
//! type and one error convention.
//!
//! ## Conventions
//!
//! - Handles are opaque `*mut FsCoreDevice`. Allocate via a constructor in
//!   one of the sister crates (e.g. `qcow2_open` from rust-img-qcow2),
//!   free via [`fs_core_device_close`] regardless of which crate created
//!   it.
//! - Error reporting is errno-style: every fallible function returns an
//!   [`FsCoreErrorCode`] (0 = OK, non-zero = failure) and stashes a human
//!   message in a thread-local. Read it via
//!   [`fs_core_last_error_message`].
//! - Every entry point catches Rust panics with `catch_unwind` and maps
//!   them to [`FsCoreErrorCode::Panic`]. Crossing an FFI boundary while
//!   unwinding is UB; the catch-net is non-negotiable.
//! - Thread safety: handles wrap `Arc<dyn BlockDevice>`, which is
//!   `Send + Sync` by trait bound. Multiple threads can call read/write
//!   concurrently as long as the underlying device's locking permits it.

#![allow(clippy::missing_safety_doc)]

use crate::block::BlockDevice;
use crate::callback_device::CallbackDevice;
use crate::error::Error;
use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CString};
use std::io;
use std::panic::AssertUnwindSafe;
use std::ptr;
use std::slice;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Error codes — kept dense and stable so consumers can hard-code them.
// ---------------------------------------------------------------------------

/// Numeric error codes mirrored across every sister crate's C ABI.
///
/// `#[repr(i32)]` so the layout is identical to the matching C `enum`.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsCoreErrorCode {
    /// Success.
    Ok = 0,
    /// Underlying I/O failed.
    Io = 1,
    /// EOF before the requested range was satisfied.
    ShortRead = 2,
    /// Write attempted on a read-only device.
    ReadOnly = 3,
    /// Read or write past the end of the device.
    OutOfBounds = 4,
    /// Driver-specific error — message in the thread-local last-error.
    Custom = 5,
    /// One of the input pointers was null.
    NullArg = 6,
    /// `catch_unwind` caught a panic crossing the FFI boundary.
    Panic = 7,
    /// Path string was not valid UTF-8 (or NUL-terminated).
    BadString = 8,
}

impl FsCoreErrorCode {
    fn from_error(e: &Error) -> Self {
        match e {
            Error::Io(_) => FsCoreErrorCode::Io,
            Error::ShortRead { .. } => FsCoreErrorCode::ShortRead,
            Error::ReadOnly => FsCoreErrorCode::ReadOnly,
            Error::OutOfBounds { .. } => FsCoreErrorCode::OutOfBounds,
            Error::Custom(_) => FsCoreErrorCode::Custom,
        }
    }
}

// ---------------------------------------------------------------------------
// Thread-local last-error — errno-style detail companion.
// ---------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Stash a message in the thread-local, replacing any previous one. Public
/// to sister crates so they can populate it for their own error paths.
pub fn set_last_error(message: impl Into<String>) {
    let s = message.into();
    let cs = CString::new(s.replace('\0', "?")).expect("contains no NUL after replace");
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = Some(cs);
    });
}

fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

/// Return a pointer to the calling thread's most recent error message, or
/// NULL if there is none. The pointer is owned by the framework and remains
/// valid until the next FFI call on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn fs_core_last_error_message() -> *const c_char {
    LAST_ERROR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|cs| cs.as_ptr())
            .unwrap_or(ptr::null())
    })
}

/// Helper for sister crates: run `body`, catch panics, map errors to codes,
/// stash the message in the thread-local. Returns the error code.
pub fn ffi_guard<F>(body: F) -> FsCoreErrorCode
where
    F: FnOnce() -> Result<(), Error>,
{
    clear_last_error();
    match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => FsCoreErrorCode::Ok,
        Ok(Err(e)) => {
            let code = FsCoreErrorCode::from_error(&e);
            set_last_error(e.to_string());
            code
        }
        Err(panic) => {
            set_last_error(panic_message(&panic));
            FsCoreErrorCode::Panic
        }
    }
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = panic.downcast_ref::<String>() {
        return s.clone();
    }
    "panic in FFI".to_string()
}

// ---------------------------------------------------------------------------
// Device handle — opaque to C callers, shared across crates.
// ---------------------------------------------------------------------------

/// Opaque handle wrapping an `Arc<dyn BlockDevice>`. Allocated by sister
/// crates' constructors and freed via [`fs_core_device_close`].
pub struct FsCoreDevice {
    inner: Arc<dyn BlockDevice>,
}

impl FsCoreDevice {
    /// Internal constructor — sister crates use this to wrap their own
    /// device types (Qcow2Reader, FileDevice, OwnedSlice, etc.) into the
    /// shared handle type. Returns a `Box::into_raw` pointer ready to hand
    /// across the FFI boundary.
    pub fn into_handle(inner: Arc<dyn BlockDevice>) -> *mut FsCoreDevice {
        Box::into_raw(Box::new(FsCoreDevice { inner }))
    }

    /// Borrow the inner device. `Arc::clone` it if you want shared
    /// ownership — e.g. when handing the device to a slice adapter while
    /// keeping the original handle alive.
    pub fn inner(&self) -> &Arc<dyn BlockDevice> {
        &self.inner
    }
}

/// Free a device handle. Safe to call with NULL (no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_close(handle: *mut FsCoreDevice) {
    if handle.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        drop(Box::from_raw(handle));
    }));
}

/// Total device size in bytes. Returns 0 if `handle` is NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_size_bytes(handle: *const FsCoreDevice) -> u64 {
    if handle.is_null() {
        return 0;
    }
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { (*handle).inner.size_bytes() }))
        .unwrap_or(0)
}

/// True if `write_at` is likely to succeed. Returns false on NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_is_writable(handle: *const FsCoreDevice) -> bool {
    if handle.is_null() {
        return false;
    }
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        (*handle).inner.is_writable()
    }))
    .unwrap_or(false)
}

/// Read exactly `len` bytes from `offset` into `buf`. `buf` must be at
/// least `len` bytes. Returns an `FsCoreErrorCode`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_read_at(
    handle: *const FsCoreDevice,
    offset: u64,
    buf: *mut u8,
    len: usize,
) -> FsCoreErrorCode {
    if handle.is_null() || (buf.is_null() && len > 0) {
        return FsCoreErrorCode::NullArg;
    }
    ffi_guard(|| {
        let slice_buf = unsafe { slice::from_raw_parts_mut(buf, len) };
        unsafe { (*handle).inner.read_at(offset, slice_buf) }
    })
}

/// Write exactly `len` bytes from `buf` to `offset`. Returns `ReadOnly`
/// for read-only devices.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_write_at(
    handle: *const FsCoreDevice,
    offset: u64,
    buf: *const u8,
    len: usize,
) -> FsCoreErrorCode {
    if handle.is_null() || (buf.is_null() && len > 0) {
        return FsCoreErrorCode::NullArg;
    }
    ffi_guard(|| {
        let slice_buf = unsafe { slice::from_raw_parts(buf, len) };
        unsafe { (*handle).inner.write_at(offset, slice_buf) }
    })
}

/// Flush pending writes to stable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_flush(handle: *const FsCoreDevice) -> FsCoreErrorCode {
    if handle.is_null() {
        return FsCoreErrorCode::NullArg;
    }
    ffi_guard(|| unsafe { (*handle).inner.flush() })
}

// ---------------------------------------------------------------------------
// Convenience: open a regular file as a device. Saves callers the trouble
// of building a Rust crate just to wrap `FileDevice`.
// ---------------------------------------------------------------------------

/// Open `path` (NUL-terminated UTF-8) as a `FileDevice` and return a
/// handle. Pass `writable=true` for RW. On failure returns NULL and the
/// thread-local last-error has detail.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_file_open(
    path: *const c_char,
    writable: bool,
) -> *mut FsCoreDevice {
    if path.is_null() {
        set_last_error("path is null");
        return ptr::null_mut();
    }
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let cstr = unsafe { std::ffi::CStr::from_ptr(path) };
        let s = match cstr.to_str() {
            Ok(s) => s,
            Err(_) => {
                set_last_error("path is not valid UTF-8");
                return ptr::null_mut();
            }
        };
        let dev = if writable {
            crate::file_device::FileDevice::open_rw(s)
        } else {
            crate::file_device::FileDevice::open(s)
        };
        match dev {
            Ok(d) => FsCoreDevice::into_handle(Arc::new(d)),
            Err(e) => {
                set_last_error(e.to_string());
                ptr::null_mut()
            }
        }
    }));
    match res {
        Ok(p) => p,
        Err(panic) => {
            set_last_error(panic_message(&panic));
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Callback-backed device. Used when the caller already owns the underlying
// resource (FSKit FSBlockDeviceResource, Go file handle, C-side fd) and
// wants to expose it as an `FsCoreDevice` so it can be stacked under a
// container reader (qcow2, vhd, ...) before reaching a filesystem driver.
// ---------------------------------------------------------------------------

/// Read callback. Returns 0 on success, non-zero (errno-like) on failure.
/// Must fully fill `len` bytes — short reads are treated as I/O errors.
pub type FsCoreReadCb =
    Option<unsafe extern "C" fn(ctx: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> c_int>;

/// Write callback. NULL → device is read-only.
pub type FsCoreWriteCb = Option<
    unsafe extern "C" fn(ctx: *mut c_void, offset: u64, buf: *const u8, len: usize) -> c_int,
>;

/// Flush/fsync callback. NULL → flush is a no-op.
pub type FsCoreFlushCb = Option<unsafe extern "C" fn(ctx: *mut c_void) -> c_int>;

/// Configuration passed to [`fs_core_device_from_callbacks`].
#[repr(C)]
pub struct FsCoreCallbackCfg {
    pub read: FsCoreReadCb,
    pub write: FsCoreWriteCb,
    pub flush: FsCoreFlushCb,
    pub ctx: *mut c_void,
    pub size: u64,
}

// `*mut c_void` is `!Send + !Sync` by default and `unsafe impl Send` on a
// NewType doesn't propagate cleanly through closure auto-traits. Round-trip
// the pointer through `usize` instead — that's `Copy + Send + Sync`, and
// the callback contract already puts the host on the hook for thread-safe
// `ctx` use.
fn cb_io_err(rc: c_int, op: &str) -> io::Error {
    io::Error::other(format!("callback {op} returned {rc}"))
}

/// Build an [`FsCoreDevice`] backed by host-provided callbacks. Returns NULL
/// on failure (config null, read callback null, etc.) and stashes detail in
/// the thread-local last-error.
///
/// `cfg.ctx` is opaque to fs-core; it is passed back verbatim to every
/// callback invocation. The caller is responsible for ensuring it remains
/// valid until [`fs_core_device_close`] is called on the returned handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_from_callbacks(
    cfg: *const FsCoreCallbackCfg,
) -> *mut FsCoreDevice {
    if cfg.is_null() {
        set_last_error("cfg is null");
        return ptr::null_mut();
    }
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let cfg = &*cfg;
        let read_fn = match cfg.read {
            Some(f) => f,
            None => {
                set_last_error("cfg.read is null");
                return ptr::null_mut();
            }
        };
        let write_fn = cfg.write;
        let flush_fn = cfg.flush;
        let ctx_addr = cfg.ctx as usize;
        let size = cfg.size;

        let read_cb: crate::callback_device::ReadCb = Box::new(move |off, buf| {
            let ctx = ctx_addr as *mut c_void;
            let rc = read_fn(ctx, off, buf.as_mut_ptr(), buf.len());
            if rc == 0 {
                Ok(())
            } else {
                Err(cb_io_err(rc, "read"))
            }
        });
        let write_cb: Option<crate::callback_device::WriteCb> = write_fn.map(|f| {
            Box::new(move |off, buf: &[u8]| {
                let ctx = ctx_addr as *mut c_void;
                let rc = f(ctx, off, buf.as_ptr(), buf.len());
                if rc == 0 {
                    Ok(())
                } else {
                    Err(cb_io_err(rc, "write"))
                }
            }) as crate::callback_device::WriteCb
        });
        let flush_cb: Option<crate::callback_device::FlushCb> = flush_fn.map(|f| {
            Box::new(move || {
                let ctx = ctx_addr as *mut c_void;
                let rc = f(ctx);
                if rc == 0 {
                    Ok(())
                } else {
                    Err(cb_io_err(rc, "flush"))
                }
            }) as crate::callback_device::FlushCb
        });

        let dev = CallbackDevice {
            size,
            read: read_cb,
            write: write_cb,
            flush: flush_cb,
        };
        FsCoreDevice::into_handle(Arc::new(dev))
    }));
    match res {
        Ok(p) => p,
        Err(panic) => {
            set_last_error(panic_message(&panic));
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Slice constructor. Returns a child `FsCoreDevice` whose byte 0 maps to
// `start` of the parent and whose addressable range is `length` bytes.
// Useful for partition-table walkers that want to hand one partition to
// a filesystem driver without copying. The slice keeps an `Arc` to the
// parent, so closing the parent before the slice is fine.
// ---------------------------------------------------------------------------

/// Read-only slice. Writes via the returned handle return
/// `FS_CORE_READ_ONLY` regardless of the parent's writability.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_slice_ro(
    parent: *const FsCoreDevice,
    start: u64,
    length: u64,
) -> *mut FsCoreDevice {
    if parent.is_null() {
        set_last_error("parent is null");
        return ptr::null_mut();
    }
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let parent_arc = (*parent).inner().clone();
        // OwnedSlice takes Arc<dyn BlockRead>; trait upcast from
        // BlockDevice -> BlockRead is supported in the pinned toolchain.
        let parent_read: Arc<dyn crate::block::BlockRead> = parent_arc;
        let slice = crate::slice::OwnedSlice::new(parent_read, start, length);
        FsCoreDevice::into_handle(Arc::new(slice))
    }));
    match res {
        Ok(p) => p,
        Err(panic) => {
            set_last_error(panic_message(&panic));
            ptr::null_mut()
        }
    }
}

/// Read-write slice. Writes are forwarded to the parent at `start +
/// offset`; writes outside `[0, length)` return `FS_CORE_OUT_OF_BOUNDS`.
/// If the parent reports `is_writable() == false`, write attempts return
/// `FS_CORE_READ_ONLY`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_slice_rw(
    parent: *const FsCoreDevice,
    start: u64,
    length: u64,
) -> *mut FsCoreDevice {
    if parent.is_null() {
        set_last_error("parent is null");
        return ptr::null_mut();
    }
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let parent_arc = (*parent).inner().clone();
        let slice = crate::slice::OwnedRwSlice::new(parent_arc, start, length);
        FsCoreDevice::into_handle(Arc::new(slice))
    }));
    match res {
        Ok(p) => p,
        Err(panic) => {
            set_last_error(panic_message(&panic));
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — exercise the FFI surface from Rust. The C side is verified by
// the consumer crates that use these functions through their own headers.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn tmp_image(bytes: &[u8]) -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static C: AtomicU32 = AtomicU32::new(0);
        let n = C.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir()
            .join(format!("fs_core_ffi_{}_{n}.img", std::process::id()))
            .to_string_lossy()
            .into_owned();
        File::create(&p).unwrap().write_all(bytes).unwrap();
        p
    }

    #[test]
    fn open_read_close_round_trip() {
        let path = tmp_image(b"hello, fs-core ffi");
        let cpath = CString::new(path.as_str()).unwrap();
        let h = unsafe { fs_core_file_open(cpath.as_ptr(), false) };
        assert!(!h.is_null(), "open failed");

        unsafe {
            assert_eq!(fs_core_device_size_bytes(h), 18);
            assert!(!fs_core_device_is_writable(h));

            let mut buf = [0u8; 5];
            let rc = fs_core_device_read_at(h, 0, buf.as_mut_ptr(), buf.len());
            assert_eq!(rc, FsCoreErrorCode::Ok);
            assert_eq!(&buf, b"hello");

            // Write should fail with ReadOnly.
            let rc = fs_core_device_write_at(h, 0, b"x".as_ptr(), 1);
            assert_eq!(rc, FsCoreErrorCode::ReadOnly);

            fs_core_device_close(h);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn null_args_return_null_arg() {
        let mut buf = [0u8; 4];
        let rc = unsafe { fs_core_device_read_at(ptr::null(), 0, buf.as_mut_ptr(), buf.len()) };
        assert_eq!(rc, FsCoreErrorCode::NullArg);
        let rc = unsafe { fs_core_device_flush(ptr::null()) };
        assert_eq!(rc, FsCoreErrorCode::NullArg);
    }

    #[test]
    fn last_error_populated_on_open_failure() {
        let cpath = CString::new("/path/that/does/not/exist/we/hope").unwrap();
        let h = unsafe { fs_core_file_open(cpath.as_ptr(), false) };
        assert!(h.is_null());
        let msg = fs_core_last_error_message();
        assert!(!msg.is_null());
        let s = unsafe { std::ffi::CStr::from_ptr(msg).to_string_lossy().into_owned() };
        assert!(!s.is_empty(), "expected an error message");
    }

    // ---- callback-backed device tests --------------------------------

    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    struct CbState {
        data: Vec<u8>,
        flushed: u32,
    }

    /// Trampoline that pulls a `*mut CbState` out of the opaque ctx.
    unsafe extern "C" fn t_read(ctx: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> c_int {
        let st = unsafe { &mut *(ctx as *mut CbState) };
        let off = offset as usize;
        if off + len > st.data.len() {
            return 5; // out of bounds
        }
        unsafe {
            std::ptr::copy_nonoverlapping(st.data.as_ptr().add(off), buf, len);
        }
        0
    }
    unsafe extern "C" fn t_write(
        ctx: *mut c_void,
        offset: u64,
        buf: *const u8,
        len: usize,
    ) -> c_int {
        let st = unsafe { &mut *(ctx as *mut CbState) };
        let off = offset as usize;
        if off + len > st.data.len() {
            return 5;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(buf, st.data.as_mut_ptr().add(off), len);
        }
        0
    }
    unsafe extern "C" fn t_flush(ctx: *mut c_void) -> c_int {
        let st = unsafe { &mut *(ctx as *mut CbState) };
        st.flushed += 1;
        0
    }

    #[test]
    fn callback_device_round_trip_rw() {
        let mut st = Box::new(CbState {
            data: vec![0u8; 32],
            flushed: 0,
        });
        for (i, b) in st.data.iter_mut().enumerate() {
            *b = i as u8;
        }
        let ctx = &mut *st as *mut CbState as *mut c_void;

        let cfg = FsCoreCallbackCfg {
            read: Some(t_read),
            write: Some(t_write),
            flush: Some(t_flush),
            ctx,
            size: 32,
        };
        let h = unsafe { fs_core_device_from_callbacks(&cfg) };
        assert!(!h.is_null(), "device_from_callbacks returned NULL");

        unsafe {
            assert_eq!(fs_core_device_size_bytes(h), 32);
            assert!(fs_core_device_is_writable(h));

            let mut buf = [0u8; 4];
            let rc = fs_core_device_read_at(h, 4, buf.as_mut_ptr(), buf.len());
            assert_eq!(rc, FsCoreErrorCode::Ok);
            assert_eq!(buf, [4, 5, 6, 7]);

            let payload = [0xDE, 0xAD, 0xBE, 0xEF];
            let rc = fs_core_device_write_at(h, 8, payload.as_ptr(), payload.len());
            assert_eq!(rc, FsCoreErrorCode::Ok);

            let rc = fs_core_device_flush(h);
            assert_eq!(rc, FsCoreErrorCode::Ok);

            let mut readback = [0u8; 4];
            let rc = fs_core_device_read_at(h, 8, readback.as_mut_ptr(), readback.len());
            assert_eq!(rc, FsCoreErrorCode::Ok);
            assert_eq!(readback, payload);

            fs_core_device_close(h);
        }
        assert_eq!(st.flushed, 1);
        assert_eq!(&st.data[8..12], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn callback_device_readonly_when_write_null() {
        let mut st = Box::new(CbState {
            data: vec![0xAAu8; 16],
            flushed: 0,
        });
        let ctx = &mut *st as *mut CbState as *mut c_void;
        let cfg = FsCoreCallbackCfg {
            read: Some(t_read),
            write: None,
            flush: None,
            ctx,
            size: 16,
        };
        let h = unsafe { fs_core_device_from_callbacks(&cfg) };
        assert!(!h.is_null());
        unsafe {
            assert!(!fs_core_device_is_writable(h));
            let rc = fs_core_device_write_at(h, 0, [1u8].as_ptr(), 1);
            assert_eq!(rc, FsCoreErrorCode::ReadOnly);
            // Flush is a no-op when callback is NULL.
            assert_eq!(fs_core_device_flush(h), FsCoreErrorCode::Ok);
            fs_core_device_close(h);
        }
        // suppress unused warning
        let _ = StdArc::new(StdMutex::new(0u8));
    }

    #[test]
    fn callback_device_null_cfg_returns_null() {
        let h = unsafe { fs_core_device_from_callbacks(ptr::null()) };
        assert!(h.is_null());
        let msg = fs_core_last_error_message();
        assert!(!msg.is_null());
    }
}

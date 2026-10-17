//! Transfer locks stay separate from driver handles so the guard is acquired
//! before a driver opens its writable target or reads any producer bytes.

use std::fs::{File, TryLockError};
use std::io::Write;
use std::path::Path;

use super::super::output::CliError;

/// A non-blocking lock on the backing image or device, independent of its
/// partition offset. Keep the guard alive through the entire transfer.
///
/// Imports take an exclusive lock; exports try a shared lock and warn when
/// a writer already holds it. These are cooperative locks: they cannot stop
/// a program that ignores the locking protocol, or prevent path replacement.
/// Host mount-state checks are separate from advisory lock ownership.
#[derive(Debug)]
#[must_use = "dropping the guard releases the image lock"]
pub struct ImageLock {
    _file: File,
}

impl ImageLock {
    /// Lock the existing image exclusively, without blocking, before opening
    /// the driver's writable target or consuming input. This opens only a
    /// read-only handle and never creates or truncates an image.
    ///
    /// Contention and unsupported or failed lock operations have exit code 1.
    pub fn for_import(image: &Path) -> Result<Self, CliError> {
        let file = open(image)?;
        lock(&file, true).map_err(|err| {
            CliError::failed(format!(
                "cannot exclusively lock image {}: {err}",
                image.display()
            ))
        })?;
        Ok(Self { _file: file })
    }

    /// Try to hold a shared image lock for an export. A competing exclusive
    /// lock writes a warning and returns `None`, allowing the export to try
    /// reading bytes that may change. Other failures, including failure to
    /// deliver the warning, return exit code 1 before any source read.
    pub fn for_export(image: &Path, warnings: &mut dyn Write) -> Result<Option<Self>, CliError> {
        let file = open(image)?;
        match lock(&file, false) {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => {
                writeln!(
                    warnings,
                    "warning: image {} is held by another writer; exported bytes may be changing",
                    image.display()
                )
                .map_err(|err| {
                    CliError::failed(format!("cannot report image lock warning: {err}"))
                })?;
                Ok(None)
            }
            Err(err) => Err(CliError::failed(format!(
                "cannot share-lock image {}: {err}",
                image.display()
            ))),
        }
    }
}

fn open(image: &Path) -> Result<File, CliError> {
    File::open(image).map_err(|err| {
        CliError::failed(format!(
            "cannot open image {} for locking: {err}",
            image.display()
        ))
    })
}

#[cfg(not(windows))]
pub(super) fn lock(file: &File, exclusive: bool) -> Result<(), TryLockError> {
    if exclusive {
        file.try_lock()
    } else {
        file.try_lock_shared()
    }
}

// Windows file locks are mandatory for byte I/O, including other handles in
// our own process. Reserve the final representable byte for the cooperative
// lock, outside usable image data, so a driver's separate handle can still
// read and write. Native whole-file locks overlap this byte too. Closing the
// guard's handle releases it. Unix flock already has these advisory semantics.
#[cfg(windows)]
pub(super) fn lock(file: &File, exclusive: bool) -> Result<(), TryLockError> {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut c_void,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LockFileEx(
            file: *mut c_void,
            flags: u32,
            reserved: u32,
            bytes_low: u32,
            bytes_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    let mut overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: u32::MAX - 1,
        offset_high: u32::MAX,
        event: std::ptr::null_mut(),
    };
    // LOCKFILE_FAIL_IMMEDIATELY | LOCKFILE_EXCLUSIVE_LOCK
    let flags = 1 | if exclusive { 2 } else { 0 };
    // SAFETY: the live File supplies a valid synchronous handle, the C-layout
    // OVERLAPPED is initialized and lives through this synchronous call, and
    // one byte is locked without reading or writing any image data.
    if unsafe { LockFileEx(file.as_raw_handle(), flags, 0, 1, 0, &mut overlapped) } != 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(33) {
        // ERROR_LOCK_VIOLATION
        Err(TryLockError::WouldBlock)
    } else {
        Err(TryLockError::Error(err))
    }
}

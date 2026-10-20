//! Transfer locks stay separate from driver handles so the guard is acquired
//! before a driver opens its writable target or reads any producer bytes.

use std::fs::{File, TryLockError};
use std::io::Write;
use std::path::Path;

use super::super::output::CliError;

#[cfg(target_os = "linux")]
#[path = "transfer_mount_linux.rs"]
mod host_mount;

#[cfg(target_os = "macos")]
#[path = "transfer_mount_macos.rs"]
mod host_mount;

/// A non-blocking lock on the backing image or device, independent of its
/// partition offset. Keep the guard alive through the entire transfer.
///
/// Imports take an exclusive lock; exports try a shared lock and warn when
/// a writer already holds it. On macOS, a mounted target is governed by the
/// host mount instead: a writable mount warns and exports proceed without an
/// image lock, since DiskImages may refuse opening its device while mounted.
/// These are cooperative locks: they cannot stop a program that ignores the
/// locking protocol, or prevent path replacement.
/// Linux also checks the current mount namespace, including loop-backed
/// images and mounted partitions. This is a preflight snapshot, not a lock
/// against subsequent mounts or mounts hidden in another namespace.
/// macOS checks attached disk images, their slices and synthesized APFS
/// containers against the mount table, with the same snapshot limits.
#[derive(Debug)]
#[must_use = "dropping the guard releases the image lock"]
pub struct ImageLock {
    _file: File,
}

#[cfg(unix)]
impl Drop for ImageLock {
    fn drop(&mut self) {
        // CLOEXEC only closes an inherited descriptor at exec. A concurrent
        // fork can still retain this open description when our File closes,
        // so release its flock explicitly before closing our descriptor.
        let _ = self._file.unlock();
    }
}

impl ImageLock {
    /// Lock the existing image exclusively, without blocking, before opening
    /// the driver's writable target or consuming input. This opens only a
    /// read-only handle and never creates or truncates an image.
    ///
    /// Contention and unsupported or failed lock operations have exit code 1.
    /// Linux mount probes also fail closed with exit code 1, including when
    /// another loop's backing identity is unreadable or a Btrfs/ZFS mount
    /// prevents reliable device membership resolution. macOS mount probes
    /// run before the lock, because DiskImages may lock an attached image.
    pub fn for_import(image: &Path) -> Result<Self, CliError> {
        let file = open(image)?;
        #[cfg(target_os = "macos")]
        refuse_mounted(image)?;
        lock(&file, true).map_err(|err| {
            CliError::failed(format!(
                "cannot exclusively lock image {}: {err}",
                image.display()
            ))
        })?;
        let guard = Self { _file: file };
        #[cfg(target_os = "linux")]
        refuse_mounted(image)?;
        Ok(guard)
    }

    /// Try to hold a shared image lock for an export. A competing exclusive
    /// lock writes a warning and returns `None`, allowing the export to try
    /// reading bytes that may change. On macOS, a mounted target is not
    /// lockable independently; a writable mount warns, and both mount modes
    /// proceed without a guard. Other failures, including failure to deliver
    /// the warning, return exit code 1 before any source read.
    pub fn for_export(image: &Path, warnings: &mut dyn Write) -> Result<Option<Self>, CliError> {
        // DiskImages can hold an attached device busy while it is mounted.
        // Consult the mount table before opening that device: exports from a
        // mounted target cannot acquire our cooperative lock, but remain
        // allowed, with an explicit warning for a writable mount.
        #[cfg(target_os = "macos")]
        {
            let state = host_mount::state(image)?;
            if state.read_write {
                writeln!(
                    warnings,
                    "warning: image {} is mounted read-write; exported bytes may be changing",
                    image.display()
                )
                .map_err(|err| {
                    CliError::failed(format!("cannot report image mount warning: {err}"))
                })?;
            }
            if state.mounted {
                return Ok(None);
            }
        }

        let file = open(image)?;
        #[cfg(target_os = "linux")]
        if host_mount::state(image)?.read_write {
            writeln!(
                warnings,
                "warning: image {} is mounted read-write; exported bytes may be changing",
                image.display()
            )
            .map_err(|err| CliError::failed(format!("cannot report image mount warning: {err}")))?;
        }
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn refuse_mounted(image: &Path) -> Result<(), CliError> {
    if host_mount::state(image)?.mounted {
        return Err(CliError::failed(format!(
            "cannot import into host-mounted image {}",
            image.display()
        )));
    }
    Ok(())
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

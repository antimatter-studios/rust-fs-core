//! C-ABI slice constructors (`fs_core_device_slice_ro`/`_rw`) end-to-end.
//!
//! Mirrors the inline FFI test style: build a file-backed parent handle,
//! slice it, then exercise reads/writes through the slice handle.

use fs_core::ffi::{
    fs_core_device_close, fs_core_device_is_writable, fs_core_device_read_at,
    fs_core_device_size_bytes, fs_core_device_slice_ro, fs_core_device_slice_rw,
    fs_core_device_write_at, fs_core_file_open, fs_core_last_error_message, FsCoreErrorCode,
};
use std::ffi::CString;
use std::fs::File;
use std::io::Write;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

fn tmp_image(bytes: &[u8]) -> String {
    static C: AtomicU32 = AtomicU32::new(0);
    let n = C.fetch_add(1, Ordering::Relaxed);
    let p = format!("/tmp/fs_core_ffi_slice_{}_{n}.img", std::process::id());
    File::create(&p).unwrap().write_all(bytes).unwrap();
    p
}

fn open_handle(path: &str, writable: bool) -> *mut fs_core::ffi::FsCoreDevice {
    let cpath = CString::new(path).unwrap();
    unsafe { fs_core_file_open(cpath.as_ptr(), writable) }
}

#[test]
fn slice_ro_reads_rebased_window_from_parent() {
    let bytes: Vec<u8> = (0..64).map(|i| i as u8).collect();
    let path = tmp_image(&bytes);
    let parent = open_handle(&path, false);
    assert!(!parent.is_null());

    unsafe {
        let slice = fs_core_device_slice_ro(parent, 16, 8);
        assert!(!slice.is_null());
        assert_eq!(fs_core_device_size_bytes(slice), 8);
        assert!(!fs_core_device_is_writable(slice));

        let mut buf = [0u8; 8];
        let rc = fs_core_device_read_at(slice, 0, buf.as_mut_ptr(), buf.len());
        assert_eq!(rc, FsCoreErrorCode::Ok);
        assert_eq!(buf, [16, 17, 18, 19, 20, 21, 22, 23]);

        fs_core_device_close(slice);
        fs_core_device_close(parent);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn slice_ro_write_returns_read_only_even_with_writable_parent() {
    let path = tmp_image(&[0u8; 64]);
    let parent = open_handle(&path, true);
    assert!(!parent.is_null());

    unsafe {
        let slice = fs_core_device_slice_ro(parent, 0, 32);
        assert!(!slice.is_null());
        assert!(!fs_core_device_is_writable(slice));

        let rc = fs_core_device_write_at(slice, 0, [0xFFu8].as_ptr(), 1);
        assert_eq!(rc, FsCoreErrorCode::ReadOnly);

        fs_core_device_close(slice);
        fs_core_device_close(parent);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn slice_rw_write_lands_at_parent_start_plus_offset() {
    let path = tmp_image(&[0u8; 64]);
    let parent = open_handle(&path, true);
    assert!(!parent.is_null());

    unsafe {
        let slice = fs_core_device_slice_rw(parent, 16, 16);
        assert!(!slice.is_null());
        assert!(fs_core_device_is_writable(slice));

        let payload = [0xAA, 0xBB, 0xCC, 0xDD];
        let rc = fs_core_device_write_at(slice, 4, payload.as_ptr(), payload.len());
        assert_eq!(rc, FsCoreErrorCode::Ok);

        // Read back through the parent at the expected absolute offset (20).
        let mut buf = [0u8; 4];
        let rc = fs_core_device_read_at(parent, 20, buf.as_mut_ptr(), buf.len());
        assert_eq!(rc, FsCoreErrorCode::Ok);
        assert_eq!(buf, payload);

        // Also read back through the slice for round-trip consistency.
        let mut sbuf = [0u8; 4];
        let rc = fs_core_device_read_at(slice, 4, sbuf.as_mut_ptr(), sbuf.len());
        assert_eq!(rc, FsCoreErrorCode::Ok);
        assert_eq!(sbuf, payload);

        fs_core_device_close(slice);
        fs_core_device_close(parent);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn slice_rw_write_past_length_returns_out_of_bounds() {
    let path = tmp_image(&[0u8; 64]);
    let parent = open_handle(&path, true);

    unsafe {
        let slice = fs_core_device_slice_rw(parent, 0, 8);
        let rc = fs_core_device_write_at(slice, 6, [1u8; 4].as_ptr(), 4);
        assert_eq!(rc, FsCoreErrorCode::OutOfBounds);

        fs_core_device_close(slice);
        fs_core_device_close(parent);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn slice_rw_over_readonly_parent_reports_readonly_on_write() {
    let path = tmp_image(&[0u8; 64]);
    let parent = open_handle(&path, false); // read-only

    unsafe {
        let slice = fs_core_device_slice_rw(parent, 0, 16);
        assert!(!slice.is_null());
        // RW slice over a RO parent: is_writable is forwarded from parent.
        assert!(!fs_core_device_is_writable(slice));

        let rc = fs_core_device_write_at(slice, 0, [1u8; 4].as_ptr(), 4);
        assert_eq!(rc, FsCoreErrorCode::ReadOnly);

        fs_core_device_close(slice);
        fs_core_device_close(parent);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn slice_ro_read_past_length_returns_short_read() {
    let path = tmp_image(&[1u8; 64]);
    let parent = open_handle(&path, false);
    unsafe {
        let slice = fs_core_device_slice_ro(parent, 0, 8);
        let mut buf = [0u8; 4];
        let rc = fs_core_device_read_at(slice, 6, buf.as_mut_ptr(), buf.len());
        assert_eq!(rc, FsCoreErrorCode::ShortRead);

        fs_core_device_close(slice);
        fs_core_device_close(parent);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn slice_ro_null_parent_returns_null_and_sets_last_error() {
    unsafe {
        let h = fs_core_device_slice_ro(ptr::null(), 0, 16);
        assert!(h.is_null());
        let msg = fs_core_last_error_message();
        assert!(!msg.is_null());
        let s = std::ffi::CStr::from_ptr(msg).to_string_lossy();
        assert!(s.contains("parent"));
    }
}

#[test]
fn slice_rw_null_parent_returns_null_and_sets_last_error() {
    unsafe {
        let h = fs_core_device_slice_rw(ptr::null(), 0, 16);
        assert!(h.is_null());
        let msg = fs_core_last_error_message();
        assert!(!msg.is_null());
    }
}

#[test]
fn slice_survives_parent_close_via_arc_ownership() {
    // OwnedSlice/OwnedRwSlice hold an Arc to the parent — closing the parent
    // handle drops one Arc reference but the slice keeps its own.
    let bytes: Vec<u8> = (0..32).map(|i| i as u8).collect();
    let path = tmp_image(&bytes);
    let parent = open_handle(&path, false);

    unsafe {
        let slice = fs_core_device_slice_ro(parent, 8, 16);
        assert!(!slice.is_null());

        // Close the parent handle FIRST.
        fs_core_device_close(parent);

        // Slice must still read correctly.
        let mut buf = [0u8; 4];
        let rc = fs_core_device_read_at(slice, 0, buf.as_mut_ptr(), buf.len());
        assert_eq!(rc, FsCoreErrorCode::Ok);
        assert_eq!(buf, [8, 9, 10, 11]);

        fs_core_device_close(slice);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn closing_null_slice_handle_is_safe_noop() {
    unsafe {
        fs_core_device_close(ptr::null_mut());
    }
}

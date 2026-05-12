//! CallbackDevice with write + flush callbacks wired up.

use fs_core::{BlockDevice, BlockRead, CallbackDevice};
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[test]
fn with_writer_is_writable_and_dispatches_writes() {
    let storage: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(vec![0u8; 64]));
    let r = storage.clone();
    let w = storage.clone();
    let dev = CallbackDevice {
        size: 64,
        read: Box::new(move |off, buf| {
            let b = r.lock().unwrap();
            let s = off as usize;
            buf.copy_from_slice(&b[s..s + buf.len()]);
            Ok(())
        }),
        write: Some(Box::new(move |off, buf| {
            let mut b = w.lock().unwrap();
            let s = off as usize;
            b[s..s + buf.len()].copy_from_slice(buf);
            Ok(())
        })),
        flush: None,
    };
    assert!(dev.is_writable());
    dev.write_at(8, &[0xAA, 0xBB, 0xCC]).unwrap();
    let mut buf = [0u8; 3];
    dev.read_at(8, &mut buf).unwrap();
    assert_eq!(buf, [0xAA, 0xBB, 0xCC]);
}

#[test]
fn flush_callback_invoked_when_present() {
    let flushes = Arc::new(AtomicU32::new(0));
    let f = flushes.clone();
    let dev = CallbackDevice {
        size: 4,
        read: Box::new(|_, buf| {
            buf.fill(0);
            Ok(())
        }),
        write: None,
        flush: Some(Box::new(move || {
            f.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })),
    };
    dev.flush().unwrap();
    dev.flush().unwrap();
    assert_eq!(flushes.load(Ordering::Relaxed), 2);
}

#[test]
fn flush_without_callback_is_noop_ok() {
    let dev = CallbackDevice {
        size: 4,
        read: Box::new(|_, buf| {
            buf.fill(0);
            Ok(())
        }),
        write: None,
        flush: None,
    };
    dev.flush().unwrap();
}

#[test]
fn read_callback_io_error_propagates_via_io_variant() {
    let dev = CallbackDevice {
        size: 16,
        read: Box::new(|_, _| Err(io::Error::new(io::ErrorKind::UnexpectedEof, "boom"))),
        write: None,
        flush: None,
    };
    let mut buf = [0u8; 8];
    let err = dev.read_at(0, &mut buf).unwrap_err();
    match err {
        fs_core::Error::Io(inner) => assert_eq!(inner.kind(), io::ErrorKind::UnexpectedEof),
        other => panic!("expected Error::Io, got {other:?}"),
    }
}

#[test]
fn write_callback_io_error_propagates_via_io_variant() {
    let dev = CallbackDevice {
        size: 8,
        read: Box::new(|_, buf| {
            buf.fill(0);
            Ok(())
        }),
        write: Some(Box::new(|_, _| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "ro"))
        })),
        flush: None,
    };
    assert!(dev.is_writable());
    let err = dev.write_at(0, &[1, 2, 3]).unwrap_err();
    match err {
        fs_core::Error::Io(inner) => assert_eq!(inner.kind(), io::ErrorKind::PermissionDenied),
        other => panic!("expected Error::Io, got {other:?}"),
    }
}

#[test]
fn size_bytes_matches_declared_size() {
    let dev = CallbackDevice {
        size: 12345,
        read: Box::new(|_, buf| {
            buf.fill(0);
            Ok(())
        }),
        write: None,
        flush: None,
    };
    assert_eq!(dev.size_bytes(), 12345);
}

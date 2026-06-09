//! FileDevice edge cases: open errors, EOF handling, zero-length reads,
//! multi-chunk reads, size reporting.

use fs_core::{BlockDevice, BlockRead, Error, FileDevice};
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

fn tmp_image(bytes: &[u8]) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir()
        .join(format!("fs_core_fd_edge_{}_{n}.img", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let mut f = File::create(&path).unwrap();
    f.write_all(bytes).unwrap();
    path
}

#[test]
fn open_nonexistent_returns_io_error() {
    let path = std::env::temp_dir()
        .join(format!(
            "fs_core_does_not_exist_{}_{}.img",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_string_lossy()
        .into_owned();
    match FileDevice::open(&path) {
        Err(Error::Io(_)) => {}
        Err(e) => panic!("expected Error::Io, got {e:?}"),
        Ok(_) => panic!("expected open() to fail on nonexistent path"),
    }
}

#[test]
fn open_rw_nonexistent_returns_io_error() {
    let path = std::env::temp_dir()
        .join(format!("fs_core_rw_missing_{}.img", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let _ = std::fs::remove_file(&path);
    match FileDevice::open_rw(&path) {
        Err(Error::Io(_)) => {}
        Err(e) => panic!("expected Error::Io, got {e:?}"),
        Ok(_) => panic!("expected open_rw() to fail on nonexistent path"),
    }
}

#[test]
fn size_bytes_matches_file_length() {
    let path = tmp_image(&[0u8; 1234]);
    let dev = FileDevice::open(&path).unwrap();
    assert_eq!(dev.size_bytes(), 1234);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn zero_length_read_succeeds() {
    let path = tmp_image(&[0u8; 16]);
    let dev = FileDevice::open(&path).unwrap();
    let mut buf: [u8; 0] = [];
    dev.read_at(0, &mut buf).unwrap();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn read_exactly_at_end_zero_length_ok() {
    // A zero-byte read at offset == size should not error (no bytes wanted).
    let path = tmp_image(&[0u8; 8]);
    let dev = FileDevice::open(&path).unwrap();
    let mut buf: [u8; 0] = [];
    dev.read_at(8, &mut buf).unwrap();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn read_past_eof_returns_short_read() {
    let path = tmp_image(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let dev = FileDevice::open(&path).unwrap();
    let mut buf = [0u8; 16];
    match dev.read_at(4, &mut buf) {
        Err(Error::ShortRead { offset, want, got }) => {
            assert_eq!(offset, 4);
            assert_eq!(want, 16);
            // got is the partial bytes actually read before EOF (4 bytes left).
            assert_eq!(got, 4);
        }
        other => panic!("expected ShortRead, got {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn read_starting_at_eof_returns_short_read_with_zero_got() {
    let path = tmp_image(&[1, 2, 3, 4]);
    let dev = FileDevice::open(&path).unwrap();
    let mut buf = [0u8; 4];
    match dev.read_at(4, &mut buf) {
        Err(Error::ShortRead {
            offset: 4,
            want: 4,
            got: 0,
        }) => {}
        other => panic!("expected ShortRead with got=0, got {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_chunk_read_walks_entire_file() {
    let total = 64 * 1024;
    let bytes: Vec<u8> = (0..total).map(|i| (i % 251) as u8).collect();
    let path = tmp_image(&bytes);
    let dev = FileDevice::open(&path).unwrap();

    let chunk = 4096;
    let mut buf = vec![0u8; chunk];
    let mut offset = 0u64;
    let mut all = Vec::with_capacity(total);
    while (offset as usize) < total {
        let remaining = total - offset as usize;
        let n = std::cmp::min(remaining, chunk);
        dev.read_at(offset, &mut buf[..n]).unwrap();
        all.extend_from_slice(&buf[..n]);
        offset += n as u64;
    }
    assert_eq!(all, bytes);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_ro_then_flush_is_noop() {
    let path = tmp_image(&[0u8; 4]);
    let dev = FileDevice::open(&path).unwrap();
    // Default flush impl on a ro FileDevice should be Ok per implementation.
    dev.flush().unwrap();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_best_effort_falls_back_to_readonly() {
    // For a brand-new file, both rw and ro will succeed; the contract is
    // that open_best_effort returns something usable.
    let path = tmp_image(&[42u8; 16]);
    let dev = FileDevice::open_best_effort(&path).unwrap();
    let mut buf = [0u8; 1];
    dev.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, [42]);
    let _ = std::fs::remove_file(&path);
}

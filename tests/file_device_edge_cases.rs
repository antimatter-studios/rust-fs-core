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

/// A WRITE PAST THE END IS REFUSED, AND THE FILE ON DISK IS THE ORACLE.
///
/// `write_all` at a seeked offset extends a file, and `write_at` had no
/// bound of its own, so a write straddling the end grew the backing
/// store while `size_bytes` went on reporting its construction-time
/// length. Measured before the bound existed, on this fixture:
/// `write_at(4094, 8)` returned `Ok`, the file became 4102 bytes,
/// `size_bytes()` stayed 4096, and `read_at(4096, 6)` handed the
/// written bytes back. See rust-fs-core#70.
///
/// `open_rw` DELIBERATELY, not `open`: on a read-only handle `write_at`
/// returns `Error::ReadOnly` before it reaches the seek, the file
/// cannot grow, and this test would pass with the bound removed --
/// pinning nothing.
#[test]
fn a_write_past_the_end_is_refused_and_does_not_extend_the_file() {
    let path = tmp_image(&[3u8; 4096]);
    let dev = FileDevice::open_rw(&path).unwrap();
    assert_eq!(dev.size_bytes(), 4096, "control: the declared size");

    // Straddling: two bytes inside, six past the end.
    match dev.write_at(4094, &[1u8; 8]) {
        Err(Error::OutOfBounds { offset, len, size }) => {
            assert_eq!((offset, len, size), (4094, 8, 4096));
        }
        Err(e) => panic!("expected OutOfBounds, got {e:?}"),
        Ok(()) => panic!("a write past the end must be refused, not extend the device"),
    }

    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        4096,
        "the refused write must not have extended the backing file -- this read 4102 \
         before the bound existed, six bytes the device then denied having"
    );
    assert_eq!(
        dev.size_bytes(),
        4096,
        "and the reported size is unmoved, which it was before too: the defect was \
         never that this number changed, it was that the file underneath it did"
    );

    // The bytes just inside the end are untouched: a refused write is
    // refused whole, not applied up to the boundary.
    let mut tail = [0u8; 2];
    dev.read_at(4094, &mut tail).unwrap();
    assert_eq!(
        tail, [3u8; 2],
        "a partial application would be worse than either"
    );

    // And an in-range write at the very end still works, so the bound
    // is off-by-one in neither direction.
    dev.write_at(4088, &[9u8; 8])
        .expect("a write ending exactly at the end is in range");
    let mut last = [0u8; 8];
    dev.read_at(4088, &mut last).unwrap();
    assert_eq!(last, [9u8; 8]);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 4096);

    // THE OTHER DIRECTION, which that sentence claimed and nothing here
    // measured. A single byte AT the end is the only write whose end is
    // exactly `size + 1`, so it is the only one that can tell `end >
    // size` from `end > size + 1`. The straddling write above cannot
    // stand in for it: its end is 4102, which an off-by-one bound
    // refuses anyway, leaving that mutation alive with the whole suite
    // green.
    match dev.write_at(4096, &[9u8; 1]) {
        Err(Error::OutOfBounds { offset, len, size }) => {
            assert_eq!((offset, len, size), (4096, 1, 4096));
        }
        Err(e) => panic!("expected OutOfBounds, got {e:?}"),
        Ok(()) => panic!("a write starting at the end must be refused"),
    }
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        4096,
        "nor may the refused one-byte write have extended the file by that byte"
    );

    let _ = std::fs::remove_file(&path);
}

/// An offset near `u64::MAX` must not wrap back inside the device.
///
/// `offset + buf.len()` on a caller-supplied offset is exactly the
/// arithmetic that lands a write somewhere it was never asked to go,
/// and the bound is useless if the sum it compares has wrapped.
#[test]
fn a_write_at_an_offset_that_would_overflow_the_sum_is_refused() {
    let path = tmp_image(&[3u8; 64]);
    let dev = FileDevice::open_rw(&path).unwrap();

    match dev.write_at(u64::MAX - 2, &[1u8; 8]) {
        Err(Error::OutOfBounds { offset, .. }) => assert_eq!(offset, u64::MAX - 2),
        Err(e) => panic!("expected OutOfBounds, got {e:?}"),
        Ok(()) => panic!("an offset whose end overflows must be refused"),
    }
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 64);
    let _ = std::fs::remove_file(&path);
}

//! A read that overlaps a write must not leave pre-write bytes in the cache.
//!
//! `CachingDevice::write_at` drops the entries a write would make stale, and
//! it does so *before* the device write, so a read that misses inside the
//! window fetches pre-write bytes and inserts them after the sweep has gone
//! past. Nothing invalidates that entry again, so the cache serves bytes the
//! device no longer holds for as long as the entry stays resident — which,
//! because every hit refreshes its recency, is potentially forever.

use fs_core::block::{BlockDevice, BlockRead};
use fs_core::error::{Error, Result};
use fs_core::CachingDevice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// A device that parks, once, inside whichever operation it was armed for,
/// after announcing that it has got that far.
///
/// The two tests below need the two halves of the same window. Arming the
/// read gives "the reader has pre-write bytes in hand and has not cached
/// them yet, and the write is free to run to completion". Arming the write
/// gives the mirror: "the write has swept the cache and has not yet
/// applied its bytes, and a whole miss can complete inside that".
struct ParkingDevice {
    data: Mutex<Vec<u8>>,
    park_in_read: AtomicBool,
    park_in_write: AtomicBool,
    sampled: Mutex<Sender<()>>,
    release: Mutex<Receiver<()>>,
}

impl ParkingDevice {
    /// Announce, then wait. The caller must not be holding `data`.
    fn park(&self) {
        self.sampled.lock().unwrap().send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
    }
}

impl BlockRead for ParkingDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        {
            let d = self.data.lock().unwrap();
            let start = offset as usize;
            let end = start + buf.len();
            if end > d.len() {
                return Err(Error::ShortRead {
                    offset,
                    want: buf.len(),
                    got: d.len().saturating_sub(start),
                });
            }
            buf.copy_from_slice(&d[start..end]);
        }
        // The data lock is released before parking, so the writer below can
        // still reach the device.
        if self.park_in_read.swap(false, Ordering::SeqCst) {
            self.park();
        }
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.data.lock().unwrap().len() as u64
    }
}

impl BlockDevice for ParkingDevice {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        // BEFORE the bytes are applied, and without the data lock held, so
        // a reader can complete an entire miss — device read included —
        // while this write is still in flight.
        if self.park_in_write.swap(false, Ordering::SeqCst) {
            self.park();
        }
        let mut d = self.data.lock().unwrap();
        let start = offset as usize;
        let end = start + buf.len();
        if end > d.len() {
            return Err(Error::OutOfBounds {
                offset,
                len: buf.len() as u64,
                size: d.len() as u64,
            });
        }
        d[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn is_writable(&self) -> bool {
        true
    }
}

#[test]
fn a_read_racing_a_write_does_not_leave_pre_write_bytes_in_the_cache() {
    let (sampled_tx, sampled_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();

    let dev = Arc::new(ParkingDevice {
        data: Mutex::new(vec![0xAA; 8]),
        park_in_read: AtomicBool::new(true),
        park_in_write: AtomicBool::new(false),
        sampled: Mutex::new(sampled_tx),
        release: Mutex::new(release_rx),
    });

    let cache = CachingDevice::new(dev.clone(), 8, 4);

    let reader = {
        let cache = cache.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8];
            cache.read_at(0, &mut buf).expect("racing read");
            buf
        })
    };

    // The reader now holds the pre-write bytes and has not cached them yet.
    sampled_rx.recv().expect("the reader sampled the device");

    // The write runs to completion inside that window: it sweeps the cache,
    // reaches the device, and returns.
    cache.write_at(0, &[0xBB; 8]).expect("write");

    // Only now does the reader get to insert what it read.
    release_tx.send(()).expect("release the reader");
    let raced = reader.join().expect("reader thread");
    assert_eq!(
        raced, [0xAA; 8],
        "the racing read legitimately saw pre-write bytes — it began before the write"
    );

    // The device holds 0xBB. Every read from here on must say so.
    let mut direct = [0u8; 8];
    dev.read_at(0, &mut direct)
        .expect("straight off the device");
    assert_eq!(direct, [0xBB; 8], "the device really did take the write");

    let mut after = [0u8; 8];
    cache.read_at(0, &mut after).expect("read after the write");
    assert_eq!(
        after, [0xBB; 8],
        "the cache is serving pre-write bytes after a completed write"
    );
}

/// THE OTHER HALF OF THE WINDOW, and the one the generation counter
/// cannot close.
///
/// Above, the read begins before the write and is caught by the counter:
/// it recorded the generation before the write's first sweep bumped it, so
/// its insert is refused. Here the read begins *after* that sweep, so it
/// records the already-bumped value and the counter agrees with it. Only
/// the second sweep — the one after the device write returns — drops the
/// entry it inserted.
///
/// Without that second sweep the whole suite stays green and the cache
/// serves pre-write bytes for the life of the mount, which is why this
/// test exists separately rather than as another assertion on the first.
#[test]
fn a_read_that_completes_inside_a_write_does_not_survive_it() {
    let (sampled_tx, sampled_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();

    let dev = Arc::new(ParkingDevice {
        data: Mutex::new(vec![0xAA; 8]),
        park_in_read: AtomicBool::new(false),
        park_in_write: AtomicBool::new(true),
        sampled: Mutex::new(sampled_tx),
        release: Mutex::new(release_rx),
    });

    let cache = CachingDevice::new(dev.clone(), 8, 4);

    let writer = {
        let cache = cache.clone();
        thread::spawn(move || cache.write_at(0, &[0xBB; 8]).expect("write"))
    };

    // The write has swept the cache and reached the device, and is holding
    // there with its bytes not yet applied.
    sampled_rx.recv().expect("the write reached the device");

    // A complete miss runs inside that window: it reads 0xAA off the
    // device and caches it, with the sweep already behind it.
    let mut during = [0u8; 8];
    cache
        .read_at(0, &mut during)
        .expect("read during the write");
    assert_eq!(
        during, [0xAA; 8],
        "the read began before the write landed, so these bytes are legitimate"
    );

    release_tx.send(()).expect("release the writer");
    writer.join().expect("writer thread");

    let mut direct = [0u8; 8];
    dev.read_at(0, &mut direct)
        .expect("straight off the device");
    assert_eq!(direct, [0xBB; 8], "the device really did take the write");

    let mut after = [0u8; 8];
    cache.read_at(0, &mut after).expect("read after the write");
    assert_eq!(
        after, [0xBB; 8],
        "the cache is serving bytes it fetched before the write landed"
    );
}

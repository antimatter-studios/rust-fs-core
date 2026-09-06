//! A device that counts what a driver asks of it.
//!
//! # Why this is in the shared crate rather than a test file
//!
//! Every driver in this family is about to be measured and then made
//! faster, and a measurement is only worth having if the drivers can be
//! compared against each other and against themselves later. Two
//! drivers each counting reads with their own wrapper would produce two
//! numbers that look alike and are not: one might count a read of a
//! whole extent as one, the other as one per block, and nothing in
//! either number would say so.
//!
//! One instrument, in the crate every driver already depends on. It
//! wraps a [`BlockRead`] and forwards every call, so a driver mounted
//! on it behaves exactly as it would otherwise.
//!
//! # What the numbers mean
//!
//! - **reads** — calls to [`BlockRead::read_at`]. This is the number a
//!   cache moves: a metadata block read twice is two reads here and one
//!   after a cache is put underneath.
//! - **bytes** — the total of every buffer those calls filled. This is
//!   what the *device* moves, which is not the same thing: a driver
//!   that reads a 4 KiB block to look at 8 bytes of it moves 4 KiB, and
//!   only the read count will show the waste.
//!
//! Both are worth having. A change that halves reads and doubles bytes
//! is a readahead that guessed wrong, and one number alone would call
//! it a win.

use crate::block::BlockRead;
use crate::error::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Wraps a device and counts the reads passing through it.
///
/// The counters are atomic and the type is `Sync`, so a driver reading
/// from several threads is measured correctly rather than approximately.
pub struct CountingDevice {
    inner: Arc<dyn BlockRead>,
    reads: AtomicU64,
    bytes: AtomicU64,
}

impl CountingDevice {
    /// Wrap `inner`, counting from zero.
    pub fn new(inner: Arc<dyn BlockRead>) -> Self {
        CountingDevice {
            inner,
            reads: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }

    /// How many times the driver called `read_at`.
    pub fn reads(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }

    /// How many bytes those calls asked for.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// Start counting again from zero.
    ///
    /// A mount reads a superblock and headers before the work being
    /// measured begins, and counting that in makes a small operation
    /// look like a large one. Reset after mounting, measure the
    /// operation, read the counters.
    pub fn reset(&self) {
        self.reads.store(0, Ordering::Relaxed);
        self.bytes.store(0, Ordering::Relaxed);
    }
}

impl BlockRead for CountingDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
        self.inner.read_at(offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.inner.size_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_device::Bytes;

    fn device(len: usize) -> Arc<CountingDevice> {
        Arc::new(CountingDevice::new(Arc::new(Bytes::new(vec![7u8; len]))))
    }

    /// One call is one read, whatever it asked for, and the bytes are
    /// what the buffer wanted rather than what the device holds.
    #[test]
    fn it_counts_calls_and_the_bytes_they_asked_for() {
        let dev = device(4096);
        let mut small = [0u8; 8];
        let mut block = [0u8; 512];

        dev.read_at(0, &mut small).expect("read");
        assert_eq!((dev.reads(), dev.bytes()), (1, 8));

        dev.read_at(1024, &mut block).expect("read");
        assert_eq!(
            (dev.reads(), dev.bytes()),
            (2, 520),
            "two calls, and the bytes are the sum of both buffers"
        );
    }

    /// A read the device refuses is still a read the driver made.
    ///
    /// The point of the count is what the driver ASKED for, so a failed
    /// call belongs in it: a driver looping on an out-of-range offset is
    /// exactly the shape this is here to make visible.
    #[test]
    fn a_failed_read_still_counts() {
        let dev = device(16);
        let mut buf = [0u8; 64];
        assert!(dev.read_at(0, &mut buf).is_err(), "past the end");
        assert_eq!(dev.reads(), 1, "the driver asked, so it counts");
    }

    /// Resetting drops the mount's own reads, which is the whole reason
    /// it exists: an operation measured with them included is measured
    /// against a constant that has nothing to do with it.
    #[test]
    fn resetting_starts_the_measurement_where_the_work_does() {
        let dev = device(4096);
        let mut buf = [0u8; 64];
        dev.read_at(0, &mut buf).expect("the mount's own reads");
        dev.reset();
        assert_eq!((dev.reads(), dev.bytes()), (0, 0));

        dev.read_at(64, &mut buf).expect("the work being measured");
        assert_eq!((dev.reads(), dev.bytes()), (1, 64));
    }
}

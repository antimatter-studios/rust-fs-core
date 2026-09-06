//! Small LRU read-cache decorator. Caches only block-aligned, block-sized
//! reads; everything else passes through. Writes invalidate any overlapping
//! cached entries.

use crate::block::{BlockDevice, BlockRead};
use crate::error::Result;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// LRU read-cache wrapper.
///
/// # It caches a READ device, and writes through one only if it has one
///
/// This took an `Arc<dyn BlockDevice>` — the read *write* trait — and
/// every driver in this family mounts a volume through an
/// `Arc<dyn BlockRead>`. So a read-only mount could not wrap it at all,
/// and four of the six drivers used no cache: not by choice, but
/// because it was not expressible.
///
/// The read path never needed to write. It holds the read half now, and
/// the writable half only when the caller had one to give:
/// [`CachingDevice::new`] for a device that can be written,
/// [`CachingDevice::read_only`] for one that cannot. A write to a cache
/// built the second way is [`Error::ReadOnly`], which is what the
/// underlying device would have said.
pub struct CachingDevice {
    inner: Arc<dyn BlockRead>,
    /// The same device again, present only when it can be written. Held
    /// separately rather than as one handle so that "can this be
    /// written" is a property of the type rather than a flag someone
    /// has to remember to check.
    writable: Option<Arc<dyn BlockDevice>>,
    block_size: u64,
    state: Mutex<CacheState>,
}

struct CacheState {
    /// Fixed-capacity LRU; head is most-recently used.
    entries: VecDeque<(u64, Arc<Vec<u8>>)>,
    capacity: usize,
    hits: u64,
    misses: u64,
}

impl CachingDevice {
    /// Cache a device that can be written. Writes invalidate the
    /// entries they overlap and go through to `inner`.
    pub fn new(inner: Arc<dyn BlockDevice>, block_size: u64, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: inner.clone(),
            writable: Some(inner),
            block_size,
            state: Mutex::new(CacheState {
                entries: VecDeque::with_capacity(capacity),
                capacity,
                hits: 0,
                misses: 0,
            }),
        })
    }

    /// Cache a device that is only ever read.
    ///
    /// The case every driver here actually has: a volume mounted for
    /// reading, behind a `BlockRead` that was never a `BlockDevice`.
    pub fn read_only(inner: Arc<dyn BlockRead>, block_size: u64, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner,
            writable: None,
            block_size,
            state: Mutex::new(CacheState {
                entries: VecDeque::with_capacity(capacity),
                capacity,
                hits: 0,
                misses: 0,
            }),
        })
    }

    pub fn stats(&self) -> (u64, u64) {
        let s = self.state.lock().unwrap();
        (s.hits, s.misses)
    }

    pub fn invalidate_all(&self) {
        let mut s = self.state.lock().unwrap();
        s.entries.clear();
    }

    fn invalidate_range(state: &mut CacheState, start: u64, end: u64, block_size: u64) {
        state.entries.retain(|(off, _)| {
            let block_end = off.saturating_add(block_size);
            *off >= end || block_end <= start
        });
    }
}

impl CachingDevice {
    /// The cached block at `block_start`, fetching it if it is not held.
    fn block(&self, block_start: u64) -> Result<Arc<Vec<u8>>> {
        {
            let mut s = self.state.lock().unwrap();
            if let Some(pos) = s.entries.iter().position(|(o, _)| *o == block_start) {
                let entry = s.entries.remove(pos).expect("position just found it");
                let data = entry.1.clone();
                s.entries.push_front(entry);
                s.hits += 1;
                return Ok(data);
            }
            s.misses += 1;
        }

        // THE LAST BLOCK OF A DEVICE IS OFTEN SHORT, and asking the
        // device for a whole one past its end is an error rather than a
        // short read. A SquashFS image is 4 KiB and its declared block
        // size 128 KiB; without this clamp, caching such an image failed
        // on the first read it ever made.
        let size = self.inner.size_bytes();
        let end = block_start.saturating_add(self.block_size).min(size);
        let len = end.saturating_sub(block_start) as usize;
        let mut block = vec![0u8; len];
        self.inner.read_at(block_start, &mut block)?;
        let data = Arc::new(block);

        let mut s = self.state.lock().unwrap();
        if s.entries.len() >= s.capacity {
            s.entries.pop_back();
        }
        s.entries.push_front((block_start, data.clone()));
        Ok(data)
    }
}

impl BlockRead for CachingDevice {
    /// # A read is served from the blocks it falls in, whatever its size
    ///
    /// This used to serve a read only when it was **exactly one aligned
    /// block**, and pass everything else through untouched — including
    /// reads of bytes it was already holding.
    ///
    /// The drivers almost never read a whole block. Measured on
    /// `am-fs-xfs` against a fixture with a 4096-byte block size, the
    /// average read during a directory walk was **1040 bytes**: inodes
    /// are read at inode size and group headers at sector size, so
    /// roughly three quarters of reads missed by construction.
    ///
    /// # What it costs
    ///
    /// A 512-byte read of an uncached block now fetches 4096. That is a
    /// trade of bytes for calls, and it is the right way round for these
    /// drivers: the block being fetched is the one holding the inode,
    /// and the next inode read is very often in it.
    ///
    /// # Where it still passes through
    ///
    /// A read larger than the cache's own capacity would evict
    /// everything to hold one answer, so anything spanning more blocks
    /// than a useful fraction of the cache goes straight to the device.
    /// File data is read in large pieces and would otherwise push out
    /// the metadata this exists to keep.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let bs = self.block_size;
        let first = offset / bs;
        let last = (offset + buf.len() as u64 - 1) / bs;
        let spanned = (last - first + 1) as usize;

        // A read big enough to sweep the cache is not worth caching.
        //
        // A SINGLE BLOCK IS NEVER "BIG ENOUGH", however small the cache.
        // Without that clause a cache of one block bypasses every read
        // it is ever given -- one block is more than half of one block --
        // so the smallest cache anybody can ask for is the one that
        // silently does nothing.
        let sweeps_the_cache = {
            let s = self.state.lock().unwrap();
            spanned > 1 && spanned * 2 > s.capacity
        };
        if sweeps_the_cache {
            return self.inner.read_at(offset, buf);
        }

        // A READ RUNNING PAST THE END OF THE DEVICE IS THE DEVICE'S TO
        // REFUSE. Serving it from clamped blocks would hand back a short
        // answer with no error, which is worse than the failure the
        // caller would otherwise have seen.
        if offset.saturating_add(buf.len() as u64) > self.inner.size_bytes() {
            return self.inner.read_at(offset, buf);
        }

        let mut done = 0usize;
        for index in first..=last {
            let block_start = index * bs;
            let block = self.block(block_start)?;
            // Where this block overlaps what was asked for.
            let from = (offset.max(block_start) - block_start) as usize;
            // Bounded by what the BLOCK holds rather than by the block
            // size, since the last one may be short.
            let take = (block.len().saturating_sub(from)).min(buf.len() - done);
            if take == 0 {
                break;
            }
            buf[done..done + take].copy_from_slice(&block[from..from + take]);
            done += take;
        }
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.inner.size_bytes()
    }
}

impl BlockDevice for CachingDevice {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        // THE CACHE IS INVALIDATED EVEN IF THE WRITE THEN FAILS, and
        // deliberately: dropping entries the write would have made stale
        // costs a re-read, while keeping them past a write that half
        // succeeded serves bytes the device no longer holds.
        let end = offset.saturating_add(buf.len() as u64);
        {
            let mut s = self.state.lock().unwrap();
            let bs = self.block_size;
            Self::invalidate_range(&mut s, offset, end, bs);
        }
        let Some(writable) = self.writable.as_ref() else {
            return Err(crate::error::Error::ReadOnly);
        };
        writable.write_at(offset, buf)
    }

    fn flush(&self) -> Result<()> {
        match self.writable.as_ref() {
            Some(writable) => writable.flush(),
            // Nothing was written, so there is nothing to flush. An
            // error here would make a caller that flushes defensively
            // fail on a read-only volume.
            None => Ok(()),
        }
    }

    fn is_writable(&self) -> bool {
        self.writable.as_ref().is_some_and(|w| w.is_writable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_device::Bytes;

    const BS: u64 = 512;

    fn backing() -> Arc<Bytes> {
        Arc::new(Bytes::new((0..4096u32).map(|i| i as u8).collect()))
    }

    /// THE CASE THAT COULD NOT BE EXPRESSED BEFORE: a device that is
    /// only ever read, wrapped in a cache.
    ///
    /// Every driver in this family mounts through a `BlockRead`, so
    /// this is not an exotic configuration — it is the ordinary one,
    /// and requiring `BlockDevice` is why four of the six drivers used
    /// no cache at all.
    #[test]
    fn a_read_only_device_can_be_cached() {
        let inner = backing();
        let cache = CachingDevice::read_only(inner, BS, 4);

        let mut first = vec![0u8; BS as usize];
        let mut again = vec![0u8; BS as usize];
        cache.read_at(0, &mut first).expect("first read");
        cache.read_at(0, &mut again).expect("second read");

        assert_eq!(first, again, "the cache must serve what the device held");
        assert_eq!(cache.stats(), (1, 1), "one hit after one miss");
    }

    /// THE CASE THE OLD HIT CONDITION MISSED: a read smaller than a
    /// block, of a block already held.
    ///
    /// Serving only exact aligned blocks meant the drivers' ordinary
    /// reads — an inode at inode size, a group header at sector size —
    /// went to the device every time, even when the block containing
    /// them was cached.
    #[test]
    fn a_read_smaller_than_a_block_is_served_from_it() {
        let cache = CachingDevice::read_only(backing(), BS, 8);

        let mut whole = vec![0u8; BS as usize];
        cache.read_at(0, &mut whole).expect("warm the block");
        assert_eq!(cache.stats(), (0, 1), "one miss to fetch it");

        // Four sub-block reads inside the block just fetched.
        for at in [0u64, 8, 100, 504] {
            let mut small = [0u8; 8];
            cache.read_at(at, &mut small).expect("sub-block read");
            assert_eq!(
                &small[..],
                &whole[at as usize..at as usize + 8],
                "the bytes must be the block's own, at the right offset"
            );
        }
        assert_eq!(cache.stats(), (4, 1), "four hits, and no further misses");
    }

    /// A read crossing a block boundary is stitched from both blocks,
    /// and each is cached.
    #[test]
    fn a_read_spanning_two_blocks_is_stitched() {
        let inner = backing();
        let mut direct = vec![0u8; 16];
        inner.read_at(BS - 8, &mut direct).expect("read it plainly");

        let cache = CachingDevice::read_only(backing(), BS, 8);
        let mut across = vec![0u8; 16];
        cache.read_at(BS - 8, &mut across).expect("spanning read");

        assert_eq!(across, direct, "the same bytes the device would give");
        assert_eq!(cache.stats(), (0, 2), "one miss per block touched");

        cache.read_at(BS - 8, &mut across).expect("again");
        assert_eq!(cache.stats(), (2, 2), "and both are held now");
    }

    /// A read big enough to sweep the cache goes straight to the device.
    ///
    /// File data arrives in large pieces, and caching it would evict the
    /// metadata this exists to hold — the opposite of the point.
    #[test]
    fn a_read_that_would_sweep_the_cache_passes_through() {
        let cache = CachingDevice::read_only(backing(), BS, 4);
        let mut big = vec![0u8; (BS * 4) as usize];
        cache.read_at(0, &mut big).expect("a large read");
        assert_eq!(
            cache.stats(),
            (0, 0),
            "neither hit nor miss: it never consulted the cache"
        );
    }

    /// A device smaller than one block still reads.
    ///
    /// THE CASE THAT BROKE. `am-fs-squashfs` declares a block size from
    /// the archive's superblock -- 128 KiB is the usual -- and a small
    /// image is a few kilobytes whole. Fetching "the block at zero"
    /// asked the device for 128 KiB it did not have, which is an error
    /// rather than a short read, so opening such an image with a cache
    /// failed on the very first read.
    #[test]
    fn a_device_shorter_than_a_block_still_reads() {
        let tiny: Arc<Bytes> = Arc::new(Bytes::new((0..100u32).map(|i| i as u8).collect()));
        let cache = CachingDevice::read_only(tiny, BS, 4);

        let mut buf = vec![0u8; 40];
        cache
            .read_at(10, &mut buf)
            .expect("a read inside the device");
        assert_eq!(buf[0], 10, "the wrong bytes came back");
        assert_eq!(buf[39], 49);

        // And the second one is a hit, so the short block was cached
        // rather than merely tolerated.
        cache.read_at(10, &mut buf).expect("again");
        assert_eq!(cache.stats(), (1, 1));
    }

    /// A read running past the end of the device still fails.
    ///
    /// The clamp above must not turn "you asked for bytes that are not
    /// there" into a short answer with no error. That failure is
    /// invisible to the caller, which is the one kind this family of
    /// crates refuses to produce.
    #[test]
    fn a_read_past_the_end_is_still_an_error() {
        let tiny: Arc<Bytes> = Arc::new(Bytes::new(vec![0u8; 100]));
        let cache = CachingDevice::read_only(tiny, BS, 4);

        let mut buf = vec![0u8; 40];
        assert!(
            cache.read_at(80, &mut buf).is_err(),
            "80 + 40 is past the end of a 100-byte device"
        );
    }

    /// A cache over a read-only device says so, and refuses a write
    /// with the answer the device underneath would have given.
    #[test]
    fn writing_through_a_read_only_cache_is_refused() {
        let cache = CachingDevice::read_only(backing(), BS, 4);
        assert!(!cache.is_writable());
        assert!(matches!(
            cache.write_at(0, &[1u8; 8]),
            Err(crate::error::Error::ReadOnly)
        ));
        // And flushing is not an error: a caller that flushes
        // defensively must not fail on a volume it never wrote.
        assert!(cache.flush().is_ok());
    }
}

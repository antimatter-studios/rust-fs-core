//! Small LRU read-cache decorator. Caches only block-aligned, block-sized
//! reads; everything else passes through. Writes invalidate any overlapping
//! cached entries.

use crate::block::{BlockDevice, BlockRead};
use crate::error::Result;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// The largest block size a cache will accept.
///
/// `block()` allocates a whole block eagerly, and the `.min(size)` clamp
/// that keeps a short device working bounds that allocation by the device
/// rather than by anything sane — so a declared block size of 2^40 over a
/// 1 TB image is a request for a terabyte. A ceiling is the only thing
/// standing between a number off a disk and that allocation.
///
/// 64 MiB is far above anything a real filesystem declares — SquashFS tops
/// out at 1 MiB, and the block sizes every other driver here uses are
/// measured in kilobytes — and far below a size that could exhaust a host.
pub const MAX_BLOCK_SIZE: u64 = 64 * 1024 * 1024;

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
    /// NOT IN `CacheState`, BECAUSE IT IS NOT STATE.
    ///
    /// It is a construction parameter, written once and never mutated,
    /// and it used to live behind the mutex. Every read therefore took
    /// the lock to compare `spanned` against it — including the reads
    /// that were about to be passed straight through and never touch
    /// the cache at all. On a hit the lock was taken twice, once here
    /// and once inside `block()`, to serve bytes already in memory.
    ///
    /// A plain field beside `block_size` makes the bypass test lock-free
    /// and says what the value is: fixed at construction, like the block
    /// size next to it.
    capacity: usize,
    state: Mutex<CacheState>,
}

struct CacheState {
    /// Fixed-capacity LRU; head is most-recently used. The capacity
    /// itself is [`CachingDevice::capacity`] — it never changes, so it
    /// is not kept under the lock.
    entries: VecDeque<(u64, Arc<Vec<u8>>)>,
    hits: u64,
    misses: u64,
    /// Bumped by every invalidation. A miss records it before it lets go
    /// of the lock to read the device, and the insert on the way back in
    /// is refused if it has moved — see `CachingDevice::block`.
    generation: u64,
}

impl CachingDevice {
    /// Cache a device that can be written. Writes invalidate the
    /// entries they overlap and go through to `inner`.
    ///
    /// The invalidation holds against concurrent readers as well as
    /// sequential ones: once `write_at` has returned, no later read can
    /// be served pre-write bytes from this cache, including a read that
    /// was already in flight when the write began. What such a read
    /// *returns* is still either side of the write — that is what racing
    /// means — but it is not remembered.
    ///
    /// `block_size` must be non-zero and no larger than
    /// [`MAX_BLOCK_SIZE`]; see [`CachingDevice::read_only`] for why that
    /// is enforced at first use rather than here.
    pub fn new(inner: Arc<dyn BlockDevice>, block_size: u64, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: inner.clone(),
            writable: Some(inner),
            block_size,
            capacity,
            state: Mutex::new(CacheState {
                entries: VecDeque::with_capacity(capacity),
                hits: 0,
                misses: 0,
                generation: 0,
            }),
        })
    }

    /// Cache a device that is only ever read.
    ///
    /// The case every driver here actually has: a volume mounted for
    /// reading, behind a `BlockRead` that was never a `BlockDevice`.
    ///
    /// `block_size` must be non-zero and no larger than
    /// [`MAX_BLOCK_SIZE`]. Construction cannot refuse — it returns
    /// `Arc<Self>`, not `Result` — so a block size outside that range is
    /// refused by every read and every write instead, with an error
    /// naming the offending size.
    pub fn read_only(inner: Arc<dyn BlockRead>, block_size: u64, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner,
            writable: None,
            block_size,
            capacity,
            state: Mutex::new(CacheState {
                entries: VecDeque::with_capacity(capacity),
                hits: 0,
                misses: 0,
                generation: 0,
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
        s.generation = s.generation.wrapping_add(1);
    }

    fn invalidate_range(state: &mut CacheState, start: u64, end: u64, block_size: u64) {
        state.entries.retain(|(off, _)| {
            let block_end = off.saturating_add(block_size);
            *off >= end || block_end <= start
        });
        // BUMPED WHETHER OR NOT ANYTHING WAS DROPPED. The counter is not a
        // record of what this sweep removed; it is a fence a concurrent
        // miss can compare itself against, and a miss that is mid-flight
        // over this range holds no entry for the sweep to find.
        state.generation = state.generation.wrapping_add(1);
    }

    /// One invalidation sweep, taking and releasing the lock.
    fn invalidate_for_write(&self, start: u64, end: u64) {
        let mut s = self.state.lock().unwrap();
        let bs = self.block_size;
        Self::invalidate_range(&mut s, start, end, bs);
    }

    /// Refuse a block size the cache cannot work with.
    ///
    /// # Why this is checked here and not in the constructors
    ///
    /// It belongs in the constructors, and they cannot express it: both
    /// return `Arc<Self>` rather than `Result`, and that signature is
    /// published API in eleven sibling crates. Making them fallible to
    /// catch a case no correct caller hits would be a breaking change to
    /// all of them. So the refusal happens at first use instead, which
    /// costs two comparisons against an immutable field per call and
    /// turns both failures into an error the caller can handle.
    ///
    /// # Why a block size is worth checking at all
    ///
    /// Zero divides by zero on the first read, and integer division by
    /// zero panics unconditionally — it is not governed by
    /// `overflow-checks`, so a release build dies too. An absurd value
    /// reaches `vec![0u8; len]` in `block()` with `len` bounded only by
    /// the device. Both numbers come off a disk: a driver reads its block
    /// size from a superblock and passes it through, so a truncated,
    /// fuzzed or hostile image reaches this.
    fn check_block_size(&self) -> Result<()> {
        if self.block_size == 0 {
            return Err(crate::error::Error::Custom(
                "cache block size is zero".to_string(),
            ));
        }
        if self.block_size > MAX_BLOCK_SIZE {
            return Err(crate::error::Error::Custom(format!(
                "cache block size {} exceeds the {MAX_BLOCK_SIZE}-byte ceiling",
                self.block_size
            )));
        }
        Ok(())
    }
}

impl CachingDevice {
    /// The cached block at `block_start`, fetching it if it is not held.
    ///
    /// # A MISS THAT OVERLAPPED AN INVALIDATION IS NOT CACHED
    ///
    /// The device read below runs with the lock released — holding a mutex
    /// across I/O would serialise every reader, which is the whole reason
    /// the lock is dropped. That leaves a window: a write can sweep the
    /// cache and land on the device while this read is in flight, and the
    /// bytes in hand are then the ones the device held *before* the write.
    /// Inserting them puts a stale entry in a cache the sweep has already
    /// gone past, and nothing would ever invalidate it again.
    ///
    /// So the miss records the generation counter before it lets go, and
    /// declines to insert if any invalidation has happened since. The
    /// caller still gets the bytes that were read — a read racing a write
    /// may legitimately see either side of it — but the cache does not keep
    /// them.
    ///
    /// The check is deliberately coarse: the counter is global rather than
    /// per range, so a write to an unrelated block also costs this miss its
    /// insert. Writes are far rarer than reads in these drivers, and the
    /// price of being conservative is one extra device read.
    fn block(&self, block_start: u64) -> Result<Arc<Vec<u8>>> {
        let generation_at_miss;
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
            generation_at_miss = s.generation;
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
        if s.generation == generation_at_miss {
            if s.entries.len() >= self.capacity {
                s.entries.pop_back();
            }
            s.entries.push_front((block_start, data.clone()));
        }
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
        self.check_block_size()?;
        if buf.is_empty() {
            return Ok(());
        }

        // THE END OF THE READ IS COMPUTED ONCE, CHECKED, AND BEFORE ANY
        // DIVISION.
        //
        // This used to be `offset + buf.len()`, twice, unchecked, above
        // the only bounds check the function has — which then made the
        // same sum a third time with `saturating_add`, so the line that
        // knew the sum could overflow sat below the two that did not.
        //
        // An offset here is computed by a driver from an on-disk field:
        // an extent pointer, an inode block number, a directory offset.
        // A wild one is an ordinary thing to be handed off a corrupt or
        // hostile image rather than a mistake in the caller, which is
        // the same argument `slice.rs` was fixed on.
        //
        // A sum that does not fit in a `u64` cannot name a byte on any
        // device, so it is refused here rather than forwarded. `got: 0`
        // because nothing was transferred — the convention the slice
        // adapters already use for a read refused before it starts.
        let Some(end) = offset.checked_add(buf.len() as u64) else {
            return Err(crate::error::Error::ShortRead {
                offset,
                want: buf.len(),
                got: 0,
            });
        };

        // A READ RUNNING PAST THE END OF THE DEVICE IS THE DEVICE'S TO
        // REFUSE. Serving it from clamped blocks would hand back a short
        // answer with no error, which is worse than the failure the
        // caller would otherwise have seen.
        if end > self.inner.size_bytes() {
            return self.inner.read_at(offset, buf);
        }

        let bs = self.block_size;
        let first = offset / bs;
        let last = (end - 1) / bs;
        let spanned = (last - first + 1) as usize;

        // A read big enough to sweep the cache is not worth caching.
        //
        // A SINGLE BLOCK IS NEVER "BIG ENOUGH", however small the cache.
        // Without that clause a cache of one block bypasses every read
        // it is ever given -- one block is more than half of one block --
        // so the smallest cache anybody can ask for is the one that
        // silently does nothing.
        //
        // AND THE TEST TAKES NO LOCK. `capacity` is a plain field on the
        // device, so a read that is about to bypass the cache decides
        // that without ever contending for the mutex -- see the field's
        // own comment.
        if spanned > 1 && spanned.saturating_mul(2) > self.capacity {
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

        // A BUFFER THIS DID NOT FILL IS A FAILURE, NOT A SUCCESS.
        //
        // The loop can run out of bytes before the buffer is full: a
        // cached block is clamped to the device's size when it is
        // fetched, so a block held from when the device measured smaller
        // is short, and the guard above — which asks the device its size
        // a second time — lets the read through as in range. Reporting
        // `Ok` there hands back exactly the short answer with no error
        // that the guard's own comment refuses.
        //
        // It cannot fire while `size_bytes` is stable, which is the
        // contract `BlockRead` now states: the guard bounds the read by
        // the device, and the blocks between them cover everything up to
        // that bound. So this is the device breaking its promise being
        // caught rather than believed.
        if done != buf.len() {
            return Err(crate::error::Error::ShortRead {
                offset,
                want: buf.len(),
                got: done,
            });
        }
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.inner.size_bytes()
    }
}

impl BlockDevice for CachingDevice {
    /// # THE CACHE IS INVALIDATED EVEN IF THE WRITE THEN FAILS
    ///
    /// Deliberately: dropping entries the write would have made stale
    /// costs a re-read, while keeping them past a write that half
    /// succeeded serves bytes the device no longer holds.
    ///
    /// # AND IT IS INVALIDATED TWICE, ONCE EITHER SIDE OF THE DEVICE
    ///
    /// One sweep before the write is not enough. Between it and the
    /// device write landing, a concurrent [`CachingDevice::read_at`] can
    /// miss, fetch pre-write bytes, and insert them behind the sweep —
    /// an entry the sweep has already gone past and nothing else would
    /// ever drop. The second sweep is what closes that window, together
    /// with the generation check on the miss path that refuses such an
    /// insert outright.
    ///
    /// Both are needed, and each covers what the other cannot. A read
    /// that began BEFORE this write is caught by the counter, because it
    /// recorded the generation before the first sweep bumped it. A read
    /// that begins AFTER that sweep records the bumped value, so the
    /// counter agrees with it and only the second sweep drops what it
    /// inserted.
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        // BEFORE EITHER SWEEP, AND THAT ORDERING IS LOAD-BEARING.
        //
        // A sweep cannot be done correctly with a block size of zero:
        // `invalidate_range` computes `block_end = off + block_size`, so a
        // zero block size makes `block_end == off` and turns the retain
        // predicate into `*off >= end || *off <= start`, which KEEPS
        // entries the write has made stale. Sweeping first and refusing
        // afterwards would therefore do the one thing this type must never
        // do, on the way to reporting an error.
        //
        // Refusing here is also the only position that cannot break the
        // two-sweep guarantee above. The invariant that guarantee rests on
        // is "if the device was written, both sweeps ran" — and returning
        // at this point means the device is never reached, so nothing was
        // written and nothing needs sweeping. An early return anywhere
        // below would skip the second sweep after a write that may have
        // landed, which is exactly the window that fix closed.
        self.check_block_size()?;
        let end = offset.saturating_add(buf.len() as u64);
        self.invalidate_for_write(offset, end);
        let Some(writable) = self.writable.as_ref() else {
            return Err(crate::error::Error::ReadOnly);
        };
        let result = writable.write_at(offset, buf);
        // Unconditionally, for the same reason the first sweep is
        // unconditional: a write that failed may still have landed.
        self.invalidate_for_write(offset, end);
        result
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

    /// A READ THAT BYPASSES THE CACHE DOES NOT WAIT FOR THE CACHE LOCK.
    ///
    /// The state lock is held for the whole of the read, by a thread
    /// that is not doing the read. With the `capacity` comparison under
    /// that mutex the bypass cannot proceed and the receive times out;
    /// with it outside there is nothing to wait for. No timing
    /// threshold and no thread count, so nothing here is flaky on a
    /// loaded machine.
    #[test]
    fn a_bypassed_read_does_not_wait_for_the_cache_lock() {
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        // Capacity 4 and a read spanning 4 blocks: 4 * 2 > 4, so this
        // read bypasses -- the same read
        // `a_read_that_would_sweep_the_cache_passes_through` makes.
        let cache = CachingDevice::read_only(backing(), BS, 4);
        let held = cache.state.lock().expect("nothing else holds it yet");

        let reader = Arc::clone(&cache);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut big = vec![0u8; (BS * 4) as usize];
            let outcome = reader.read_at(0, &mut big);
            // Sent whatever it is: the point is that the read RETURNED.
            let _ = tx.send(outcome);
        });

        let outcome = rx.recv_timeout(Duration::from_secs(5)).expect(
            "a read that bypasses the cache must not block on the cache lock; \
             it timed out waiting for a mutex it has no reason to take",
        );
        outcome.expect("and the bypassed read itself must succeed");

        // Released only now, and it has to be explicit: `stats()` takes
        // the same lock, so leaving the guard to the end of scope would
        // deadlock the assertion below.
        drop(held);

        assert_eq!(
            cache.stats(),
            (0, 0),
            "it bypassed, so it neither hit nor missed"
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

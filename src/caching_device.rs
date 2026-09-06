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

impl BlockRead for CachingDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let cacheable =
            buf.len() as u64 == self.block_size && offset.is_multiple_of(self.block_size);
        if !cacheable {
            return self.inner.read_at(offset, buf);
        }

        {
            let mut s = self.state.lock().unwrap();
            if let Some(pos) = s.entries.iter().position(|(o, _)| *o == offset) {
                let entry = s.entries.remove(pos).unwrap();
                buf.copy_from_slice(&entry.1);
                s.entries.push_front(entry);
                s.hits += 1;
                return Ok(());
            }
            s.misses += 1;
        }

        self.inner.read_at(offset, buf)?;
        let data = Arc::new(buf.to_vec());
        let mut s = self.state.lock().unwrap();
        if s.entries.len() >= s.capacity {
            s.entries.pop_back();
        }
        s.entries.push_front((offset, data));
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

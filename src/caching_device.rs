//! Small LRU read-cache decorator. Caches only block-aligned, block-sized
//! reads; everything else passes through. Writes invalidate any overlapping
//! cached entries.

use crate::block::{BlockDevice, BlockRead};
use crate::error::Result;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// LRU read-cache wrapper. Wraps any `BlockDevice` (writable or not).
pub struct CachingDevice {
    inner: Arc<dyn BlockDevice>,
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
    pub fn new(inner: Arc<dyn BlockDevice>, block_size: u64, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner,
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
        let end = offset.saturating_add(buf.len() as u64);
        {
            let mut s = self.state.lock().unwrap();
            let bs = self.block_size;
            Self::invalidate_range(&mut s, offset, end, bs);
        }
        self.inner.write_at(offset, buf)
    }

    fn flush(&self) -> Result<()> {
        self.inner.flush()
    }

    fn is_writable(&self) -> bool {
        self.inner.is_writable()
    }
}

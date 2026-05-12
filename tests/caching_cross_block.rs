//! CachingDevice write-invalidation across block boundaries.
//!
//! The single inline test (`write_invalidates_overlapping_cache_blocks` in
//! `tests/caching_lru.rs`) writes wholly within one block. This file covers
//! cross-block writes — spanning two or three blocks — plus writes that
//! touch no cached block at all.

use fs_core::{BlockDevice, BlockRead, CachingDevice, Result};
use std::sync::{Arc, Mutex};

struct Mem {
    bytes: Mutex<Vec<u8>>,
}
impl Mem {
    fn new(size: usize) -> Self {
        Self {
            bytes: Mutex::new(vec![0u8; size]),
        }
    }
}
impl BlockRead for Mem {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let b = self.bytes.lock().unwrap();
        let s = offset as usize;
        buf.copy_from_slice(&b[s..s + buf.len()]);
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        self.bytes.lock().unwrap().len() as u64
    }
}
impl BlockDevice for Mem {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let mut b = self.bytes.lock().unwrap();
        let s = offset as usize;
        b[s..s + buf.len()].copy_from_slice(buf);
        Ok(())
    }
    fn is_writable(&self) -> bool {
        true
    }
}

fn populate(cache: &CachingDevice, block_size: u64, block_indices: &[u64]) {
    let mut buf = vec![0u8; block_size as usize];
    for &i in block_indices {
        cache.read_at(i * block_size, &mut buf).unwrap();
    }
}

#[test]
fn write_spanning_two_blocks_invalidates_both() {
    let bs = 4096u64;
    let parent: Arc<dyn BlockDevice> = Arc::new(Mem::new(8 * bs as usize));
    let cache = CachingDevice::new(parent, bs, 8);

    populate(&cache, bs, &[0, 1, 2]);
    let (h0, m0) = cache.stats();
    assert_eq!((h0, m0), (0, 3));

    // Write straddles block 0..block 1 boundary.
    let write_offset = bs - 8;
    cache.write_at(write_offset, &[0xFFu8; 32]).unwrap();

    // Block 2 should still be cached (untouched); block 0 + 1 should miss.
    let mut buf = vec![0u8; bs as usize];
    cache.read_at(0, &mut buf).unwrap(); // miss
    cache.read_at(bs, &mut buf).unwrap(); // miss
    cache.read_at(2 * bs, &mut buf).unwrap(); // hit

    let (h, m) = cache.stats();
    assert_eq!((h - h0, m - m0), (1, 2));
}

#[test]
fn write_spanning_three_blocks_invalidates_all_three() {
    let bs = 1024u64;
    let parent: Arc<dyn BlockDevice> = Arc::new(Mem::new(16 * bs as usize));
    let cache = CachingDevice::new(parent, bs, 8);

    populate(&cache, bs, &[0, 1, 2, 3, 4]);
    let (h0, m0) = cache.stats();
    assert_eq!((h0, m0), (0, 5));

    // Write spans blocks 1, 2, 3 (touches partial of 1, all of 2, partial of 3).
    let write_offset = bs + 100;
    let write_len = (bs * 2 + 200) as usize;
    cache
        .write_at(write_offset, &vec![0xAAu8; write_len])
        .unwrap();

    let mut buf = vec![0u8; bs as usize];
    cache.read_at(0, &mut buf).unwrap(); // hit (block 0 untouched)
    cache.read_at(4 * bs, &mut buf).unwrap(); // hit (block 4 untouched)
    cache.read_at(bs, &mut buf).unwrap(); // miss (block 1 invalidated)
    cache.read_at(2 * bs, &mut buf).unwrap(); // miss (block 2)
    cache.read_at(3 * bs, &mut buf).unwrap(); // miss (block 3)

    let (h, m) = cache.stats();
    assert_eq!((h - h0, m - m0), (2, 3));
}

#[test]
fn write_entirely_outside_any_cached_block_leaves_cache_intact() {
    let bs = 512u64;
    let parent: Arc<dyn BlockDevice> = Arc::new(Mem::new(16 * bs as usize));
    let cache = CachingDevice::new(parent, bs, 8);

    populate(&cache, bs, &[0, 1, 2]);
    let (h0, m0) = cache.stats();

    // Write into block 8 — nowhere near the cached set [0, 1, 2].
    cache.write_at(8 * bs, &[0xFFu8; 16]).unwrap();

    let mut buf = vec![0u8; bs as usize];
    cache.read_at(0, &mut buf).unwrap(); // hit
    cache.read_at(bs, &mut buf).unwrap(); // hit
    cache.read_at(2 * bs, &mut buf).unwrap(); // hit

    let (h, m) = cache.stats();
    assert_eq!((h - h0, m - m0), (3, 0));
}

#[test]
fn write_exactly_at_block_boundary_invalidates_only_that_block() {
    let bs = 4096u64;
    let parent: Arc<dyn BlockDevice> = Arc::new(Mem::new(8 * bs as usize));
    let cache = CachingDevice::new(parent, bs, 8);

    populate(&cache, bs, &[0, 1, 2]);
    let (h0, m0) = cache.stats();

    // Aligned write covering exactly block 1 — block 0 and 2 must survive.
    cache.write_at(bs, &vec![0x55u8; bs as usize]).unwrap();

    let mut buf = vec![0u8; bs as usize];
    cache.read_at(0, &mut buf).unwrap(); // hit
    cache.read_at(2 * bs, &mut buf).unwrap(); // hit
    cache.read_at(bs, &mut buf).unwrap(); // miss (was invalidated)

    let (h, m) = cache.stats();
    assert_eq!((h - h0, m - m0), (2, 1));
}

#[test]
fn write_touching_only_the_end_byte_of_a_block_still_invalidates() {
    // Edge case: write at the very last byte of block 0. Block 0 must miss
    // on the next read; block 1 must survive.
    let bs = 1024u64;
    let parent: Arc<dyn BlockDevice> = Arc::new(Mem::new(8 * bs as usize));
    let cache = CachingDevice::new(parent, bs, 8);

    populate(&cache, bs, &[0, 1]);
    let (h0, m0) = cache.stats();

    cache.write_at(bs - 1, &[0xFFu8; 1]).unwrap();

    let mut buf = vec![0u8; bs as usize];
    cache.read_at(bs, &mut buf).unwrap(); // hit
    cache.read_at(0, &mut buf).unwrap(); // miss

    let (h, m) = cache.stats();
    assert_eq!((h - h0, m - m0), (1, 1));
}

//! Realistic adapter stacks — each layer is unit-tested in isolation
//! elsewhere; this file verifies they compose without surprise.

use fs_core::{
    BlockDevice, BlockRead, BlockReadStreamer, CachingDevice, Error, FileDevice, OwnedRwSlice,
    OwnedSlice, ReadOnlyDevice,
};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

fn tmp_image(bytes: &[u8]) -> String {
    static C: AtomicU32 = AtomicU32::new(0);
    let n = C.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir()
        .join(format!("fs_core_compo_{}_{n}.img", std::process::id()))
        .to_string_lossy()
        .into_owned();
    File::create(&p).unwrap().write_all(bytes).unwrap();
    p
}

#[test]
fn stream_over_caching_over_file_reads_full_contents() {
    let bytes: Vec<u8> = (0..(64 * 1024)).map(|i| (i % 251) as u8).collect();
    let path = tmp_image(&bytes);

    let file = FileDevice::open(&path).unwrap();
    let parent: Arc<dyn BlockDevice> = Arc::new(file);
    let cached = CachingDevice::new(parent, 4096, 8);
    let mut stream = BlockReadStreamer::new(cached);

    let mut out = Vec::with_capacity(bytes.len());
    let n = stream.read_to_end(&mut out).unwrap();
    assert_eq!(n, bytes.len());
    assert_eq!(out, bytes);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn re_streaming_through_cache_records_hits() {
    let bytes: Vec<u8> = (0..(8 * 1024)).map(|i| (i % 17) as u8).collect();
    let path = tmp_image(&bytes);

    let file = FileDevice::open(&path).unwrap();
    let parent: Arc<dyn BlockDevice> = Arc::new(file);
    let cached = CachingDevice::new(parent, 4096, 8);

    // First pass populates the cache for the two aligned blocks.
    {
        let mut buf = vec![0u8; 4096];
        cached.read_at(0, &mut buf).unwrap();
        cached.read_at(4096, &mut buf).unwrap();
    }
    let (h0, _m0) = cached.stats();
    assert_eq!(h0, 0);

    // Second pass — same block-aligned reads should hit.
    {
        let mut buf = vec![0u8; 4096];
        cached.read_at(0, &mut buf).unwrap();
        cached.read_at(4096, &mut buf).unwrap();
    }
    let (h1, _m1) = cached.stats();
    assert_eq!(h1, 2);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn rw_slice_over_caching_over_file_write_persists_to_disk() {
    let path = tmp_image(&[0u8; 4096]);

    // Build the stack with an RW FileDevice at the bottom.
    let file = FileDevice::open_rw(&path).unwrap();
    let parent: Arc<dyn BlockDevice> = Arc::new(file);
    let cached: Arc<CachingDevice> = CachingDevice::new(parent, 512, 4);
    let cached_dev: Arc<dyn BlockDevice> = cached.clone();
    let slice = OwnedRwSlice::new(cached_dev, 1024, 512);

    assert!(slice.is_writable());
    slice.write_at(8, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
    slice.flush().unwrap();
    drop(slice);
    drop(cached);

    // Reopen and verify the bytes landed at the correct absolute offset.
    let mut f = OpenOptions::new().read(true).open(&path).unwrap();
    f.seek(SeekFrom::Start(1024 + 8)).unwrap();
    let mut buf = [0u8; 4];
    f.read_exact(&mut buf).unwrap();
    assert_eq!(buf, [0xDE, 0xAD, 0xBE, 0xEF]);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn ro_slice_over_caching_over_file_reads_window() {
    let bytes: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
    let path = tmp_image(&bytes);

    let file = FileDevice::open(&path).unwrap();
    let parent: Arc<dyn BlockDevice> = Arc::new(file);
    let cached: Arc<CachingDevice> = CachingDevice::new(parent, 512, 4);
    let cached_read: Arc<dyn BlockRead> = cached.clone();
    let slice = OwnedSlice::new(cached_read, 256, 64);

    assert_eq!(slice.size_bytes(), 64);
    let mut buf = [0u8; 64];
    slice.read_at(0, &mut buf).unwrap();
    assert_eq!(&buf[..], &bytes[256..320]);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn stream_over_readonly_over_file_reads_match_writes_denied() {
    let bytes: Vec<u8> = (0..256).map(|i| i as u8).collect();
    let path = tmp_image(&bytes);

    // Underlying RW capable, but the wrapper enforces RO at the type level.
    let file = FileDevice::open_rw(&path).unwrap();
    let wrapper = ReadOnlyDevice::new(file);

    // BlockDevice surface still rejects writes.
    assert!(!BlockDevice::is_writable(&wrapper));
    let err = BlockDevice::write_at(&wrapper, 0, &[0xFF]).unwrap_err();
    assert!(matches!(err, Error::ReadOnly));

    // BlockRead path streams correctly via Read+Seek.
    let mut stream = BlockReadStreamer::new(wrapper);
    let mut out = Vec::new();
    let n = stream.read_to_end(&mut out).unwrap();
    assert_eq!(n, 256);
    assert_eq!(out, bytes);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_slice_partition_walk_does_not_share_cache_entries_across_slices() {
    // Simulate partition-walker: cached file as the bottom, then two
    // OwnedSlice windows pointing at disjoint regions. Each slice should
    // read its region correctly without confusing the cache.
    let bytes: Vec<u8> = (0..(16 * 1024)).map(|i| (i % 211) as u8).collect();
    let path = tmp_image(&bytes);

    let file = FileDevice::open(&path).unwrap();
    let parent: Arc<dyn BlockDevice> = Arc::new(file);
    let cached: Arc<CachingDevice> = CachingDevice::new(parent, 1024, 8);
    let cached_read: Arc<dyn BlockRead> = cached.clone();

    let p1 = OwnedSlice::new(cached_read.clone(), 0, 4 * 1024);
    let p2 = OwnedSlice::new(cached_read, 8 * 1024, 4 * 1024);

    let mut buf1 = vec![0u8; 4 * 1024];
    let mut buf2 = vec![0u8; 4 * 1024];
    p1.read_at(0, &mut buf1).unwrap();
    p2.read_at(0, &mut buf2).unwrap();

    assert_eq!(buf1, &bytes[..4 * 1024]);
    assert_eq!(buf2, &bytes[8 * 1024..12 * 1024]);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn stream_with_seek_over_caching_walks_to_arbitrary_offset() {
    let bytes: Vec<u8> = (0..(4 * 1024)).map(|i| (i % 67) as u8).collect();
    let path = tmp_image(&bytes);

    let file = FileDevice::open(&path).unwrap();
    let parent: Arc<dyn BlockDevice> = Arc::new(file);
    let cached = CachingDevice::new(parent, 1024, 4);
    let mut stream = BlockReadStreamer::new(cached);

    stream.seek(SeekFrom::Start(2000)).unwrap();
    let mut buf = [0u8; 16];
    stream.read_exact(&mut buf).unwrap();
    assert_eq!(&buf[..], &bytes[2000..2016]);

    let _ = std::fs::remove_file(&path);
}

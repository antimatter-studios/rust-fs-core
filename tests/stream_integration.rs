//! BlockReadStreamer over a real FileDevice — end-to-end Read/Seek through
//! the file-backed BlockRead implementation.

use fs_core::{BlockRead, BlockReadStreamer, FileDevice};
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

fn tmp_image(bytes: &[u8]) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir()
        .join(format!("fs_core_stream_int_{}_{n}.img", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let mut f = File::create(&path).unwrap();
    f.write_all(bytes).unwrap();
    path
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

#[test]
fn read_to_end_over_file_device_matches_original_bytes() {
    let bytes: Vec<u8> = (0..(128 * 1024)).map(|i| (i % 251) as u8).collect();
    let path = tmp_image(&bytes);

    let dev = FileDevice::open(&path).unwrap();
    let mut s = BlockReadStreamer::new(dev);
    let mut out = Vec::with_capacity(bytes.len());
    let n = s.read_to_end(&mut out).unwrap();

    assert_eq!(n, bytes.len());
    assert_eq!(hash(&out), hash(&bytes));
    assert_eq!(out, bytes);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn seek_then_read_returns_expected_window() {
    let bytes: Vec<u8> = (0..512).map(|i| i as u8).collect();
    let path = tmp_image(&bytes);
    let dev = FileDevice::open(&path).unwrap();
    let mut s = BlockReadStreamer::new(dev);

    let pos = s.seek(SeekFrom::Start(100)).unwrap();
    assert_eq!(pos, 100);
    let mut buf = [0u8; 16];
    s.read_exact(&mut buf).unwrap();
    let expected: Vec<u8> = (100u16..116).map(|n| n as u8).collect();
    assert_eq!(buf.to_vec(), expected);

    // Seek backwards then forwards.
    let pos = s.seek(SeekFrom::Start(50)).unwrap();
    assert_eq!(pos, 50);
    s.read_exact(&mut buf).unwrap();
    let expected: Vec<u8> = (50u16..66).map(|n| n as u8).collect();
    assert_eq!(buf.to_vec(), expected);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn seek_from_end_lands_before_size() {
    let bytes: Vec<u8> = (0..256).map(|i| i as u8).collect();
    let path = tmp_image(&bytes);
    let dev = FileDevice::open(&path).unwrap();
    let mut s = BlockReadStreamer::new(dev);

    let pos = s.seek(SeekFrom::End(-4)).unwrap();
    assert_eq!(pos, 252);
    let mut buf = [0u8; 4];
    s.read_exact(&mut buf).unwrap();
    assert_eq!(buf, [252, 253, 254, 255]);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn streamer_works_through_arc_dyn_blockread() {
    let bytes: Vec<u8> = (0..1024).map(|i| (i % 13) as u8).collect();
    let path = tmp_image(&bytes);
    let dev: Arc<dyn BlockRead> = Arc::new(FileDevice::open(&path).unwrap());
    let mut s = BlockReadStreamer::new(dev);
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    assert_eq!(out, bytes);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn read_to_end_on_empty_file_yields_zero_bytes() {
    let path = tmp_image(&[]);
    let dev = FileDevice::open(&path).unwrap();
    let mut s = BlockReadStreamer::new(dev);
    let mut out = Vec::new();
    let n = s.read_to_end(&mut out).unwrap();
    assert_eq!(n, 0);
    assert!(out.is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn io_copy_streams_full_contents() {
    let bytes: Vec<u8> = (0..(32 * 1024)).map(|i| (i % 199) as u8).collect();
    let path = tmp_image(&bytes);
    let dev = FileDevice::open(&path).unwrap();
    let mut s = BlockReadStreamer::new(dev);
    let mut out: Vec<u8> = Vec::new();
    let n = std::io::copy(&mut s, &mut out).unwrap();
    assert_eq!(n as usize, bytes.len());
    assert_eq!(out, bytes);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn position_advances_as_chunks_are_read() {
    let bytes: Vec<u8> = (0..1024).map(|i| i as u8).collect();
    let path = tmp_image(&bytes);
    let dev = FileDevice::open(&path).unwrap();
    let mut s = BlockReadStreamer::new(dev);

    let mut buf = [0u8; 256];
    s.read_exact(&mut buf).unwrap();
    assert_eq!(s.position(), 256);
    s.read_exact(&mut buf).unwrap();
    assert_eq!(s.position(), 512);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn into_inner_returns_underlying_file_device() {
    let path = tmp_image(&[1u8; 64]);
    let dev = FileDevice::open(&path).unwrap();
    let s = BlockReadStreamer::new(dev);
    let recovered = s.into_inner();
    assert_eq!(recovered.size_bytes(), 64);
    let _ = std::fs::remove_file(&path);
}

//! Growing a device through `BlockDevice::set_len`, and the three things
//! that have to move together when it does.
//!
//! # WHY THIS EXISTS AT ALL
//!
//! #75 (`4e19fc9`) bounded `FileDevice::write_at` against the device's
//! own size, and it was right to: `write_all` at a seeked offset extends
//! a file, so a write straddling the end grew the backing store while
//! `size_bytes` went on reporting its construction-time length, and a
//! caller bounding its reads by `size_bytes` -- which is what
//! [`CachingDevice`] does, clamping every block it fetches -- could
//! never reach the bytes it had just written. That is rust-fs-core#70.
//!
//! What it did not do is leave anything in its place. **Appending a
//! block and recording where it went is how all four image formats in
//! this family allocate**, and a write past the end was the only tool
//! any of them had for it. Measured against `main`, every failure in
//! `rust-img-vhd` (7), `rust-img-qcow2` (3), `rust-img-vhdx` (1) and
//! `rust-img-vmdk` (1) is one write landing exactly at the device's
//! current end -- `OutOfBounds { offset: 67108864, len: 1048576, size:
//! 67108864 }`. That is rust-fs-core#147 and #129.
//!
//! # THE SIGNATURE IS THE SMALL HALF OF THIS
//!
//! `set_len` that extends the file and leaves `size_bytes()` reporting
//! the old number, or leaves a cache holding a block clamped to the old
//! end, **re-creates #70 exactly** -- and it passes a naive test, because
//! the write it enables succeeds and only a later cached read finds the
//! hole. So the assertions here are about the three things moving
//! together: the file on disk, the number the device reports, and what a
//! [`CachingDevice`] in front of it will serve.

use fs_core::block::{BlockDevice, BlockRead};
use fs_core::error::Error;
use fs_core::{CachingDevice, FileDevice, OwnedRwSlice};
use std::sync::Arc;

/// A temp image of `fill`, at a path unique to this process and caller.
fn tmp_image(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "fs_core_growth_{tag}_{}_{:?}.img",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, bytes).expect("write the fixture image");
    path
}

fn on_disk(path: &std::path::Path) -> u64 {
    std::fs::metadata(path)
        .expect("stat the backing file")
        .len()
}

// ---------------------------------------------------------------------
// FileDevice
// ---------------------------------------------------------------------

/// THE ALLOCATION EVERY IMAGE WRITER MAKES, end to end.
///
/// Append a block, then write into it: refused outright before this
/// method existed, and the refusal is the whole of #147.
///
/// The file's length on disk is the oracle for the growth, and
/// `size_bytes()` is asserted BEFORE and AFTER -- a `set_len` that moved
/// the file and not the number is the defect this test is shaped around,
/// not a variant of it.
#[test]
fn a_block_appended_with_set_len_can_then_be_written_and_read() {
    let path = tmp_image("append", &[0x11u8; 4096]);
    let dev = FileDevice::open_rw(&path).unwrap();

    assert!(dev.can_grow(), "a writable regular file is growable");
    assert_eq!(dev.size_bytes(), 4096, "control: the declared size");

    // The write an image writer makes, before the device has room for
    // it. This is the exact shape of every failure in #147.
    match dev.write_at(4096, &[0xABu8; 512]) {
        Err(Error::OutOfBounds { offset, len, size }) => {
            assert_eq!((offset, len, size), (4096, 512, 4096));
        }
        other => panic!("a write past the end must still be refused: {other:?}"),
    }

    dev.set_len(4608).expect("grow by one 512-byte block");

    assert_eq!(on_disk(&path), 4608, "the file on disk must have grown");
    assert_eq!(
        dev.size_bytes(),
        4608,
        "AND the number the device reports must have moved WITH it. A set_len \
         that grows the file and leaves this at 4096 is rust-fs-core#70 back \
         again: the device would deny having bytes it holds."
    );

    dev.write_at(4096, &[0xABu8; 512])
        .expect("the same write, into the block just appended");

    let mut back = [0u8; 512];
    dev.read_at(4096, &mut back)
        .expect("read the new block back");
    assert_eq!(back, [0xABu8; 512]);

    // And the bound still holds at the NEW end, so growth moved the
    // bound rather than removing it.
    match dev.write_at(4608, &[0xCDu8; 1]) {
        Err(Error::OutOfBounds { size, .. }) => assert_eq!(size, 4608),
        other => panic!("the bound must follow the new end: {other:?}"),
    }

    let _ = std::fs::remove_file(&path);
}

/// The extension reads as zeros, not as whatever the filesystem had.
///
/// `ftruncate` past the end is specified to read back zero, and a
/// consumer that appends a cluster and reads part of it before writing
/// all of it depends on that.
#[test]
fn the_region_a_grow_adds_reads_as_zeros() {
    let path = tmp_image("zeros", &[0xFFu8; 1024]);
    let dev = FileDevice::open_rw(&path).unwrap();
    dev.set_len(2048).expect("grow");

    let mut back = [0x5Au8; 1024];
    dev.read_at(1024, &mut back).expect("read the new region");
    assert_eq!(back, [0u8; 1024], "a grown region reads as zeros");
    let _ = std::fs::remove_file(&path);
}

/// SHRINKING PUBLISHES THE SMALLER SIZE BEFORE IT TRUNCATES.
///
/// The declared size must never exceed the file's real length, in either
/// order of the two steps -- a window where it does is a window in which
/// a read inside the declared device falls off the end of the file. So
/// the narrower of the two numbers is published first, and this arm
/// pins the outcome: after the call the two agree, and a read past the
/// new end is refused rather than served short.
#[test]
fn set_len_downwards_truncates_and_the_reported_size_follows() {
    let path = tmp_image("shrink", &[0x22u8; 8192]);
    let dev = FileDevice::open_rw(&path).unwrap();

    dev.set_len(4096).expect("shrink");

    assert_eq!(on_disk(&path), 4096);
    assert_eq!(dev.size_bytes(), 4096);
    match dev.write_at(4096, &[1u8; 1]) {
        Err(Error::OutOfBounds { size, .. }) => assert_eq!(size, 4096),
        other => panic!("the bound must follow a shrink too: {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}

/// A READ-ONLY HANDLE CANNOT GROW, AND SAYS SO BEFORE TOUCHING THE FILE.
///
/// `FileDevice::open` is the read-only constructor, and the file's
/// length on disk is the oracle: a refusal that had already called
/// `ftruncate` would pass an `is_err()` assertion while having done the
/// damage.
#[test]
fn a_read_only_file_device_refuses_to_grow_and_does_not_touch_the_file() {
    let path = tmp_image("ro", &[7u8; 2048]);
    let dev = FileDevice::open(&path).unwrap();

    assert!(!dev.can_grow(), "a read-only handle is not growable");
    match dev.set_len(4096) {
        Err(Error::ReadOnly) => {}
        other => panic!("expected ReadOnly, got {other:?}"),
    }
    assert_eq!(on_disk(&path), 2048, "the refused grow must not have run");
    assert_eq!(dev.size_bytes(), 2048);
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------
// The devices that keep the refusing default
// ---------------------------------------------------------------------

/// A SLICE IS A WINDOW, AND A WINDOW HAS NO LENGTH OF ITS OWN TO SET.
///
/// The parent file's length on disk is the oracle, for the reason
/// `slice_rw_length_is_clamped_and_a_write_past_it_does_not_grow_the_image`
/// already gives: an unclamped window here did real damage.
#[test]
fn a_slice_keeps_the_refusing_default_and_its_parent_does_not_grow() {
    let path = tmp_image("slice", &[3u8; 4096]);
    let parent: Arc<dyn BlockDevice> = Arc::new(FileDevice::open_rw(&path).unwrap());
    let slice = OwnedRwSlice::new(parent.clone(), 512, 1024);

    assert!(
        !slice.can_grow(),
        "a slice cannot grow: its length is the window it was cut to"
    );
    match slice.set_len(2048) {
        Err(Error::ReadOnly) => {}
        other => panic!("expected the refusing default, got {other:?}"),
    }
    assert_eq!(slice.size_bytes(), 1024, "the window is unmoved");
    assert_eq!(
        on_disk(&path),
        4096,
        "and nothing reached the parent's backing file"
    );
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------
// CachingDevice -- the half that a naive set_len gets wrong
// ---------------------------------------------------------------------

/// Block size and capacity for the cache arms. 4096 is what the drivers
/// in this family actually declare; the capacity is large enough that
/// nothing here is evicted, so a re-read can only be an invalidation.
const BS: u64 = 4096;
const CAP: usize = 16;

fn cached(path: &std::path::Path) -> (Arc<CachingDevice>, Arc<FileDevice>) {
    let file = Arc::new(FileDevice::open_rw(path).unwrap());
    let dev: Arc<dyn BlockDevice> = file.clone();
    (CachingDevice::new(dev, BS, CAP), file)
}

/// THE ONE THAT MATTERS: a grow through the cache, read back through the
/// cache, WITH NO INTERVENING WRITE.
///
/// The fixture is 6000 bytes and the block size 4096, so the block at
/// 4096 is SHORT -- `CachingDevice::block` clamps every fetch to
/// `size_bytes()`, and that one comes back 1904 bytes long. Warming it
/// is the whole setup: after `set_len(8192)` the device is 8192 bytes
/// and the cache is holding a block that ends at 6000.
///
/// A `set_len` that moves the file and the reported size but leaves that
/// entry in place fails HERE and nowhere else. Measured with
/// `CachingDevice::set_len` forwarding to the file and sweeping nothing:
///
/// ```text
/// ShortRead { offset: 5000, want: 3000, got: 1000 }
/// ```
///
/// -- which is #70's signature exactly: the write succeeded, the numbers
/// agreed, and only a later cached read found the hole.
///
/// THE MISS COUNTER IS WHY THIS CANNOT PASS VACUOUSLY. An `is_ok()` on
/// the read cannot tell "the cache re-fetched" from "the cache happened
/// to hold enough", the way `tests/caching_wild_offset.rs` cannot tell a
/// refusal from a forward without counting device reads. So the arm
/// asserts the fetch count went up across the grow.
#[test]
fn a_device_grown_through_the_cache_reads_back_through_the_cache() {
    let path = tmp_image("cache_grow", &[0x11u8; 6000]);
    let (cache, _file) = cached(&path);

    // Warm the SHORT last block.
    let mut warm = [0u8; 1000];
    cache.read_at(5000, &mut warm).expect("warm the last block");
    assert_eq!(warm, [0x11u8; 1000]);
    let (_, misses_before) = cache.stats();
    assert_eq!(misses_before, 1, "control: one block was fetched");

    assert!(
        cache.can_grow(),
        "the cache reports its writable half's answer"
    );
    cache.set_len(8192).expect("grow through the cache");
    assert_eq!(cache.size_bytes(), 8192, "the cache reports the new size");
    assert_eq!(on_disk(&path), 8192);

    // ACROSS THE OLD END, and with no write in between -- a write would
    // sweep the same entry for its own reasons and hide the defect.
    let mut back = [0x5Au8; 3000];
    cache
        .read_at(5000, &mut back)
        .expect("a read across the old end must be served from re-fetched blocks");
    assert_eq!(&back[..1000], &[0x11u8; 1000], "the bytes that were there");
    assert_eq!(&back[1000..], &[0u8; 2000], "and the zeros the grow added");

    let (_, misses_after) = cache.stats();
    assert!(
        misses_after > misses_before,
        "the cache served the grown region from the block it fetched BEFORE the \
         grow -- {misses_after} fetches, same as before. set_len must invalidate \
         the entries the new length makes short."
    );
    let _ = std::fs::remove_file(&path);
}

/// The allocation an image writer makes, through the cache this time:
/// grow, write into the new room, read it back.
#[test]
fn an_appended_block_written_through_the_cache_reads_back() {
    let path = tmp_image("cache_append", &[0x11u8; 4096]);
    let (cache, _file) = cached(&path);

    match cache.write_at(4096, &[0xABu8; 4096]) {
        Err(Error::OutOfBounds { .. }) => {}
        other => panic!("a write past the end is still refused: {other:?}"),
    }

    cache.set_len(8192).expect("append a block");
    cache
        .write_at(4096, &[0xABu8; 4096])
        .expect("write into the block just appended");

    let mut back = [0u8; 4096];
    cache.read_at(4096, &mut back).expect("read it back");
    assert_eq!(back, [0xABu8; 4096]);

    // Through a SECOND, cold device, so the bytes are on disk rather
    // than only in this cache.
    let cold = FileDevice::open(&path).unwrap();
    let mut disk = [0u8; 4096];
    cold.read_at(4096, &mut disk).unwrap();
    assert_eq!(disk, [0xABu8; 4096], "the bytes reached the file");
    let _ = std::fs::remove_file(&path);
}

/// A SHRINK THROUGH THE CACHE DROPS WHAT IS NO LONGER THERE.
///
/// # THE ROUND TRIP IS THE ASSERTION, AND THE SHRINK ALONE IS NOT
///
/// Shrinking and then reading past the new end proves little on its own:
/// `CachingDevice::read_at` refuses any read whose end passes
/// `size_bytes()` by forwarding it to the device, so that arm is green
/// with no invalidation at all. It is kept below as a statement about
/// the bound, not as the discriminator.
///
/// What discriminates is shrinking and GROWING BACK. The bytes between
/// the two lengths are zeros on disk after that -- `ftruncate` down then
/// up does not restore anything -- while a cache that swept nothing
/// still holds the block that was there before. Measured against the
/// staged version of `CachingDevice::set_len` that forwarded to the
/// device and swept nothing, this arm serves 2192 bytes of `0x33` where
/// the device holds zeros: stale data returned as success, which is
/// worse than the `ShortRead` the grow arm catches.
///
/// WHAT IT DOES NOT DISCRIMINATE is the sweep's lower bound. `set_len`
/// sweeps from `min(old, new)`; replacing that with `old` alone leaves
/// this suite EXIT=0, 13 passed, measured. Those entries are
/// unreachable while the device is short and are swept by the regrow
/// before they stop being, so no test can reach them -- see that
/// method's own note for why `min` is kept regardless.
#[test]
fn a_shrink_and_regrow_through_the_cache_does_not_serve_the_bytes_that_went() {
    let path = tmp_image("cache_shrink", &[0x33u8; 8192]);
    let (cache, _file) = cached(&path);

    let mut warm = [0u8; 4096];
    cache
        .read_at(4096, &mut warm)
        .expect("warm the second block");
    assert_eq!(warm, [0x33u8; 4096]);

    cache.set_len(6000).expect("shrink through the cache");
    assert_eq!(cache.size_bytes(), 6000);
    assert_eq!(on_disk(&path), 6000);
    assert!(
        cache.read_at(4096, &mut warm).is_err(),
        "a read past the new end must be refused"
    );

    cache.set_len(8192).expect("and grow it back");
    assert_eq!(on_disk(&path), 8192);

    let mut back = [0x5Au8; 2192];
    cache
        .read_at(6000, &mut back)
        .expect("the region between the two lengths");
    assert_eq!(
        back, [0u8; 2192],
        "the cache served the bytes that were there before the shrink -- \
         set_len swept nothing."
    );
    let _ = std::fs::remove_file(&path);
}

/// A READ-ONLY CACHE HAS NO WRITABLE HALF, so there is nothing to grow,
/// and the refusal happens above every sweep -- the cache is left
/// exactly as it was, the way a refused write leaves it.
#[test]
fn a_read_only_cache_refuses_to_grow_and_keeps_what_it_held() {
    let path = tmp_image("cache_ro", &[0x44u8; 4096]);
    let file: Arc<dyn BlockRead> = Arc::new(FileDevice::open(&path).unwrap());
    let cache = CachingDevice::read_only(file, BS, CAP);

    let mut warm = [0u8; 512];
    cache.read_at(0, &mut warm).unwrap();
    let (_, misses) = cache.stats();

    assert!(!cache.can_grow());
    match cache.set_len(8192) {
        Err(Error::ReadOnly) => {}
        other => panic!("expected ReadOnly, got {other:?}"),
    }
    assert_eq!(on_disk(&path), 4096, "nothing reached the file");

    cache.read_at(0, &mut warm).unwrap();
    assert_eq!(
        cache.stats().1,
        misses,
        "a refusal that never reached the device cannot have staled anything, \
         so it must not have swept the cache either"
    );
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------
// The forwarding impls -- the single most likely way to ship nothing
// ---------------------------------------------------------------------

/// `Arc<dyn BlockDevice>` IS WHAT ALL FOUR CONSUMERS HOLD.
///
/// vhdx at 15 sites, qcow2 at 7, vhd at 6, vmdk at 5. Method resolution
/// on an `Arc<dyn BlockDevice>` finds `impl BlockDevice for Arc<T>`
/// BEFORE it derefs, so a forwarding impl that does not override the two
/// new methods hands every one of those call sites the refusing default
/// -- `can_grow()` false and `set_len` `Err(ReadOnly)` -- over a device
/// that can grow perfectly well. The whole change would compile, pass
/// every other test here, and do nothing for the crates it exists for.
#[test]
fn an_arc_dyn_block_device_forwards_growth_to_the_device_inside_it() {
    let path = tmp_image("arc_dyn", &[1u8; 4096]);
    let dev: Arc<dyn BlockDevice> = Arc::new(FileDevice::open_rw(&path).unwrap());

    assert!(
        dev.can_grow(),
        "Arc<dyn BlockDevice> answered the refusing default over a FileDevice \
         that can grow: impl BlockDevice for Arc<T> does not forward can_grow"
    );
    dev.set_len(8192)
        .expect("Arc<dyn BlockDevice> must forward set_len, not refuse it");
    assert_eq!(on_disk(&path), 8192);
    assert_eq!(dev.size_bytes(), 8192);
    let _ = std::fs::remove_file(&path);
}

/// The same for `Box<dyn BlockDevice>`, which has its own impl and so
/// its own chance to forget.
#[test]
fn a_box_dyn_block_device_forwards_growth_to_the_device_inside_it() {
    let path = tmp_image("box_dyn", &[1u8; 4096]);
    let dev: Box<dyn BlockDevice> = Box::new(FileDevice::open_rw(&path).unwrap());

    assert!(
        dev.can_grow(),
        "Box<dyn BlockDevice> does not forward can_grow"
    );
    dev.set_len(8192)
        .expect("Box<dyn BlockDevice> must forward set_len");
    assert_eq!(on_disk(&path), 8192);
    assert_eq!(dev.size_bytes(), 8192);
    let _ = std::fs::remove_file(&path);
}

/// AND THE CONTROL FOR BOTH. A forwarding impl that answered `true` and
/// `Ok(())` unconditionally -- rather than asking the device -- would
/// satisfy the two arms above. This one holds a device that cannot grow
/// and asserts the refusal survives the wrapper.
#[test]
fn the_forwarding_impls_forward_a_refusal_too() {
    let path = tmp_image("arc_dyn_ro", &[1u8; 4096]);
    let arc: Arc<dyn BlockDevice> = Arc::new(FileDevice::open(&path).unwrap());
    assert!(!arc.can_grow());
    assert!(matches!(arc.set_len(8192), Err(Error::ReadOnly)));

    let boxed: Box<dyn BlockDevice> = Box::new(FileDevice::open(&path).unwrap());
    assert!(!boxed.can_grow());
    assert!(matches!(boxed.set_len(8192), Err(Error::ReadOnly)));

    assert_eq!(on_disk(&path), 4096);
    let _ = std::fs::remove_file(&path);
}

/// `size_bytes` IS STILL STABLE AGAINST EVERYTHING THAT IS NOT `set_len`.
///
/// The contract that number carries did not become "anything goes". It
/// moves through this one method and through nothing else -- including a
/// second handle growing the same file underneath the device, which is
/// what `tests/size_stability_contract.rs` measures and what #70 was
/// about. Both halves are asserted on one device so neither can be
/// weakened without the other noticing.
#[test]
fn only_set_len_moves_the_reported_size() {
    let path = tmp_image("stability", &[5u8; 4096]);
    let dev = FileDevice::open_rw(&path).unwrap();

    {
        use std::io::Write as _;
        let mut second = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("a second handle on the same file");
        second.write_all(&[9u8; 512]).expect("grow it underneath");
        second.flush().unwrap();
    }
    assert_eq!(on_disk(&path), 4608, "precondition: the file really grew");
    assert_eq!(
        dev.size_bytes(),
        4096,
        "a device does not follow its backing file. A live metadata().len() \
         would report 4608 here."
    );

    dev.set_len(4608)
        .expect("and this is the one thing that does move it");
    assert_eq!(dev.size_bytes(), 4608);
    let _ = std::fs::remove_file(&path);
}

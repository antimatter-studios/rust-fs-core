//! `BlockRead::size_bytes`'s stability contract, tested rather than
//! asserted in prose.
//!
//! The trait's documentation used to claim "every implementation in this
//! crate takes its size once, at construction". That was wrong about
//! every shape in here, and it is the sentence an implementer in a
//! sibling crate reads before deciding whether their own device may
//! report a live length -- a wrong belief there produces a device that
//! resizes under a cache, which is a defect in someone else's
//! repository. `tests/caching_size_change.rs` stated the opposite
//! contract outright, and `caching_device.rs` a third thing, so the
//! crate said three incompatible things about one contract.
//!
//! A comment asserting an invariant is not the invariant. These tests
//! pin what the types actually do, so the narrowed paragraph in
//! `block.rs` has something behind it.

use fs_core::{
    BlockDevice, BlockRead, CachingDevice, CallbackDevice, CountingDevice, FileDevice,
    OwnedRwSlice, OwnedSlice, ReadOnlyDevice, SliceReader,
};
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

fn tmp_image(bytes: &[u8]) -> String {
    static C: AtomicU32 = AtomicU32::new(0);
    let n = C.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir()
        .join(format!(
            "fs_core_sizecontract_{}_{n}.img",
            std::process::id()
        ))
        .to_string_lossy()
        .into_owned();
    File::create(&p).unwrap().write_all(bytes).unwrap();
    p
}

/// A device whose reported size is whatever was last stored, so a
/// wrapper's forwarding is observable. The same shape
/// `tests/caching_size_change.rs` uses, and equally deliberate: it
/// breaks the contract so something else can be measured.
struct Movable {
    data: Vec<u8>,
    reported: AtomicU64,
}

impl Movable {
    fn new(len: usize) -> Self {
        Movable {
            data: vec![7u8; len],
            reported: AtomicU64::new(len as u64),
        }
    }
    fn report(&self, n: u64) {
        self.reported.store(n, Ordering::SeqCst);
    }
}

impl BlockRead for Movable {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        let start = offset as usize;
        let end = start + buf.len();
        if end > self.data.len() {
            return Err(fs_core::Error::ShortRead {
                offset,
                want: buf.len(),
                got: self.data.len().saturating_sub(start),
            });
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        self.reported.load(Ordering::SeqCst)
    }
}

impl BlockDevice for Movable {
    fn is_writable(&self) -> bool {
        true
    }
}

/// Every trait operation, run against a device, twice around a size
/// reading. A read that fails is as much part of the round as one that
/// succeeds: a device that recomputed its length on an error path
/// would move here and nowhere else.
fn exercise(dev: &dyn BlockDevice) {
    let mut buf = [0u8; 8];
    let _ = dev.read_at(0, &mut buf);
    let _ = dev.read_at(u64::MAX - 4, &mut buf);
    let _ = dev.read_at(dev.size_bytes().saturating_sub(2), &mut buf);
    let _ = dev.write_at(0, &[1u8; 8]);
    let _ = dev.write_at(dev.size_bytes().saturating_sub(2), &[1u8; 8]);
    let _ = dev.flush();
    let _ = dev.is_writable();
}

fn assert_stable(name: &str, dev: &dyn BlockDevice) {
    let before = dev.size_bytes();
    assert!(
        before > 0,
        "{name}: precondition -- a zero-sized device cannot show a size moving"
    );
    exercise(dev);
    assert_eq!(
        dev.size_bytes(),
        before,
        "{name} moved its reported size across a round of trait operations. \
         Callers read it once, act on it, and read it again expecting the same \
         answer -- CachingDevice bounds a read against it and then clamps each \
         block it fetches against it."
    );
}

/// The implementations that DO take their size once, at construction.
#[test]
fn the_devices_that_own_their_size_report_a_stable_one() {
    // THE BACKING IS MOVED THROUGH A SECOND HANDLE, and that is not an
    // ornament -- it is the only thing making this arm able to fail.
    //
    // It used to rely on `exercise` writing past the end of an `open_rw`
    // handle, which grew the real file while the device went on
    // reporting its construction-time size. #70 bounded `write_at`
    // against `self.size`, so that write is now `Error::OutOfBounds`,
    // the file does not grow, and NOTHING IN A ROUND OF TRAIT
    // OPERATIONS PERTURBS THE BACKING. Measured across that merge with
    // one mutation -- `size_bytes` returning a live `metadata().len()`
    // -- this arm was EXIT=101 at f1e4aa9 and EXIT=0 at 4e19fc9. It
    // stopped discriminating without going red, which is why nobody saw
    // it.
    //
    // The bound is right; the instrument was wrong. What has to move is
    // the BACKING STORE, so a second plain handle moves it explicitly,
    // the way the slice arms below move theirs through `Movable`.
    let path = tmp_image(&vec![3u8; 4096]);
    let file = FileDevice::open_rw(&path).unwrap();
    assert_stable("FileDevice", &file);

    {
        use std::io::Write as _;
        let mut grow = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open a second handle on the backing file");
        grow.write_all(&[9u8; 6]).expect("grow the backing file");
        grow.flush().expect("flush the growth");
    }
    assert_eq!(
        std::fs::metadata(&path)
            .expect("stat the backing file")
            .len(),
        4102,
        "precondition -- the backing file must really have grown, or the assertion \
         below says nothing about the device in front of it"
    );
    assert_eq!(
        file.size_bytes(),
        4096,
        "FileDevice took its size at construction and must keep reporting that. A \
         live metadata().len() would follow the file here."
    );
    let _ = std::fs::remove_file(&path);

    // THE BACKING HAS TO MOVE FOR THESE THREE, and `assert_stable`
    // alone does not move it. The same is now true of `FileDevice`
    // above, which is why it grows its file through a second handle
    // rather than relying on a write the crate no longer performs.
    //
    // The slice devices take their length from the parent at
    // construction and nothing in a round of trait operations changes
    // what the parent reports. So a slice that RECOMPUTED its length
    // from the parent on every call -- exactly the bug this file
    // exists to catch -- passed every assertion here. Measured: all
    // three snapshots replaced at once with `self.parent.size_bytes()
    // / 4`, chosen to agree at construction and diverge only when the
    // backing moves, and this suite was EXIT=0, 3 passed. Three
    // guarantees swapped for live values and the tests that exist to
    // pin them noticed nothing.
    let rw_backing = Arc::new(Movable::new(4096));
    let parent: Arc<dyn BlockDevice> = rw_backing.clone();
    let rw_slice = OwnedRwSlice::new(parent, 512, 1024);
    assert_stable("OwnedRwSlice", &rw_slice);
    assert_survives_the_backing_moving("OwnedRwSlice", &rw_slice, &rw_backing);

    let ro_backing = Arc::new(Movable::new(4096));
    let ro: Arc<dyn BlockRead> = ro_backing.clone();
    let owned = OwnedSlice::new(ro, 512, 1024);
    assert_stable("OwnedSlice", &ro_as_device(&owned));
    assert_survives_the_backing_moving("OwnedSlice", &owned, &ro_backing);

    let backing = Movable::new(4096);
    let slice = SliceReader::new(&backing, 512, 1024);
    assert_stable("SliceReader", &ro_as_device(&slice));
    assert_survives_the_backing_moving("SliceReader", &slice, &backing);
}

/// A snapshotted length must not follow the parent when the parent's
/// reported size moves.
///
/// The device is asked its size, the BACKING is then told to report
/// something else, and the device must give the same answer as before.
/// `Movable::report` breaks `size_bytes`'s contract on purpose, which
/// is the only way to tell a snapshot from a live read.
fn assert_survives_the_backing_moving(name: &str, dev: &dyn BlockRead, backing: &Movable) {
    let before = dev.size_bytes();
    assert!(
        before > 0,
        "{name}: precondition -- a zero-length device cannot show a length moving"
    );
    backing.report(before / 2);
    assert_ne!(
        backing.size_bytes(),
        before,
        "{name}: precondition -- the BACKING must actually report something different, \
         or this asserts nothing about the device in front of it"
    );
    assert_eq!(
        dev.size_bytes(),
        before,
        "{name} followed its parent's new size. It took its length at construction, so \
         it must keep reporting that -- a length recomputed from the parent on every \
         call is the bug this file exists to catch, and it passed every other \
         assertion here."
    );
}

/// `BlockRead`-only types still have to answer the same question, so
/// wrap them in the crate's own read-only adapter to run the round.
fn ro_as_device<T: BlockRead>(inner: &T) -> ReadOnlyDevice<&T> {
    ReadOnlyDevice::new(inner)
}

/// A WRAPPER IS EXACTLY AS STABLE AS WHAT IT WRAPS, and cannot be more.
///
/// This is the clause the old sentence got wrong in the direction that
/// matters: `ReadOnlyDevice`, `CountingDevice` and `CachingDevice` do
/// not snapshot anything at construction, they forward the question on
/// every call. An implementer who read "every implementation takes its
/// size once" would conclude a wrapper protects them from a moving
/// device. It does not, and there is no way for it to.
///
/// Snapshotting the inner size at construction is the plausible "fix"
/// this test exists to refuse: it would make a wrapper disagree with
/// the device underneath it, which is worse than reporting a number
/// that moved -- `CachingDevice`'s own resizing path relies on seeing
/// the device's current answer in order to catch it.
#[test]
fn a_wrapper_forwards_its_inner_devices_size_rather_than_snapshotting_it() {
    let inner = Arc::new(Movable::new(4096));

    let counting = CountingDevice::new(inner.clone() as Arc<dyn BlockRead>);
    let readonly = ReadOnlyDevice::new(inner.clone() as Arc<dyn BlockRead>);
    let caching = CachingDevice::new(inner.clone() as Arc<dyn BlockDevice>, 512, 4);
    let arced: Arc<dyn BlockRead> = inner.clone();
    // OVER THE SAME BACKING as everything else here. This was a
    // separate `Movable`, so the move below could not reach it and
    // `Box<T>`'s forwarding was never observed under a moving value --
    // only asserted once, before the move, where a snapshot would have
    // agreed too.
    let boxed: Box<dyn BlockRead> = Box::new(inner.clone());
    // The `&T` blanket impl, which had no coverage at all. `&by_ref`
    // is a `&&Movable`, so the method resolves through
    // `impl BlockRead for &T` rather than through `Movable`'s own.
    let by_ref: &Movable = &inner;

    assert_eq!(counting.size_bytes(), 4096, "control: before the move");
    assert_eq!(readonly.size_bytes(), 4096, "control: before the move");
    assert_eq!(caching.size_bytes(), 4096, "control: before the move");
    assert_eq!(arced.size_bytes(), 4096, "control: before the move");
    assert_eq!(boxed.size_bytes(), 4096, "control: before the move");
    assert_eq!(
        BlockRead::size_bytes(&by_ref),
        4096,
        "control: before the move"
    );

    inner.report(2048);

    for (name, got) in [
        ("CountingDevice", counting.size_bytes()),
        ("ReadOnlyDevice", readonly.size_bytes()),
        ("CachingDevice", caching.size_bytes()),
        ("Arc<dyn BlockRead>", arced.size_bytes()),
        ("Box<dyn BlockRead>", boxed.size_bytes()),
        ("&T", BlockRead::size_bytes(&by_ref)),
    ] {
        assert_eq!(
            got, 2048,
            "{name} must report the device's CURRENT size, not one snapshotted at \
             construction. A wrapper cannot hold a moving device still, and pretending \
             to would hide the violation instead of letting the cache catch it."
        );
    }
}

/// `CallbackDevice`'s size is a PUBLIC field, so stability there is the
/// caller's to keep rather than the type's to enforce -- which is why
/// the trait's paragraph cannot say "at construction" about it.
///
/// The control for this one is a compile error rather than a failure:
/// the only way to break it is to make the field private or snapshot
/// it, and then this test does not build. That is the complete control
/// here, because the public field IS the property being pinned.
#[test]
fn callback_devices_size_is_a_field_the_caller_owns() {
    let mut dev = CallbackDevice {
        size: 12345,
        read: Box::new(|_, _| Ok(())),
        write: None,
        flush: None,
    };
    assert_eq!(dev.size_bytes(), 12345, "control: as constructed");

    dev.size = 99;
    assert_eq!(
        dev.size_bytes(),
        99,
        "the field is public and size_bytes reads it, so a caller can move the \
         reported size after construction. Nothing in the type prevents that, and \
         the trait's documentation must not claim otherwise."
    );
}

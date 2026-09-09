//! Concurrent misses on one block: one device read, and nothing evicted.
//!
//! Threads that all miss the same block used to all read the device and
//! all insert, so a capacity-4 cache could finish holding four copies of
//! one block having evicted the three legitimate entries to make room.
//! A cache that gets worse under contention is the opposite of a cache.
//!
//! # Why the gate is not a `Barrier`
//!
//! The obvious harness is a `Barrier` sized to the racer count, held
//! inside the device read. It reproduces the defect perfectly and
//! **deadlocks the fix**: once one fetch per block is the rule, only one
//! thread ever reaches the device, so a barrier waiting for four is
//! waiting for three threads that are correctly asleep. A test that
//! hangs on the fixed code and passes on the broken code is worse than
//! no test.
//!
//! So the gate releases when the racers are all inside **or** when the
//! test says so, and it reports how many were inside when it opened.
//! That count is the measurement: it is the number of threads that got
//! into the device for one block, which is the defect stated as a
//! number. Four before, one after.
//!
//! The fixed path therefore waits out [`SETTLE`] once. That is a
//! lower-bound sleep rather than a race — no assertion depends on how
//! long anything took, and a loaded machine makes it slower, not
//! flakier.

use fs_core::{BlockRead, CachingDevice, Result};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const BS: u64 = 8;
/// The block every racer asks for.
const TARGET: u64 = 24;
const RACERS: usize = 4;
const CAPACITY: usize = 4;
/// Blocks primed before the race. Three, so that with the target's entry
/// the cache is exactly full and correct behaviour evicts nothing.
const PRIMED: [u64; 3] = [0, 8, 16];
/// How long the test waits for the racers to gather before opening the
/// gate on whoever turned up.
const SETTLE: Duration = Duration::from_secs(2);
/// How long a racer is given to come back once the gate is open. Long
/// enough that only a thread that is never woken at all runs it out.
const JOIN_DEADLINE: Duration = Duration::from_secs(10);

struct GateState {
    arrived: usize,
    released: bool,
}

/// Holds readers inside the device read until they have all arrived or
/// the test opens it.
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
    want: usize,
}

impl Gate {
    fn new(want: usize) -> Self {
        Gate {
            state: Mutex::new(GateState {
                arrived: 0,
                released: false,
            }),
            changed: Condvar::new(),
            want,
        }
    }

    /// Called from inside the device read, once per read of the target.
    fn park(&self) {
        let mut g = self.state.lock().expect("gate lock");
        g.arrived += 1;
        self.changed.notify_all();
        while g.arrived < self.want && !g.released {
            g = self.changed.wait(g).expect("gate wait");
        }
    }

    /// Called from the test. Waits up to `SETTLE` for every racer to be
    /// inside the device, then opens the gate on however many are and
    /// returns that number.
    fn open_when_full_or_settled(&self) -> usize {
        let mut g = self.state.lock().expect("gate lock");
        let deadline = Instant::now() + SETTLE;
        while g.arrived < self.want {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let (next, _) = self.changed.wait_timeout(g, left).expect("gate wait");
            g = next;
        }
        let inside = g.arrived;
        g.released = true;
        self.changed.notify_all();
        inside
    }
}

/// A byte-buffer device that parks inside reads of one chosen offset and
/// counts every read it serves, by offset.
struct GatedDevice {
    bytes: Vec<u8>,
    gate: Arc<Gate>,
    target: u64,
    reads: Mutex<Vec<u64>>,
}

impl GatedDevice {
    fn new(bytes: Vec<u8>, gate: Arc<Gate>, target: u64) -> Self {
        GatedDevice {
            bytes,
            gate,
            target,
            reads: Mutex::new(Vec::new()),
        }
    }

    /// How many reads this device served at `offset`.
    fn reads_at(&self, offset: u64) -> usize {
        self.reads
            .lock()
            .expect("reads lock")
            .iter()
            .filter(|o| **o == offset)
            .count()
    }

    fn total_reads(&self) -> usize {
        self.reads.lock().expect("reads lock").len()
    }
}

impl BlockRead for GatedDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.reads.lock().expect("reads lock").push(offset);
        if offset == self.target {
            self.gate.park();
        }
        let start = offset as usize;
        let end = start + buf.len();
        assert!(end <= self.bytes.len(), "the test never reads past the end");
        buf.copy_from_slice(&self.bytes[start..end]);
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.bytes.len() as u64
    }
}

fn read_block(cache: &CachingDevice, offset: u64) -> Vec<u8> {
    let mut buf = vec![0u8; BS as usize];
    cache
        .read_at(offset, &mut buf)
        .unwrap_or_else(|e| panic!("read at {offset}: {e:?}"));
    buf
}

fn expected_block(bytes: &[u8], offset: u64) -> Vec<u8> {
    bytes[offset as usize..(offset + BS) as usize].to_vec()
}

#[test]
fn concurrent_misses_on_one_block_read_the_device_once_and_evict_nothing() {
    let bytes: Vec<u8> = (0..64u8).collect();
    let gate = Arc::new(Gate::new(RACERS));
    let device = Arc::new(GatedDevice::new(bytes.clone(), Arc::clone(&gate), TARGET));
    let cache = CachingDevice::read_only(Arc::clone(&device) as Arc<dyn BlockRead>, BS, CAPACITY);

    // Prime three distinct blocks, then confirm they really are cached.
    // Without this the "nothing was evicted" assertion at the end could
    // hold because they were never cached in the first place.
    for off in PRIMED {
        assert_eq!(read_block(&cache, off), expected_block(&bytes, off));
    }
    assert_eq!(
        device.total_reads(),
        PRIMED.len(),
        "priming should be one device read per block"
    );
    for off in PRIMED {
        read_block(&cache, off);
    }
    assert_eq!(
        device.total_reads(),
        PRIMED.len(),
        "the primed blocks must be served from the cache before the race, \
         or the eviction assertion at the end proves nothing"
    );

    // The race. Every thread asks for the same absent block.
    //
    // Results come back over a channel with a deadline rather than
    // through `join`. A waiter that is never woken would block `join`
    // for ever and hang the suite; here it is a named failure instead,
    // which is the difference between a test that reports and a test
    // that has to be killed.
    let (tx, rx) = std::sync::mpsc::channel();
    for n in 0..RACERS {
        let cache = Arc::clone(&cache);
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((n, read_block(&cache, TARGET)));
        });
    }
    drop(tx);

    let inside = gate.open_when_full_or_settled();

    for _ in 0..RACERS {
        let (n, got) = rx.recv_timeout(JOIN_DEADLINE).expect(
            "a racer never returned. A thread waiting for a fetch that is \
             already in flight must be woken when it lands",
        );
        assert_eq!(
            got,
            expected_block(&bytes, TARGET),
            "racer {n} got the wrong bytes"
        );
    }

    // FIRST HALF: one thread reached the device, not four.
    assert_eq!(
        inside, 1,
        "{inside} of {RACERS} threads were inside the device read for block \
         {TARGET} at once; a block already being fetched must be waited for, \
         not fetched again"
    );
    assert_eq!(
        device.reads_at(TARGET),
        1,
        "block {TARGET} was read from the device {} times for one miss",
        device.reads_at(TARGET)
    );

    // SECOND HALF, AND THE ONE THAT MATTERS MORE. Deduplicating the
    // fetch is not enough on its own: a fix that still inserted once per
    // racer would evict all three primed blocks to store copies of a
    // block it already held, and the read count above would not notice.
    let before = device.total_reads();
    for off in PRIMED {
        assert_eq!(read_block(&cache, off), expected_block(&bytes, off));
    }
    assert_eq!(
        device.total_reads(),
        before,
        "the {} primed entries did not survive one concurrent miss -- a \
         capacity-{CAPACITY} cache evicted them to hold duplicates of block \
         {TARGET}",
        PRIMED.len()
    );

    // And the accounting. `misses` is the number of fetches the device
    // actually served, so a hit rate read off these numbers describes
    // the traffic rather than the contention.
    let (hits, misses) = cache.stats();
    let calls = (PRIMED.len() * 3 + RACERS) as u64;
    assert_eq!(
        hits + misses,
        calls,
        "every call is one hit or one miss: {hits} + {misses} != {calls}"
    );
    assert_eq!(
        misses,
        device.total_reads() as u64,
        "one miss per device fetch: {misses} misses against {} reads",
        device.total_reads()
    );
}

/// A failed fetch must not strand the threads waiting for it.
///
/// The marker saying "this block is being fetched" is cleared by a
/// guard rather than by a line at the end of the fetch, because the
/// device read returns through `?`. Without that, one `Err` would leave
/// the block marked as in flight for ever and every later reader of it
/// would wait for a thread that had already gone.
#[test]
fn a_fetch_that_fails_releases_the_block_it_was_holding() {
    /// Fails the first read of each offset and serves every later one.
    struct FailsOnce {
        bytes: Vec<u8>,
        failed: Mutex<Vec<u64>>,
    }
    impl BlockRead for FailsOnce {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
            let mut failed = self.failed.lock().expect("lock");
            if !failed.contains(&offset) {
                failed.push(offset);
                return Err(fs_core::Error::ShortRead {
                    offset,
                    want: buf.len(),
                    got: 0,
                });
            }
            drop(failed);
            let start = offset as usize;
            buf.copy_from_slice(&self.bytes[start..start + buf.len()]);
            Ok(())
        }
        fn size_bytes(&self) -> u64 {
            self.bytes.len() as u64
        }
    }

    let device = Arc::new(FailsOnce {
        bytes: (0..64u8).collect(),
        failed: Mutex::new(Vec::new()),
    });
    let cache = CachingDevice::read_only(device as Arc<dyn BlockRead>, BS, CAPACITY);

    let mut buf = vec![0u8; BS as usize];
    assert!(
        cache.read_at(TARGET, &mut buf).is_err(),
        "the first read of this block fails at the device"
    );

    // The retry is the assertion. If the failed fetch left the block
    // marked in flight, this waits for ever instead of fetching it.
    let (tx, rx) = std::sync::mpsc::channel();
    let retry = Arc::clone(&cache);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; BS as usize];
        let outcome = retry.read_at(TARGET, &mut buf);
        let _ = tx.send(outcome.map(|()| buf));
    });

    let got = rx
        .recv_timeout(Duration::from_secs(10))
        .expect(
            "a read of a block whose previous fetch failed must not block; \
             the failed fetch never released it",
        )
        .expect("and the retry itself succeeds");
    assert_eq!(got, (24..32u8).collect::<Vec<u8>>());
}

/// A DEVICE THAT READS BACK THROUGH THE CACHE WRAPPING IT DOES NOT
/// DEADLOCK, and does not leave two entries for one block.
///
/// One fetch per block is enforced by a marker, and a marker a thread
/// can block on is a marker a thread can block on *itself*. A device
/// whose `read_at` re-enters the cache for the block it is being asked
/// for would wait for a fetch it is holding up. Before one fetch per
/// block was the rule this merely did a redundant read, so the guard
/// exists to keep that outcome rather than to support re-entrancy: the
/// drivers built on this crate run as filesystem extensions, where a
/// deadlock in a fetch is a spinning cursor on a volume that cannot be
/// unmounted rather than an error anybody sees.
///
/// The deadline is what makes this a test. Without it the failure is a
/// hung suite that has to be killed, and a hang reports nothing.
#[test]
fn a_device_that_re_enters_the_cache_reads_again_instead_of_waiting_for_itself() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::OnceLock;

    /// Re-enters the cache once, for the same offset it is serving.
    struct Reentrant {
        bytes: Vec<u8>,
        cache: OnceLock<std::sync::Weak<CachingDevice>>,
        reads: AtomicUsize,
        /// Reads of the target, whether or not they recursed.
        saw_target: AtomicUsize,
        /// Reads that actually recursed into the cache.
        nested: AtomicUsize,
    }

    impl BlockRead for Reentrant {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            // Once only, or it recurses for ever. The first read of the
            // target asks the cache for the very block it is fetching.
            if offset == TARGET && self.saw_target.fetch_add(1, Ordering::SeqCst) == 0 {
                self.nested.fetch_add(1, Ordering::SeqCst);
                let cache = self
                    .cache
                    .get()
                    .expect("the test wires this before reading")
                    .upgrade()
                    .expect("the cache outlives the read");
                let mut inner = vec![0u8; buf.len()];
                cache
                    .read_at(offset, &mut inner)
                    .expect("the re-entrant read itself succeeds");
                assert_eq!(
                    inner,
                    self.bytes[offset as usize..offset as usize + buf.len()],
                    "the re-entrant read returned the wrong bytes"
                );
            }
            let start = offset as usize;
            buf.copy_from_slice(&self.bytes[start..start + buf.len()]);
            Ok(())
        }
        fn size_bytes(&self) -> u64 {
            self.bytes.len() as u64
        }
    }

    let device = Arc::new(Reentrant {
        bytes: (0..64u8).collect(),
        cache: OnceLock::new(),
        reads: AtomicUsize::new(0),
        saw_target: AtomicUsize::new(0),
        nested: AtomicUsize::new(0),
    });
    let cache = CachingDevice::read_only(Arc::clone(&device) as Arc<dyn BlockRead>, BS, CAPACITY);
    device.cache.set(Arc::downgrade(&cache)).expect("set once");

    // Fill the cache to capacity minus one, so that a duplicate entry
    // for the target has to evict one of these to fit. That is how a
    // second entry for one block is observable from outside.
    for off in PRIMED {
        read_block(&cache, off);
    }

    let (tx, rx) = std::sync::mpsc::channel();
    {
        let cache = Arc::clone(&cache);
        std::thread::spawn(move || {
            let _ = tx.send(read_block(&cache, TARGET));
        });
    }
    let got = rx.recv_timeout(JOIN_DEADLINE).expect(
        "a device that read back through the cache wrapping it waited for its \
         own fetch instead of reading again -- this is the deadlock the owner \
         on each in-flight marker exists to prevent",
    );
    assert_eq!(got, expected_block(&device.bytes, TARGET));

    // NEGATIVE CONTROL. If the device never actually recursed, every
    // other assertion here is about an ordinary cached read and this
    // test is about nothing.
    assert_eq!(
        device.nested.load(Ordering::SeqCst),
        1,
        "the device must actually have re-entered the cache once, or this \
         test asserts nothing"
    );
    assert_eq!(
        device.saw_target.load(Ordering::SeqCst),
        2,
        "the target must have been read twice: the outer fetch and the \
         re-entrant one that could not wait for it"
    );
    assert_eq!(
        device.reads.load(Ordering::SeqCst),
        PRIMED.len() + 2,
        "the primed blocks, plus the outer fetch and the re-entrant one: a \
         thread that cannot wait for itself reads the device again"
    );

    // The block is held once, not twice, so nothing was evicted to make
    // room for a duplicate of it.
    let before = device.reads.load(Ordering::SeqCst);
    for off in PRIMED {
        read_block(&cache, off);
    }
    assert_eq!(
        device.reads.load(Ordering::SeqCst),
        before,
        "a re-entrant fetch inserted block {TARGET} twice and evicted a primed \
         entry to make room for the copy"
    );
}

/// TWO THREADS RE-ENTERING INTO EACH OTHER'S FETCHES, AND NEITHER IS
/// WAITING FOR ITSELF.
///
/// The test above covers the cycle of length one: a thread that
/// re-enters for the block it is already fetching. Carrying the owner on
/// each in-flight marker is what makes that visible — but a test that
/// compares the owner against the thread asking can only ever see that
/// one shape.
///
/// Here thread A fetches block A and re-enters for block B, while thread
/// B fetches block B and re-enters for block A. Neither thread finds its
/// own id against the block it wants, so an owner-vs-self test lets both
/// of them wait — each for a fetch the other is holding up, on a
/// condition variable only a completed fetch can signal. Nothing
/// completes. It is the same deadlock one link longer.
///
/// # Why the barrier, and why it is not a race
///
/// The deadlock needs both fetches registered as in flight *before*
/// either re-enters; if A's re-entry lands before B has claimed block B,
/// A simply fetches it and there is no cycle to see. The barrier makes
/// that ordering the only possible one, so this test does not depend on
/// which thread the scheduler favours. There are no sleeps here and no
/// assertion reads a clock.
///
/// The one-shot flags are what stop the recursion: without them the two
/// devices re-enter for each other for ever, which is a stack overflow
/// rather than the hang under test.
///
/// # The deadline is what makes this a test
///
/// The failure mode is a hang, and a hung suite reports nothing and has
/// to be killed. Coming back over a channel with a deadline turns it
/// into a named assertion instead.
#[test]
fn two_threads_re_entering_into_each_others_fetches_do_not_deadlock() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Barrier, OnceLock};

    /// The two blocks the threads fetch, each re-entering for the other.
    const BLOCK_A: u64 = 24;
    const BLOCK_B: u64 = 32;
    /// Two primed blocks. With an entry for each of the two above, a
    /// capacity-4 cache is then exactly full, so a duplicate entry for
    /// either block would have to evict a primed one to fit — which is
    /// how "held once, not twice" is observable from outside.
    const PRIMED_PAIR: [u64; 2] = [0, 8];

    /// Re-enters the cache once per block, for the *other* block.
    struct CrossReentrant {
        bytes: Vec<u8>,
        cache: OnceLock<std::sync::Weak<CachingDevice>>,
        /// Opens once both blocks are marked in flight.
        gate: Barrier,
        /// One shot each, or the two reads recurse into each other
        /// without end.
        recursed_a: AtomicBool,
        recursed_b: AtomicBool,
        /// Reads that actually re-entered the cache.
        nested: AtomicUsize,
        reads: Mutex<Vec<u64>>,
    }

    impl CrossReentrant {
        fn total_reads(&self) -> usize {
            self.reads.lock().expect("reads lock").len()
        }
    }

    impl BlockRead for CrossReentrant {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
            self.reads.lock().expect("reads lock").push(offset);

            // The partner block this read re-enters for, claimed once.
            let partner = match offset {
                BLOCK_A if !self.recursed_a.swap(true, Ordering::SeqCst) => Some(BLOCK_B),
                BLOCK_B if !self.recursed_b.swap(true, Ordering::SeqCst) => Some(BLOCK_A),
                _ => None,
            };

            if let Some(partner) = partner {
                // Hold here until BOTH blocks are in flight, so the
                // re-entry below is certain to land on a fetch another
                // thread owns.
                self.gate.wait();
                self.nested.fetch_add(1, Ordering::SeqCst);
                let cache = self
                    .cache
                    .get()
                    .expect("the test wires this before reading")
                    .upgrade()
                    .expect("the cache outlives the read");
                let mut inner = vec![0u8; buf.len()];
                cache
                    .read_at(partner, &mut inner)
                    .expect("the re-entrant read itself succeeds");
                assert_eq!(
                    inner,
                    self.bytes[partner as usize..partner as usize + buf.len()],
                    "the re-entrant read of block {partner} returned the wrong bytes"
                );
            }

            let start = offset as usize;
            buf.copy_from_slice(&self.bytes[start..start + buf.len()]);
            Ok(())
        }

        fn size_bytes(&self) -> u64 {
            self.bytes.len() as u64
        }
    }

    let device = Arc::new(CrossReentrant {
        bytes: (0..64u8).collect(),
        cache: OnceLock::new(),
        gate: Barrier::new(2),
        recursed_a: AtomicBool::new(false),
        recursed_b: AtomicBool::new(false),
        nested: AtomicUsize::new(0),
        reads: Mutex::new(Vec::new()),
    });
    let cache = CachingDevice::read_only(Arc::clone(&device) as Arc<dyn BlockRead>, BS, CAPACITY);
    device.cache.set(Arc::downgrade(&cache)).expect("set once");

    for off in PRIMED_PAIR {
        read_block(&cache, off);
    }
    assert_eq!(
        device.total_reads(),
        PRIMED_PAIR.len(),
        "priming should be one device read per block"
    );

    // The cycle. Each thread asks for one block; the device read for
    // that block re-enters for the other.
    let (tx, rx) = std::sync::mpsc::channel();
    for block in [BLOCK_A, BLOCK_B] {
        let cache = Arc::clone(&cache);
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((block, read_block(&cache, block)));
        });
    }
    drop(tx);

    let mut returned = Vec::new();
    for _ in 0..2 {
        let (block, got) = rx.recv_timeout(JOIN_DEADLINE).expect(
            "one of the two threads never came back. Each re-entered the cache \
             for the block the other was fetching, so each waited for a fetch \
             the other was holding up and neither could ever be signalled. A \
             thread that already owns a fetch must read again rather than wait, \
             or a cycle of length two hangs both of them",
        );
        assert_eq!(
            got,
            expected_block(&device.bytes, block),
            "block {block} came back wrong"
        );
        returned.push(block);
    }
    returned.sort_unstable();
    assert_eq!(
        returned,
        vec![BLOCK_A, BLOCK_B],
        "both threads must report, once each"
    );

    // NEGATIVE CONTROL. If the reads never actually re-entered, this is
    // two ordinary concurrent misses and every assertion above is about
    // nothing. Both flags are claimed before the barrier opens, so this
    // count is not a race.
    assert_eq!(
        device.nested.load(Ordering::SeqCst),
        2,
        "both device reads must have re-entered the cache for the other's \
         block, or the cycle this test exists for never formed"
    );

    // AND EACH BLOCK IS HELD ONCE, NOT TWICE. Breaking the cycle by
    // reading again puts two fetches in flight for one block, so the
    // insert on the way back has to notice the block is already held
    // rather than storing a second copy of it. With the two primed
    // entries the cache is exactly full, so a duplicate would have
    // evicted one of them and this would go back to the device.
    let before = device.total_reads();
    for off in PRIMED_PAIR.iter().copied().chain([BLOCK_A, BLOCK_B]) {
        read_block(&cache, off);
    }
    assert_eq!(
        device.total_reads(),
        before,
        "after the race all four blocks must be cached; a duplicate entry for \
         block {BLOCK_A} or {BLOCK_B} evicted a primed one to make room"
    );
}

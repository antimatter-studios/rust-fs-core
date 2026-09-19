//! The stable-toolchain half of the fuzzing setup: replay the corpus,
//! then mutate it, and refuse if a decoder panics, hangs, or if the
//! suite quietly stopped doing any work.
//!
//! # Why there are two halves
//!
//! `fuzz/` holds `cargo-fuzz` targets. Those are the explorer: they run
//! for as long as you give them and find inputs nobody thought of. They
//! cannot be a required check, because how long they ran decides what
//! they found, and a fresh discovery would fail whichever unrelated
//! pull request happened to be open.
//!
//! This suite is the gate. Deterministic, on the stable toolchain, in
//! every pull request, reading the same `fuzz/corpus/` the explorer
//! does. Anything the explorer finds is committed there and replayed
//! here from then on.
//!
//! # Fuzzing this crate is not like fuzzing the drivers
//!
//! There are no structures here to mutate, because nothing in this
//! crate parses a filesystem. What it does is arithmetic on offsets and
//! lengths that ultimately came from an image -- a partition start, an
//! extent offset, a declared device size -- and the release profile has
//! `overflow-checks` off.
//!
//! So the input is read as GEOMETRY rather than as a structure, and
//! each target asserts a PROPERTY rather than merely surviving:
//!
//! - a slice answers a read inside it with the parent's bytes from
//!   `start + offset`, and refuses anything else;
//! - `SliceReader` and `OwnedSlice` agree, being two implementations of
//!   one contract;
//! - a cache answers exactly what the device underneath it would, at
//!   every block size, including on the second read of the same range.
//!
//! A target that only checked for panics would pass on a slice that
//! answered with somebody else's bytes -- and that is the defect that
//! actually happened here, in the 2026-09-06 wave: "arithmetic that
//! wrapped in the release profile and answered a read inside a slice's
//! own declared length with somebody else's bytes".
//!
//! # Why the parent is a pattern and not zeros
//!
//! A slice reading from the wrong offset returns bytes that are
//! perfectly valid and belong somewhere else. Only a parent whose
//! content differs per position can tell the two apart, so
//! `parent_byte` is a position hash rather than a constant -- and one
//! chosen so that bytes a power of two apart differ, which a
//! shift-based bug would otherwise line up.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// Where this suite looks for the corpus.
fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz/corpus")
}

// The geometry decoder and the properties, shared verbatim with the
// explorer. See fuzz/shared/props.rs for why they are included rather
// than depended on.
include!("../fuzz/shared/props.rs");

/// Distinct starting points for the mutation stream. Fixed, so a
/// failure reproduces from the message alone.
const SEEDS: u64 = 6;

/// Below this, the suite is not doing its job.
const CASE_FLOOR: usize = 6_000;

/// Long enough that a loaded machine is never the reason, short enough
/// that a genuine hang is reported rather than left to the job timeout.
const DEADLINE: Duration = Duration::from_secs(180);

// ---------------------------------------------------------------- targets

struct Target {
    corpus: &'static str,
    name: &'static str,
    /// Mutated cases per (seed, starting point) pair.
    ///
    /// Per target rather than one constant, because a case costs what
    /// its seed costs to copy and to run. A 64 KiB region table is
    /// cheap; a 16 MB image opened and read is not, and giving both the
    /// same budget would mean either a slow gate or a shallow one.
    cases: usize,
    run: fn(&[u8]),
}

fn targets() -> Vec<Target> {
    vec![
        Target {
            corpus: "slice",
            name: "slice",
            cases: 512,
            run: |b| {
                check_slice(&geometry_from(b));
            },
        },
        Target {
            corpus: "cache",
            name: "cache",
            cases: 256,
            run: |b| {
                check_cache(&geometry_from(b));
            },
        },
    ]
}

// ---------------------------------------------------------------- corpus

fn seeds(corpus: &str) -> Vec<(String, Vec<u8>)> {
    let dir = corpus_root().join(corpus);
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading the corpus directory {}: {e}", dir.display()))
        .map(|entry| {
            let path = entry.expect("corpus directory entry").path();
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("reading the seed {}: {e}", path.display()));
            let name = path
                .file_name()
                .expect("seed file name")
                .to_string_lossy()
                .into_owned();
            (name, bytes)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------------------------------------------------------------- mutation

/// xorshift64*. Small, deterministic, and not a dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }
}

/// One mutation of a real structure or a real image, preserving length.
///
/// Length is preserved because a device answers a read past its end
/// with `ShortRead` before any of this code is reached -- a hostile
/// image controls what is in a block, not how many bytes the device
/// hands back.
///
/// The `header` bias exists because an image is mostly file data: a
/// uniformly random offset in a 48 KiB image lands in somebody's text
/// file nine times out of ten, where nothing parses it. Half the
/// mutations are aimed at the first two blocks, which is where the
/// superblock, the inode table and the directory blocks are.
fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut out = seed.to_vec();
    if out.is_empty() {
        return out;
    }
    let metadata_end = out.len().min(8192);
    let region = if rng.next() & 1 == 0 {
        metadata_end
    } else {
        out.len()
    };

    match rng.below(5) {
        0 => {
            for _ in 0..=rng.below(8) {
                let at = rng.below(region);
                out[at] ^= 1u8 << rng.below(8);
            }
        }
        1 => {
            let at = rng.below(region);
            let len = 1 + rng.below(16.min(out.len() - at));
            let fill = if rng.next() & 1 == 0 { 0x00 } else { 0xff };
            out[at..at + len].fill(fill);
        }
        2 => {
            let width = [2usize, 4, 8][rng.below(3)];
            if out.len() >= width {
                let at = rng.below(region.saturating_sub(width) + 1) & !(width - 1);
                if at + width <= out.len() {
                    let value: u64 = match rng.below(4) {
                        0 => 0,
                        1 => 1,
                        2 => u64::MAX,
                        _ => rng.next(),
                    };
                    // Little-endian: every multi-byte field in partition table is.
                    out[at..at + width].copy_from_slice(&value.to_le_bytes()[..width]);
                }
            }
        }
        3 => {
            if out.len() >= 8 {
                let a = rng.below(region / 4) * 4;
                let b = rng.below(region / 4) * 4;
                if a + 4 <= out.len() && b + 4 <= out.len() {
                    for i in 0..4 {
                        out.swap(a + i, b + i);
                    }
                }
            }
        }
        _ => {
            if out.len() >= 4 {
                let at = rng.below(region / 4) * 4;
                if at + 4 <= out.len() {
                    let word = u32::from_le_bytes(out[at..at + 4].try_into().expect("4 bytes"));
                    let delta = [1i64, -1, 2, -2, 255, -255][rng.below(6)];
                    let changed = (i64::from(word).wrapping_add(delta)) as u32;
                    out[at..at + 4].copy_from_slice(&changed.to_le_bytes());
                }
            }
        }
    }
    out
}

/// The case in flight, readable even if the lock was poisoned by the
/// panic we are trying to describe.
fn describe(current: &Arc<Mutex<String>>) -> String {
    match current.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

// ---------------------------------------------------------------- tests

#[test]
fn every_target_has_a_corpus() {
    for target in targets() {
        assert!(
            !seeds(target.corpus).is_empty(),
            "the target {} reads fuzz/corpus/{}, which holds no seeds -- a target with an \
             empty corpus runs no cases and would pass in silence. Rebuild it with \
             scripts/make-fuzz-corpus.sh",
            target.name,
            target.corpus,
        );
    }
}

/// The committed seeds are the boundaries a bounds check can be off by
/// one about, and each must still mean what its name says.
///
/// Without this, a change to `geometry_from` -- the folding that keeps
/// most cases near the parent's edges -- could quietly turn every
/// named boundary into some other number, and the corpus would go on
/// looking like a careful list of edge cases while testing none of
/// them.
#[test]
fn the_named_boundaries_still_decode_to_the_boundaries() {
    let cases = [
        ("whole-parent", 0u64, PARENT_LEN, 0u64, PARENT_LEN as usize),
        ("empty-read", 0, PARENT_LEN, 0, 0),
        ("last-byte", 0, PARENT_LEN, PARENT_LEN - 1, 1),
        ("one-past-the-end", 0, PARENT_LEN, PARENT_LEN, 1),
        ("ends-exactly-at-end", 16, 64, 0, 64),
        ("one-past-slice-end", 16, 64, 0, 65),
    ];

    let found = seeds("slice");
    assert!(
        found.len() >= 12,
        "only {} slice seeds; the corpus has shrunk",
        found.len()
    );

    for (name, start, length, offset, read_len) in cases {
        let (_, bytes) = found
            .iter()
            .find(|(n, _)| n == &format!("{name}.bin"))
            .unwrap_or_else(|| panic!("the corpus has no seed called {name}.bin"));
        let geom = geometry_from(bytes);
        assert_eq!(
            (geom.start, geom.length, geom.offset, geom.read_len),
            (start, length, offset, read_len),
            "the seed {name}.bin no longer decodes to the boundary it is named for"
        );
    }
}

/// The parent is only useful if its bytes differ by position. A
/// constant parent would make "read from the wrong offset" and "read
/// from the right one" indistinguishable, and every slice property here
/// would pass on a broken slice.
#[test]
fn the_parent_pattern_distinguishes_every_offset() {
    let parent = Parent::new();
    assert_eq!(parent.0.len() as u64, PARENT_LEN);

    // Not a claim that every byte is unique -- there are 4096 of them
    // and 256 values -- but that no long run repeats, which is what a
    // misread of a plausible length would need in order to go unnoticed.
    for window in 1..=64u64 {
        let mut same = 0;
        for at in 0..PARENT_LEN - window {
            if parent_byte(at) == parent_byte(at + window) {
                same += 1;
            }
        }
        let ratio = same as f64 / (PARENT_LEN - window) as f64;
        assert!(
            ratio < 0.10,
            "at a shift of {window} bytes, {:.1}% of the parent matches itself, so a read \
             misplaced by that much could return the right bytes by accident",
            ratio * 100.0
        );
    }
}

#[test]
fn deterministic_mutations_of_the_boundary_seeds_are_survived() {
    let cases = Arc::new(AtomicUsize::new(0));
    let current = Arc::new(Mutex::new(String::from("(not started)")));
    let (done_tx, done_rx) = mpsc::channel();

    let hook_current = Arc::clone(&current);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("\nfuzz gate: panicked at {}", describe(&hook_current));
        previous_hook(info);
    }));

    let worker_cases = Arc::clone(&cases);
    let worker_current = Arc::clone(&current);
    let worker = std::thread::spawn(move || {
        for target in targets() {
            for (seed_name, bytes) in seeds(target.corpus) {
                for start in 0..SEEDS {
                    let mut rng = Rng::new(start);
                    for case in 0..target.cases {
                        *worker_current.lock().expect("progress lock") =
                            format!("{} / {seed_name} / seed {start} / case {case}", target.name);
                        let mutated = mutate(&bytes, &mut rng);
                        (target.run)(&mutated);
                        worker_cases.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        let _ = done_tx.send(());
    });

    // A timeout means the worker is still running: a hang. A disconnect
    // means it panicked, and the panic is what is worth reporting.
    match done_rx.recv_timeout(DEADLINE) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Written to the process's stderr rather than through
            // `eprintln!`, which the harness captures into a buffer it
            // only prints when a test finishes -- and exiting here means
            // it never finishes.
            let _ = writeln!(
                std::io::stderr(),
                "\nhung: no progress for {:?} at {}\n\
                 A read did not return. A bounds check that clamps to a length it then \
                 loops on looks exactly like this.",
                DEADLINE,
                describe(&current),
            );
            let _ = std::io::stderr().flush();
            std::process::exit(1);
        }
    }

    let outcome = worker.join();
    let _ = std::panic::take_hook();
    if outcome.is_err() {
        panic!("a decoder panicked at {}", describe(&current));
    }

    let total = cases.load(Ordering::Relaxed);
    assert!(
        total >= CASE_FLOOR,
        "only {total} mutated cases ran, below the floor of {CASE_FLOOR} -- the target \
         list or the corpus has collapsed, and a suite that runs nothing passes quickly",
    );
    eprintln!("{total} mutated cases");
}

#[test]
fn the_gate_covers_every_explorer_target() {
    // The two tiers drift apart the moment somebody adds a cargo-fuzz
    // target and forgets that nothing gates it on the stable toolchain.
    let manifest =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz/Cargo.toml"))
            .expect("reading fuzz/Cargo.toml");

    let explorer: Vec<String> = manifest
        .lines()
        .filter_map(|line| line.strip_prefix("name = \""))
        .filter_map(|rest| rest.strip_suffix('"'))
        .map(str::to_owned)
        .skip(1) // the package name is the first `name =` in the file
        .collect();

    assert!(
        !explorer.is_empty(),
        "fuzz/Cargo.toml declares no [[bin]] targets",
    );

    let gated: Vec<&str> = targets().iter().map(|t| t.name).collect();
    for name in &explorer {
        assert!(
            gated.contains(&name.as_str()),
            "fuzz/fuzz_targets/{name}.rs has no counterpart in this suite, so nothing \
             replays its corpus on the stable toolchain and anything it finds would only \
             stay fixed for as long as somebody keeps running the fuzzer by hand",
        );
    }
}

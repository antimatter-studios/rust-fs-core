//! An export's memory does not grow with the file it copies.
//!
//! Measured, not inferred from reading the loop: this binary's global
//! allocator counts every live byte and keeps the peak, and a 256 MiB
//! generated file is exported to a local file with read-back verification.
//! The peak above what was
//! live before the copy has to stay under 4 MiB — room for the one chunk
//! buffer and the file handles, and nowhere near the file.
//!
//! It is the only test in this binary, so no other test's allocations land
//! in the measurement.

#![cfg(feature = "cli")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use fs_core::cli::transfer::{self, Destination, FileSource, CHUNK};
use fs_core::cli::CliError;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(live, Ordering::SeqCst);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(layout);
        if !p.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(live, Ordering::SeqCst);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        System.dealloc(p, layout);
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// A file of `size` bytes made up as it is read, so the test itself holds
/// none of it.
struct Generated {
    size: u64,
    largest_read: usize,
}

impl FileSource for Generated {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        self.largest_read = self.largest_read.max(buf.len());
        let n = buf.len().min((self.size - offset) as usize);
        for (i, b) in buf[..n].iter_mut().enumerate() {
            *b = ((offset + i as u64).wrapping_mul(0x9e37_79b9) >> 11) as u8;
        }
        Ok(n)
    }
}

#[test]
fn exporting_256_mib_holds_under_4_mib_of_heap() {
    const SIZE: u64 = 256 << 20;
    const BOUND: usize = 4 << 20;

    let dir = std::env::temp_dir().join(format!(
        "rust-fs-core-transfer-bounded-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("big");

    let mut source = Generated {
        size: SIZE,
        largest_read: 0,
    };
    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let copied = transfer::export_file_with_options(
        &mut source,
        Destination::File {
            path: &dest,
            replace: false,
        },
        transfer::ExportOptions { verify: true },
    )
    .unwrap_or_else(|e| panic!("export: {}", e.message));
    let grew = PEAK.load(Ordering::SeqCst) - before;

    assert_eq!(copied.bytes, SIZE);
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), SIZE);
    assert!(source.largest_read <= CHUNK);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        grew < BOUND,
        "exporting {SIZE} bytes grew the heap by {grew} bytes (bound {BOUND})"
    );
}

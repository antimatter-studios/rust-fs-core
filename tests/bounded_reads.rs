//! The bounded-memory file helpers (`tests/support/bounded.rs`, #450).

#[path = "support/bounded.rs"]
mod bounded;

use bounded::{Difference, SMALL_FILE_LIMIT};
use std::path::PathBuf;

const MIB: usize = 1 << 20;

/// A scratch file holding `bytes`, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, bytes: &[u8]) -> Scratch {
        let path = std::env::temp_dir().join(format!("core-bounded-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn the_first_differing_byte_is_named_beyond_the_first_chunk() {
    let left = vec![0u8; 3 * MIB];
    let mut right = left.clone();
    assert_eq!(
        bounded::first_difference(left.as_slice(), right.as_slice()).unwrap(),
        None
    );
    right[2 * MIB + 7] = 9;
    right[2 * MIB + 9] = 4;
    assert_eq!(
        bounded::first_difference(left.as_slice(), right.as_slice()).unwrap(),
        Some(Difference {
            offset: 2 * MIB as u64 + 7,
            left: Some(0),
            right: Some(9)
        })
    );
}

#[test]
fn a_shorter_side_differs_where_it_ends() {
    for len in [0, 10, MIB, MIB + 1] {
        let short = vec![5u8; len];
        let long = vec![5u8; len + 1];
        assert_eq!(
            bounded::first_difference(short.as_slice(), long.as_slice()).unwrap(),
            Some(Difference {
                offset: len as u64,
                left: None,
                right: Some(5)
            }),
            "{len} bytes"
        );
    }
}

#[test]
#[should_panic(expected = "offset: 4")]
fn a_file_that_differs_from_the_expected_bytes_fails_naming_the_offset() {
    let file = Scratch::new("holds", b"abcdXf");
    bounded::assert_holds(&file.0, b"abcdef");
}

#[test]
fn a_file_over_the_limit_is_not_read_whole() {
    let limit = SMALL_FILE_LIMIT as usize;
    let small = Scratch::new("small", &vec![1; limit]);
    assert_eq!(bounded::small_file(&small.0).unwrap().len(), limit);
    let large = Scratch::new("large", &vec![1; limit + 1]);
    assert!(bounded::small_file(&large.0).is_err());
    bounded::assert_holds(&large.0, &vec![1; limit + 1]);
    bounded::assert_same_files(&large.0, &large.0, "one file");
}

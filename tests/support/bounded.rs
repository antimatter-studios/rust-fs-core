//! Reading files in bounded memory (#450). Each test crate that includes this
//! file uses a subset.
//!
//! A test checks a result against what it should be one mebibyte at a time, so
//! its memory does not grow with the file. A whole-file read is only for a file
//! small by construction, and [`small_file`] enforces that limit.
//! `tests/scripts/no-whole-image-reads.sh` refuses any other whole-file read.
#![allow(dead_code)]

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

const CHUNK: usize = 1 << 20;

/// The largest file [`small_file`] reads whole.
pub const SMALL_FILE_LIMIT: u64 = 1 << 20;

/// A file small by construction, read whole: a literal payload, a seed, a
/// marker. A file over [`SMALL_FILE_LIMIT`] is refused.
pub fn small_file(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(SMALL_FILE_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > SMALL_FILE_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the file is over the {SMALL_FILE_LIMIT} byte whole-read limit"),
        ));
    }
    Ok(bytes)
}

/// Where two streams first differ: the offset, and each side's byte there, or
/// `None` for the side that has ended.
#[derive(Debug, PartialEq, Eq)]
pub struct Difference {
    pub offset: u64,
    pub left: Option<u8>,
    pub right: Option<u8>,
}

/// Fill `buf` from `source`, short only at its end.
fn fill(source: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match source.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// The first difference between two streams, holding one chunk of each.
pub fn first_difference(
    mut left: impl Read,
    mut right: impl Read,
) -> io::Result<Option<Difference>> {
    let mut a = vec![0u8; CHUNK];
    let mut b = vec![0u8; CHUNK];
    let mut offset = 0u64;
    loop {
        let n = fill(&mut left, &mut a)?;
        let m = fill(&mut right, &mut b)?;
        if a[..n] != b[..m] {
            let at = (0..n.min(m)).find(|&i| a[i] != b[i]).unwrap_or(n.min(m));
            return Ok(Some(Difference {
                offset: offset + at as u64,
                left: a[..n].get(at).copied(),
                right: b[..m].get(at).copied(),
            }));
        }
        if n == 0 {
            return Ok(None);
        }
        offset += n as u64;
    }
}

fn open(path: &Path) -> File {
    File::open(path).unwrap_or_else(|error| panic!("open {}: {error}", path.display()))
}

/// Fail, naming the first differing byte, unless the file holds exactly
/// `expected`.
#[track_caller]
pub fn assert_holds(path: impl AsRef<Path>, expected: &[u8]) {
    let path = path.as_ref();
    let difference = first_difference(open(path), expected)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    if let Some(difference) = difference {
        panic!(
            "{} does not hold the expected {} bytes: {difference:?} (left is the file)",
            path.display(),
            expected.len()
        );
    }
}

/// Fail, naming the first differing byte, unless two files hold the same bytes.
#[track_caller]
pub fn assert_same_files(left: impl AsRef<Path>, right: impl AsRef<Path>, context: &str) {
    let (left, right) = (left.as_ref(), right.as_ref());
    let difference = first_difference(open(left), open(right)).unwrap_or_else(|error| {
        panic!(
            "compare {} and {}: {error}",
            left.display(),
            right.display()
        )
    });
    if let Some(difference) = difference {
        panic!(
            "{context}: {} and {} differ: {difference:?}",
            left.display(),
            right.display()
        );
    }
}

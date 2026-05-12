//! Display / Debug / source() / From<io::Error> for fs_core::Error.

use fs_core::Error;
use std::error::Error as _;
use std::io;

#[test]
fn display_io_wraps_inner() {
    let inner = io::Error::new(io::ErrorKind::NotFound, "missing");
    let e = Error::Io(inner);
    assert_eq!(format!("{e}"), "io: missing");
}

#[test]
fn display_short_read_includes_fields() {
    let e = Error::ShortRead {
        offset: 100,
        want: 64,
        got: 32,
    };
    assert_eq!(format!("{e}"), "short read at 100: wanted 64 got 32");
}

#[test]
fn display_read_only_message() {
    assert_eq!(format!("{}", Error::ReadOnly), "device is read-only");
}

#[test]
fn display_out_of_bounds_includes_fields() {
    let e = Error::OutOfBounds {
        offset: 4096,
        len: 512,
        size: 4097,
    };
    assert_eq!(format!("{e}"), "4096+512 past device size 4097");
}

#[test]
fn display_custom_is_passthrough() {
    let e = Error::Custom("driver-specific failure".into());
    assert_eq!(format!("{e}"), "driver-specific failure");
}

#[test]
fn debug_formats_all_variants() {
    // We don't pin exact Debug output (it's not contract), but verify each
    // variant produces something non-empty so a logger or assert_eq! diff
    // never panics on an Error value.
    for e in [
        Error::Io(io::Error::other("x")),
        Error::ShortRead {
            offset: 0,
            want: 1,
            got: 0,
        },
        Error::ReadOnly,
        Error::OutOfBounds {
            offset: 0,
            len: 1,
            size: 0,
        },
        Error::Custom("y".into()),
    ] {
        let s = format!("{e:?}");
        assert!(!s.is_empty());
    }
}

#[test]
fn source_is_some_only_for_io() {
    let io_err = Error::Io(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
    assert!(io_err.source().is_some());

    for e in [
        Error::ShortRead {
            offset: 0,
            want: 1,
            got: 0,
        },
        Error::ReadOnly,
        Error::OutOfBounds {
            offset: 0,
            len: 1,
            size: 0,
        },
        Error::Custom("x".into()),
    ] {
        assert!(e.source().is_none(), "expected None for {e:?}");
    }
}

#[test]
fn from_io_error_wraps_to_io_variant() {
    let io_err = io::Error::new(io::ErrorKind::UnexpectedEof, "eof");
    let e: Error = io_err.into();
    match e {
        Error::Io(inner) => assert_eq!(inner.kind(), io::ErrorKind::UnexpectedEof),
        other => panic!("expected Error::Io, got {other:?}"),
    }
}

#[test]
fn question_mark_operator_works_via_from() {
    fn inner() -> fs_core::Result<()> {
        // Manufacture an io::Result and propagate via `?`.
        let r: io::Result<()> = Err(io::Error::other("fail"));
        r?;
        Ok(())
    }
    let e = inner().unwrap_err();
    assert!(matches!(e, Error::Io(_)));
}

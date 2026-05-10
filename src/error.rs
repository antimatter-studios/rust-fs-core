//! Unified error type. Each driver still keeps its own rich error type for
//! internal use; conversions to/from this one happen at the trait boundary.

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    /// Underlying I/O failure (open, seek, read, write).
    Io(io::Error),
    /// Device returned fewer bytes than requested before EOF.
    ShortRead {
        offset: u64,
        want: usize,
        got: usize,
    },
    /// `write_at` invoked on a device opened read-only.
    ReadOnly,
    /// Read or write past the end of the device.
    OutOfBounds { offset: u64, len: u64, size: u64 },
    /// Driver-specific error lifted to the trait boundary. Each driver's
    /// internal error type implements `Into<Error>` via this variant.
    Custom(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::ShortRead { offset, want, got } => {
                write!(f, "short read at {offset}: wanted {want} got {got}")
            }
            Error::ReadOnly => write!(f, "device is read-only"),
            Error::OutOfBounds { offset, len, size } => {
                write!(f, "{offset}+{len} past device size {size}")
            }
            Error::Custom(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

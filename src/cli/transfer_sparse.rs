//! Sparse policy and staging for bounded file copies; hashes cover logical bytes.
use super::*;
use std::io::{Seek, SeekFrom};

/// Opt-in sparse behavior, separate from existing request and option literals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SparseOptions {
    /// Omit all-zero chunks on files and on sinks declaring sparse support.
    /// Streaming writers still receive every byte because pipes have no holes.
    pub preserve_sparse: bool,
}
impl SparseOptions {
    /// Parse the shared comma-delimited, repeatable `--preserve` argument.
    /// Older argument sets without preservation default to dense copies.
    /// Pass this policy to the sparse file-copy entry points to apply it.
    pub fn from_matches(m: &ArgMatches) -> Result<Self, CliError> {
        Ok(Self {
            preserve_sparse: m
                .try_get_many::<String>("preserve")
                .ok()
                .flatten()
                .is_some_and(|mut values| values.any(|v| v == "sparse")),
        })
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct CopyOptions {
    pub verify: bool,
    pub sparse: SparseOptions,
}
impl From<ExportOptions> for CopyOptions {
    fn from(options: ExportOptions) -> Self {
        Self {
            verify: options.verify,
            ..Self::default()
        }
    }
}
impl From<ImportOptions> for CopyOptions {
    fn from(options: ImportOptions) -> Self {
        Self {
            verify: options.verify,
            ..Self::default()
        }
    }
}

pub(super) struct SparseFile<'a>(pub &'a mut File);
impl Write for SparseFile<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.iter().all(|b| *b == 0) {
            let len = i64::try_from(bytes.len()).map_err(io::Error::other)?;
            self.0.seek(SeekFrom::Current(len))?;
            Ok(bytes.len())
        } else {
            self.0.write(bytes)
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

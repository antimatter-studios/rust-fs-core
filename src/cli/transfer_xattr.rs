//! Optional host extended-attribute preservation; no implicit side-file layout.
use super::*;

/// One opaque extended attribute. Names retain the source's namespace spelling.
/// Named streams are a separate capability and must not be encoded as xattrs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedAttribute {
    /// Host attribute name, without a terminating NUL.
    pub name: OsString,
    /// Opaque attribute value, including empty values and embedded NUL bytes.
    pub value: Vec<u8>,
}

/// One opaque named stream, distinct from the file's unnamed data and xattrs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedStream {
    /// Stream name in the source namespace, never a relative side-file path.
    pub name: OsString,
    /// Opaque stream contents, including empty values and embedded NUL bytes.
    pub value: Vec<u8>,
}

/// Optional named-stream destination capability, independent of file staging.
/// The destination file or directory must already exist. Implementations must
/// preserve the name and bytes or refuse, never encode streams as side files
/// or ordinary extended attributes. Unsupported destinations use the default.
pub trait NamedStreamSink {
    /// Set one named stream on a published item without following symbolic links.
    fn set_named_stream(&mut self, _path: &Path, _stream: &NamedStream) -> Result<(), CliError> {
        Err(CliError::refused(
            "named streams not supported by this sink",
        ))
    }
}

/// Copy named streams only when `--preserve=xattrs` is enabled. Neither optional
/// capability is queried otherwise. Errors name the affected stream and retain
/// the sink's refusal/failure classification. Earlier stream writes remain.
/// Host tree exports use an explicitly incapable sink; host stream import is a
/// separate platform capability and no side-file layout is implied here.
pub fn preserve_named_streams(
    source: &mut dyn TreeSource,
    path: &Path,
    sink: &mut dyn NamedStreamSink,
    destination: &Path,
    options: XattrOptions,
) -> Result<(), CliError> {
    if options.preserve_xattrs {
        for stream in source.named_streams(path)? {
            sink.set_named_stream(destination, &stream)
                .map_err(|mut error| {
                    error.message = format!("named stream {:?}: {}", stream.name, error.message);
                    error
                })?;
        }
    }
    Ok(())
}

// Host streams remain unsupported until a platform has a native stream adapter.
// Refusing here never interprets a stream name as a host path or an xattr name.
pub(super) struct HostStreams;
impl NamedStreamSink for HostStreams {
    fn set_named_stream(&mut self, _path: &Path, _stream: &NamedStream) -> Result<(), CliError> {
        Err(CliError::refused(
            "named streams not supported by this host",
        ))
    }
}

/// Opt-in attribute policy, separate from existing option literals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XattrOptions {
    /// Preserve extended attributes and named streams through native capabilities.
    pub preserve_xattrs: bool,
}
impl XattrOptions {
    /// Parse the existing comma-delimited, repeatable preservation argument.
    /// Older argument sets without preservation default to no attribute calls.
    pub fn from_matches(m: &ArgMatches) -> Result<Self, CliError> {
        Ok(Self {
            preserve_xattrs: m
                .try_get_many::<String>("preserve")
                .ok()
                .flatten()
                .is_some_and(|mut values| values.any(|v| v == "xattrs")),
        })
    }
}

/// Tree export arguments adding `xattrs` to the existing preservation policies.
/// Pass [`XattrOptions::from_matches`] to [`export_tree_with_xattrs`].
pub fn export_args_with_xattrs() -> [Arg; 7] {
    let [path, dest, replace, verify, recursive, depth, preserve] = export_args_with_times();
    [path, dest, replace, verify, recursive, depth,
        preserve.value_parser(["links", "times", "sparse", "xattrs"])
            .help("Preserve times, links, sparse holes and/or extended attributes and named streams; unsupported metadata is reported per item")]
}

/// Export with opt-in extended attributes and named streams.
/// Attribute errors refuse the item after its contents have been published;
/// already-written attributes and contents remain, and siblings continue.
/// Host calls never follow symbolic links or manufacture metadata side files.
/// Named streams are refused by name until a native host adapter is available.
pub fn export_tree_with_xattrs(
    request: &ExportRequest,
    source: &mut dyn TreeSource,
    options: TreeOptions,
    links: LinkOptions,
    times: TimeOptions,
    xattrs: XattrOptions,
) -> Result<Outcome, CliError> {
    let mut stderr = io::stderr();
    let terminal = stderr.is_terminal();
    let mut progress = Progress::new(&mut stderr, terminal, ProgressOptions::default());
    tree::export_tree_with_xattrs_and_progress(
        request,
        source,
        options,
        links,
        times,
        xattrs,
        &mut progress,
    )
}

pub(super) fn preserve(local: &Path, attributes: Vec<ExtendedAttribute>) -> Result<(), CliError> {
    for attribute in attributes {
        let refused = |e: String| {
            CliError::refused(format!(
                "extended attribute {:?} refused: {e}",
                attribute.name
            ))
        };
        let actual = set_host_attribute(local, &attribute).map_err(|e| refused(e.to_string()))?;
        if actual.as_deref() != Some(attribute.value.as_slice()) {
            return Err(refused("host read-back differs from source".into()));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_host_attribute(local: &Path, attribute: &ExtendedAttribute) -> io::Result<Option<Vec<u8>>> {
    xattr::set(local, &attribute.name, &attribute.value)?;
    xattr::get(local, &attribute.name)
}

#[cfg(not(unix))]
fn set_host_attribute(
    _local: &Path,
    _attribute: &ExtendedAttribute,
) -> io::Result<Option<Vec<u8>>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "host extended attributes are unsupported",
    ))
}

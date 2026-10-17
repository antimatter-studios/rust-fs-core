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

/// Opt-in attribute policy, separate from existing option literals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XattrOptions {
    /// Read source attributes and write them with the host's no-follow calls.
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
            .help("Preserve times, links, sparse holes and/or extended attributes; unsupported metadata is reported per item")]
}

/// Export with opt-in extended attributes, leaving existing entry points intact.
/// Attribute errors refuse the item after its contents have been published;
/// already-written attributes and contents remain, and siblings continue.
/// Host calls never follow symbolic links or manufacture metadata side files.
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

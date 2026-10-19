//! Local tree imports; the driver owns name equivalence and directory creation.
use super::*;
use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

/// Additive directory capability over the existing single-file staging API.
pub trait TreeSink: FileSink {
    /// Validate encoding, length and reserved names without mutating the image.
    /// Refuse differently spelled existing aliases under the format's own case
    /// rules, returning `CliError::refused`. Return a comparison key for sibling
    /// collision detection; this key is never used as the destination spelling.
    /// Exact existing names are handled by `create_dir` or file staging.
    fn validate_name(&mut self, parent: &Path, name: &OsStr) -> Result<OsString, CliError>;
    /// Create a directory, or merge an exact existing directory and return true.
    /// Refuse files, symbolic links and aliases; never replace them to make room.
    fn create_dir(&mut self, path: &Path) -> Result<bool, CliError>;
    /// The destination format named when an optional capability is refused.
    fn format_name(&self) -> &str {
        "this sink"
    }
    /// Preserve a symbolic link without following its opaque target. Drivers
    /// lacking this capability refuse it even when preservation is requested.
    /// Never overwrite an existing name; creation must be atomic and exclusive.
    fn create_symbolic_link(&mut self, _path: &Path, _target: &Path) -> Result<(), CliError> {
        Err(unsupported(self.format_name(), "symbolic links"))
    }
    /// Add `path` as another name for the already published file `existing`.
    /// Never overwrite an existing name; drivers lacking hard links refuse.
    fn create_hard_link(&mut self, _path: &Path, _existing: &Path) -> Result<(), CliError> {
        Err(unsupported(self.format_name(), "hard links"))
    }
    /// Set the access and modification times of a published item; `None`
    /// leaves that time unchanged. Drivers lacking settable times refuse.
    fn set_times(
        &mut self,
        _path: &Path,
        _accessed: Option<SystemTime>,
        _modified: Option<SystemTime>,
    ) -> Result<(), CliError> {
        Err(unsupported(self.format_name(), "times"))
    }
    /// Set one opaque extended attribute on a published item without following
    /// symbolic links. Preserve the name and bytes exactly or refuse; never
    /// rename, truncate or drop it. Drivers lacking attributes refuse.
    fn set_extended_attribute(
        &mut self,
        _path: &Path,
        _attribute: &ExtendedAttribute,
    ) -> Result<(), CliError> {
        Err(unsupported(self.format_name(), "extended attributes"))
    }
}

fn unsupported(format: &str, capability: &str) -> CliError {
    CliError::refused(format!("{capability} not supported by {format}"))
}

/// Recursive import options kept separate from existing request literals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportTreeOptions {
    /// Permit local directory sources.
    pub recursive: bool,
    /// Maximum depth below the root, which is depth zero.
    pub max_depth: usize,
    /// Recreate symbolic links rather than follow their targets, and repeat
    /// names of one source file as hard links rather than copies.
    pub preserve_links: bool,
    /// Set access and modification times, directories after their children.
    pub preserve_times: bool,
}
impl Default for ImportTreeOptions {
    fn default() -> Self {
        Self {
            recursive: false,
            max_depth: 64,
            preserve_links: false,
            preserve_times: false,
        }
    }
}
impl ImportTreeOptions {
    /// Parse the additive tree arguments.
    pub fn from_matches(m: &ArgMatches) -> Result<Self, CliError> {
        Ok(Self {
            recursive: m.get_flag("recursive"),
            max_depth: *m.get_one::<usize>("max-depth").expect("clap default"),
            preserve_links: LinkOptions::from_matches(m)?.preserve_links,
            preserve_times: TimeOptions::from_matches(m)?.preserve_times,
        })
    }
}
/// Import arguments with recursion, depth bounds, and link and time preservation.
pub fn import_args_with_tree() -> [Arg; 6] {
    let [source, path, replace] = import_args();
    [
        source,
        path,
        replace,
        Arg::new("recursive")
            .long("recursive")
            .action(ArgAction::SetTrue)
            .help("Import a local directory tree; merge existing directories"),
        Arg::new("max-depth")
            .long("max-depth")
            .default_value("64")
            .value_parser(clap::value_parser!(usize))
            .help("Maximum entry depth below SOURCE (root is zero)"),
        Arg::new("preserve")
            .long("preserve")
            .value_parser(["links", "times", "sparse"])
            .value_delimiter(',')
            .action(ArgAction::Append)
            .help("Preserve links, times and/or sparse holes; a sink lacking one refuses each affected item"),
    ]
}

/// Import a local file or directory, reporting each visited item like tree
/// export. Names are checked before creation; the first sibling spelling wins.
/// Existing directories merge. Refused/failed entries leave siblings running
/// and make the outcome nonzero. Symbolic links are never followed. Each file
/// uses the single-file bounded transaction and source-identity checks; this is
/// not a snapshot of the tree. Callers must exclude concurrent tree mutation.
/// The sink holds one exclusive backing-image lock for the entire traversal.
/// Preserved times and links use optional sink capabilities; a sink lacking
/// one refuses each affected item by name rather than dropping the metadata.
/// Directory times are set after the traversal, deepest first.
pub fn import_tree(
    request: &ImportRequest,
    sink: Option<&mut dyn TreeSink>,
    options: ImportTreeOptions,
) -> Result<Outcome, CliError> {
    import_tree_recorded(
        request,
        sink,
        options,
        ImportOptions::default().into(),
        None,
    )
}

/// Import a tree with sparse regular files alongside the existing links/times
/// policies. A sink lacking sparse support refuses each file by destination
/// format before staging; siblings continue and no refused file is published.
pub fn import_tree_with_sparse(
    request: &ImportRequest,
    sink: Option<&mut dyn TreeSink>,
    options: ImportTreeOptions,
    sparse: SparseOptions,
) -> Result<Outcome, CliError> {
    import_tree_recorded(
        request,
        sink,
        options,
        CopyOptions {
            sparse,
            ..CopyOptions::default()
        },
        None,
    )
}

pub(super) fn import_tree_recorded(
    request: &ImportRequest,
    sink: Option<&mut dyn TreeSink>,
    options: ImportTreeOptions,
    copy_options: CopyOptions,
    mut ledger: Option<&mut manifest::ImportLedger>,
) -> Result<Outcome, CliError> {
    let sink = sink.ok_or_else(|| CliError::refused("this driver has no tree import sink"))?;
    if request.replace && !sink.supports_atomic_replace() {
        return Err(CliError::refused(
            "this sink cannot replace a name atomically",
        ));
    }
    let Input::File(root) = &request.input else {
        return Err(CliError::usage("tree import needs a local SOURCE"));
    };
    let metadata = fs::symlink_metadata(root).map_err(|e| CliError::failed(e.to_string()))?;
    if metadata.is_dir() && !options.recursive {
        return Err(CliError::usage("directory source requires --recursive"));
    }
    let image = sink.image_path().map(Path::to_path_buf);
    let _lock = image.as_deref().map(ImageLock::for_import).transpose()?;
    let cancellation = Cancellation::install()?;
    let mut pending = vec![(root.clone(), request.path.clone(), 0usize, Ok(()))];
    let mut items = Vec::new();
    let mut counts = [0u64; 5];
    let mut verified_skips = 0u64;
    let mut merged = 0u64;
    let mut cancel_code = 0;
    let mut in_flight = Json::Null;
    let mut linked: HashMap<(u64, u64), PathBuf> = HashMap::new();
    let mut directory_times = Vec::new();
    while let Some((local, dest, depth, checked)) = pending.pop() {
        let mut copy = Json::Null;
        let mut skipped = None;
        let result = (|| -> Result<(), CliError> {
            cancellation.check(0, 0)?;
            checked?;
            if depth > options.max_depth {
                return Err(CliError::failed("maximum depth exceeded"));
            }
            if let Some(image) = &image {
                refuse_image_alias(image, &local)?;
            }
            if depth == 0 {
                if let Some(name) = dest.file_name() {
                    sink.validate_name(dest.parent().unwrap_or(Path::new("")), name)?;
                }
            }
            let m = fs::symlink_metadata(&local).map_err(|e| CliError::failed(e.to_string()))?;
            if m.file_type().is_symlink() {
                if !options.preserve_links {
                    return Err(CliError::refused("symbolic link requires --preserve=links"));
                }
                let target = fs::read_link(&local).map_err(|e| CliError::failed(e.to_string()))?;
                sink.create_symbolic_link(&dest, &target)?;
                xattrs::import_attributes(sink, &local, &dest, copy_options.xattrs)?;
                return set_times(sink, &dest, &m, options);
            }
            if m.is_file() && copy_options.sparse.preserve_sparse && !sink.supports_sparse() {
                return Err(unsupported(sink.format_name(), "sparse files"));
            }
            let inode = file_identity(&m).filter(|_| options.preserve_links && m.is_file());
            if let Some(existing) = inode.and_then(|id| linked.get(&id)) {
                return sink.create_hard_link(&dest, existing);
            }
            if m.is_file() {
                // Revalidated evidence of an earlier verified copy skips this file.
                if let Some(ledger) = ledger.as_deref() {
                    let file =
                        fs::File::open(&local).map_err(|e| CliError::failed(e.to_string()))?;
                    if let Some(entry) = ledger.resume(&file, &local, sink, &dest)? {
                        let outcome = manifest::import_skipped(
                            &ImportRequest {
                                input: Input::File(local.clone()),
                                path: dest.clone(),
                                replace: request.replace,
                            },
                            entry,
                        );
                        copy = outcome.report.expect("earlier verification has evidence");
                        skipped = outcome.text;
                        verified_skips += 1;
                        if let Some(id) = inode {
                            linked.insert(id, dest.clone());
                        }
                        return Ok(());
                    }
                }
                let copied = import::import_tree_file(
                    &local,
                    sink,
                    &dest,
                    request.replace,
                    &cancellation,
                    copy_options,
                    ledger.as_deref_mut(),
                )?;
                copy = Json::object([
                    ("bytes", Json::from(copied.bytes)),
                    ("sha256", Json::from(copied.sha256_hex())),
                    ("created", Json::from(!copied.replaced)),
                    ("replaced", Json::from(copied.replaced)),
                ]);
                if let Some(id) = inode {
                    linked.insert(id, dest.clone());
                }
                xattrs::import_attributes(sink, &local, &dest, copy_options.xattrs)?;
                set_times(sink, &dest, &m, options)?;
            } else if m.is_dir() {
                if sink.create_dir(&dest)? {
                    merged += 1;
                }
                if options.preserve_times {
                    directory_times.push((items.len(), m.clone()));
                }
                let entries = fs::read_dir(&local)
                    .map_err(|e| CliError::failed(e.to_string()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| CliError::failed(e.to_string()))?;
                let mut entries = entries;
                entries.sort_by_key(|e| e.file_name());
                let mut names = HashSet::new();
                let mut children = Vec::new();
                for entry in entries {
                    cancellation.check(0, 0)?;
                    let name = entry.file_name();
                    let checked = sink.validate_name(&dest, &name).and_then(|key| {
                        if names.insert(key) {
                            Ok(())
                        } else {
                            Err(CliError::refused(
                                "source names collide under sink case rules",
                            ))
                        }
                    });
                    children.push((
                        entry.path(),
                        dest.join(name),
                        depth.saturating_add(1),
                        checked,
                    ));
                }
                pending.extend(children.into_iter().rev());
                // Queued children still run when the directory's attributes refuse.
                xattrs::import_attributes(sink, &local, &dest, copy_options.xattrs)?;
            } else {
                return Err(CliError::refused("unsupported entry type"));
            }
            Ok(())
        })();
        // A ledger error aborts traversal: later copies cannot outrun evidence.
        if ledger.as_ref().is_some_and(|ledger| ledger.failed) {
            return Err(result.expect_err("a failed capture or append stops the transfer"));
        }
        let (status, reason) = match result {
            Ok(()) => match skipped {
                Some(text) => (1, text),
                None => (0, String::new()),
            },
            Err(e) => {
                let status = if cancel::is_cancelled(&e) {
                    cancel_code = e.code;
                    4
                } else if e.code == super::super::output::EXIT_UNSUPPORTED {
                    2
                } else {
                    3
                };
                (status, e.message)
            }
        };
        counts[status] += 1;
        items.push(Json::object([
            ("path", Json::from(local.display().to_string())),
            ("destination", Json::from(dest.display().to_string())),
            (
                "status",
                Json::from(["completed", "skipped", "refused", "failed", "cancelled"][status]),
            ),
            ("reason", Json::from(reason)),
            ("copy", copy),
        ]));
        if cancel_code != 0 {
            in_flight = items.last().unwrap().clone();
            break;
        }
    }
    for (index, m) in directory_times.into_iter().rev() {
        if cancel_code != 0 {
            break;
        }
        let Some(Json::Obj(fields)) = items.get_mut(index) else {
            continue;
        };
        let field = |fields: &[(String, Json)], key: &str| {
            fields
                .iter()
                .position(|(k, _)| k == key)
                .expect("item field")
        };
        if !matches!(&fields[field(fields, "status")].1, Json::Str(s) if s == "completed") {
            continue;
        }
        let Json::Str(dest) = &fields[field(fields, "destination")].1 else {
            continue;
        };
        if let Err(e) = set_times(sink, Path::new(dest), &m, options) {
            let status = if e.code == super::super::output::EXIT_UNSUPPORTED {
                2
            } else {
                3
            };
            counts[0] -= 1;
            counts[status] += 1;
            let at = field(fields, "status");
            fields[at].1 = Json::from(["refused", "failed"][status - 2]);
            let at = field(fields, "reason");
            fields[at].1 = Json::from(e.message);
        }
    }
    Ok(Outcome::report(Json::object([
        ("completed", Json::from(counts[0])),
        ("skipped", Json::from(counts[1])),
        ("refused", Json::from(counts[2])),
        ("failed", Json::from(counts[3])),
        ("merged_directories", Json::from(merged)),
        ("cancelled", Json::from(cancel_code != 0)),
        (
            "signal",
            if cancel_code == 0 {
                Json::Null
            } else {
                Json::from(u64::from(cancel_code - 128))
            },
        ),
        ("in_flight", in_flight),
        (
            "pending",
            Json::Arr(
                pending
                    .into_iter()
                    .rev()
                    .map(|(local, dest, _, _)| {
                        Json::object([
                            ("path", Json::from(local.display().to_string())),
                            ("destination", Json::from(dest.display().to_string())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("items", Json::Arr(items)),
        ("limitations", Json::Arr(if cfg!(unix) { Vec::new() } else {
            vec![Json::from("source change time is unavailable; same-size rewrites with restored modification time may be undetectable")]
        })),
    ]))
    .with_code(if cancel_code != 0 {
        cancel_code
    } else {
        u8::from(counts[1] > verified_skips || counts[2..].iter().any(|n| *n != 0))
    }))
}

/// Apply the source's access and modification times when they are preserved.
fn set_times(
    sink: &mut dyn TreeSink,
    dest: &Path,
    metadata: &fs::Metadata,
    options: ImportTreeOptions,
) -> Result<(), CliError> {
    if !options.preserve_times {
        return Ok(());
    }
    sink.set_times(dest, metadata.accessed().ok(), metadata.modified().ok())
}

/// The host identity shared by every hard link to one file, where the host
/// exposes it; elsewhere each name is imported as an independent copy.
fn file_identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.nlink() > 1).then(|| (metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

//! Local tree imports; the driver owns name equivalence and directory creation.
use super::*;
use std::collections::HashSet;

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
    /// Preserve a symbolic link without following its opaque target. Drivers
    /// lacking this capability refuse it even when preservation is requested.
    /// Never overwrite an existing name; creation must be atomic and exclusive.
    fn create_symbolic_link(&mut self, _path: &Path, _target: &Path) -> Result<(), CliError> {
        Err(CliError::refused(
            "this sink cannot preserve symbolic links",
        ))
    }
}

/// Recursive import options kept separate from existing request literals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportTreeOptions {
    /// Permit local directory sources.
    pub recursive: bool,
    /// Maximum depth below the root, which is depth zero.
    pub max_depth: usize,
    /// Recreate symbolic links rather than follow their targets.
    pub preserve_links: bool,
}
impl Default for ImportTreeOptions {
    fn default() -> Self {
        Self {
            recursive: false,
            max_depth: 64,
            preserve_links: false,
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
        })
    }
}
/// Import arguments with recursion, depth bounds and symbolic link preservation.
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
            .value_parser(["links"])
            .help("Preserve symbolic links without following their targets"),
    ]
}

/// Import a local file or directory, reporting each visited item like tree
/// export. Names are checked before creation; the first sibling spelling wins.
/// Existing directories merge. Refused/failed entries leave siblings running
/// and make the outcome nonzero. Symbolic links are never followed. Each file
/// uses the single-file bounded transaction and source-identity checks; this is
/// not a snapshot of the tree. Callers must exclude concurrent tree mutation.
/// The sink holds one exclusive backing-image lock for the entire traversal.
pub fn import_tree(
    request: &ImportRequest,
    sink: Option<&mut dyn TreeSink>,
    options: ImportTreeOptions,
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
    let mut merged = 0u64;
    let mut cancel_code = 0;
    let mut in_flight = Json::Null;
    while let Some((local, dest, depth, checked)) = pending.pop() {
        let mut copy = Json::Null;
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
                return sink.create_symbolic_link(&dest, &target);
            }
            if m.is_file() {
                let copied =
                    import::import_tree_file(&local, sink, &dest, request.replace, &cancellation)?;
                copy = Json::object([
                    ("bytes", Json::from(copied.bytes)),
                    ("sha256", Json::from(copied.sha256_hex())),
                    ("created", Json::from(!copied.replaced)),
                    ("replaced", Json::from(copied.replaced)),
                ]);
            } else if m.is_dir() {
                if sink.create_dir(&dest)? {
                    merged += 1;
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
            } else {
                return Err(CliError::refused("unsupported entry type"));
            }
            Ok(())
        })();
        let (status, reason) = match result {
            Ok(()) => (0, String::new()),
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
        u8::from(counts[1..].iter().any(|n| *n != 0))
    }))
}

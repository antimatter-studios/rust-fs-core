//! Local tree exports. Names and existing directory components are checked
//! before joining paths; callers must exclude concurrent destination mutation.
use super::*;
use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

/// A source entry's type. Symbolic links use `Other` and are never followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symlink or another unsupported type.
    Other,
}
/// Metadata supplied by a driver; times are exposed without promising preservation.
#[derive(Clone, Debug)]
pub struct TreeMetadata {
    /// Entry type.
    pub kind: TreeKind,
    /// File length in bytes.
    pub size: u64,
    /// Stable identity within this source: equal for hard links, unique otherwise.
    /// Used to preserve file links and refuse repeated directory visits.
    pub inode: u64,
    /// Last access time, when available.
    pub accessed: Option<SystemTime>,
    /// Last modification time, when available.
    pub modified: Option<SystemTime>,
}
/// One directory child; its name must be a single safe path component.
#[derive(Clone, Debug)]
pub struct TreeEntry {
    /// The child's name, checked before use as a path.
    pub name: OsString,
    /// Metadata for the child.
    pub metadata: TreeMetadata,
}
/// A driver's directory listing, metadata and positioned file reads.
pub trait TreeSource {
    /// Backing image identity; supplying it enables alias refusal and image locks.
    fn image_path(&self) -> Option<&Path> {
        None
    }
    /// For an `Other` entry, return its symbolic target without following it.
    /// The default leaves `Other` unsupported, preserving existing drivers.
    fn symbolic_link(&mut self, _path: &Path) -> Result<Option<SymbolicLink>, CliError> {
        Ok(None)
    }
    /// Metadata without following links, including for the selected root.
    /// Symbolic links must be `Other`; `symbolic_link` supplies their targets.
    fn metadata(&mut self, path: &Path) -> Result<TreeMetadata, CliError>;
    /// Children with metadata that does not follow links. Listing failure is
    /// reported for the directory.
    fn read_dir(&mut self, path: &Path) -> Result<Vec<TreeEntry>, CliError>;
    /// Read a regular file at an offset, with the same contract as FileSource.
    fn read_file_at(&mut self, path: &Path, offset: u64, buf: &mut [u8])
        -> Result<usize, CliError>;
}
/// Opt-in tree behavior; the existing single-file API is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeOptions {
    /// Permit a directory source.
    pub recursive: bool,
    /// Maximum depth below the source root (root is depth zero).
    pub max_depth: usize,
    /// Verify each staged file before publishing.
    pub verify: bool,
}
impl Default for TreeOptions {
    fn default() -> Self {
        Self {
            recursive: false,
            max_depth: 64,
            verify: false,
        }
    }
}
impl TreeOptions {
    /// Parse arguments supplied by [`export_args_with_tree`].
    pub fn from_matches(m: &ArgMatches) -> Result<Self, CliError> {
        Ok(Self {
            recursive: m.get_flag("recursive"),
            max_depth: *m
                .get_one::<usize>("max-depth")
                .expect("clap supplies the default"),
            verify: ExportOptions::from_matches(m)?.verify,
        })
    }
}
/// Export arguments including verification, recursion and a depth bound.
pub fn export_args_with_tree() -> [Arg; 6] {
    let [path, dest, replace, verify] = export_args_with_verify();
    [path, dest, replace.help("Replace existing files; existing directories are merged into, preserving unrelated entries"), verify,
        Arg::new("recursive").long("recursive").action(ArgAction::SetTrue).help("Copy a directory tree; existing destination directories are merged into"),
        Arg::new("max-depth").long("max-depth").default_value("64").value_parser(clap::value_parser!(usize)).help("Maximum entry depth below the source root (root is zero)")]
}
/// A symbolic link's opaque target and Windows link type; never resolve it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolicLink {
    /// Target as stored by the source, including non-Unicode names.
    pub target: PathBuf,
    /// Windows requires the source link's directory bit even for dangling links.
    /// Unix ignores this field.
    pub target_is_directory: bool,
}
/// Link behavior kept separate so existing `TreeOptions` literals still compile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkOptions {
    /// Recreate symbolic links and copy each regular inode only once.
    pub preserve_links: bool,
}
impl LinkOptions {
    /// Parse [`export_args_with_links`]; older argument sets default to false.
    pub fn from_matches(m: &ArgMatches) -> Result<Self, CliError> {
        Ok(Self {
            preserve_links: m.try_get_one::<String>("preserve").ok().flatten().is_some(),
        })
    }
}
/// Tree arguments with opt-in `--preserve=links`; targets are never followed.
pub fn export_args_with_links() -> [Arg; 7] {
    let [path, dest, replace, verify, recursive, depth] = export_args_with_tree();
    [path, dest, replace, verify, recursive, depth,
        Arg::new("preserve").long("preserve").value_parser(["links"])
            .help("Preserve hard links and symbolic links without following targets; report targets escaping DEST")]
}

// Use exclusive creation; replacing a name stages the link beside it and
// renames it atomically, so an error cannot remove the previous destination.
fn publish_link(
    local: &Path,
    replace: bool,
    create: impl Fn(&Path) -> io::Result<()>,
) -> Result<(), CliError> {
    let parent = local
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let failed = |e| CliError::failed(format!("publish link {}: {e}", local.display()));
    if !replace {
        create(local).map_err(failed)?;
    } else {
        let mut staging = None;
        for n in 0..1000u32 {
            let candidate = parent.join(format!(".core-link-{}-{n}.partial", std::process::id()));
            match create(&candidate) {
                Ok(()) => {
                    staging = Some(candidate);
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(failed(e)),
            }
        }
        let staging = staging.ok_or_else(|| CliError::failed("no free link staging name"))?;
        if let Err(e) = fs::rename(&staging, local) {
            let _ = fs::remove_file(&staging);
            return Err(failed(e));
        }
    }
    sync_dir(parent).map_err(failed)
}
fn create_symbolic_link(link: &SymbolicLink, local: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&link.target, local)
    }
    #[cfg(windows)]
    {
        if link.target_is_directory {
            std::os::windows::fs::symlink_dir(&link.target, local)
        } else {
            std::os::windows::fs::symlink_file(&link.target, local)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (link, local);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "symbolic links unsupported",
        ))
    }
}
fn target_escapes(target: &Path, mut parent_depth: usize) -> bool {
    use std::path::Component;
    for part in target.components() {
        match part {
            Component::RootDir | Component::Prefix(_) => return true,
            Component::ParentDir if parent_depth == 0 => return true,
            Component::ParentDir => parent_depth -= 1,
            Component::Normal(_) => parent_depth += 1,
            Component::CurDir => {}
        }
    }
    false
}
struct TreeFile<'a> {
    source: &'a mut dyn TreeSource,
    path: &'a Path,
    size: u64,
}
impl FileSource for TreeFile<'_> {
    fn image_path(&self) -> Option<&Path> {
        self.source.image_path()
    }
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        self.source.read_file_at(self.path, offset, buf)
    }
}
fn safe_name(name: &OsStr) -> bool {
    use std::path::Component;
    let bytes = name.as_encoded_bytes();
    !bytes.is_empty()
        && name != "."
        && name != ".."
        && !bytes.iter().any(|b| [0, b'/', b'\\'].contains(b))
        // Win32 strips trailing dots/spaces and interprets colons as streams.
        && (!cfg!(windows)
            || (!bytes.contains(&b':') && !matches!(bytes.last(), Some(b'.' | b' '))))
        && matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
}

/// Reserve source names on the destination filesystem before copying anything.
/// Real lookups, rather than a Unicode case table, decide which names alias.
/// A private child directory inherits the destination's case behavior; its
/// markers disappear before any of the queued children are exported.
struct NameProbe(Option<PathBuf>);
impl NameProbe {
    fn new(parent: &Path) -> Result<Self, CliError> {
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        for n in 0..1000u32 {
            let dir = parent.join(format!(".core-name-probe-{}-{n}", std::process::id()));
            match builder.create(&dir) {
                Ok(()) => return Ok(Self(Some(dir))),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(CliError::failed(format!("probe destination names: {e}"))),
            }
        }
        Err(CliError::failed("no free destination name-probe directory"))
    }

    fn reserve(&self, name: &OsStr) -> Result<(), CliError> {
        let marker = self.0.as_ref().expect("probe is active").join(name);
        let failed = |e| {
            CliError::failed(format!(
                "probe destination name {}: {e}",
                name.to_string_lossy()
            ))
        };
        match fs::symlink_metadata(&marker) {
            Ok(_) => {
                let first = fs::read_to_string(&marker).map_err(failed)?;
                return Err(CliError::refused(format!("collides with {first}")));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(failed(e)),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)
            .map_err(failed)?;
        file.write_all(name.to_string_lossy().as_bytes())
            .map_err(failed)
    }

    fn finish(mut self) -> Result<(), CliError> {
        fs::remove_dir_all(self.0.as_ref().expect("probe is active"))
            .map_err(|e| CliError::failed(format!("remove destination name probes: {e}")))?;
        self.0 = None;
        Ok(())
    }
}
impl Drop for NameProbe {
    fn drop(&mut self) {
        if let Some(dir) = &self.0 {
            let _ = fs::remove_dir_all(dir);
        }
    }
}
/// Export a file or, with `recursive`, a directory into a local destination.
/// Every visited item appears once in the report, with counts for all four
/// statuses. A refused/skipped directory is not enumerated. Any incomplete
/// item makes the outcome nonzero; item errors do not stop siblings. Existing
/// directories merge; symlinks and directory cycles are refused. Within each
/// directory, the first source name wins and later names that the destination
/// filesystem considers equivalent are refused, even with `--replace`.
/// No snapshot or protection against concurrent source/destination mutation
/// is promised.
pub fn export_tree(
    request: &ExportRequest,
    source: &mut dyn TreeSource,
    options: TreeOptions,
) -> Result<Outcome, CliError> {
    export_tree_with_links(request, source, options, LinkOptions::default())
}
/// Export with explicit link preservation. File identities are source-wide;
/// only successfully published files become hard-link anchors. Symbolic targets
/// are copied unchanged, with escaping targets reported as completed warnings.
/// Repeated directories are refused as `loop` under either link policy.
/// Drivers must supply [`TreeSource::symbolic_link`] to identify symbolic links;
/// unclassified `Other` entries retain their existing unsupported status.
pub fn export_tree_with_links(
    request: &ExportRequest,
    source: &mut dyn TreeSource,
    options: TreeOptions,
    links: LinkOptions,
) -> Result<Outcome, CliError> {
    let Output::File(dest) = &request.output else {
        return Err(CliError::usage("tree export needs a local DEST"));
    };
    let _lock = source
        .image_path()
        .map(|p| ImageLock::for_export(p, &mut io::stderr().lock()))
        .transpose()?;
    let root = PathBuf::from(&request.path);
    let metadata = source.metadata(&root)?;
    if metadata.kind == TreeKind::Directory && !options.recursive {
        return Err(CliError::usage("directory source requires --recursive"));
    }
    let mut pending = vec![(root, dest.clone(), metadata, 0usize)];
    let mut directories = HashSet::new();
    let mut files: HashMap<u64, (PathBuf, Json)> = HashMap::new();
    let mut items = Vec::new();
    let mut counts = [0u64; 4];
    let mut merged = 0u64;
    while let Some((path, local, metadata, depth)) = pending.pop() {
        let mut reason = String::new();
        let mut evidence = Json::Null;
        let result = (|| -> Result<usize, CliError> {
            if depth > options.max_depth {
                return Err(CliError::failed("maximum depth exceeded"));
            }
            if let Some(image) = source.image_path() {
                refuse_image_alias(image, &local).map_err(|e| CliError::refused(e.message))?;
            }
            let existing = match fs::symlink_metadata(&local) {
                Ok(m) => Some(m),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(CliError::failed(e.to_string())),
            };
            // Every parent below DEST was checked as a directory before its
            // children were queued. Do not follow a destination entry's link.
            if existing
                .as_ref()
                .is_some_and(|m| m.file_type().is_symlink())
            {
                return Err(CliError::refused("destination is a symlink"));
            }
            match metadata.kind {
                TreeKind::Other => {
                    if let Some(link) = source.symbolic_link(&path)? {
                        if !links.preserve_links {
                            return Err(CliError::refused(
                                "symbolic link requires --preserve=links",
                            ));
                        }
                        if existing
                            .as_ref()
                            .is_some_and(|m| m.is_dir() || !request.replace)
                        {
                            return Err(CliError::refused(
                                "destination exists; --replace permits existing files only",
                            ));
                        }
                        publish_link(&local, request.replace, |p| create_symbolic_link(&link, p))?;
                        reason = if target_escapes(&link.target, depth.saturating_sub(1)) {
                            "symbolic link preserved; target escapes DEST"
                        } else {
                            "symbolic link preserved"
                        }
                        .into();
                        return Ok(0);
                    }
                    reason = "unsupported entry type".into();
                    return Ok(1);
                }
                TreeKind::File => {
                    if existing
                        .as_ref()
                        .is_some_and(|m| m.is_dir() || !request.replace)
                    {
                        return Err(CliError::refused(
                            "destination exists; --replace permits existing files only",
                        ));
                    }
                    if let Some((first, copied)) = files.get(&metadata.inode) {
                        if links.preserve_links {
                            publish_link(&local, request.replace, |p| fs::hard_link(first, p))?;
                            evidence = copied.clone();
                            reason = "hard link preserved".into();
                            return Ok(0);
                        }
                        reason = "hard link copied as separate file; links not preserved".into();
                    }
                    let copied = export_file_with_options(
                        &mut TreeFile {
                            source,
                            path: &path,
                            size: metadata.size,
                        },
                        Destination::File {
                            path: &local,
                            replace: request.replace,
                        },
                        ExportOptions {
                            verify: options.verify,
                        },
                    )?;
                    evidence = Json::object([
                        ("bytes", Json::from(copied.bytes)),
                        ("sha256", Json::from(copied.sha256_hex())),
                    ]);
                    files
                        .entry(metadata.inode)
                        .or_insert_with(|| (local.clone(), evidence.clone()));
                }
                TreeKind::Directory => {
                    if !directories.insert(metadata.inode) {
                        return Err(CliError::refused("loop: directory inode already visited"));
                    }
                    if existing.as_ref().is_some_and(|m| !m.is_dir()) {
                        return Err(CliError::refused(
                            "destination exists and is not a directory",
                        ));
                    }
                    if existing.is_some() {
                        merged += 1;
                        reason = "merged into existing directory".into();
                    } else {
                        fs::create_dir(&local).map_err(|e| CliError::failed(e.to_string()))?;
                    }
                    let children = source.read_dir(&path)?;
                    let probe = NameProbe::new(&local)?;
                    let mut queued = Vec::new();
                    for child in children {
                        let checked = if safe_name(&child.name) {
                            probe.reserve(&child.name)
                        } else {
                            Err(CliError::refused("unsafe entry name"))
                        };
                        if let Err(e) = checked {
                            let status = if e.code == super::super::output::EXIT_UNSUPPORTED {
                                2
                            } else {
                                3
                            };
                            counts[status] += 1;
                            items.push(Json::object([
                                ("parent", Json::from(path.display().to_string())),
                                (
                                    "name",
                                    Json::from(child.name.to_string_lossy().into_owned()),
                                ),
                                (
                                    "status",
                                    Json::from(
                                        ["completed", "skipped", "refused", "failed"][status],
                                    ),
                                ),
                                ("reason", Json::from(e.message)),
                            ]));
                        } else {
                            queued.push((
                                path.join(&child.name),
                                local.join(&child.name),
                                child.metadata,
                                depth.saturating_add(1),
                            ));
                        }
                    }
                    probe.finish()?;
                    // The stack is LIFO; reversing preserves the driver's
                    // listing order, including which spelling wins a collision.
                    pending.extend(queued.into_iter().rev());
                }
            }
            Ok(0)
        })();
        let status = match result {
            Ok(s) => s,
            Err(e) => {
                reason = e.message;
                if e.code == super::super::output::EXIT_UNSUPPORTED {
                    2
                } else {
                    3
                }
            }
        };
        counts[status] += 1;
        items.push(Json::object([
            ("path", Json::from(path.display().to_string())),
            ("destination", Json::from(local.display().to_string())),
            (
                "status",
                Json::from(["completed", "skipped", "refused", "failed"][status]),
            ),
            ("reason", Json::from(reason)),
            ("copy", evidence),
        ]));
    }
    Ok(Outcome::report(Json::object([
        ("completed", Json::from(counts[0])),
        ("skipped", Json::from(counts[1])),
        ("refused", Json::from(counts[2])),
        ("failed", Json::from(counts[3])),
        ("merged_directories", Json::from(merged)),
        ("items", Json::Arr(items)),
    ]))
    .with_code(u8::from(counts[1..].iter().any(|n| *n != 0))))
}

//! Driver-owned staging keeps imports transactional without buffering the input.
use super::*;

/// One destination's staging transaction, implemented over a driver's write API.
/// Methods must never modify the final name before `publish`. Drivers without
/// this capability pass `None` to the import entry points and refuse with exit 3.
pub trait FileSink {
    /// The backing image, for cooperative exclusive locking and alias checks.
    /// Imports lock this path before `stage`. Open writable driver state lazily
    /// in `stage`; do not pre-acquire a second lock on the same image.
    fn image_path(&self) -> Option<&Path> {
        None
    }
    /// Whether publication can atomically replace an existing name.
    fn supports_atomic_replace(&self) -> bool {
        false
    }
    /// Create a private staged file beside `path`. Refuse existing names when
    /// `replace` is false. On failure, abort must still be safe to call.
    fn stage(&mut self, path: &Path, replace: bool) -> Result<(), CliError>;
    /// Write the entire slice or fail. Offsets increase from zero without gaps.
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), CliError>;
    /// Persist all staged content and metadata before publication.
    fn persist(&mut self) -> Result<(), CliError>;
    /// Read the staged file back, for callers requesting verification.
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<usize, CliError>;
    /// Atomically publish the staged file; return whether a name was replaced.
    /// No-replace must also refuse names created since `stage`. On failure the
    /// final name must remain untouched; never delete first to emulate rename.
    fn publish(&mut self, replace: bool) -> Result<bool, CliError>;
    /// Remove staging without touching the final name, including after a failed
    /// stage. Report cleanup failures so abandoned staging is not hidden.
    fn abort(&mut self) -> Result<(), CliError>;
}

fn capable(sink: Option<&mut dyn FileSink>, replace: bool) -> Result<&mut dyn FileSink, CliError> {
    let sink = sink.ok_or_else(|| CliError::refused("this driver has no import sink"))?;
    if replace && !sink.supports_atomic_replace() {
        return Err(CliError::refused(
            "this sink cannot replace a name atomically",
        ));
    }
    Ok(sink)
}

/// An opened local file, optionally wrapped by a reader that observes chunks.
/// Reads must come from `file`, starting at offset zero. Supplying `local_path`
/// also detects unlinking or replacing the source name while its handle is open.
pub trait LocalSource: Read {
    /// The opened file whose identity and metadata describe the input bytes.
    fn file(&self) -> &File;
    /// The source name, when available; it must still name the opened file.
    fn local_path(&self) -> Option<&Path> {
        None
    }
}

impl LocalSource for File {
    fn file(&self) -> &File {
        self
    }
}

struct NamedSource<'a> {
    file: File,
    path: &'a Path,
}
impl Read for NamedSource<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.file.read(bytes)
    }
}
impl LocalSource for NamedSource<'_> {
    fn file(&self) -> &File {
        &self.file
    }
    fn local_path(&self) -> Option<&Path> {
        Some(self.path)
    }
}

struct SourceIdentity {
    metadata: fs::Metadata,
    #[cfg(not(unix))]
    identity: same_file::Handle,
}
impl SourceIdentity {
    fn capture(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::other(
                "a local import source must be a regular file",
            ));
        }
        metadata.modified()?;
        Ok(Self {
            metadata,
            #[cfg(not(unix))]
            identity: same_file::Handle::from_file(file.try_clone()?)?,
        })
    }

    fn matches(&self, metadata: &fs::Metadata) -> io::Result<bool> {
        if !metadata.is_file()
            || self.metadata.len() != metadata.len()
            || self.metadata.modified()? != metadata.modified()?
        {
            return Ok(false);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(self.metadata.dev() == metadata.dev()
                && self.metadata.ino() == metadata.ino()
                && self.metadata.ctime() == metadata.ctime()
                && self.metadata.ctime_nsec() == metadata.ctime_nsec())
        }
        #[cfg(not(unix))]
        Ok(true)
    }

    fn check(&self, source: &dyn LocalSource, bytes: u64) -> Result<(), CliError> {
        let unchanged = (|| -> io::Result<bool> {
            if bytes != self.metadata.len() || !self.matches(&source.file().metadata()?)? {
                return Ok(false);
            }
            #[cfg(not(unix))]
            if self.identity != same_file::Handle::from_file(source.file().try_clone()?)? {
                return Ok(false);
            }
            if let Some(path) = source.local_path() {
                if !self.matches(&fs::metadata(path)?)? {
                    return Ok(false);
                }
                #[cfg(not(unix))]
                if self.identity != same_file::Handle::from_path(path)? {
                    return Ok(false);
                }
            }
            Ok(true)
        })();
        match unchanged {
            Ok(true) => Ok(()),
            Ok(false) => Err(CliError::failed("source changed during copy")),
            Err(error) => Err(CliError::failed(format!(
                "source changed during copy: {error}"
            ))),
        }
    }
}

/// Import a local file only if its identity, size and timestamps stay unchanged.
/// The checks run after EOF and again after persistence, before publication;
/// failure aborts staging and preserves the old destination. Unix includes
/// device, inode and change time. Other hosts cannot detect every same-size,
/// restored-mtime rewrite; [`import`] reports that limitation. The digest covers
/// exactly the bytes read. Generic streams continue to use [`import_file`].
pub fn import_local_file(
    source: &mut dyn LocalSource,
    sink: Option<&mut dyn FileSink>,
    path: &Path,
    replace: bool,
) -> Result<Copied, CliError> {
    let sink = capable(sink, replace)?;
    let identity = SourceIdentity::capture(source.file())
        .map_err(|e| CliError::failed(format!("cannot inspect import source: {e}")))?;
    identity.check(source, identity.metadata.len())?;
    import_stream(
        source,
        Some(sink),
        path,
        replace,
        Some(identity.metadata.len()),
        |source, bytes| identity.check(source, bytes),
    )
}

/// Stream a reader (a local file or locked stdin) into a driver's staged file.
/// Refusals consume no input. A source, write, persistence or publication error
/// aborts staging and preserves an existing destination. Memory is one `CHUNK`.
/// SIGINT/SIGTERM use the existing transfer cancellation status and cleanup.
/// Use [`import_local_file`] to validate a local file's identity as well.
pub fn import_file(
    source: &mut dyn Read,
    sink: Option<&mut dyn FileSink>,
    path: &Path,
    replace: bool,
) -> Result<Copied, CliError> {
    import_stream(source, sink, path, replace, None, |_, _| Ok(()))
}

fn import_stream<S: Read + ?Sized>(
    source: &mut S,
    sink: Option<&mut dyn FileSink>,
    path: &Path,
    replace: bool,
    size: Option<u64>,
    mut validate: impl FnMut(&S, u64) -> Result<(), CliError>,
) -> Result<Copied, CliError> {
    let sink = capable(sink, replace)?;
    let _lock = sink.image_path().map(ImageLock::for_import).transpose()?;
    let cancellation = Cancellation::install()?;
    let mut bytes = 0u64;
    let result = (|| {
        cancellation.check(0, 0)?;
        sink.stage(path, replace)?;
        let mut buffer = vec![0u8; CHUNK];
        let mut digest = Sha256::new();
        loop {
            cancellation.check(bytes, bytes)?;
            let read = source.read(&mut buffer);
            cancellation.check(bytes, bytes)?;
            let n = match read {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => result
                    .map_err(|e| CliError::failed(format!("input failed at byte {bytes}: {e}")))?,
            };
            if n == 0 {
                break;
            }
            if n > buffer.len() {
                return Err(CliError::failed("input returned an invalid byte count"));
            }
            let next = bytes
                .checked_add(n as u64)
                .ok_or_else(|| CliError::failed("import byte count overflow"))?;
            if size.is_some_and(|size| next > size) {
                return Err(CliError::failed("source changed during copy"));
            }
            sink.write_at(bytes, &buffer[..n])?;
            digest.update(&buffer[..n]);
            bytes = next;
        }
        cancellation.check(bytes, bytes)?;
        let sha256 = digest.finalize().into();
        validate(source, bytes)?;
        let persisted = sink.persist();
        cancellation.check(bytes, bytes)?;
        persisted?;
        validate(source, bytes)?;
        cancellation.check(bytes, bytes)?;
        let replaced = sink.publish(replace)?;
        Ok(Copied {
            bytes,
            sha256,
            replaced,
        })
    })();
    if let Err(mut error) = result {
        if let Err(cleanup) = sink.abort() {
            error
                .message
                .push_str(&format!("; abort failed: {}", cleanup.message));
        }
        return Err(error);
    }
    result
}

/// A local import source, parsed without opening it or consuming stdin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// `-`, consumed only after sink capabilities have been checked.
    Stdin,
    /// An ordinary local file; its identity is checked against the image.
    File(PathBuf),
}

/// Shared `import SOURCE PATH [--replace]` request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportRequest {
    /// Local file or stdin.
    pub input: Input,
    /// Destination inside the image.
    pub path: PathBuf,
    /// Ask for atomic replacement; no-replace is the default.
    pub replace: bool,
}

/// The arguments every driver's single-file `import` takes.
pub fn import_args() -> [Arg; 3] {
    let [path, _, replace] = export_args();
    [
        Arg::new("source")
            .value_name("SOURCE")
            .required(true)
            .value_parser(clap::value_parser!(OsString))
            .help("The local file to import, or - for stdin"),
        path,
        replace.help("Replace PATH atomically; without this an existing PATH is refused"),
    ]
}

impl ImportRequest {
    /// Parse [`import_args`] without performing I/O.
    pub fn from_matches(matches: &ArgMatches) -> Result<Self, CliError> {
        let given = |id: &str| {
            matches
                .get_one::<OsString>(id)
                .cloned()
                .ok_or_else(|| CliError::usage(format!("missing {}", id.to_uppercase())))
        };
        let source = given("source")?;
        Ok(Self {
            input: if source == "-" {
                Input::Stdin
            } else {
                Input::File(source.into())
            },
            path: given("path")?.into(),
            replace: matches.get_flag("replace"),
        })
    }
}

/// Open a local source or consume stdin only after checking sink capability.
/// The result reports bytes, SHA-256 and whether the name was created/replaced.
pub fn import(
    request: &ImportRequest,
    sink: Option<&mut dyn FileSink>,
) -> Result<Outcome, CliError> {
    let sink = capable(sink, request.replace)?;
    let copied = match &request.input {
        Input::Stdin => import_file(
            &mut io::stdin().lock(),
            Some(sink),
            &request.path,
            request.replace,
        )?,
        Input::File(local) => {
            if let Some(image) = sink.image_path() {
                refuse_image_alias(image, local)?;
            }
            let file = File::open(local)
                .map_err(|e| CliError::failed(format!("open {}: {e}", local.display())))?;
            let mut source = NamedSource { file, path: local };
            import_local_file(&mut source, Some(sink), &request.path, request.replace)?
        }
    };
    let limitation = (!cfg!(unix) && matches!(&request.input, Input::File(_))).then_some(
        "source change time is unavailable; same-size rewrites with restored modification time may be undetectable",
    );
    let limitation_text = limitation
        .map(|text| format!("; limitation: {text}"))
        .unwrap_or_default();
    let report = Json::object([
        (
            "destination",
            Json::from(request.path.display().to_string()),
        ),
        ("bytes", Json::from(copied.bytes)),
        ("sha256", Json::from(copied.sha256_hex())),
        ("created", Json::from(!copied.replaced)),
        ("replaced", Json::from(copied.replaced)),
        (
            "limitations",
            Json::Arr(limitation.into_iter().map(Json::from).collect()),
        ),
    ]);
    Ok(Outcome::report(report).with_text(format!(
        "imported {} bytes to {} (sha256 {}){}",
        copied.bytes,
        request.path.display(),
        copied.sha256_hex(),
        limitation_text
    )))
}

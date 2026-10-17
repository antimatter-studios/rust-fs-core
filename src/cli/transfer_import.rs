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

/// Stream a reader (a local file or locked stdin) into a driver's staged file.
/// Refusals consume no input. A source, write, persistence or publication error
/// aborts staging and preserves an existing destination. Memory is one `CHUNK`.
/// SIGINT/SIGTERM use the existing transfer cancellation status and cleanup.
pub fn import_file(
    source: &mut dyn Read,
    sink: Option<&mut dyn FileSink>,
    path: &Path,
    replace: bool,
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
            sink.write_at(bytes, &buffer[..n])?;
            digest.update(&buffer[..n]);
            bytes = next;
        }
        cancellation.check(bytes, bytes)?;
        let persisted = sink.persist();
        cancellation.check(bytes, bytes)?;
        persisted?;
        let replaced = sink.publish(replace)?;
        Ok(Copied {
            bytes,
            sha256: digest.finalize().into(),
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
            let mut source = File::open(local)
                .map_err(|e| CliError::failed(format!("open {}: {e}", local.display())))?;
            import_file(&mut source, Some(sink), &request.path, request.replace)?
        }
    };
    let report = Json::object([
        (
            "destination",
            Json::from(request.path.display().to_string()),
        ),
        ("bytes", Json::from(copied.bytes)),
        ("sha256", Json::from(copied.sha256_hex())),
        ("created", Json::from(!copied.replaced)),
        ("replaced", Json::from(copied.replaced)),
    ]);
    Ok(Outcome::report(report).with_text(format!(
        "imported {} bytes to {} (sha256 {})",
        copied.bytes,
        request.path.display(),
        copied.sha256_hex()
    )))
}

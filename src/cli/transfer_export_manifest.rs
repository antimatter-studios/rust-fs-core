//! Export adapters capture identities around verified publication.
use super::super::super::output::{Json, Outcome};
use super::super::{self as transfer, ExportOptions, ExportRequest, Output, TreeOptions};
use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

impl Timestamp {
    /// Convert a host time without losing subsecond or pre-epoch precision.
    pub fn from_system_time(time: SystemTime) -> Result<Self, CliError> {
        let (seconds, nanoseconds) = match time.duration_since(UNIX_EPOCH) {
            Ok(d) => (i64::try_from(d.as_secs()).ok(), d.subsec_nanos()),
            Err(e) => {
                let d = e.duration();
                let seconds = i64::try_from(d.as_secs())
                    .ok()
                    .and_then(|n| n.checked_neg());
                if d.subsec_nanos() == 0 {
                    (seconds, 0)
                } else {
                    (
                        seconds.and_then(|n| n.checked_sub(1)),
                        1_000_000_000 - d.subsec_nanos(),
                    )
                }
            }
        };
        Ok(Self {
            seconds: seconds.ok_or_else(|| CliError::failed("manifest timestamp overflow"))?,
            nanoseconds,
        })
    }
}

impl LocalIdentity {
    /// Capture host identity and timestamps from an opened object, not a path lookup.
    pub fn capture(path: &Path, file: &File) -> Result<Self, CliError> {
        let failed = |e| CliError::failed(format!("manifest identity {}: {e}", path.display()));
        let metadata = file.metadata().map_err(failed)?;
        #[cfg(unix)]
        let (device, inode, changed) = {
            use std::os::unix::fs::MetadataExt;
            (
                metadata.dev(),
                metadata.ino(),
                Timestamp {
                    seconds: metadata.ctime(),
                    nanoseconds: metadata.ctime_nsec() as u32,
                },
            )
        };
        #[cfg(windows)]
        let (device, inode, changed) = windows_identity(file).map_err(failed)?;
        #[cfg(not(any(unix, windows)))]
        return Err(CliError::refused("host manifest identities unsupported"));
        Ok(Self {
            path: path.into(),
            device,
            inode,
            size: metadata.len(),
            accessed: Timestamp::from_system_time(metadata.accessed().map_err(failed)?)?,
            modified: Timestamp::from_system_time(metadata.modified().map_err(failed)?)?,
            changed,
        })
    }
}

#[cfg(windows)]
fn windows_identity(file: &File) -> io::Result<(u64, u64, Timestamp)> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct Basic {
        creation: i64,
        access: i64,
        write: i64,
        change: i64,
        attributes: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct FileId {
        volume: u64,
        id: [u8; 16],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            handle: *mut c_void,
            class: i32,
            info: *mut c_void,
            size: u32,
        ) -> i32;
    }
    let mut id = FileId::default();
    let mut basic = Basic::default();
    // FILE_ID_INFO (FileIdInfo = 18) and FILE_BASIC_INFO (FileBasicInfo = 0).
    // https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            18,
            (&mut id as *mut FileId).cast(),
            std::mem::size_of::<FileId>() as u32,
        )
    } == 0
        || unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                0,
                (&mut basic as *mut Basic).cast(),
                std::mem::size_of::<Basic>() as u32,
            )
        } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if id.id[8..].iter().any(|byte| *byte != 0) {
        return Err(io::Error::other(
            "128-bit host file IDs cannot fit the manifest inode",
        ));
    }
    let ticks = basic.change - 116_444_736_000_000_000;
    Ok((
        id.volume,
        u64::from_le_bytes(id.id[..8].try_into().expect("eight inode bytes")),
        Timestamp {
            seconds: ticks.div_euclid(10_000_000),
            nanoseconds: (ticks.rem_euclid(10_000_000) * 100) as u32,
        },
    ))
}

pub(in crate::cli::transfer) fn export_identity(
    identity: Identity,
    path: &Path,
    image_path: Option<&Path>,
) -> Result<Identity, CliError> {
    if !matches!(&identity, Identity::Image { path: p, image, .. } if p == path && Some(image.path.as_path()) == image_path)
    {
        return Err(CliError::refused(
            "manifest identity must describe the exported image file",
        ));
    }
    Ok(identity)
}

pub(in crate::cli::transfer) struct ExportLedger {
    manifest: Manifest,
    pub failed: bool,
    read: ManifestRead,
    resume: bool,
}

/// Sequential reads of a driver source, for the resume digest.
struct SourceReader<'a> {
    source: &'a mut dyn transfer::FileSource,
    offset: u64,
    size: u64,
}
impl Read for SourceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // An early end is a length mismatch, so the file is copied again.
        let want = buf
            .len()
            .min(usize::try_from(self.size - self.offset).unwrap_or(usize::MAX));
        if want == 0 {
            return Ok(0);
        }
        let n = self
            .source
            .read_at(self.offset, &mut buf[..want])
            .map_err(|e| io::Error::other(e.message))?;
        if n > want {
            return Err(io::Error::other(
                "the source answered more bytes than asked",
            ));
        }
        self.offset += n as u64;
        Ok(n)
    }
}

// Any unusable destination is copied again; the copy reports why it fails.
fn destination_identity(dest: &Path) -> Result<Option<Identity>, CliError> {
    match File::open(dest) {
        Ok(file) if file.metadata().is_ok_and(|m| m.is_file()) => {
            Ok(Some(Identity::Local(LocalIdentity::capture(dest, &file)?)))
        }
        _ => Ok(None),
    }
}

/// The report for a file skipped on earlier evidence. It is not a verification
/// performed now, so `verified` stays false beside the original completion time.
pub(in crate::cli::transfer) fn skipped(
    request: &ExportRequest,
    dest: &Path,
    entry: &ManifestEntry,
) -> Outcome {
    let path = request.path.to_string_lossy();
    let at = entry.completed_at;
    let report = Json::object([
        ("path", Json::from(path.as_ref())),
        ("destination", Json::from(dest.display().to_string())),
        ("bytes", Json::from(entry.bytes)),
        ("sha256", Json::from(entry.sha256.as_str())),
        ("replaced", Json::from(false)),
        ("verified", Json::from(false)),
        ("skipped", Json::from("verified earlier")),
        (
            "verified_earlier_at",
            Json::object([
                ("seconds", Json::from(at.seconds)),
                ("nanoseconds", Json::from(i64::from(at.nanoseconds))),
            ]),
        ),
    ]);
    let text = format!(
        "{path} to {}: skipped: verified earlier at {}.{:09} (not verified now; {} bytes, sha256 {})",
        dest.display(),
        at.seconds,
        at.nanoseconds,
        entry.bytes,
        entry.sha256
    );
    Outcome::report(report).with_text(text)
}
impl ExportLedger {
    fn open(
        request: &ExportRequest,
        image: Option<&Path>,
        options: &ExportManifestOptions,
        tree: bool,
    ) -> Result<Option<Self>, CliError> {
        if options.resume && tree {
            return Err(CliError::usage(
                "export tree resume execution is not supported by this recording adapter",
            ));
        }
        let Some(path) = &options.manifest else {
            return Ok(None);
        };
        let Output::File(dest) = &request.output else {
            return Err(CliError::usage("--manifest needs a file DEST, not stdout"));
        };
        let image =
            image.ok_or_else(|| CliError::refused("export manifest needs a backing image path"))?;
        let (manifest, read) = Manifest::open_for_export(path, image, dest, tree)?;
        if read.torn_tail {
            eprintln!("manifest {}: recovered torn tail", path.display());
        }
        Ok(Some(Self {
            manifest,
            failed: false,
            read,
            resume: options.resume,
        }))
    }
    fn report(&self, outcome: &mut Outcome) {
        if let Some(Json::Obj(fields)) = &mut outcome.report {
            fields.push(("manifest_torn_tail".into(), Json::from(self.read.torn_tail)));
        }
    }
    /// With `--resume`, the evidence that lets a single-file export skip its copy.
    /// The caller holds the image lock. Fresh identities of the opened endpoints
    /// and a complete source digest must match the latest record, and both
    /// endpoints must be unchanged after the digest. Anything else returns
    /// `None`, and the export copies from byte zero under its usual policy.
    pub fn resume(
        &self,
        source: &mut dyn transfer::FileSource,
        path: &Path,
        dest: &Path,
    ) -> Result<Option<&ManifestEntry>, CliError> {
        if !self.resume {
            return Ok(None);
        }
        let capture = |source: &dyn transfer::FileSource| {
            export_identity(source.manifest_identity()?, path, source.image_path())
        };
        let source_before = capture(source)?;
        let Some(dest_before) = destination_identity(dest)? else {
            return Ok(None);
        };
        let size = source.size();
        let mut reader = SourceReader {
            source: &mut *source,
            offset: 0,
            size,
        };
        let decision = self
            .read
            .check_resume(&source_before, &dest_before, &mut reader)?;
        if decision == ResumeDecision::Copy {
            return Ok(None);
        }
        // Revalidate both snapshots: a change during the digest restarts the copy.
        let source_after = capture(source)?;
        let unchanged = destination_identity(dest)?
            .is_some_and(|dest_after| same_identity(&dest_before, &dest_after));
        if !unchanged || !same_identity(&source_before, &source_after) {
            return Ok(None);
        }
        Ok(self
            .read
            .entries
            .iter()
            .rev()
            .find(|entry| same_location(&entry.destination, &dest_before)))
    }
    pub fn record(
        &mut self,
        source: Identity,
        destination: &Path,
        copied: &transfer::Copied,
    ) -> Result<(), CliError> {
        self.failed = true;
        let file = File::open(destination)
            .map_err(|e| CliError::failed(format!("manifest published file: {e}")))?;
        let entry = ManifestEntry {
            source,
            destination: Identity::Local(LocalIdentity::capture(destination, &file)?),
            bytes: copied.bytes,
            sha256: copied.sha256_hex(),
            completed_at: Timestamp::from_system_time(SystemTime::now())?,
        };
        self.manifest.append(&entry, true)?;
        self.failed = false;
        Ok(())
    }
}

/// Export with an optional ledger, opened safely before content reads or writes.
/// Driver identity is required for verified recording and for resume. With
/// resume, a file is skipped only on revalidated evidence of earlier
/// verification; otherwise it is copied from byte zero. Legacy request and
/// option literals remain valid.
pub fn export_with_manifest(
    request: &ExportRequest,
    source: &mut dyn transfer::FileSource,
    options: ExportOptions,
    manifest: &ExportManifestOptions,
) -> Result<Outcome, CliError> {
    let _lock = if manifest.manifest.is_some() {
        source
            .image_path()
            .map(|image| transfer::ImageLock::for_export(image, &mut io::stderr().lock()))
            .transpose()?
    } else {
        None
    };
    let mut ledger = ExportLedger::open(request, source.image_path(), manifest, false)?;
    let mut stderr = io::stderr();
    let terminal = std::io::IsTerminal::is_terminal(&stderr);
    let mut progress =
        transfer::Progress::new(&mut stderr, terminal, transfer::ProgressOptions::default());
    let mut outcome =
        transfer::export_recorded(request, source, options, &mut progress, ledger.as_mut())?;
    if let Some(ledger) = ledger {
        ledger.report(&mut outcome);
    }
    Ok(outcome)
}

/// Export a tree with immediate durable evidence for each verified publication.
/// Unverified copies record nothing; item failures retain earlier records.
/// Manifest capture or append errors stop traversal immediately.
pub fn export_tree_with_manifest(
    request: &ExportRequest,
    source: &mut dyn transfer::TreeSource,
    options: TreeOptions,
    manifest: &ExportManifestOptions,
) -> Result<Outcome, CliError> {
    let mut ledger = ExportLedger::open(request, source.image_path(), manifest, true)?;
    let mut stderr = io::stderr();
    let terminal = std::io::IsTerminal::is_terminal(&stderr);
    let mut progress =
        transfer::Progress::new(&mut stderr, terminal, transfer::ProgressOptions::default());
    let mut outcome = transfer::tree::export_tree_recorded(
        request,
        source,
        options,
        Default::default(),
        &mut progress,
        ledger.as_mut(),
    )?;
    if let Some(ledger) = ledger {
        ledger.report(&mut outcome);
    }
    Ok(outcome)
}

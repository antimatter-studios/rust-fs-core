//! Import adapters capture identities around verified publication.
use super::super::super::output::{Json, Outcome};
use super::super::{
    self as transfer, FileSink, ImportOptions, ImportRequest, ImportTreeOptions, Input, TreeSink,
};
use super::*;
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::time::SystemTime;

/// Accept only a driver identity naming the published image file.
pub(in crate::cli::transfer) fn import_identity(
    identity: Identity,
    path: &Path,
    image_path: Option<&Path>,
) -> Result<Identity, CliError> {
    if !matches!(&identity, Identity::Image { path: p, image, .. } if p == path && Some(image.path.as_path()) == image_path)
    {
        return Err(CliError::refused(
            "manifest identity must describe the imported image file",
        ));
    }
    Ok(identity)
}

pub(in crate::cli::transfer) struct ImportLedger {
    manifest: Manifest,
    /// Set while a capture or append is in flight; a tree stops on it.
    pub failed: bool,
    read: ManifestRead,
    /// With `--resume`, revalidated evidence may skip a single-file copy.
    pub resume: bool,
}

// Any unusable or unsupported destination is copied again.
fn destination_identity(sink: &mut dyn FileSink, path: &Path) -> Option<Identity> {
    let image = sink.image_path().map(Path::to_path_buf);
    let identity = sink.manifest_identity(path).ok()?;
    import_identity(identity, path, image.as_deref()).ok()
}

/// The report for an import skipped on earlier evidence. It is not a
/// verification performed now, so `verified` stays false beside the original
/// completion time.
pub(in crate::cli::transfer) fn import_skipped(
    request: &ImportRequest,
    entry: &ManifestEntry,
) -> Outcome {
    let dest = request.path.display().to_string();
    let at = entry.completed_at;
    let report = Json::object([
        ("destination", Json::from(dest.as_str())),
        ("bytes", Json::from(entry.bytes)),
        ("sha256", Json::from(entry.sha256.as_str())),
        ("created", Json::from(false)),
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
        "{dest}: skipped: verified earlier at {}.{:09} (not verified now; {} bytes, sha256 {})",
        at.seconds, at.nanoseconds, entry.bytes, entry.sha256
    );
    Outcome::report(report).with_text(text)
}

impl ImportLedger {
    fn open(
        request: &ImportRequest,
        image: Option<&Path>,
        options: &ImportManifestOptions,
        tree: bool,
    ) -> Result<Option<Self>, CliError> {
        let Some(path) = &options.manifest else {
            return Ok(None);
        };
        let Input::File(source) = &request.input else {
            return Err(CliError::usage("--manifest needs a file SOURCE, not stdin"));
        };
        let image =
            image.ok_or_else(|| CliError::refused("import manifest needs a backing image path"))?;
        let (manifest, read) = Manifest::open_for_import(path, image, source, tree)?;
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
    /// With `--resume`, the evidence that lets a single-file import skip its
    /// copy. The caller holds the image lock and keeps it for any copy. Fresh
    /// identities of the opened source and the driver's destination and a
    /// complete source digest must match the latest record, and both must be
    /// unchanged after the digest. Anything else returns `None`, and the import
    /// stages and copies from byte zero; the source is rewound for that copy.
    pub fn resume(
        &self,
        file: &File,
        local: &Path,
        sink: &mut dyn FileSink,
        path: &Path,
    ) -> Result<Option<&ManifestEntry>, CliError> {
        if !self.resume {
            return Ok(None);
        }
        let capture = || -> Result<Identity, CliError> {
            Ok(Identity::Local(LocalIdentity::capture(local, file)?))
        };
        let source_before = capture()?;
        let Some(dest_before) = destination_identity(sink, path) else {
            return Ok(None);
        };
        let mut reader = file;
        let decision = self
            .read
            .check_resume(&source_before, &dest_before, &mut reader);
        (&mut reader)
            .seek(SeekFrom::Start(0))
            .map_err(|e| CliError::failed(format!("rewind import source: {e}")))?;
        if decision? == ResumeDecision::Copy {
            return Ok(None);
        }
        // Revalidate both snapshots: a change during the digest restarts the copy.
        let source_after = capture()?;
        let unchanged = destination_identity(sink, path)
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
    /// Durably append one verified, published import before returning.
    pub fn record(
        &mut self,
        source: Identity,
        destination: Identity,
        copied: &transfer::Copied,
    ) -> Result<(), CliError> {
        self.failed = true;
        let entry = ManifestEntry {
            source,
            destination,
            bytes: copied.bytes,
            sha256: copied.sha256_hex(),
            completed_at: Timestamp::from_system_time(SystemTime::now())?,
        };
        self.manifest.append(&entry, true)?;
        self.failed = false;
        Ok(())
    }
}

/// Import a local file with an optional ledger, opened safely before any
/// source read or staged write. A verified, published import appends the
/// opened source's host identity, the driver-supplied destination identity
/// captured under the image lock, and the verified SHA-256; unverified imports
/// record nothing. With resume, a file is skipped only on revalidated evidence
/// of earlier verification; otherwise it is staged and copied from byte zero.
pub fn import_with_manifest(
    request: &ImportRequest,
    options: ImportOptions,
    sink: Option<&mut dyn FileSink>,
    manifest: &ImportManifestOptions,
) -> Result<Outcome, CliError> {
    let image = sink
        .as_deref()
        .and_then(|sink| sink.image_path())
        .map(Path::to_path_buf);
    let mut ledger = ImportLedger::open(request, image.as_deref(), manifest, false)?;
    let mut outcome = transfer::import::import_recorded(request, options, sink, ledger.as_mut())?;
    if let Some(ledger) = ledger {
        ledger.report(&mut outcome);
    }
    Ok(outcome)
}

/// Import a local tree with immediate durable evidence for each verified
/// publication. The ledger opens before any source read or staged write; each
/// verified, published file appends the opened source's host identity, the
/// driver-supplied destination identity and the verified SHA-256, synced
/// before traversal continues. Unverified copies record nothing; item failures
/// retain earlier records. Manifest capture or append errors stop traversal.
/// Resume is not applied yet: every file is copied.
pub fn import_tree_with_manifest(
    request: &ImportRequest,
    sink: Option<&mut dyn TreeSink>,
    options: ImportTreeOptions,
    verify: ImportOptions,
    manifest: &ImportManifestOptions,
) -> Result<Outcome, CliError> {
    let image = sink
        .as_deref()
        .and_then(|sink| sink.image_path())
        .map(Path::to_path_buf);
    let mut ledger = ImportLedger::open(request, image.as_deref(), manifest, true)?;
    let mut outcome = transfer::import_tree::import_tree_recorded(
        request,
        sink,
        options,
        verify,
        ledger.as_mut(),
    )?;
    if let Some(ledger) = ledger {
        ledger.report(&mut outcome);
    }
    Ok(outcome)
}

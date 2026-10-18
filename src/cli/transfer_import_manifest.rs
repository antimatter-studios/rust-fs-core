//! Import adapters capture identities around verified publication.
use super::super::super::output::{Json, Outcome};
use super::super::{
    self as transfer, FileSink, ImportOptions, ImportRequest, ImportTreeOptions, Input, TreeSink,
};
use super::*;
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
        }))
    }
    fn report(&self, outcome: &mut Outcome) {
        if let Some(Json::Obj(fields)) = &mut outcome.report {
            fields.push(("manifest_torn_tail".into(), Json::from(self.read.torn_tail)));
        }
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
/// record nothing. Resume is not applied yet: every file is copied.
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

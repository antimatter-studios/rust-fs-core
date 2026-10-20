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
    torn_tail: bool,
}
impl ExportLedger {
    fn open(
        request: &ExportRequest,
        image: Option<&Path>,
        options: &ExportManifestOptions,
        tree: bool,
    ) -> Result<Option<Self>, CliError> {
        if options.resume {
            return Err(CliError::usage(
                "export resume execution is not supported by this recording adapter",
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
            torn_tail: read.torn_tail,
        }))
    }
    fn report(&self, outcome: &mut Outcome) {
        if let Some(Json::Obj(fields)) = &mut outcome.report {
            fields.push(("manifest_torn_tail".into(), Json::from(self.torn_tail)));
        }
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
/// Driver identity is required only for verified recording. Legacy request and
/// option literals remain valid. Resume execution uses a separate adapter.
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

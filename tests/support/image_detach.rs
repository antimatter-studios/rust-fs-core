//! Support for DiskImages tests: detach by node, and an owned attachment
//! that detaches on drop. Each test crate includes this file and uses a subset.
#![allow(dead_code)]
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Detach attempts before a busy node is reported as still attached.
pub const ATTEMPTS: u32 = 6;

/// Detach `device`, which holds `image`, retrying while it reports "Resource
/// busy" (a scanning app holds the node for a moment). Any other failure, or a
/// node still busy after the last attempt, returns the tool's stderr.
///
/// `hdiutil info` is read before every attempt, the first included. The node may
/// have been released and given to another image since the attach or the last
/// refusal, so a detach runs only while the listing still names `image` on
/// `device`. An image that is no longer listed is no longer attached, which is
/// success. An image listed elsewhere, or a listing that cannot be read, is an
/// error.
pub fn detach(program: &OsStr, image: &Path, device: &OsStr) -> Result<(), String> {
    for attempt in 1..=ATTEMPTS {
        let listing = info(program)?;
        if !listed_on(&listing, image, device)? {
            eprintln!(
                "hdiutil info did not list {} before detach attempt {attempt} of {}; \
                 no detach run:\n{listing}",
                image.display(),
                Path::new(device).display()
            );
            return Ok(());
        }
        let mut command = Command::new(program);
        command.arg("detach");
        if attempt > 1 {
            command.arg("-force");
        }
        let output = command
            .arg(device)
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !stderr.to_ascii_lowercase().contains("resource busy") || attempt == ATTEMPTS {
            return Err(stderr);
        }
        std::thread::sleep(Duration::from_millis(200 * u64::from(attempt)));
    }
    unreachable!("the loop returns on its last attempt")
}

/// Everything `program info` lists.
fn info(program: &OsStr) -> Result<String, String> {
    let output = Command::new(program)
        .arg("info")
        .output()
        .map_err(|error| format!("hdiutil info failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "hdiutil info failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("hdiutil info failed: {error}"))
}

/// Whether `listing`, an `hdiutil info` text, lists `image`. `target` is what
/// the detach names: the image's whole-disk node, or a mount point of it. An
/// image listed without `target` is an error.
fn listed_on(listing: &str, image: &Path, target: &OsStr) -> Result<bool, String> {
    let Some(section) = listing
        .split("================================================")
        .find(|section| {
            section.lines().any(|line| {
                line.strip_prefix("image-path")
                    .and_then(|field| field.split_once(": "))
                    .is_some_and(|(_, path)| Path::new(path.trim()) == image)
            })
        })
    else {
        return Ok(false);
    };
    // One line per node, the whole disk first: node, content, mount point.
    let nodes: Vec<&str> = section
        .lines()
        .filter(|line| line.starts_with("/dev/disk"))
        .collect();
    let target = Path::new(target);
    let whole = nodes
        .first()
        .and_then(|line| line.split_whitespace().next());
    // `hdiutil info` may list a mount point with its symbolic links resolved.
    let resolved = std::fs::canonicalize(target).ok();
    let mounted = nodes
        .iter()
        .filter_map(|line| line.split('\t').nth(2))
        .map(|mount| Path::new(mount.trim()))
        .any(|mount| mount == target || Some(mount) == resolved.as_deref());
    if whole.map(Path::new) == Some(target) || mounted {
        return Ok(true);
    }
    Err(format!(
        "{} is no longer listed on {}; no detach run:\n{listing}",
        image.display(),
        target.display()
    ))
}

/// Detach what one attach reported: its whole-disk `device` node when attach
/// named one, otherwise the `mount` point, which detaches the same image.
pub fn detach_attached(
    program: &OsStr,
    image: &Path,
    device: Option<&str>,
    mount: &OsStr,
) -> Result<(), String> {
    match device {
        Some(node) => detach(program, image, OsStr::new(node)),
        None => detach(program, image, mount),
    }
}

/// One DiskImages attachment owned by one test. Dropping it detaches the
/// whole-disk node `hdiutil attach` reported, or the mount point when attach
/// named none, on every exit: a normal return, and a panic from an assertion.
/// Then it removes the scratch directory.
pub struct AttachedImage {
    program: std::ffi::OsString,
    root: std::path::PathBuf,
    // Set once attach has succeeded, so cleanup runs on every later exit.
    mount: Option<std::path::PathBuf>,
    image: std::path::PathBuf,
    device: Option<String>,
}

impl AttachedImage {
    /// An image that has not attached yet. Dropping it detaches nothing.
    pub fn new(program: impl Into<std::ffi::OsString>, root: std::path::PathBuf) -> Self {
        Self {
            program: program.into(),
            root,
            mount: None,
            image: Default::default(),
            device: None,
        }
    }

    /// Record a successful attach of `image`, before anything else can fail.
    /// `image` is the path `hdiutil info` lists it by.
    pub fn attached(
        &mut self,
        image: std::path::PathBuf,
        mount: std::path::PathBuf,
        device: Option<String>,
    ) {
        self.image = image;
        self.mount = Some(mount);
        self.device = device;
    }
}

impl Drop for AttachedImage {
    fn drop(&mut self) {
        if let Some(mount) = self.mount.take() {
            let device = self.device.take();
            if let Err(error) = detach_attached(
                self.program.as_os_str(),
                &self.image,
                device.as_deref(),
                mount.as_os_str(),
            ) {
                let target = device.unwrap_or_else(|| mount.display().to_string());
                let message =
                    format!("detach of {target} failed; the image stays attached: {error}");
                // A second panic during unwinding would abort before the
                // first failure reports, so report it and let that stand.
                if std::thread::panicking() {
                    eprintln!("{message}");
                    return;
                }
                panic!("{message}");
            }
        }
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

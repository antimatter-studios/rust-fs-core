use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAGIC: &str = "fs-core-owned-hdiutil-v1";
static ATTACHMENTS: OnceLock<Mutex<Vec<Record>>> = OnceLock::new();
static NEXT_IMAGE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    fn geteuid() -> u32;
    fn kill(pid: i32, signal: i32) -> i32;
}

fn uid() -> u32 {
    unsafe { geteuid() }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

fn registry() -> MutexGuard<'static, Vec<Record>> {
    ATTACHMENTS
        .get_or_init(|| {
            let signal = Arc::new(AtomicUsize::new(0));
            for number in [
                signal_hook::consts::SIGTERM,
                signal_hook::consts::SIGINT,
                signal_hook::consts::SIGHUP,
            ] {
                signal_hook::flag::register_usize(number, Arc::clone(&signal), number as usize)
                    .expect("install owned DiskImages teardown signal handler");
            }
            std::thread::spawn(move || loop {
                let number = signal.load(Ordering::SeqCst);
                if number != 0 {
                    let records = registry();
                    for record in records.iter() {
                        let outcome = match record.cleanup() {
                            Ok(true) => "detached".to_owned(),
                            Ok(false) => "not listed by hdiutil info, no detach run".to_owned(),
                            Err(error) => format!("failed: {error}"),
                        };
                        eprintln!(
                            "owned DiskImages signal {number} cleanup of {}: {outcome} ({})",
                            record.device,
                            record.image.display()
                        );
                    }
                    std::process::exit(128 + number as i32);
                }
                std::thread::sleep(Duration::from_millis(10));
            });
            Mutex::new(Vec::new())
        })
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn private_directory(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.is_dir() && metadata.uid() == uid() && metadata.mode() & 0o077 == 0)
}

fn scope_lock(root: &Path) -> io::Result<File> {
    let path = root.join(".lock");
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if !metadata.is_file() || metadata.uid() != uid() || metadata.mode() & 0o077 != 0 {
            return Err(invalid(
                "DiskImages scope lock is not a private regular file",
            ));
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

fn whole_device(device: &str) -> bool {
    device.strip_prefix("/dev/disk").is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    }) && !std::env::var("FS_CORE_HDIUTIL_FORBIDDEN_DEVICES")
        .unwrap_or_default()
        .split(':')
        .filter(|prefix| !prefix.is_empty())
        .any(|prefix| device.starts_with(prefix))
}

fn owner_pid(name: &str) -> Option<u32> {
    let mut parts = name.strip_prefix("fs_core_attach_")?.split('_');
    let pid: u32 = parts.next()?.parse().ok()?;
    let _: u128 = parts.next()?.parse().ok()?;
    let _: usize = parts.next()?.parse().ok()?;
    (parts.next().is_none() && pid > 1 && pid <= i32::MAX as u32).then_some(pid)
}

fn dead(pid: u32) -> bool {
    unsafe { kill(pid as i32, 0) == -1 && io::Error::last_os_error().raw_os_error() == Some(3) }
}

#[derive(Clone)]
struct Record {
    dir: PathBuf,
    image: PathBuf,
    device: String,
    file_device: u64,
    file_inode: u64,
}

impl Record {
    fn write(&self) -> io::Result<()> {
        let mut owner = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.dir.join(".owner"))?;
        writeln!(
            owner,
            "{MAGIC}\n{}\n{}\n{}\n{}\n{}",
            uid(),
            std::process::id(),
            self.image.file_name().unwrap().to_str().unwrap(),
            self.file_device,
            self.file_inode,
        )?;
        owner.sync_all()
    }

    fn read(dir: &Path, pid: u32) -> io::Result<Option<Self>> {
        if !private_directory(dir)? {
            return Ok(None);
        }
        let owner = dir.join(".owner");
        let metadata = match fs::symlink_metadata(&owner) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !metadata.is_file()
            || metadata.uid() != uid()
            || metadata.mode() & 0o077 != 0
            || metadata.len() > 4096
        {
            return Ok(None);
        }
        let text = fs::read_to_string(owner)?;
        let fields: Vec<_> = text.lines().collect();
        if fields.len() != 6
            || fields[0] != MAGIC
            || fields[1].parse::<u32>().ok() != Some(uid())
            || fields[2].parse::<u32>().ok() != Some(pid)
            || !matches!(fields[3], "raw.img" | "volume.dmg" | "oracle.sparseimage")
        {
            return Ok(None);
        }
        let (Ok(file_device), Ok(file_inode)) = (fields[4].parse(), fields[5].parse()) else {
            return Ok(None);
        };
        let output_path = dir.join(".attach.stdout");
        let output_metadata = match fs::symlink_metadata(&output_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !output_metadata.is_file()
            || output_metadata.uid() != uid()
            || output_metadata.mode() & 0o077 != 0
            || output_metadata.len() > 4096
        {
            return Ok(None);
        }
        let output = fs::read_to_string(output_path)?;
        let Some(device) = output
            .split_whitespace()
            .find(|word| word.starts_with("/dev/disk"))
        else {
            return Ok(None);
        };
        if !whole_device(device) {
            return Ok(None);
        }
        let record = Self {
            dir: dir.to_path_buf(),
            image: dir.join(fields[3]),
            device: device.to_owned(),
            file_device,
            file_inode,
        };
        Ok(record.same_image()?.then_some(record))
    }

    fn same_image(&self) -> io::Result<bool> {
        match fs::symlink_metadata(&self.image) {
            Ok(metadata) => Ok(metadata.is_file()
                && metadata.uid() == uid()
                && metadata.dev() == self.file_device
                && metadata.ino() == self.file_inode),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error),
        }
    }

    fn attached(&self) -> io::Result<bool> {
        let output = Command::new("/usr/bin/hdiutil").arg("info").output()?;
        if !output.status.success() {
            return Err(invalid("hdiutil info failed during owned-image teardown"));
        }
        let info = String::from_utf8(output.stdout).map_err(|error| invalid(error.to_string()))?;
        for section in info.split("================================================") {
            let ours = section.lines().any(|line| {
                line.strip_prefix("image-path")
                    .and_then(|field| field.split_once(": "))
                    .is_some_and(|(_, path)| Path::new(path.trim()) == self.image)
            });
            if !ours {
                continue;
            }
            let device = section
                .lines()
                .filter_map(|line| line.split_whitespace().next())
                .find(|word| word.starts_with("/dev/disk"));
            if device != Some(self.device.as_str()) {
                return Err(invalid(
                    "owned image no longer names its recorded attach node",
                ));
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Whether a detach ran; see `detach_locked`.
    fn cleanup(&self) -> io::Result<bool> {
        let _lock = scope_lock(self.dir.parent().unwrap())?;
        self.cleanup_locked()
    }

    fn cleanup_locked(&self) -> io::Result<bool> {
        let detached = self.detach_locked()?;
        match fs::remove_file(&self.image) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::remove_file(self.dir.join(".attach.stdout"))?;
        fs::remove_file(self.dir.join(".owner"))?;
        fs::remove_dir(&self.dir)?;
        Ok(detached)
    }

    /// `Ok(true)` when the image was listed and is now detached, `Ok(false)`
    /// when `hdiutil info` did not list it and no detach ran.
    fn detach_locked(&self) -> io::Result<bool> {
        if self.device.is_empty() {
            let pid = self
                .dir
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(owner_pid)
                .ok_or_else(|| invalid("pending attach has no valid owner PID"))?;
            return Self::read(&self.dir, pid)?
                .ok_or_else(|| {
                    invalid("pending attach has no identifiable node; preserving fixture")
                })?
                .detach_locked();
        }
        if !whole_device(&self.device) || !self.same_image()? {
            return Err(invalid("owned image identity changed; refusing teardown"));
        }
        let listed = self.attached()?;
        if listed {
            for attempt in 0..=5 {
                let mut command = Command::new("/usr/bin/hdiutil");
                command.arg("detach");
                if attempt != 0 {
                    command.arg("-force");
                }
                let output = command.arg(&self.device).output()?;
                if output.status.success() {
                    break;
                }
                if !String::from_utf8_lossy(&output.stderr)
                    .to_ascii_lowercase()
                    .contains("resource busy")
                    || attempt == 5
                {
                    return Err(invalid(format!("owned hdiutil detach failed: {output:?}")));
                }
                std::thread::sleep(Duration::from_millis(200 * (attempt + 1)));
            }
            if self.attached()? {
                return Err(invalid(
                    "owned image remains attached after detach succeeded",
                ));
            }
        }
        Ok(listed)
    }
}

pub struct OwnedImage {
    pub dir: PathBuf,
    pub path: PathBuf,
    record: Option<Record>,
}

impl OwnedImage {
    pub fn new(scope: &str, filename: &str) -> Self {
        assert!(matches!(scope, "device-node" | "mounted"));
        assert!(matches!(
            filename,
            "raw.img" | "volume.dmg" | "oracle.sparseimage"
        ));
        let root = std::env::temp_dir().join(format!("fs_core_attach_{scope}_v1"));
        match DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("create private DiskImages scope: {error}"),
        }
        assert!(
            private_directory(&root).unwrap(),
            "DiskImages scope must be private"
        );
        let root = fs::canonicalize(root).unwrap();
        let _lock = scope_lock(&root).expect("lock owned DiskImages scope");
        for entry in fs::read_dir(&root).unwrap() {
            let entry = entry.unwrap();
            let Some(pid) = entry.file_name().to_str().and_then(owner_pid) else {
                continue;
            };
            if dead(pid) {
                if let Some(record) = Record::read(&entry.path(), pid).unwrap() {
                    record
                        .cleanup_locked()
                        .expect("recover dead owner's DiskImages fixture");
                }
            }
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = root.join(format!(
            "fs_core_attach_{}_{nonce}_{}",
            std::process::id(),
            NEXT_IMAGE.fetch_add(1, Ordering::Relaxed),
        ));
        DirBuilder::new().mode(0o700).create(&dir).unwrap();
        Self {
            path: dir.join(filename),
            dir,
            record: None,
        }
    }

    pub fn attach(&mut self, command: &mut Command) -> Output {
        let mut records = registry();
        let metadata = fs::symlink_metadata(&self.path).unwrap();
        let mut record = Record {
            dir: self.dir.clone(),
            image: self.path.clone(),
            device: String::new(),
            file_device: metadata.dev(),
            file_inode: metadata.ino(),
        };
        record
            .write()
            .expect("persist ownership before hdiutil attach");
        self.record = Some(record.clone());
        records.push(record.clone());
        let stdout_path = self.dir.join(".attach.stdout");
        let stdout = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&stdout_path)
            .unwrap();
        let mut output = command
            .stdout(Stdio::from(stdout.try_clone().unwrap()))
            .output()
            .expect("run owned hdiutil attach");
        stdout
            .sync_all()
            .expect("persist hdiutil's own attach receipt");
        output.stdout = fs::read(stdout_path).unwrap();
        if let Some(device) = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .find(|word| word.starts_with("/dev/disk"))
        {
            assert!(
                whole_device(device),
                "hdiutil returned a forbidden or non-whole device"
            );
            record.device = device.to_owned();
            self.record = Some(record.clone());
            let entry = records
                .iter_mut()
                .find(|entry| entry.dir == record.dir)
                .unwrap();
            *entry = record;
        }
        output
    }

    pub fn detach(&mut self) -> io::Result<()> {
        let _records = registry();
        if let Some(record) = &self.record {
            let _lock = scope_lock(record.dir.parent().unwrap())?;
            record.detach_locked()?;
        }
        Ok(())
    }

    fn cleanup(&mut self) -> io::Result<()> {
        self.detach()?;
        let mut records = registry();
        if let Some(record) = &self.record {
            record.cleanup()?;
            records.retain(|entry| entry.dir != record.dir);
            self.record = None;
        }
        Ok(())
    }
}

impl Drop for OwnedImage {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            if std::thread::panicking() {
                eprintln!("owned DiskImages panic cleanup failed: {error}");
            } else {
                panic!("owned DiskImages teardown failed: {error}");
            }
        }
    }
}

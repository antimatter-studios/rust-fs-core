//! Stderr-only progress; the content writer and final report never reach here.
use super::*;
use std::time::{Duration, Instant};

/// Progress behavior kept separate so existing option literals still compile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProgressOptions {
    /// Emit JSON lines even when stderr is not a terminal.
    pub progress: bool,
}
impl ProgressOptions {
    /// Parse [`export_args_with_progress`]; older argument sets select auto mode.
    pub fn from_matches(m: &ArgMatches) -> Self {
        Self {
            progress: m
                .try_get_one::<bool>("progress")
                .ok()
                .flatten()
                .copied()
                .unwrap_or(false),
        }
    }
}
/// All tree, link and time arguments plus opt-in progress on stderr.
pub fn export_args_with_progress() -> [Arg; 8] {
    let [path, dest, replace, verify, recursive, depth, preserve] = export_args_with_times();
    [
        path,
        dest,
        replace,
        verify,
        recursive,
        depth,
        preserve,
        Arg::new("progress")
            .long("progress")
            .action(ArgAction::SetTrue)
            .help("Report progress on stderr: a terminal line, or throttled JSON lines when piped"),
    ]
}

/// Progress for one transfer, writing only to the supplied stderr stream.
/// Terminal mode rewrites one line; pipes are silent unless explicitly enabled.
/// JSON emits one completion line per file and at most one intermediate line
/// per second. Unknown totals are null. Counters describe delivered logical
/// file bytes, including preserved hard links, not physical device I/O.
/// Diagnostic write failures never change the content transfer's result.
pub struct Progress<'a> {
    stderr: &'a mut dyn Write,
    terminal: bool,
    enabled: bool,
    path: String,
    file_bytes: u64,
    file_total: Option<u64>,
    bytes: u64,
    files: u64,
    total_bytes: Option<u64>,
    total_files: Option<u64>,
    last: Instant,
    rendered: bool,
}
impl<'a> Progress<'a> {
    /// `terminal` must describe stderr, independently of stdout's destination.
    pub fn new(stderr: &'a mut dyn Write, terminal: bool, options: ProgressOptions) -> Self {
        Self {
            stderr,
            terminal,
            enabled: terminal || options.progress,
            path: String::new(),
            file_bytes: 0,
            file_total: None,
            bytes: 0,
            files: 0,
            total_bytes: None,
            total_files: None,
            last: Instant::now(),
            rendered: false,
        }
    }
    /// Set totals obtained before reading content; use None for unknown values.
    pub fn set_totals(&mut self, bytes: Option<u64>, files: Option<u64>) {
        self.total_bytes = bytes;
        self.total_files = files;
    }
    /// Begin a file. The size may be unknown, as for stdin.
    pub fn begin_file(&mut self, path: &Path, bytes: Option<u64>) {
        self.path = path.to_string_lossy().into_owned();
        self.file_bytes = 0;
        self.file_total = bytes;
        self.last = Instant::now();
        if self.terminal {
            self.render(false);
        }
    }
    /// Update the current file's cumulative successfully delivered byte count.
    pub fn advance(&mut self, bytes: u64) {
        self.bytes = self
            .bytes
            .saturating_add(bytes.saturating_sub(self.file_bytes));
        self.file_bytes = bytes;
        if self.file_total != Some(bytes) && self.last.elapsed() >= Duration::from_secs(1) {
            self.render(false);
        }
    }
    /// Finish a file, including a failed attempt without counting it as complete.
    pub fn finish_file(&mut self, complete: bool) {
        if complete {
            self.files = self.files.saturating_add(1);
        }
        self.render(complete);
    }
    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }
    fn render(&mut self, complete: bool) {
        if !self.enabled {
            return;
        }
        if self.terminal {
            let total = |n: Option<u64>| n.map_or_else(|| "?".into(), |n| n.to_string());
            // Escape terminal control characters, including newlines in filenames.
            let path: String = self.path.chars().flat_map(char::escape_default).collect();
            let _ = write!(
                self.stderr,
                "\r\x1b[2K{path}: {}/{} bytes; {}/{} bytes; {}/{} files",
                self.file_bytes,
                total(self.file_total),
                self.bytes,
                total(self.total_bytes),
                self.files,
                total(self.total_files)
            );
        } else {
            let number = |n: Option<u64>| n.map_or(Json::Null, Json::from);
            let json = Json::object([
                ("path", Json::from(self.path.clone())),
                ("file_bytes", Json::from(self.file_bytes)),
                ("file_total", number(self.file_total)),
                ("bytes", Json::from(self.bytes)),
                ("total_bytes", number(self.total_bytes)),
                ("files", Json::from(self.files)),
                ("total_files", number(self.total_files)),
                ("complete", Json::from(complete)),
            ]);
            let _ = writeln!(self.stderr, "{}", json.to_compact());
        }
        let _ = self.stderr.flush();
        self.last = Instant::now();
        self.rendered = true;
    }
}
impl Drop for Progress<'_> {
    fn drop(&mut self) {
        if self.terminal && self.rendered {
            let _ = writeln!(self.stderr);
        }
    }
}

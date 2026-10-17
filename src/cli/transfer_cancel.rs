//! Signal handlers only record the first signal; normal transfer code cleans up.
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering::SeqCst},
    Arc,
};

#[cfg(unix)]
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

// signal-hook leaves its OS handler installed after unregistering actions.
// Keep one fallback per signal so an idle CLI still terminates normally.
#[cfg(unix)]
fn defaults_when_idle() -> Result<(), CliError> {
    static INSTALLED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    INSTALLED
        .get_or_init(|| {
            for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
                // SAFETY: atomic load and signal-hook's async-signal-safe default
                // emulation are the only operations performed by this handler.
                unsafe {
                    signal_hook::low_level::register(signal, move || {
                        if ACTIVE.load(SeqCst) == 0 {
                            let _ = signal_hook::low_level::emulate_default_handler(signal);
                        }
                    })
                }
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .as_ref()
        .map(|_| ())
        .map_err(|e| CliError::failed(format!("install transfer signal defaults: {e}")))
}

pub(super) struct Cancellation {
    signal: Arc<AtomicUsize>,
    #[cfg(unix)]
    handlers: Vec<signal_hook::SigId>,
}
impl Cancellation {
    pub(super) fn install() -> Result<Self, CliError> {
        #[cfg(unix)]
        {
            defaults_when_idle()?;
            ACTIVE.fetch_add(1, SeqCst);
        }
        let this = Self {
            signal: Arc::new(AtomicUsize::new(0)),
            #[cfg(unix)]
            handlers: Vec::new(),
        };
        #[cfg(unix)]
        {
            let mut this = this;
            for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
                let flag = Arc::clone(&this.signal);
                // SAFETY: the handler only touches a lock-free atomic. It does
                // not allocate, lock, format output or invoke filesystem code.
                let id = unsafe {
                    signal_hook::low_level::register(signal, move || {
                        let _ = flag.compare_exchange(0, signal as usize, SeqCst, SeqCst);
                    })
                }
                .map_err(|e| CliError::failed(format!("install transfer signal handler: {e}")))?;
                this.handlers.push(id);
            }
            Ok(this)
        }
        #[cfg(not(unix))]
        Ok(this)
    }
    pub(super) fn check(&self, bytes: u64, total: u64) -> Result<(), CliError> {
        let signal = self.signal.load(SeqCst);
        if signal == 0 {
            return Ok(());
        }
        Err(CliError::failed(format!(
            "transfer cancelled by signal {signal} with {bytes} of {total} bytes delivered"
        ))
        .with_code(128 + signal as u8))
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            for id in self.handlers.drain(..) {
                signal_hook::low_level::unregister(id);
            }
            ACTIVE.fetch_sub(1, SeqCst);
        }
    }
}
pub(super) fn is_cancelled(error: &CliError) -> bool {
    matches!(error.code, 130 | 143)
}

pub(super) fn cancelled_file(request: &ExportRequest, error: CliError) -> Outcome {
    let path = request.path.to_string_lossy().into_owned();
    let destination = match &request.output {
        Output::File(path) => path.display().to_string(),
        Output::Stdout => "-".into(),
    };
    Outcome::report(Json::object([
        ("cancelled", Json::from(true)),
        ("signal", Json::from(u64::from(error.code - 128))),
        ("completed", Json::Arr(Vec::new())),
        (
            "in_flight",
            Json::object([
                ("path", Json::from(path)),
                ("destination", Json::from(destination)),
                ("reason", Json::from(error.message)),
            ]),
        ),
        ("pending", Json::Arr(Vec::new())),
    ]))
    .with_code(error.code)
}

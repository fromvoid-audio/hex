//! Shared scan deadlines, modes, cancellation tokens, and diagnostics.
use super::*;

/// Per-bundle scan deadline. Interactive scans let the native plug-in run
/// until it returns or the user cancels. The scanner still bounds preflight
/// and the wait for the helper to exit after its result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanDeadline {
    pub(super) millis: u32,
    pub(super) wait_for_plugin: bool,
}

impl ScanDeadline {
    /// Reject values the bundled scanner cannot accept.
    pub fn from_millis(milliseconds: u32) -> Result<Self, &'static str> {
        if !(1..=30_000).contains(&milliseconds) {
            return Err("scan deadline must be 1 to 30000 milliseconds");
        }
        Ok(Self {
            millis: milliseconds,
            wait_for_plugin: false,
        })
    }

    pub fn interactive() -> Self {
        Self {
            wait_for_plugin: true,
            ..Self::default()
        }
    }

    pub(super) fn outer_limit(self) -> Duration {
        Duration::from_millis(u64::from(self.millis)) + Duration::from_secs(2)
    }

    pub(super) fn expired(self, elapsed: Duration) -> bool {
        !self.wait_for_plugin && elapsed > self.outer_limit()
    }
}

impl Default for ScanDeadline {
    fn default() -> Self {
        Self {
            millis: 10_000,
            wait_for_plugin: false,
        }
    }
}

/// Policy for a trusted folder scan. Selected-bundle scans always run discovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanMode {
    /// Reuse verified unchanged, cacheable results and skip remembered failures.
    Incremental,
    /// Run discovery for every eligible bundle, including remembered failures.
    Full,
}

pub(super) struct Operation {
    pub(super) phase: AtomicU8,
    pub(super) scanner_started: AtomicBool,
    pub(super) current_module: Mutex<Option<PathBuf>>,
}
/// UI/control owner. Dropping a view requests cancellation without waiting.
pub struct Cancellation(pub(super) Arc<Operation>);
impl Default for Cancellation {
    fn default() -> Self {
        Self(Arc::new(Operation {
            phase: AtomicU8::new(RUNNING),
            scanner_started: AtomicBool::new(false),
            current_module: Mutex::new(None),
        }))
    }
}
impl Cancellation {
    pub fn token(&self) -> Token {
        Token(Arc::clone(&self.0))
    }
    /// False means publication has already started or the operation is finished.
    pub fn cancel(&self) -> bool {
        matches!(
            self.0
                .phase
                .compare_exchange(RUNNING, CANCELLED, Ordering::AcqRel, Ordering::Acquire),
            Ok(_) | Err(CANCELLED)
        )
    }
    /// Bundle currently being inspected or executed, for UI progress only.
    pub fn current_module(&self) -> Option<PathBuf> {
        self.0.current_module.lock().ok()?.clone()
    }

    /// Historical observation of this operation's actual outer scanner spawn.
    /// This proves neither current liveness nor native child entry/retirement.
    pub fn has_started_scanner(&self) -> bool {
        self.0.scanner_started.load(Ordering::Acquire)
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}
pub struct Token(pub(super) Arc<Operation>);
impl Token {
    pub(super) fn set_current_module(&self, path: Option<&Path>) {
        if let Ok(mut current) = self.0.current_module.lock() {
            *current = path.map(Path::to_path_buf);
        }
    }

    pub(super) fn cancelled(&self) -> bool {
        self.0.phase.load(Ordering::Acquire) == CANCELLED
    }
    pub(super) fn begin_publication(&self) -> Result<bool, String> {
        match self.0.phase.compare_exchange(
            RUNNING,
            COMMITTING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(true),
            Err(CANCELLED) => {
                self.0.phase.store(COMMITTING, Ordering::Release);
                Ok(false)
            }
            Err(_) => Err("scan publication state invalid".into()),
        }
    }
}
impl Drop for Token {
    fn drop(&mut self) {
        self.0.phase.store(FINISHED, Ordering::Release);
    }
}

/// Copied observations, never child custody or permission to publish metadata.
/// Keeps the original scanner output buffer (at most OUTPUT_LIMIT logical bytes),
/// rather than cloning decoded metadata. Outer exit and reported native cleanup
/// stay distinct; malformed output remains available without implying cleanup.
#[derive(Default)]
pub struct ScanDiagnostics {
    pub module: PathBuf,
    pub elapsed: Duration,
    pub scanner_started: bool,
    pub scanner_exit: Option<std::process::ExitStatus>,
    pub native_cleanup: Option<Cleanup>,
    pub report: Vec<u8>,
    pub stderr: Vec<u8>,
}
impl std::fmt::Debug for ScanDiagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanDiagnostics")
            .field("module", &self.module)
            .field("elapsed", &self.elapsed)
            .field("scanner_started", &self.scanner_started)
            .field("scanner_exit", &self.scanner_exit)
            .field("native_cleanup", &self.native_cleanup)
            .field("report", &String::from_utf8_lossy(&self.report))
            .field("stderr", &String::from_utf8_lossy(&self.stderr))
            .finish()
    }
}

pub struct Update {
    pub catalog: Option<Catalog>,
    pub status: String,
    pub diagnostics: Option<ScanDiagnostics>,
}

/// RAII guard keeping `Cancellation::current_module()` updated for the active
/// bundle and clearing it automatically on completion, error, or cancellation.
pub(super) struct CurrentModuleGuard<'a>(&'a Token);

impl<'a> CurrentModuleGuard<'a> {
    pub(super) fn enter(token: &'a Token, module: &Path) -> Self {
        token.set_current_module(Some(module));
        Self(token)
    }
}

impl Drop for CurrentModuleGuard<'_> {
    fn drop(&mut self) {
        self.0.set_current_module(None);
    }
}

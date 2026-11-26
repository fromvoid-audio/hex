//! Source validation, helper timeout limits, and cancellation tokens for native hosting.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileFingerprint {
    pub(super) dev: u64,
    pub(super) ino: u64,
    pub(super) len: u64,
    pub(super) mtime: i64,
    pub(super) mtime_nsec: i64,
    pub(super) ctime: i64,
    pub(super) ctime_nsec: i64,
}

impl FileFingerprint {
    pub(super) fn capture(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
            ctime: meta.ctime(),
            ctime_nsec: meta.ctime_nsec(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Source {
    pub(super) bundle: Bundle,
    pub(super) executable: Executable,
    pub(super) identity: Identity,
    pub(super) info_fingerprint: Option<FileFingerprint>,
    pub(super) executable_fingerprint: Option<FileFingerprint>,
    pub(super) grouped_editor_callbacks: bool,
    pub(super) transport_space: bool,
    pub(super) live_deadline: bool,
}
impl Source {
    pub fn inspect_on_control(path: &Path, executable: &str, class: ClassId) -> Result<Self> {
        let bundle = Bundle::inspect(path).map_err(Error::Source)?;
        let executable = bundle.admit(executable).map_err(Error::Source)?;
        let info_fingerprint =
            FileFingerprint::capture(&bundle.canonical_path.join("Contents/Info.plist"));
        let executable_fingerprint =
            FileFingerprint::capture(&bundle.canonical_path.join(&executable.relative_path));
        let identity = Identity {
            class,
            binary: BinaryIdentity {
                executable_sha256: digest(&executable.sha256)?,
                info_sha256: digest(&bundle.info_sha256)?,
            },
        };
        Ok(Self {
            bundle,
            executable,
            identity,
            info_fingerprint,
            executable_fingerprint,
            grouped_editor_callbacks: false,
            transport_space: false,
            live_deadline: false,
        })
    }
    /// Explicit opt-in on the exact inspected source before the SDK creates
    /// its component handler. This enables callbacks, not recording custody.
    pub fn with_grouped_editor_callbacks(mut self) -> Self {
        self.grouped_editor_callbacks = true;
        self
    }
    pub fn with_transport_space(mut self) -> Self {
        self.transport_space = true;
        self
    }
    /// Request paired scheduling for a live source on an admitted output.
    /// Unsupported periods or exhausted capacity leave the source ordinary.
    pub fn with_live_audio_deadline(mut self, requested: bool) -> Self {
        self.live_deadline = requested;
        self
    }
    pub fn identity(&self) -> Identity {
        self.identity
    }
    /// Fast identity check avoiding redundant multi-MB Mach-O disk SHA-256 reads
    /// once `inspect_on_control` has already hashed both `Info.plist` and the
    /// executable. Verifies canonical bundle path and exact `stat` fingerprints
    /// (`dev`, `ino`, `len`, `mtime`, `ctime`); falls back to full SHA-256
    /// revalidation on any metadata change so tampering is always refused with
    /// the exact validation error.
    pub(super) fn unchanged_fast(&self) -> Result<()> {
        let current = std::fs::canonicalize(&self.bundle.supplied_path).map_err(|_| {
            Error::Validation(hex_plugin_scan::macos::validation::Error::Invalid(
                "selected bundle identity changed",
            ))
        })?;
        if current != self.bundle.canonical_path {
            return self.unchanged();
        }
        let info_matches = self.info_fingerprint.is_some()
            && FileFingerprint::capture(&current.join("Contents/Info.plist"))
                == self.info_fingerprint;
        let exec_matches = self.executable_fingerprint.is_some()
            && FileFingerprint::capture(&current.join(&self.executable.relative_path))
                == self.executable_fingerprint;
        if info_matches && exec_matches {
            return Ok(());
        }
        self.unchanged()
    }
    pub(super) fn unchanged(&self) -> Result<()> {
        self.bundle
            .executable_unchanged(&self.executable)
            .map_err(Error::Validation)
    }
}

/// Optional observation of the synchronous control-side startup. The caller
/// supplies its separate selection-inspection interval. Absent child intervals
/// mean the corresponding handoff never completed.
#[derive(Default)]
pub struct StartupTimings {
    pub selection_inspection: Duration,
    pub revalidation: Duration,
    pub waitable_probe: Option<Duration>,
    pub instance_to_facts: Option<Duration>,
}

pub(super) fn measure_revalidation<T>(
    total: Option<&mut Duration>,
    validate: impl FnOnce() -> T,
) -> T {
    let Some(total) = total else {
        return validate();
    };
    let started = Instant::now();
    let result = validate();
    *total = total.saturating_add(started.elapsed());
    result
}

pub(super) fn digest(text: &str) -> Result<[u8; 32]> {
    if text.len() != 64 {
        return Err(Error::Protocol("source digest width"));
    }
    let mut out = [0; 32];
    for (n, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        let number = |c: u8| match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            _ => Err(Error::Protocol("source digest encoding")),
        };
        out[n] = number(pair[0])? * 16 + number(pair[1])?;
    }
    Ok(out)
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub startup: Duration,
    pub exchange: Duration,
    pub retirement: Duration,
}
impl Limits {
    pub(super) fn validate(self) -> Result<()> {
        if [self.startup, self.exchange, self.retirement]
            .iter()
            .any(|v| v.is_zero() || *v > Duration::from_secs(30))
        {
            return Err(Error::Protocol(
                "supervision limits must be positive and at most30 seconds",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Preparation {
    pub binding: Binding,
    pub sample_rate: f64,
    pub quantum: u16,
    pub input_channels: u8,
    pub output_channels: u8,
    pub limits: Limits,
}
/// Known-presentation convenience preparation uses the same two-phase protocol.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub preparation: Preparation,
    pub presentation: PresentationLatency,
}

/// Explicitly trusted user-selected module. This is caller consent, not a source
/// authenticity or native resource/publication permit.
pub enum TrustUserCode {
    Acknowledged,
}
/// Construct on control before starting preparation; retain its token for load
/// cancellation. The unique receiver can be moved into exactly one helper.
pub struct PreparationCancellation {
    pub(super) token: Cancellation,
    pub(super) receiver: UnixStream,
}
impl PreparationCancellation {
    pub fn new_on_control() -> Result<Self> {
        let (sender, receiver) = UnixStream::pair().map_err(Error::Io)?;
        // Only the parent reads shortcuts and shuts down its write half. The
        // helper's distinct socket still blocks in its cancellation guardian.
        sender.set_nonblocking(true).map_err(Error::Io)?;
        Ok(Self {
            token: Cancellation {
                stream: Arc::new(sender),
                cancelled: Arc::new(AtomicBool::new(false)),
            },
            receiver,
        })
    }
    pub fn token_on_control(&self) -> Cancellation {
        self.token.clone()
    }
}
#[derive(Clone)]
pub struct Cancellation {
    pub(super) stream: Arc<UnixStream>,
    pub(super) cancelled: Arc<AtomicBool>,
}
impl Cancellation {
    /// One editor Space token on the helper-to-parent half of this exact
    /// source's socket. The parent-to-helper half remains cancellation only.
    pub fn take_transport_space_on_control(&self) -> bool {
        let mut token = [0];
        !self.cancelled.load(Ordering::Acquire)
            && self.stream.as_ref().read(&mut token).ok() == Some(1)
            && token == [1]
    }
    /// Test-only lifetime observation; never resource provenance or admission.
    /// A weak observer cannot keep the descriptor payload alive.
    #[cfg(feature = "test-support")]
    pub fn descriptor_lifetime_probe_on_control(&self) -> std::sync::Weak<UnixStream> {
        Arc::downgrade(&self.stream)
    }
    /// Control-only, one-shot and independent of the PCM queue. The guardian's
    /// blocking read wakes on descriptor shutdown, even during foreign entry.
    pub fn cancel_on_control(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.stream.shutdown(std::net::Shutdown::Write);
    }
    /// Whether cancellation has been requested on control for this guardian.
    pub fn is_cancelled_on_control(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub(super) fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Retirement {
    pub status: ExitStatus,
    pub terminal: Option<Terminal>,
}

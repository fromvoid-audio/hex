//! # Purpose
//! Own the control/worker client for one source-bound ordinary VST3 instance.
//! The SDK component/controller remain together in the existing native helper.
//! Its independent cancellation guardian starts before foreign code, so control
//! can retire even a startup-hung instance without signalling numeric PIDs.
//! A real child-custody preflight refuses unsupported inherited auto-reap;
//! the client never changes caller signal dispositions.
//! Optional source-bound startup restore precedes configuration and raw facts.
//! Facts precede presentation/final state capture/processing finalization. The
//! pending type cannot exchange PCM or grant graph/canonical acceptance.
//! Configuration, prepared state and PCM are copied and strictly bounded.
//! A serialized quiescent owner can retain the same loaded component/controller
//! through a newer epoch. Actual stop/facts/restart acknowledgements fence that
//! transition; original startup capture stays historical. Old copied queues and
//! graph/callback custody remain the audio owner's separate responsibility.
//!
//! # Dependencies
//! Uses hex-plugin-scan's fresh source/helper provenance, hex-plugin-state's
//! strict provider codec and hex-lattice-contract's immutable state envelope.
//! Owns the bounded copied transport vocabulary/rings independently of native
//! output. std and rustix event-only readiness serve control/worker I/O;
//! there are no process-global handlers or rustix process APIs in this client.
//!
//! # Does not own
//! No SDK ABI/pointers, unsafe Rust, native device/callback, worker scheduling, canonical
//! edits/history, graph/publication/retirement or plugin admission authority.
//! All client I/O, cancellation, waiting and destruction belong on control or
//! the existing joining transport worker, never an audio callback. Ready is a
//! prepared instance fact, not canonical/audio application. Numeric child IDs
//! are never used after reap/unknown custody; no descendant containment is claimed.
//! Retained preparation and epoch failures preserve their actual child lease for Root's
//! independent disposal polling. Synchronous convenience APIs join before
//! returning an error; neither path grants graph/callback retirement authority.

#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), all(test, unix)))]
mod channel;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod native;
pub mod protocol;

pub type Result<T> = std::result::Result<T, Error>;
/// Actual native preparation/retirement paths retain at most two original
/// boxed causes and one shared lost-custody I/O payload. The enclosing Error
/// values are inline in their owners. This is known DATA, not a bound on
/// arbitrary externally constructed recursive Errors, std I/O internals,
/// allocator/Arc headers or unexpected unwind payloads. Those paths cannot
/// establish native publication; Root never adopts caller-created Errors.
pub const NATIVE_DIAGNOSTIC_DATA_BYTES: usize =
    2 * std::mem::size_of::<Error>() + std::mem::size_of::<std::io::Error>();

/// Sticky child-custody refusal. Lost OS custody retains one original I/O
/// error in a shared payload so later observations cannot clone formatted
/// Strings or retry a numeric PID. Arc/OS internals remain opaque resources;
/// the known payload is `size_of::<std::io::Error>()`, not its rendered text.
#[derive(Clone, Debug)]
pub enum CustodyError {
    Refused(&'static str),
    Lost(std::sync::Arc<std::io::Error>),
}
impl From<&'static str> for CustodyError {
    fn from(message: &'static str) -> Self {
        Self::Refused(message)
    }
}
impl std::fmt::Display for CustodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(message) => f.write_str(message),
            Self::Lost(source) => source.fmt(f),
        }
    }
}
impl std::error::Error for CustodyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lost(source) => Some(source.as_ref()),
            Self::Refused(_) => None,
        }
    }
}
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    IoAt(&'static str, std::io::Error),
    Protocol(&'static str),
    State(hex_plugin_state::Error),
    Source(String),
    Validation(hex_plugin_scan::macos::validation::Error),
    Cancelled,
    Timeout,
    Worker(u32),
    Custody(CustodyError),
    /// Non-terminal: nothing was captured or saved and the plugin keeps running;
    /// only the refused capture operation is retired.
    CaptureRefused(protocol::StateCaptureRefusal),
    Preparation {
        cause: Box<Error>,
        exit: Option<std::process::ExitStatus>,
        custody_error: Option<Box<Error>>,
    },
}
impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(out, "instance i/o failed: {e}"),
            Self::IoAt(at, e) => write!(out, "instance {at} failed: {e}"),
            Self::Protocol(s) => out.write_str(s),
            Self::State(e) => e.fmt(out),
            Self::Source(s) => write!(out, "instance source refused: {s}"),
            Self::Validation(error) => write!(out, "instance source refused: {error}"),
            Self::Cancelled => out.write_str("instance cancelled"),
            Self::Timeout => out.write_str("instance supervision timed out"),
            Self::Worker(n) => write!(out, "instance worker refused operation: {n}"),
            Self::Custody(s) => write!(out, "instance child custody is unconfirmed: {s}"),
            Self::CaptureRefused(refusal) => refusal.fmt(out),
            Self::Preparation { cause, .. } => write!(out, "instance preparation failed: {cause}"),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) | Self::IoAt(_, error) => Some(error),
            Self::Validation(error) => Some(error),
            Self::Custody(error) => Some(error),
            Self::State(error) => Some(error),
            Self::Preparation { cause, .. } => Some(cause.as_ref()),
            _ => None,
        }
    }
}

/// Copied bounded exchange; native callback and joining-worker custody belong to audio.
pub mod transport;

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
            Self::Worker(n) if runtime_host_refusal(*n) => format_runtime_host_refusal(*n, out),
            Self::Worker(n) => match worker_refusal(*n) {
                Some(reason) => write!(out, "vst3 {reason} (worker code {n})"),
                None => write!(out, "instance worker refused operation: {n}"),
            },
            Self::Custody(s) => write!(out, "instance child custody is unconfirmed: {s}"),
            Self::CaptureRefused(refusal) => refusal.fmt(out),
            Self::Preparation { cause, .. } => write!(out, "instance preparation failed: {cause}"),
        }
    }
}

// Extended detail in the existing worker error field. Keep the low byte
// equal to the helper's process exit code (see macos_instance_worker.mm).
fn runtime_host_refusal(code: u32) -> bool {
    code & 0x8000_00ff == 0x8000_000b
}

fn format_runtime_host_refusal(code: u32, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let phase = if code & 0x4000_0000 == 0 {
        "callback dispatch"
    } else {
        "controller feedback"
    };
    let flags = (code >> 8) & 0x003f_ffff;
    write!(
        out,
        "vst3 host refused {phase} (worker code 11, flags 0x{flags:x}"
    )?;
    for (bit, reason) in [
        (0, "wrong thread"),
        (1, "unknown restart"),
        (2, "restart requires graph changes"),
        (3, "queue full"),
        (4, "recursive controller callback"),
        (5, "invalid metadata"),
        (6, "source retiring"),
        (7, "unsupported capability"),
        (8, "unsupported edit"),
        (9, "message pool full"),
        (10, "metadata capacity"),
        (11, "invalid edit"),
        (12, "unknown edit parameter"),
        (13, "read-only edit parameter"),
        (14, "invalid edit value"),
        (15, "duplicate begin"),
        (16, "perform without begin"),
        (17, "end without begin"),
        (18, "unknown feedback parameter"),
        (19, "controller rejected setParamNormalized"),
        (20, "invalid controller feedback value"),
    ] {
        if flags & (1 << bit) != 0 {
            write!(out, ", {reason}")?;
        }
    }
    out.write_str(")")
}

// Wire refusal codes from macos_instance_worker.mm. Keep the legacy setup
// stage readable when a desktop is using an older helper.
fn worker_refusal(code: u32) -> Option<&'static str> {
    Some(match code {
        1 => "worker configuration refused",
        2 => "bundle admission refused",
        3 => "effect class not found",
        4 => "processing setup refused",
        5 => "presentation latency refused",
        6 => "plug-in state capture refused",
        7 => "processing refused",
        8 => "worker protocol refused",
        9 => "load cancelled",
        10 => "plug-in shutdown refused",
        11 => "host callback refused",
        12 => "worker stopped after an exception",
        13 => "component or controller initialization refused",
        14 => "plug-in state restore refused",
        15 => "plug-in suspension refused",
        16 => "plug-in resume refused",
        17 => "processing setup has an invalid instance state",
        18 => "processing configuration refused",
        19 => "plug-in refused 32-bit audio processing",
        20 => "plug-in audio bus topology is unsupported",
        21 => "plug-in refused the requested audio bus arrangement",
        22 => "plug-in reported a different audio bus arrangement",
        23 => "plug-in rejected processing setup",
        24 => "plug-in activation refused",
        25 => "plug-in controller parameter inventory exceeds the host capacity",
        26 => "plug-in reported an invalid controller parameter inventory",
        _ => return None,
    })
}

#[cfg(test)]
#[test]
fn worker_refusal_explains_topology_and_retains_numeric_code() {
    assert_eq!(
        Error::Worker(20).to_string(),
        "vst3 plug-in audio bus topology is unsupported (worker code 20)"
    );
    assert_eq!(
        Error::Worker(4).to_string(),
        "vst3 processing setup refused (worker code 4)"
    );
    assert_eq!(
        Error::Worker(255).to_string(),
        "instance worker refused operation: 255"
    );
}

#[cfg(test)]
#[test]
fn runtime_refusal_logs_callback_and_feedback_details() {
    assert_eq!(
        Error::Worker(0x8108_000b).to_string(),
        "vst3 host refused callback dispatch (worker code 11, flags 0x10800, invalid edit, perform without begin)"
    );
    assert_eq!(
        Error::Worker(0xc800_000b).to_string(),
        "vst3 host refused controller feedback (worker code 11, flags 0x80000, controller rejected setParamNormalized)"
    );
    assert_eq!(
        Error::Worker(11).to_string(),
        "vst3 host callback refused (worker code 11)"
    );
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

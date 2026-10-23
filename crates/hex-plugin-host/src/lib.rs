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
//! hex-audio supplies its existing copied Block/Binding vocabulary, not callback
//! ownership. std and rustix event-only readiness serve control/worker I/O;
//! there are no process-global handlers or rustix process APIs in this client.
//!
//! # Does not own
//! No SDK ABI/pointers, unsafe Rust, device/callback, worker scheduling, canonical
//! edits/history, graph/publication/retirement or plugin admission authority.
//! All client I/O, cancellation, waiting and destruction belong on control or
//! the existing joining transport worker, never an audio callback. Ready is a
//! prepared instance fact, not canonical/audio application. Numeric child IDs
//! are never used after reap/unknown custody; no descendant containment is claimed.

#[cfg(unix)]
mod channel;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod native;
pub mod protocol;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    IoAt(&'static str, std::io::Error),
    Protocol(&'static str),
    State(hex_plugin_state::Error),
    Source(String),
    Cancelled,
    Timeout,
    Worker(u32),
    Custody(String),
    Preparation {
        cause: Box<Error>,
        exit: Option<std::process::ExitStatus>,
        custody_error: Option<String>,
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
            Self::Cancelled => out.write_str("instance cancelled"),
            Self::Timeout => out.write_str("instance supervision timed out"),
            Self::Worker(n) => write!(out, "instance worker refused operation: {n}"),
            Self::Custody(s) => write!(out, "instance child custody is unconfirmed: {s}"),
            Self::Preparation { cause, .. } => write!(out, "instance preparation failed: {cause}"),
        }
    }
}
impl std::error::Error for Error {}

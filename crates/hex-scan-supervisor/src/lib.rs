//! # Purpose
//! Own the shared standalone scanner utilities and Linux child supervisor used
//! by the VST3 and CLAP controlled-scan executables: controlled-file validation,
//! SHA-256 identity digests, bounded pipe draining, offline packet framing,
//! CLI argument/report helpers, signal-disposition guards, and (on Linux)
//! process-group helper supervision.
//!
//! # Dependencies
//! Uses `serde`/`serde_json` for scanner records and reports, `sha2` for
//! artifact identity digests, `signal-hook` on Unix for explicit standalone
//! executable signal disposition, and `rustix` process/event APIs on Linux for
//! child ownership. Only standalone scanner crates may depend on this crate,
//! and only standalone executables may invoke process-wide signal or subreaper
//! APIs (`signal` and `run`).
//!
//! # Does not own
//! Does not decode format-specific VST3 or CLAP scan packets; does not load
//! plugins or own any engine, host, or realtime audio path.
pub mod cli;
pub mod common;
pub mod framing;
pub mod io;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod signal;

#[cfg(target_os = "linux")]
pub use linux::{Cleanup, Output, Pending, run};

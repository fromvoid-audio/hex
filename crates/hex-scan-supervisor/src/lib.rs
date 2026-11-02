//! # Purpose
//! Own the one Linux standalone scanner supervisor shared by the VST3 and CLAP
//! controlled-scan executables: spawn one helper in its own process group,
//! exchange one bounded request, enforce the deadline and cancellation, then
//! terminate and reap the whole group before reporting. Each format passes its
//! own protocol payload bound.
//!
//! # Dependencies
//! Uses rustix process/event APIs and signal-hook on Linux for child ownership
//! and serde for the reported outcome. Only the standalone `hex-plugin-scan` and
//! `hex-clap-scan` executables may depend on it; it sets the process-wide child
//! subreaper and SIGCHLD disposition, so it never belongs in a library or host.
//!
//! # Does not own
//! Does not decode, validate or record scan packets; does not load plugins or
//! own any engine, host or realtime path. Other platforms compile it empty.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{Cleanup, Output, Pending, run};

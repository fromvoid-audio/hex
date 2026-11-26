//! # Purpose
//! Own typed issue/bug report envelopes, deterministic redaction, intake
//! configuration loading, atomic offline outbox persistence, and HTTP intake
//! transport for Pulse closed-loop reporting.
//!
//! # Dependencies
//! Uses `hex-diagnostics` for report-bundle inputs plus pinned `serde`,
//! `serde_json`, `base64`, `flate2`, `uuid`, and the standard library. No GPUI
//! or realtime audio dependencies.
//!
//! # Does not own
//! Does not own UI state, modals, GPUI rendering, audio callbacks, project
//! edits, or Pulse ticket triage workflows. Never logs or leaks intake bearer
//! tokens in process arguments, diagnostics, or report payloads.

mod config;
mod model;
mod outbox;
mod redact;
mod transport;

pub use config::{
    ConfigError, DEFAULT_INTAKE_URL, DEFAULT_PROJECT_ID, IntakeConfig, default_config_path,
};
pub use model::{
    AudioRuntimeSnapshot, IntakeReceipt, MAX_REPORT_PAYLOAD_BYTES, REPORT_SOURCE_HEX_APP,
    ReportEnvelope, ReportKind, RuntimeSnapshot,
};
pub use outbox::{FlushReport, MAX_RETRY_ATTEMPTS, OutboxItem, ReportOutbox, default_outbox_dir};
pub use redact::{RedactionContext, format_redacted_preview, redact_envelope, redact_text};
pub use transport::{CurlTransport, ReportTransport, TransportError};

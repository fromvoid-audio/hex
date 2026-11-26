//! Shared wire data types for Pulse `/api/intake/reports`.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use hex_diagnostics::report::DiagnosticReportBundle;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const REPORT_SOURCE_HEX_APP: &str = "hex-app";
pub const MAX_REPORT_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LOG_GZ_BYTES: usize = 2 * 1024 * 1024;

/// Category selected by the user in the in-app report dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportKind {
    Bug,
    Feature,
    Performance,
}

impl ReportKind {
    pub const ALL: [Self; 3] = [Self::Bug, Self::Feature, Self::Performance];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::Feature => "feature",
            Self::Performance => "performance",
        }
    }

    pub const fn ui_label(self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::Feature => "feature",
            Self::Performance => "audio & perf",
        }
    }
}

/// Read-only audio snapshot captured at frozen `T_report`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioRuntimeSnapshot {
    pub sample_rate: u32,
    pub mode: String,
    pub playing: bool,
    pub buffering: bool,
    pub callback_dropped: u64,
    pub logger_dropped: u64,
    pub degraded_plugins: Vec<String>,
}

/// Read-only runtime state captured at frozen `T_report` before user typing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub app_version: String,
    pub source_sha: String,
    pub tree_state: String,
    pub build_label: String,
    pub platform: String,
    pub reported_at_unix_nanos: u128,
    pub view_mode: String,
    pub status: String,
    pub last_error: Option<String>,
    pub audio: AudioRuntimeSnapshot,
    pub loaded_plugins: Vec<String>,
}

/// Wire JSON payload posted to `POST /api/intake/reports`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportEnvelope {
    pub request_id: String,
    pub project_id: u64,
    pub source: String,
    pub kind: ReportKind,
    pub user_summary: String,
    pub runtime_context: RuntimeSnapshot,
    pub digest_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_gz_base64: Option<String>,
    pub log_line_count: usize,
    pub log_clipped_segments: bool,
}

impl ReportEnvelope {
    /// Construct a new report envelope with a fresh UUID v4 `request_id`.
    pub fn new(
        project_id: u64,
        kind: ReportKind,
        user_summary: String,
        runtime_context: RuntimeSnapshot,
        bundle: Option<&DiagnosticReportBundle>,
    ) -> Self {
        Self::with_request_id(
            Uuid::new_v4().to_string(),
            project_id,
            kind,
            user_summary,
            runtime_context,
            bundle,
        )
    }

    /// Construct a report envelope with an explicit `request_id` (useful for
    /// deterministic previews and tests).
    pub fn with_request_id(
        request_id: String,
        project_id: u64,
        kind: ReportKind,
        user_summary: String,
        runtime_context: RuntimeSnapshot,
        bundle: Option<&DiagnosticReportBundle>,
    ) -> Self {
        let (digest_text, log_gz_base64, log_line_count, log_clipped_segments) = match bundle {
            Some(bundle) => {
                let encoded = if bundle.log_gz_bytes.is_empty()
                    || bundle.log_gz_bytes.len() > MAX_LOG_GZ_BYTES
                {
                    None
                } else {
                    Some(BASE64_STANDARD.encode(&bundle.log_gz_bytes))
                };
                (
                    bundle.digest_text.clone(),
                    encoded,
                    bundle.total_lines,
                    bundle.clipped_segments,
                )
            }
            None => (String::new(), None, 0, false),
        };
        Self {
            request_id,
            project_id,
            source: REPORT_SOURCE_HEX_APP.to_owned(),
            kind,
            user_summary,
            runtime_context,
            digest_text,
            log_gz_base64,
            log_line_count,
            log_clipped_segments,
        }
    }

    /// Serialize to compact JSON, verifying the 4 MiB intake payload cap.
    pub fn to_wire_json(&self) -> Result<Vec<u8>, String> {
        let bytes = serde_json::to_vec(self)
            .map_err(|error| format!("could not serialize report envelope: {error}"))?;
        if bytes.len() > MAX_REPORT_PAYLOAD_BYTES {
            return Err(format!(
                "report payload ({} bytes) exceeds {} byte limit",
                bytes.len(),
                MAX_REPORT_PAYLOAD_BYTES
            ));
        }
        Ok(bytes)
    }
}

/// Receipt returned by `POST /api/intake/reports` inside `{"data": ...}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntakeReceipt {
    pub report_id: u64,
    pub project_id: u64,
    pub request_id: String,
    pub status: String,
    pub deduplicated: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IntakeResponseEnvelope {
    pub data: IntakeReceipt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_envelope_matches_pulse_wire_contract() {
        let snapshot = RuntimeSnapshot {
            app_version: "0.1.0".into(),
            source_sha: "abc12345".into(),
            tree_state: "clean".into(),
            build_label: "0.1.0 · abc12345".into(),
            platform: "macos-aarch64".into(),
            reported_at_unix_nanos: 1_700_000_000_000_000_000,
            view_mode: "arrangement".into(),
            status: "ready".into(),
            last_error: None,
            audio: AudioRuntimeSnapshot {
                sample_rate: 48_000,
                mode: "resident".into(),
                playing: true,
                buffering: false,
                callback_dropped: 0,
                logger_dropped: 0,
                degraded_plugins: vec![],
            },
            loaded_plugins: vec!["hex.utility".into()],
        };
        let bundle = DiagnosticReportBundle {
            digest_text: "# digest\nline\n".into(),
            log_gz_bytes: vec![0x1f, 0x8b, 0x08, 0x00],
            total_lines: 42,
            omitted_digest_lines: 0,
            clipped_segments: false,
            warnings: vec![],
        };
        let env = ReportEnvelope::with_request_id(
            "11111111-2222-4333-8444-555555555555".into(),
            2,
            ReportKind::Performance,
            "audio stutter on seek".into(),
            snapshot,
            Some(&bundle),
        );
        let json: serde_json::Value = serde_json::from_slice(&env.to_wire_json().unwrap()).unwrap();
        assert_eq!(json["request_id"], "11111111-2222-4333-8444-555555555555");
        assert_eq!(json["project_id"], 2);
        assert_eq!(json["source"], "hex-app");
        assert_eq!(json["kind"], "performance");
        assert_eq!(json["runtime_context"]["audio"]["sample_rate"], 48_000);
        assert_eq!(json["log_line_count"], 42);
        assert_eq!(json["log_clipped_segments"], false);
        assert!(json["log_gz_base64"].is_string());
    }
}

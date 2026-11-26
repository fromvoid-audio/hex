//! Deterministic redaction for report envelopes and human-readable previews.
//!
//! Scrubs `$HOME` paths to `~`, strips OS username/hostname occurrences, and
//! removes `plsr_` / Bearer / Authorization tokens. Never reads clipboard,
//! project files, or audio media.

use crate::model::ReportEnvelope;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::{
    env,
    io::{Read, Write},
    path::Path,
};

const MAX_UNZIPPED_REDACT_BYTES: u64 = 40 * 1024 * 1024;

/// Environment-derived or explicitly injected redaction parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RedactionContext {
    pub home_dir: Option<String>,
    pub username: Option<String>,
    pub hostname: Option<String>,
}

impl RedactionContext {
    pub fn new(
        home_dir: Option<String>,
        username: Option<String>,
        hostname: Option<String>,
    ) -> Self {
        Self {
            home_dir: home_dir
                .map(|v| v.trim_end_matches('/').to_owned())
                .filter(|v| !v.is_empty()),
            username: username.filter(|v| v.len() >= 2),
            hostname: hostname.filter(|v| v.len() >= 2),
        }
    }

    pub fn from_env() -> Self {
        let home_dir = env::var("HOME")
            .ok()
            .map(|v| v.trim_end_matches('/').to_owned())
            .filter(|v| !v.is_empty());
        let username = env::var("USER")
            .ok()
            .or_else(|| env::var("USERNAME").ok())
            .or_else(|| {
                home_dir.as_deref().and_then(|home| {
                    Path::new(home)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                })
            })
            .filter(|v| v.len() >= 2);
        let hostname = env::var("HOSTNAME")
            .ok()
            .or_else(|| env::var("HOST").ok())
            .or_else(|| env::var("COMPUTERNAME").ok())
            .filter(|v| v.len() >= 2);
        Self {
            home_dir,
            username,
            hostname,
        }
    }
}

/// Deterministically scrub sensitive paths, identifiers, and tokens from `input`.
pub fn redact_text(input: &str, ctx: &RedactionContext) -> String {
    let mut text = input.to_owned();

    // 1. Scrub exact $HOME paths (and common macOS/Linux user-home patterns) -> ~
    if let Some(home) = &ctx.home_dir {
        text = text.replace(home.as_str(), "~");
        // Also handle escaped backslashes/JSON strings if any
        if home.contains('\\') {
            let escaped = home.replace('\\', "\\\\");
            text = text.replace(&escaped, "~");
        }
    }
    if let Some(user) = &ctx.username {
        for prefix in ["/Users/", "/home/"] {
            let needle = format!("{prefix}{user}");
            text = text.replace(&needle, "~");
        }
    }

    // 2. Scrub tokens (`plsr_...`, `Bearer ...`, `Authorization: ...`)
    text = scrub_plsr_tokens(&text);
    text = scrub_bearer_tokens(&text);
    text = scrub_authorization_headers(&text);

    // 3. Scrub hostname occurrences (including `<host>.local`)
    if let Some(host) = &ctx.hostname
        && host.len() >= 3
    {
        let local_variant = if host.ends_with(".local") {
            host.trim_end_matches(".local").to_owned()
        } else {
            format!("{host}.local")
        };
        if host.len() >= local_variant.len() {
            text = replace_bounded_identifier(&text, host, "<host>");
            if local_variant.len() >= 3 {
                text = replace_bounded_identifier(&text, &local_variant, "<host>");
            }
        } else {
            text = replace_bounded_identifier(&text, &local_variant, "<host>");
            text = replace_bounded_identifier(&text, host, "<host>");
        }
    }

    // 4. Scrub remaining standalone OS username occurrences
    if let Some(user) = &ctx.username
        && user.len() >= 3
    {
        text = replace_bounded_identifier(&text, user, "<user>");
    }

    text
}

/// Produce a fully redacted copy of `envelope` (including gzipped session log).
pub fn redact_envelope(envelope: &ReportEnvelope, ctx: &RedactionContext) -> ReportEnvelope {
    let mut out = envelope.clone();
    out.user_summary = redact_text(&out.user_summary, ctx);
    out.runtime_context.status = redact_text(&out.runtime_context.status, ctx);
    out.runtime_context.last_error = out
        .runtime_context
        .last_error
        .as_deref()
        .map(|err| redact_text(err, ctx));
    out.runtime_context.audio.degraded_plugins = out
        .runtime_context
        .audio
        .degraded_plugins
        .iter()
        .map(|item| redact_text(item, ctx))
        .collect();
    out.runtime_context.loaded_plugins = out
        .runtime_context
        .loaded_plugins
        .iter()
        .map(|item| redact_text(item, ctx))
        .collect();
    out.digest_text = redact_text(&out.digest_text, ctx);
    out.log_gz_base64 = out
        .log_gz_base64
        .as_deref()
        .and_then(|encoded| redact_gzip_base64(encoded, ctx));
    out
}

fn redact_gzip_base64(encoded: &str, ctx: &RedactionContext) -> Option<String> {
    let gz_bytes = BASE64_STANDARD.decode(encoded).ok()?;
    let mut decoder = GzDecoder::new(gz_bytes.as_slice()).take(MAX_UNZIPPED_REDACT_BYTES);
    let mut raw = Vec::new();
    decoder.read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let redacted = redact_text(&text, ctx);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(redacted.as_bytes()).ok()?;
    let recompressed = encoder.finish().ok()?;
    if recompressed.len() > crate::model::MAX_LOG_GZ_BYTES {
        return None;
    }
    Some(BASE64_STANDARD.encode(recompressed))
}

/// Format a human-readable preview of a redacted report envelope for in-app inspection.
pub fn format_redacted_preview(envelope: &ReportEnvelope) -> String {
    let ctx = &envelope.runtime_context;
    let audio = &ctx.audio;
    let attachment = match &envelope.log_gz_base64 {
        Some(b64) => {
            let approx_gz_bytes = (b64.len() * 3) / 4;
            format!(
                "attached ({} lines, ~{} bytes gzipped{})",
                envelope.log_line_count,
                approx_gz_bytes,
                if envelope.log_clipped_segments {
                    ", oldest segments clipped"
                } else {
                    ""
                }
            )
        }
        None => "omitted".to_owned(),
    };
    let last_error = ctx.last_error.as_deref().unwrap_or("none");
    let degraded = if audio.degraded_plugins.is_empty() {
        "none".to_owned()
    } else {
        audio.degraded_plugins.join(", ")
    };
    let loaded = if ctx.loaded_plugins.is_empty() {
        "none".to_owned()
    } else {
        ctx.loaded_plugins.join(", ")
    };
    let digest_preview = if envelope.digest_text.is_empty() {
        "(diagnostics not included)".to_owned()
    } else {
        let mut snippet: String = envelope.digest_text.chars().take(4096).collect();
        if snippet.len() < envelope.digest_text.len() {
            snippet.push_str("\n[preview truncated]");
        }
        snippet
    };

    format!(
        "kind: {}\n\
         version: {} ({}, {})\n\
         platform: {}\n\
         reported_at_unix_nanos: {}\n\
         view: {} | status: {} | last_error: {}\n\
         audio: {} hz, mode={}, playing={}, buffering={}, cb_dropped={}, log_dropped={}\n\
         degraded_plugins: {}\n\
         loaded_plugins: {}\n\
         session_log: {}\n\
         \n\
         --- summary ---\n\
         {}\n\
         \n\
         --- diagnostic digest ---\n\
         {}",
        envelope.kind.as_str(),
        ctx.build_label,
        ctx.source_sha,
        ctx.tree_state,
        ctx.platform,
        ctx.reported_at_unix_nanos,
        ctx.view_mode,
        ctx.status,
        last_error,
        audio.sample_rate,
        audio.mode,
        audio.playing,
        audio.buffering,
        audio.callback_dropped,
        audio.logger_dropped,
        degraded,
        loaded,
        attachment,
        if envelope.user_summary.trim().is_empty() {
            "(empty)"
        } else {
            envelope.user_summary.as_str()
        },
        digest_preview
    )
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+' | b'/' | b'=')
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn scrub_plsr_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"plsr_") {
            let mut end = i + 5;
            while end < bytes.len() && is_token_char(bytes[end]) {
                end += 1;
            }
            out.push_str("[redacted-token]");
            i = end;
        } else if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn scrub_bearer_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 7 <= bytes.len()
            && bytes[i..i + 6].eq_ignore_ascii_case(b"bearer")
            && bytes[i + 6].is_ascii_whitespace()
            && (i == 0 || !is_ident_char(bytes[i - 1]))
        {
            out.push_str(&input[i..i + 7]);
            let mut j = i + 7;
            while j < bytes.len() && bytes[j] == b' ' {
                out.push(' ');
                j += 1;
            }
            let start_tok = j;
            while j < bytes.len() && is_token_char(bytes[j]) {
                j += 1;
            }
            if j > start_tok {
                out.push_str("[redacted-token]");
                i = j;
            } else {
                i += 7;
            }
        } else if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn scrub_authorization_headers(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut first = true;
    for line in input.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        let trimmed = line.trim_start();
        if trimmed.len() >= 14 && trimmed[..14].eq_ignore_ascii_case("authorization:") {
            let leading = line.len() - trimmed.len();
            out.push_str(&line[..leading]);
            out.push_str("Authorization: [redacted]");
        } else {
            out.push_str(line);
        }
    }
    out
}

fn replace_bounded_identifier(input: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() || !input.contains(needle) {
        return input.to_owned();
    }
    let bytes = input.as_bytes();
    let needle_len = needle.len();
    let mut out = String::with_capacity(input.len());
    let mut pos = 0;
    while let Some(offset) = input[pos..].find(needle) {
        let start = pos + offset;
        let end = start + needle_len;
        let left_ok = start == 0 || !is_ident_char(bytes[start - 1]);
        let right_ok = end == bytes.len() || !is_ident_char(bytes[end]);
        out.push_str(&input[pos..start]);
        if left_ok && right_ok {
            out.push_str(replacement);
        } else {
            out.push_str(needle);
        }
        pos = end;
    }
    out.push_str(&input[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioRuntimeSnapshot, ReportKind, RuntimeSnapshot};
    use hex_diagnostics::report::DiagnosticReportBundle;

    #[test]
    fn redacts_home_username_hostname_and_tokens_deterministically() {
        let ctx = RedactionContext::new(
            Some("/Users/alice".into()),
            Some("alice".into()),
            Some("studio-mac.local".into()),
        );
        let raw = concat!(
            "opened /Users/alice/Projects/track.hex on studio-mac.local by alice\n",
            "Authorization: Bearer super_secret_xyz123\n",
            "leaked token=plsr_0123456789abcdef and Bearer another_token_99\n",
            "unrelated word malice should stay intact\n",
        );
        let scrubbed = redact_text(raw, &ctx);
        assert!(scrubbed.contains("~/Projects/track.hex"));
        assert!(scrubbed.contains("on <host> by <user>"));
        assert!(!scrubbed.contains("/Users/alice"));
        assert!(!scrubbed.contains("studio-mac"));
        assert!(!scrubbed.contains("plsr_0123456789abcdef"));
        assert!(!scrubbed.contains("super_secret_xyz123"));
        assert!(!scrubbed.contains("another_token_99"));
        assert!(
            scrubbed.contains("malice"),
            "must not mangle embedded substrings"
        );
    }

    #[test]
    fn redacts_gzipped_attachment_and_formats_preview() {
        let ctx = RedactionContext::new(
            Some("/Users/bob".into()),
            Some("bob".into()),
            Some("bob-studio".into()),
        );
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(b"event message=\"path=/Users/bob/secret.wav token=plsr_topsecret\"\n")
            .unwrap();
        let gz = enc.finish().unwrap();
        let bundle = DiagnosticReportBundle {
            digest_text: "digest /Users/bob plsr_topsecret\n".into(),
            log_gz_bytes: gz,
            total_lines: 1,
            omitted_digest_lines: 0,
            clipped_segments: false,
            warnings: vec![],
        };
        let env = ReportEnvelope::with_request_id(
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".into(),
            2,
            ReportKind::Bug,
            "failed at /Users/bob/file".into(),
            RuntimeSnapshot {
                build_label: "0.1.0 · deadbeef".into(),
                source_sha: "deadbeef".into(),
                tree_state: "clean".into(),
                platform: "macos-aarch64".into(),
                view_mode: "arrangement".into(),
                status: "error in /Users/bob".into(),
                last_error: Some("token plsr_topsecret".into()),
                audio: AudioRuntimeSnapshot {
                    sample_rate: 48_000,
                    mode: "resident".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            Some(&bundle),
        );

        let redacted = redact_envelope(&env, &ctx);
        assert!(!redacted.user_summary.contains("/Users/bob"));
        assert!(!redacted.digest_text.contains("plsr_topsecret"));
        assert!(
            !redacted
                .runtime_context
                .last_error
                .as_deref()
                .unwrap()
                .contains("plsr_topsecret")
        );

        let decoded_gz = BASE64_STANDARD
            .decode(redacted.log_gz_base64.as_ref().unwrap())
            .unwrap();
        let mut plain = String::new();
        GzDecoder::new(decoded_gz.as_slice())
            .read_to_string(&mut plain)
            .unwrap();
        assert!(plain.contains("~/secret.wav"));
        assert!(!plain.contains("/Users/bob"));
        assert!(!plain.contains("plsr_topsecret"));

        let preview = format_redacted_preview(&redacted);
        assert!(preview.contains("kind: bug"));
        assert!(preview.contains("48000 hz"));
        assert!(preview.contains("~/file"));
        assert!(!preview.contains("plsr_topsecret"));
    }
}

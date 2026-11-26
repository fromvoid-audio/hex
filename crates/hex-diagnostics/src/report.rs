//! Smart diagnostic report slicer over the rolling session log reader.
//!
//! Runs strictly on control/background threads, never on the GPUI render thread
//! or audio callback thread. Produces a bounded high-signal digest plus a
//! bounded gzip-compressed rolling log archive across retained segments in
//! oldest-to-newest order.

use crate::reader::{open_segment, parse_event, segment_paths, stream_segment_lines};
use flate2::{Compression, write::GzEncoder};
use std::{
    collections::{HashMap, VecDeque},
    io::{self, Write},
    path::Path,
};

pub const DEFAULT_CAUSAL_WINDOW_SECS: u64 = 60;
pub const DEFAULT_MAX_CAUSAL_LINES: usize = 400;
pub const DEFAULT_MAX_DIGEST_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_GZIP_BYTES: usize = 2 * 1024 * 1024;

const MAX_UNIQUE_DEDUP_ENTRIES: usize = 256;
const MAX_HEADER_CHARS: usize = 4096;
const NANOS_PER_SEC: u128 = 1_000_000_000;

/// Bounds for slicing one session diagnostic report bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportSliceConfig {
    /// Causal window duration preceding `reported_at_unix_nanos`, in seconds.
    pub causal_window_secs: u64,
    /// Maximum lines retained from the causal window (keeping newest lines).
    pub max_causal_lines: usize,
    /// Strict maximum UTF-8 byte length of `digest_text`.
    pub max_digest_bytes: usize,
    /// Strict maximum compressed byte length of `log_gz_bytes`.
    pub max_gzip_bytes: usize,
}

impl Default for ReportSliceConfig {
    fn default() -> Self {
        Self {
            causal_window_secs: DEFAULT_CAUSAL_WINDOW_SECS,
            max_causal_lines: DEFAULT_MAX_CAUSAL_LINES,
            max_digest_bytes: DEFAULT_MAX_DIGEST_BYTES,
            max_gzip_bytes: DEFAULT_MAX_GZIP_BYTES,
        }
    }
}

/// Bounded report digest and compressed full rolling session log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticReportBundle {
    pub digest_text: String,
    pub log_gz_bytes: Vec<u8>,
    pub total_lines: usize,
    pub omitted_digest_lines: usize,
    pub clipped_segments: bool,
    pub warnings: Vec<String>,
}

struct DedupEntry {
    sample_line: String,
    count: usize,
}

/// Build a bounded diagnostic report bundle from `active_log` and its retained
/// rolling siblings (`.3.log` .. `.1.log` -> active) anchored at frozen
/// `reported_at_unix_nanos`.
pub fn build_report_bundle(
    active_log: &Path,
    reported_at_unix_nanos: u128,
    config: &ReportSliceConfig,
) -> io::Result<DiagnosticReportBundle> {
    if config.max_digest_bytes == 0 || config.max_gzip_bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "report slice byte budgets must be non-zero",
        ));
    }

    let window_span = u128::from(config.causal_window_secs).saturating_mul(NANOS_PER_SEC);
    let window_start = reported_at_unix_nanos.saturating_sub(window_span);

    let paths = segment_paths(active_log);
    let mut raw_line = Vec::new();
    let mut header: Option<String> = None;
    let mut warnings: Vec<String> = Vec::new();
    let mut total_lines = 0usize;

    let mut causal_lines: VecDeque<String> =
        VecDeque::with_capacity(config.max_causal_lines.min(512));
    let mut omitted_causal_lines = 0usize;

    let mut dedup_entries: Vec<DedupEntry> = Vec::new();
    let mut dedup_index: HashMap<String, usize> = HashMap::new();
    let mut omitted_dedup_lines = 0usize;

    let mut segment_texts: Vec<Vec<String>> = Vec::new();

    let mut previous_sequence: Option<u64> = None;
    let mut previous_segment: Option<u64> = None;
    let mut sequence_discontinuity = false;
    let mut truncated_messages = false;

    for (index, path) in paths {
        let Some(file) = open_segment(&path, index)? else {
            continue;
        };
        let mut first_line = true;
        let mut malformed = false;
        let mut seg_lines: Vec<String> = Vec::new();

        let stats = stream_segment_lines(file, &path, &mut raw_line, |line| {
            total_lines = total_lines.saturating_add(1);
            seg_lines.push(line.to_owned());

            if first_line {
                first_line = false;
                if line.starts_with("session_started ") {
                    if header.is_none() {
                        let clipped: String = line.chars().take(MAX_HEADER_CHARS).collect();
                        header = Some(if clipped.len() < line.len() {
                            format!("{clipped} [header clipped]")
                        } else {
                            clipped
                        });
                    }
                    if let Some(segment) = line
                        .split_ascii_whitespace()
                        .find_map(|field| field.strip_prefix("segment="))
                        .and_then(|value| value.parse::<u64>().ok())
                    {
                        if previous_segment.map_or(segment != 0, |previous| {
                            previous.checked_add(1) != Some(segment)
                        }) {
                            warnings.push(
                                "session segments are missing or changed during reading".into(),
                            );
                        }
                        previous_segment = Some(segment);
                    }
                }
            }

            let event = if line.starts_with("event ") {
                match parse_event(line) {
                    Some(event) => Some(event),
                    None => {
                        malformed = true;
                        None
                    }
                }
            } else {
                None
            };

            if let Some(event) = &event {
                if previous_sequence
                    .is_some_and(|previous| previous.checked_add(1) != Some(event.sequence))
                {
                    sequence_discontinuity = true;
                }
                previous_sequence = Some(event.sequence);
                truncated_messages |= line.split_once(" message=").is_some_and(|(prefix, _)| {
                    prefix
                        .split_ascii_whitespace()
                        .any(|field| field == "truncated=true")
                });
            }

            // 1. Causal window [T_report - window, T_report]
            let line_ts = event
                .as_ref()
                .map(|ev| ev.unix_nanos)
                .or_else(|| parse_writer_unix_nanos(line));
            if let Some(ts) = line_ts
                && (window_start..=reported_at_unix_nanos).contains(&ts)
            {
                if config.max_causal_lines == 0 {
                    omitted_causal_lines = omitted_causal_lines.saturating_add(1);
                } else {
                    if causal_lines.len() >= config.max_causal_lines {
                        causal_lines.pop_front();
                        omitted_causal_lines = omitted_causal_lines.saturating_add(1);
                    }
                    causal_lines.push_back(line.to_owned());
                }
            }

            // 2. Session-wide high-signal entries (deduplicated)
            if is_high_signal_line(line, event.as_ref().map(|ev| ev.level)) {
                let key = dedup_key(line, event.as_ref().map(|ev| (ev.level, ev.category)));
                if let Some(&slot) = dedup_index.get(&key) {
                    dedup_entries[slot].count = dedup_entries[slot].count.saturating_add(1);
                } else if dedup_entries.len() < MAX_UNIQUE_DEDUP_ENTRIES {
                    let slot = dedup_entries.len();
                    dedup_entries.push(DedupEntry {
                        sample_line: line.to_owned(),
                        count: 1,
                    });
                    dedup_index.insert(key, slot);
                } else {
                    omitted_dedup_lines = omitted_dedup_lines.saturating_add(1);
                }
            }
        })?;

        if stats.exceeded_limit {
            warnings.push(format!("{}: segment exceeds read limit", path.display()));
        }
        if stats.incomplete_tail {
            warnings.push(format!("{}: incomplete final line omitted", path.display()));
        }
        if stats.invalid_utf8 {
            warnings.push(format!("{}: invalid utf-8 replaced", path.display()));
        }
        if malformed {
            warnings.push(format!("{}: malformed events retained", path.display()));
        }
        if !seg_lines.is_empty() {
            segment_texts.push(seg_lines);
        }
    }

    if sequence_discontinuity {
        warnings.push(
            "event sequence has a gap or out-of-order records; loss or concurrent rotation may have occurred".into(),
        );
    }
    if truncated_messages {
        warnings.push("one or more event messages were truncated by the logger".into());
    }

    let (log_gz_bytes, clipped_segments) =
        compress_bounded_segments(&segment_texts, config.max_gzip_bytes)?;
    if clipped_segments {
        warnings.push(format!(
            "older rolling log segments clipped to fit {} byte gzip budget",
            config.max_gzip_bytes
        ));
    }

    let (digest_text, omitted_by_digest_cap) = assemble_digest(
        header.as_deref(),
        &warnings,
        &dedup_entries,
        omitted_dedup_lines,
        &causal_lines,
        omitted_causal_lines,
        reported_at_unix_nanos,
        config,
    );

    let omitted_digest_lines = omitted_causal_lines
        .saturating_add(omitted_dedup_lines)
        .saturating_add(omitted_by_digest_cap);

    Ok(DiagnosticReportBundle {
        digest_text,
        log_gz_bytes,
        total_lines,
        omitted_digest_lines,
        clipped_segments,
        warnings,
    })
}

fn parse_writer_unix_nanos(line: &str) -> Option<u128> {
    if !line.starts_with("diagnostics_loss ")
        && !line.starts_with("diagnostics_retention_warning ")
        && !line.starts_with("session_finished ")
    {
        return None;
    }
    line.split_ascii_whitespace()
        .find_map(|field| field.strip_prefix("writer_unix_nanos="))
        .and_then(|value| value.parse::<u128>().ok())
}

fn is_high_signal_line(line: &str, event_level: Option<&str>) -> bool {
    if matches!(event_level, Some("warn" | "error")) {
        return true;
    }
    if line.starts_with("diagnostics_loss ") || line.starts_with("diagnostics_retention_warning ") {
        return true;
    }
    line.contains("loop_refused")
        || line.contains("worker_refused")
        || line.contains("worker_fault")
        || line.contains("degraded_plugins")
}

fn dedup_key(line: &str, level_category: Option<(&str, &str)>) -> String {
    if let Some((level, category)) = level_category
        && let Some((_, message)) = line.split_once(" message=")
    {
        return format!("{level}:{category}:{message}");
    }
    let mut normalized = String::with_capacity(line.len().min(256));
    for token in line.split_ascii_whitespace() {
        if token.starts_with("unix_nanos=")
            || token.starts_with("writer_unix_nanos=")
            || token.starts_with("elapsed_nanos=")
            || token.starts_with("sequence=")
        {
            continue;
        }
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(token);
        if normalized.len() >= 512 {
            break;
        }
    }
    normalized
}

fn gzip_segments(
    segments: &[Vec<String>],
    start_segment: usize,
    start_line_in_first: usize,
) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    for (seg_offset, lines) in segments[start_segment..].iter().enumerate() {
        let skip = if seg_offset == 0 {
            start_line_in_first
        } else {
            0
        };
        for line in lines.iter().skip(skip) {
            encoder.write_all(line.as_bytes())?;
            encoder.write_all(b"\n")?;
        }
    }
    encoder.finish()
}

fn compress_bounded_segments(
    segments: &[Vec<String>],
    max_gzip_bytes: usize,
) -> io::Result<(Vec<u8>, bool)> {
    if segments.is_empty() {
        let empty = gzip_segments(segments, 0, 0)?;
        if empty.len() > max_gzip_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_gzip_bytes is smaller than an empty gzip stream",
            ));
        }
        return Ok((empty, false));
    }

    // Try oldest-to-newest dropping whole oldest segments first.
    for start_segment in 0..segments.len() {
        let gz = gzip_segments(segments, start_segment, 0)?;
        if gz.len() <= max_gzip_bytes {
            return Ok((gz, start_segment > 0));
        }
    }

    // Even the newest single segment exceeded `max_gzip_bytes`: binary-search the
    // longest newest suffix of lines in the active segment that fits within budget.
    let last_idx = segments.len() - 1;
    let line_count = segments[last_idx].len();
    let mut lo = 1usize;
    let mut hi = line_count;
    let mut best: Option<Vec<u8>> = None;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let gz = gzip_segments(segments, last_idx, mid)?;
        if gz.len() <= max_gzip_bytes {
            best = Some(gz);
            if mid == 0 {
                break;
            }
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    if let Some(gz) = best {
        Ok((gz, true))
    } else {
        let empty = gzip_segments(&[], 0, 0)?;
        if empty.len() <= max_gzip_bytes {
            Ok((empty, true))
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_gzip_bytes is smaller than an empty gzip stream",
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn assemble_digest(
    header: Option<&str>,
    warnings: &[String],
    dedup_entries: &[DedupEntry],
    omitted_dedup_lines: usize,
    causal_lines: &VecDeque<String>,
    omitted_causal_lines: usize,
    reported_at_unix_nanos: u128,
    config: &ReportSliceConfig,
) -> (String, usize) {
    let mut candidate_lines: Vec<String> = Vec::new();
    candidate_lines.push(format!(
        "# hex diagnostic digest (reported_at_unix_nanos={reported_at_unix_nanos})"
    ));
    candidate_lines.push("# session".to_owned());
    candidate_lines.push(
        header
            .unwrap_or("(missing session_started header)")
            .to_owned(),
    );

    if !warnings.is_empty() {
        candidate_lines.push("# warnings".to_owned());
        for warning in warnings {
            candidate_lines.push(format!("- {warning}"));
        }
    }

    candidate_lines.push("# session_high_signal".to_owned());
    if dedup_entries.is_empty() {
        candidate_lines.push("(none)".to_owned());
    } else {
        for entry in dedup_entries {
            if entry.count > 1 {
                candidate_lines.push(format!("[x{}] {}", entry.count, entry.sample_line));
            } else {
                candidate_lines.push(entry.sample_line.clone());
            }
        }
    }
    if omitted_dedup_lines > 0 {
        candidate_lines.push(format!(
            "[omitted {omitted_dedup_lines} additional unique high-signal entries]"
        ));
    }

    candidate_lines.push(format!(
        "# causal_window ({}s before report, max {} lines)",
        config.causal_window_secs, config.max_causal_lines
    ));
    if omitted_causal_lines > 0 {
        candidate_lines.push(format!(
            "[omitted {omitted_causal_lines} earlier lines in causal window]"
        ));
    }
    if causal_lines.is_empty() {
        candidate_lines.push("(no events in causal window)".to_owned());
    } else {
        for line in causal_lines {
            candidate_lines.push(line.clone());
        }
    }

    let full_len: usize = candidate_lines.iter().map(|line| line.len() + 1).sum();
    if full_len <= config.max_digest_bytes {
        let mut out = String::with_capacity(full_len);
        for line in candidate_lines {
            out.push_str(&line);
            out.push('\n');
        }
        return (out, 0);
    }

    // Strictly cap `digest_text` to `config.max_digest_bytes` while preserving a
    // clear omission/truncation marker whenever space permits.
    const FOOTER_RESERVE: usize = 96;
    let body_budget = config.max_digest_bytes.saturating_sub(FOOTER_RESERVE);
    let mut out = String::with_capacity(config.max_digest_bytes);
    let mut omitted_by_cap = 0usize;

    for (idx, line) in candidate_lines.iter().enumerate() {
        let needed = line.len() + 1;
        if out.len().saturating_add(needed) <= body_budget {
            out.push_str(line);
            out.push('\n');
        } else {
            omitted_by_cap = candidate_lines.len().saturating_sub(idx);
            break;
        }
    }

    if omitted_by_cap == 0 {
        // Reached because total > max_digest_bytes while body_budget left room.
        omitted_by_cap = 1;
    }

    let footer = format!(
        "[digest truncated: omitted {omitted_by_cap} lines at {} byte budget]\n",
        config.max_digest_bytes
    );
    if out.len().saturating_add(footer.len()) <= config.max_digest_bytes {
        out.push_str(&footer);
    } else if footer.len() <= config.max_digest_bytes {
        // Trim body lines to guarantee footer fits cleanly.
        let keep_limit = config.max_digest_bytes - footer.len();
        truncate_utf8_on_line(&mut out, keep_limit);
        out.push_str(&footer);
    } else {
        truncate_utf8(&mut out, config.max_digest_bytes);
    }

    (out, omitted_by_cap)
}

fn truncate_utf8_on_line(text: &mut String, max_bytes: usize) {
    truncate_utf8(text, max_bytes);
    if let Some(pos) = text.rfind('\n') {
        text.truncate(pos + 1);
    }
}

fn truncate_utf8(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::{fs, io::Read};

    fn event_line(level: &str, category: &str, ts_secs: u64, seq: u64, msg: &str) -> String {
        let nanos = u128::from(ts_secs) * NANOS_PER_SEC;
        format!(
            "event level={level} category={category} unix_nanos={nanos} elapsed_nanos={seq} sequence={seq} truncated=false original_bytes={} message=\"{msg}\"\n",
            msg.len()
        )
    }

    #[test]
    fn causal_window_filters_bounds_and_caps_keeping_newest_lines() {
        let dir = crate::tests::fixture();
        let path = dir.join("session.log");
        let mut content = String::from("session_started log_schema=3 segment=0 build=test\n");
        // Before window (t=100s, report at t=200s, window=60s -> [140s, 200s])
        content.push_str(&event_line("info", "ui", 100, 1, "too early"));
        // Inside window: 6 lines, cap at 3 -> keeps seq 5, 6, 7 and omits 3
        for idx in 0..6u64 {
            content.push_str(&event_line(
                "info",
                "ui",
                150 + idx,
                2 + idx,
                &format!("inside-{idx}"),
            ));
        }
        // After T_report (t=205s) -> must be excluded from causal window
        content.push_str(&event_line("info", "ui", 205, 8, "after report"));
        fs::write(&path, content).unwrap();

        let config = ReportSliceConfig {
            causal_window_secs: 60,
            max_causal_lines: 3,
            max_digest_bytes: DEFAULT_MAX_DIGEST_BYTES,
            max_gzip_bytes: DEFAULT_MAX_GZIP_BYTES,
        };
        let bundle = build_report_bundle(&path, 200 * NANOS_PER_SEC, &config).unwrap();

        assert_eq!(bundle.total_lines, 9);
        assert_eq!(bundle.omitted_digest_lines, 3);
        assert!(bundle.digest_text.contains("session_started log_schema=3"));
        assert!(!bundle.digest_text.contains("too early"));
        assert!(!bundle.digest_text.contains("after report"));
        assert!(!bundle.digest_text.contains("inside-0"));
        assert!(!bundle.digest_text.contains("inside-2"));
        assert!(bundle.digest_text.contains("inside-3"));
        assert!(bundle.digest_text.contains("inside-5"));
        assert!(
            bundle
                .digest_text
                .contains("omitted 3 earlier lines in causal window")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deduplicates_high_signal_spam_with_repeat_counts() {
        let dir = crate::tests::fixture();
        let path = dir.join("session.log");
        let mut content = String::from("session_started log_schema=3 segment=0\n");
        let mut seq = 1u64;
        for _ in 0..12 {
            content.push_str(&event_line("warn", "audio", 10, seq, "callback underrun"));
            seq += 1;
        }
        for _ in 0..4 {
            content.push_str(&event_line(
                "debug",
                "ui",
                11,
                seq,
                "action_diag v=1 stage=loop_refused gates=scrub",
            ));
            seq += 1;
        }
        for _ in 0..3 {
            content.push_str(&event_line(
                "info",
                "audio",
                12,
                seq,
                "audio_diag v=1 stage=worker_fault output=1",
            ));
            seq += 1;
        }
        content.push_str(&event_line(
            "info",
            "audio",
            13,
            seq,
            "audio_diag v=1 stage=worker_refused output=1",
        ));
        seq += 1;
        content.push_str(&event_line(
            "info",
            "audio",
            14,
            seq,
            "audio_diag v=1 stage=degraded_plugins devices=[7]",
        ));
        content.push_str("diagnostics_loss writer_unix_nanos=15000000000 elapsed_nanos=15 failed=false statistics=LogStatistics { queue_full: 2 } failure=\"none\"\n");
        content.push_str("diagnostics_loss writer_unix_nanos=16000000000 elapsed_nanos=16 failed=false statistics=LogStatistics { queue_full: 2 } failure=\"none\"\n");
        fs::write(&path, content).unwrap();

        // Report at t=500s so causal window is empty, testing session-wide high-signal capture.
        let bundle =
            build_report_bundle(&path, 500 * NANOS_PER_SEC, &ReportSliceConfig::default()).unwrap();

        assert!(bundle.digest_text.contains("[x12] "));
        assert!(bundle.digest_text.contains("callback underrun"));
        assert!(bundle.digest_text.contains("[x4] "));
        assert!(bundle.digest_text.contains("loop_refused"));
        assert!(bundle.digest_text.contains("[x3] "));
        assert!(bundle.digest_text.contains("worker_fault"));
        assert!(bundle.digest_text.contains("worker_refused"));
        assert!(bundle.digest_text.contains("degraded_plugins"));
        assert!(bundle.digest_text.contains("[x2] diagnostics_loss"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn enforces_digest_byte_cap_with_explicit_truncation_marker() {
        let dir = crate::tests::fixture();
        let path = dir.join("session.log");
        let mut content = String::from("session_started log_schema=3 segment=0\n");
        for seq in 1..=200u64 {
            content.push_str(&event_line(
                "info",
                "ui",
                100,
                seq,
                &format!("unique payload entry {seq} {}", "a".repeat(80)),
            ));
        }
        fs::write(&path, content).unwrap();

        let config = ReportSliceConfig {
            causal_window_secs: 60,
            max_causal_lines: 400,
            max_digest_bytes: 1024,
            max_gzip_bytes: DEFAULT_MAX_GZIP_BYTES,
        };
        let bundle = build_report_bundle(&path, 100 * NANOS_PER_SEC, &config).unwrap();
        assert!(bundle.digest_text.len() <= 1024);
        assert!(bundle.omitted_digest_lines > 0);
        assert!(bundle.digest_text.contains("[digest truncated:"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn gzip_preserves_oldest_to_newest_segments_and_clips_oldest_under_budget() {
        let dir = crate::tests::fixture();
        let path = dir.join("session.log");
        // Write 3 segments (.2.log oldest -> .1.log -> .log newest)
        let mut oldest = String::from("session_started log_schema=3 segment=0\n");
        for i in 0..300u64 {
            // Pseudo-random hex pattern so gzip size is non-trivial
            oldest.push_str(&format!(
                "event level=info category=ui unix_nanos=1000 elapsed_nanos={i} sequence={} truncated=false message=\"seg0-{i}-{:016x}\"\n",
                i + 1,
                i.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ));
        }
        let mut middle = String::from("session_started log_schema=3 segment=1\n");
        for i in 0..300u64 {
            middle.push_str(&format!(
                "event level=info category=ui unix_nanos=2000 elapsed_nanos={i} sequence={} truncated=false message=\"seg1-{i}-{:016x}\"\n",
                301 + i,
                (i + 1000).wrapping_mul(0xBF58_476D_1CE4_E5B9)
            ));
        }
        let newest = concat!(
            "session_started log_schema=3 segment=2\n",
            "event level=info category=ui unix_nanos=3000 elapsed_nanos=1 sequence=601 truncated=false message=\"newest-marker\"\n",
        );
        fs::write(path.with_extension("2.log"), &oldest).unwrap();
        fs::write(path.with_extension("1.log"), &middle).unwrap();
        fs::write(&path, newest).unwrap();

        // Full budget: all segments preserved oldest-to-newest
        let full = build_report_bundle(&path, 4000, &ReportSliceConfig::default()).unwrap();
        assert!(!full.clipped_segments);
        let mut decoded = String::new();
        GzDecoder::new(full.log_gz_bytes.as_slice())
            .read_to_string(&mut decoded)
            .unwrap();
        let pos0 = decoded.find("seg0-0-").unwrap();
        let pos1 = decoded.find("seg1-0-").unwrap();
        let pos2 = decoded.find("newest-marker").unwrap();
        assert!(pos0 < pos1 && pos1 < pos2);

        // Tight gzip budget: must clip oldest segment(s) while staying valid gzip & keeping newest
        let tight = ReportSliceConfig {
            max_gzip_bytes: 512,
            ..ReportSliceConfig::default()
        };
        let clipped = build_report_bundle(&path, 4000, &tight).unwrap();
        assert!(clipped.clipped_segments);
        assert!(clipped.log_gz_bytes.len() <= 512);
        let mut decoded_tight = String::new();
        GzDecoder::new(clipped.log_gz_bytes.as_slice())
            .read_to_string(&mut decoded_tight)
            .unwrap();
        assert!(decoded_tight.contains("newest-marker"));
        assert!(!decoded_tight.contains("seg0-0-"));
        assert!(clipped.warnings.iter().any(|w| w.contains("clipped")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn handles_missing_active_log_and_concurrent_rotation_warnings() {
        let dir = crate::tests::fixture();
        assert!(
            build_report_bundle(&dir.join("missing.log"), 100, &ReportSliceConfig::default())
                .is_err()
        );

        let path = dir.join("session.log");
        // Missing .2.log is normal; segment jump 2 -> 4 and sequence gap 1 -> 5 + incomplete tail
        fs::write(
            path.with_extension("1.log"),
            "session_started log_schema=3 segment=2\nevent level=info category=ui unix_nanos=10 elapsed_nanos=1 sequence=1 truncated=false message=\"a\"\n",
        )
        .unwrap();
        fs::write(
            &path,
            "session_started log_schema=3 segment=4\nevent level=info category=ui unix_nanos=20 elapsed_nanos=2 sequence=5 truncated=true message=\"b\"\nincomplete-tail",
        )
        .unwrap();

        let bundle = build_report_bundle(&path, 100, &ReportSliceConfig::default()).unwrap();
        assert!(
            bundle
                .warnings
                .iter()
                .any(|w| w.contains("segments are missing"))
        );
        assert!(bundle.warnings.iter().any(|w| w.contains("sequence")));
        assert!(
            bundle
                .warnings
                .iter()
                .any(|w| w.contains("incomplete final line"))
        );
        assert!(bundle.warnings.iter().any(|w| w.contains("truncated")));
        fs::remove_dir_all(dir).unwrap();
    }
}

//! Bounded, control-side reading of the writer's rolling session files.
//! Callers run filesystem access and filtering off the UI and audio threads.
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use crate::{Level, MAX_EVENT_BYTES, MAX_SEGMENT_BYTES, RETAINED_SEGMENTS};

// A writer may put one bounded rollover header before a full segment. Older
// unbounded files are clipped rather than causing unbounded viewer allocation.
const SEGMENT_READ_LIMIT: u64 = MAX_SEGMENT_BYTES + (MAX_EVENT_BYTES as u64 * 6 + 1024);
const MAX_LINES: usize = 100_000;
const FILTERED_TEXT_LIMIT: usize = 1024 * 1024;
const FILTER_QUERY_LIMIT: usize = 4096;

/// An original complete log line, with event fields when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub text: String,
    pub event: Option<LogEvent>,
}

/// Fields used to filter an event without changing its original text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEvent {
    pub unix_nanos: u128,
    pub elapsed_nanos: u128,
    pub sequence: u64,
    pub level: String,
    pub category: String,
}

/// Snapshot in oldest-to-newest segment order. Warnings describe omitted data.
#[derive(Debug, Default)]
pub struct LogSnapshot {
    pub lines: Vec<LogLine>,
    pub warnings: Vec<String>,
    /// Session header when present, even if it falls outside the page.
    pub header: Option<String>,
}

/// Composable event filters. Non-event context and loss lines are retained.
#[derive(Debug, Default)]
pub struct LogFilter<'a> {
    /// Minimum severity threshold (`Some(Level::Error)` matches errors only;
    /// `Some(Level::Warn)` matches errors and warnings; `None` matches all levels).
    pub level: Option<Level>,
    pub category: Option<&'a str>,
    pub text: Option<&'a str>,
    pub since_unix_nanos: Option<u128>,
    pub until_unix_nanos: Option<u128>,
}
/// Pre-compiled event filter with reusable scratch capacity for zero-allocation
/// repeated matching across many log lines.
#[derive(Debug)]
pub struct CompiledLogFilter<'a> {
    level: Option<Level>,
    category: Option<&'a str>,
    lowercase_query: Option<String>,
    since_unix_nanos: Option<u128>,
    until_unix_nanos: Option<u128>,
    scratch: String,
}

impl<'a> CompiledLogFilter<'a> {
    /// Match one parsed [`LogLine`] reusing internal scratch capacity.
    pub fn matches(&mut self, line: &LogLine) -> bool {
        let Some(event) = line.event.as_ref() else {
            return true;
        };
        self.matches_parts(
            &line.text,
            Some((
                event.level.as_str(),
                event.category.as_str(),
                event.unix_nanos,
            )),
        )
    }

    /// Match one parsed [`LogLine`] using a caller-supplied scratch buffer.
    pub fn matches_with_scratch(&self, line: &LogLine, scratch: &mut String) -> bool {
        let Some(event) = line.event.as_ref() else {
            return true;
        };
        matches_event_fields(
            self.level,
            self.category,
            self.since_unix_nanos,
            self.until_unix_nanos,
            &line.text,
            Some((
                event.level.as_str(),
                event.category.as_str(),
                event.unix_nanos,
            )),
            self.lowercase_query.as_deref(),
            scratch,
        )
    }

    fn matches_parts(&mut self, text: &str, event: Option<(&str, &str, u128)>) -> bool {
        matches_event_fields(
            self.level,
            self.category,
            self.since_unix_nanos,
            self.until_unix_nanos,
            text,
            event,
            self.lowercase_query.as_deref(),
            &mut self.scratch,
        )
    }
}

impl<'a> LogFilter<'a> {
    /// Fold any text query once and allocate reusable scratch space for repeated matches.
    pub fn compile(&self) -> CompiledLogFilter<'a> {
        CompiledLogFilter {
            level: self.level,
            category: self.category,
            lowercase_query: self.text.map(str::to_lowercase),
            since_unix_nanos: self.since_unix_nanos,
            until_unix_nanos: self.until_unix_nanos,
            scratch: String::new(),
        }
    }

    pub fn matches(&self, line: &LogLine) -> bool {
        if line.event.is_none() {
            return true;
        }
        let query = self.text.map(str::to_lowercase);
        let mut scratch = String::new();
        self.matches_record(
            &line.text,
            line.event.as_ref().map(|event| {
                (
                    event.level.as_str(),
                    event.category.as_str(),
                    event.unix_nanos,
                )
            }),
            query.as_deref(),
            &mut scratch,
        )
    }

    fn matches_record(
        &self,
        text: &str,
        event: Option<(&str, &str, u128)>,
        query: Option<&str>,
        scratch: &mut String,
    ) -> bool {
        matches_event_fields(
            self.level,
            self.category,
            self.since_unix_nanos,
            self.until_unix_nanos,
            text,
            event,
            query,
            scratch,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn matches_event_fields(
    level: Option<Level>,
    category: Option<&str>,
    since_unix_nanos: Option<u128>,
    until_unix_nanos: Option<u128>,
    text: &str,
    event: Option<(&str, &str, u128)>,
    query: Option<&str>,
    scratch: &mut String,
) -> bool {
    let Some((event_level, event_category, unix_nanos)) = event else {
        return true;
    };
    level.is_none_or(|minimum| {
        Level::parse(event_level).is_some_and(|actual| actual.meets_minimum(minimum))
    }) && category.is_none_or(|expected| event_category == expected)
        && since_unix_nanos.is_none_or(|since| unix_nanos >= since)
        && until_unix_nanos.is_none_or(|until| unix_nanos <= until)
        && query.is_none_or(|query| contains_case_insensitive(text, query, scratch))
}

fn contains_case_insensitive(text: &str, lowercase_query: &str, scratch: &mut String) -> bool {
    if text.is_ascii() {
        scratch.clear();
        scratch.push_str(text);
        scratch.make_ascii_lowercase();
        scratch.contains(lowercase_query)
    } else {
        text.to_lowercase().contains(lowercase_query)
    }
}

/// One bounded page, numbered from the newest retained lines (page zero).
#[derive(Debug)]
pub struct LogPage {
    pub snapshot: LogSnapshot,
    pub page: usize,
    pub has_older: bool,
    pub has_newer: bool,
}

/// Oldest matching entries across the entire retained session, not one page.
/// Non-event context/loss lines remain present regardless of event filters.
#[derive(Debug)]
pub struct FilteredLog {
    pub snapshot: LogSnapshot,
    /// All matching events counted before export limits, excluding context.
    pub matching_events: usize,
    /// Matching/context entries omitted after reaching a result limit.
    pub omitted_lines: usize,
}

/// Read/filter the same bounded segment snapshot as the paged reader. Retain
/// at most 100,000 lines and 1 MiB of original line text/newlines; keep scanning
/// to count omissions and detect warnings after the retained prefix. Queries
/// are limited to 4096 bytes and folded once per scan. Time bounds are inclusive
/// absolute Unix nanoseconds; callers resolve a relative window once, before
/// dispatch. Missing/unreadable active files or archives other than absent
/// rollover slots return an error, never a falsely complete partial export.
/// Concurrent rotation remains detectable best-effort, not an atomic log freeze.
pub fn read_session_filtered(active: &Path, filter: &LogFilter<'_>) -> io::Result<FilteredLog> {
    if filter
        .text
        .is_some_and(|text| text.len() > FILTER_QUERY_LIMIT)
        || filter
            .since_unix_nanos
            .zip(filter.until_unix_nanos)
            .is_some_and(|(since, until)| since > until)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid or oversized log filter",
        ));
    }
    let (page, matching_events, omitted_lines) = read_selected(active, None, filter)?;
    Ok(FilteredLog {
        snapshot: page.snapshot,
        matching_events,
        omitted_lines,
    })
}

/// Read the newest page. Older retained lines are reported as omitted; use
/// [`read_session_page`] to navigate them.
pub fn read_session(active: &Path) -> io::Result<LogSnapshot> {
    let mut page = read_session_page(active, 0)?;
    if page.has_older {
        page.snapshot
            .warnings
            .push("older lines omitted at viewer limit".into());
    }
    Ok(page.snapshot)
}

/// Read one page of at most 100,000 lines, in oldest-to-newest order. The
/// function streams segments sequentially without holding all segment texts in
/// memory at once, retaining only the requested page. Missing archives are normal;
/// missing active files and other I/O failures are returned. A page request
/// beyond the available history is clamped and reported. Concurrent rotation
/// may change the snapshot; segment and sequence discontinuities are reported.
/// Page numbers can shift as a live session grows, so callers should tell users
/// that paging and refresh use a new snapshot.
pub fn read_session_page(active: &Path, requested_page: usize) -> io::Result<LogPage> {
    read_selected(active, Some(requested_page), &LogFilter::default()).map(|(page, _, _)| page)
}

pub(crate) fn open_segment(path: &Path, index: usize) -> io::Result<Option<File>> {
    match File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if index != 0 && error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{}: {error}", path.display()),
        )),
    }
}

pub(crate) fn segment_paths(active: &Path) -> Vec<(usize, PathBuf)> {
    (0..RETAINED_SEGMENTS)
        .rev()
        .map(|index| {
            let path = if index == 0 {
                active.to_owned()
            } else {
                active.with_extension(format!("{index}.log"))
            };
            (index, path)
        })
        .collect()
}

pub(crate) struct SegmentStreamStats {
    pub(crate) exceeded_limit: bool,
    pub(crate) incomplete_tail: bool,
    pub(crate) invalid_utf8: bool,
}

/// Stream complete `\n`-terminated lines from one segment up to `SEGMENT_READ_LIMIT`
/// bytes using a single reusable line buffer, never holding the entire 8 MiB segment
/// in memory at once.
pub(crate) fn stream_segment_lines(
    file: File,
    path: &Path,
    raw_line: &mut Vec<u8>,
    mut on_line: impl FnMut(&str),
) -> io::Result<SegmentStreamStats> {
    let mut reader = BufReader::new(file.take(SEGMENT_READ_LIMIT + 1));
    let mut bytes_seen = 0_u64;
    let mut exceeded_limit = false;
    let mut incomplete_tail = false;
    let mut invalid_utf8 = false;

    loop {
        raw_line.clear();
        let read = reader.read_until(b'\n', raw_line).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        let next_total = bytes_seen.saturating_add(read as u64);
        if next_total > SEGMENT_READ_LIMIT {
            exceeded_limit = true;
            let keep = SEGMENT_READ_LIMIT.saturating_sub(bytes_seen) as usize;
            raw_line.truncate(keep);
            if !raw_line.is_empty() && raw_line.last() != Some(&b'\n') {
                incomplete_tail = true;
                break;
            }
            if raw_line.is_empty() {
                break;
            }
        }
        bytes_seen = next_total;
        if raw_line.last() != Some(&b'\n') {
            incomplete_tail = true;
            break;
        }
        let mut slice = raw_line.as_slice();
        if let Some(stripped) = slice.strip_suffix(b"\n") {
            slice = stripped;
        }
        if let Some(stripped) = slice.strip_suffix(b"\r") {
            slice = stripped;
        }
        match std::str::from_utf8(slice) {
            Ok(line) => on_line(line),
            Err(_) => {
                invalid_utf8 = true;
                let lossy = String::from_utf8_lossy(slice);
                on_line(lossy.as_ref());
            }
        }
        if exceeded_limit {
            break;
        }
    }

    Ok(SegmentStreamStats {
        exceeded_limit,
        incomplete_tail,
        invalid_utf8,
    })
}

fn read_selected(
    active: &Path,
    requested_page: Option<usize>,
    filter: &LogFilter<'_>,
) -> io::Result<(LogPage, usize, usize)> {
    let mut snapshot = LogSnapshot::default();
    let paths = segment_paths(active);
    let mut raw_line = Vec::new();

    // Determine page window via a lightweight streaming line count only when paging is requested;
    // filtered exports (`requested_page == None`) run in a single sequential streaming pass.
    let (page, start, end) = if let Some(requested) = requested_page {
        let mut total = 0_usize;
        for &(index, ref path) in &paths {
            let Some(file) = open_segment(path, index)? else {
                continue;
            };
            let _ = stream_segment_lines(file, path, &mut raw_line, |_| {
                total = total.saturating_add(1);
            })?;
        }
        let last_page = total.saturating_sub(1) / MAX_LINES;
        let page = requested.min(last_page);
        if page != requested {
            snapshot
                .warnings
                .push("requested page is no longer available".into());
        }
        let end = total.saturating_sub(page.saturating_mul(MAX_LINES));
        let start = end.saturating_sub(MAX_LINES);
        (page, start, end)
    } else {
        (0, 0, usize::MAX)
    };

    let mut compiled = filter.compile();
    let mut matching_events = 0;
    let mut omitted_lines = 0;
    let mut retained_bytes = 0_usize;
    let mut position = 0usize;
    let mut previous_sequence: Option<u64> = None;
    let mut previous_segment: Option<u64> = None;
    let mut sequence_discontinuity = false;
    let mut truncated = false;

    for (index, path) in paths {
        let Some(file) = open_segment(&path, index)? else {
            continue;
        };
        let mut first_line = true;
        let mut malformed = false;
        let stats = stream_segment_lines(file, &path, &mut raw_line, |line| {
            if first_line {
                first_line = false;
                if line.starts_with("session_started ") {
                    if snapshot.header.is_none() {
                        let clipped: String = line.chars().take(4096).collect();
                        snapshot.header = Some(if clipped.len() < line.len() {
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
                            snapshot.warnings.push(
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
                truncated |= line.split_once(" message=").is_some_and(|(prefix, _)| {
                    prefix
                        .split_ascii_whitespace()
                        .any(|field| field == "truncated=true")
                });
            }
            if (start..end).contains(&position)
                && compiled.matches_parts(
                    line,
                    event
                        .as_ref()
                        .map(|event| (event.level, event.category, event.unix_nanos)),
                )
            {
                matching_events += usize::from(event.is_some());
                let size = retained_bytes.saturating_add(line.len()).saturating_add(1);
                if requested_page.is_none()
                    && (omitted_lines > 0
                        || snapshot.lines.len() >= MAX_LINES
                        || size > FILTERED_TEXT_LIMIT)
                {
                    omitted_lines += 1;
                } else {
                    retained_bytes = size;
                    snapshot.lines.push(LogLine {
                        text: line.to_owned(),
                        event: event.map(ParsedEvent::into_log_event),
                    });
                }
            }
            position += 1;
        })?;
        if stats.exceeded_limit {
            snapshot
                .warnings
                .push(format!("{}: segment exceeds read limit", path.display()));
        }
        if stats.incomplete_tail {
            snapshot
                .warnings
                .push(format!("{}: incomplete final line omitted", path.display()));
        }
        if stats.invalid_utf8 {
            snapshot
                .warnings
                .push(format!("{}: invalid utf-8 replaced", path.display()));
        }
        if malformed {
            snapshot
                .warnings
                .push(format!("{}: malformed events retained", path.display()));
        }
    }
    if sequence_discontinuity {
        snapshot.warnings.push("event sequence has a gap or out-of-order records; loss or concurrent rotation may have occurred".into());
    }
    if truncated {
        snapshot
            .warnings
            .push("one or more event messages were truncated by the logger".into());
    }
    if omitted_lines > 0 {
        snapshot.warnings.push(format!(
            "{omitted_lines} matching/context lines omitted at export limit (1 mib or 100000 lines)"
        ));
    }
    Ok((
        LogPage {
            snapshot,
            page,
            has_older: start > 0,
            has_newer: page > 0,
        },
        matching_events,
        omitted_lines,
    ))
}

pub(crate) struct ParsedEvent<'a> {
    pub(crate) unix_nanos: u128,
    pub(crate) elapsed_nanos: u128,
    pub(crate) sequence: u64,
    pub(crate) level: &'a str,
    pub(crate) category: &'a str,
}
impl ParsedEvent<'_> {
    fn into_log_event(self) -> LogEvent {
        LogEvent {
            unix_nanos: self.unix_nanos,
            elapsed_nanos: self.elapsed_nanos,
            sequence: self.sequence,
            level: self.level.to_owned(),
            category: self.category.to_owned(),
        }
    }
}

pub(crate) fn parse_event(line: &str) -> Option<ParsedEvent<'_>> {
    // Only inspect the prefix, never parse fields from a quoted message.
    let prefix = line.split_once(" message=")?.0;
    let field = |name: &str| {
        prefix
            .split_ascii_whitespace()
            .find_map(|part| part.strip_prefix(name))
    };
    Some(ParsedEvent {
        unix_nanos: field("unix_nanos=")?.parse().ok()?,
        elapsed_nanos: field("elapsed_nanos=")?.parse().ok()?,
        sequence: field("sequence=")?.parse().ok()?,
        level: field("level=").unwrap_or("info"),
        category: field("category=").unwrap_or("uncategorised"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reads_rotated_legacy_and_typed_records_and_filters_them() {
        let dir = crate::tests::fixture();
        let path = dir.join("session.log");
        fs::write(path.with_extension("1.log"), concat!(
            "session_started log_schema=2\n",
            "event unix_nanos=100 elapsed_nanos=10 sequence=1 truncated=false original_bytes=3 message=\"old\"\n",
            "diagnostics_loss queue_full=1\n",
        )).unwrap();
        fs::write(&path, concat!(
            "session_started log_schema=3\n",
            "event level=error category=audio unix_nanos=200 elapsed_nanos=20 sequence=2 truncated=false original_bytes=6 message=\"Device\"\n",
            "event level=info category=ui unix_nanos=300 elapsed_nanos=30 sequence=3 truncated=false original_bytes=2 message=\"ui\"\n",
        )).unwrap();
        let snapshot = read_session(&path).unwrap();
        assert!(snapshot.warnings.is_empty());
        let events: Vec<_> = snapshot
            .lines
            .iter()
            .filter_map(|line| line.event.as_ref())
            .collect();
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(events[0].category, "uncategorised");
        let filter = LogFilter {
            level: None,
            category: Some("audio"),
            text: Some("device"),
            since_unix_nanos: Some(150),
            until_unix_nanos: Some(250),
        };
        let selected: Vec<_> = snapshot
            .lines
            .iter()
            .filter(|line| line.event.is_some() && filter.matches(line))
            .collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].event.as_ref().unwrap().sequence, 2);
        assert!(
            snapshot
                .lines
                .iter()
                .any(|line| line.text.starts_with("diagnostics_loss") && filter.matches(line))
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retains_malformed_lines_and_reports_incomplete_tail() {
        let dir = crate::tests::fixture();
        let path = dir.join("session.log");
        fs::write(&path, b"event broken\nevent unix_nanos=1").unwrap();
        let snapshot = read_session(&path).unwrap();
        assert_eq!(snapshot.lines.len(), 1);
        assert_eq!(snapshot.lines[0].text, "event broken");
        assert_eq!(snapshot.warnings.len(), 2);
        assert!(read_session(&dir.join("missing.log")).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reports_sequence_gaps_and_truncated_messages() {
        let dir = crate::tests::fixture();
        let path = dir.join("session.log");
        fs::write(
            &path,
            concat!(
                "event unix_nanos=1 elapsed_nanos=1 sequence=1 truncated=false message=\"one\"\n",
                "event unix_nanos=2 elapsed_nanos=2 sequence=3 truncated=true message=\"cut\"\n",
            ),
        )
        .unwrap();
        let snapshot = read_session(&path).unwrap();
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("sequence"))
        );
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("truncated"))
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn line_limit_keeps_the_latest_records_and_reports_omissions() {
        let dir = crate::tests::fixture();
        let path = dir.join("session.log");
        let mut content = "oldest\n".to_owned();
        content.push_str(&"line\n".repeat(MAX_LINES));
        fs::write(&path, content).unwrap();
        let snapshot = read_session(&path).unwrap();
        assert_eq!(snapshot.lines.len(), MAX_LINES);
        assert_eq!(snapshot.lines[0].text, "line");
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("older lines omitted"))
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reports_missing_rolling_history() {
        let dir = crate::tests::fixture();
        let path = dir.join("session.log");
        fs::write(
            path.with_extension("1.log"),
            "session_started log_schema=3 segment=2\n",
        )
        .unwrap();
        fs::write(&path, "session_started log_schema=3 segment=4\n").unwrap();
        let snapshot = read_session(&path).unwrap();
        assert!(!snapshot.warnings.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pages_reach_older_segments_without_overlap_and_keep_header() {
        let dir = crate::tests::fixture();
        let path = dir.join("session.log");
        let mut older = "session_started log_schema=3 segment=0 build=fixture\nfirst\n".to_owned();
        older.push_str(&"old\n".repeat(MAX_LINES - 1));
        fs::write(path.with_extension("1.log"), older).unwrap();
        fs::write(&path, "session_started log_schema=3 segment=1\nnew\n").unwrap();
        let newest = read_session_page(&path, 0).unwrap();
        assert!(newest.has_older);
        assert!(!newest.has_newer);
        assert_eq!(newest.snapshot.lines.len(), MAX_LINES);
        assert_eq!(newest.snapshot.lines.last().unwrap().text, "new");
        assert!(
            newest
                .snapshot
                .header
                .as_deref()
                .unwrap()
                .contains("build=fixture")
        );
        let previous = read_session_page(&path, 1).unwrap();
        assert!(!previous.has_older);
        assert!(previous.has_newer);
        assert_eq!(previous.snapshot.lines.len(), 3);
        assert_eq!(previous.snapshot.lines[1].text, "first");
        let missing = read_session_page(&path, usize::MAX).unwrap();
        assert_eq!(missing.page, 1);
        assert!(
            missing
                .snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("no longer available"))
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn case_insensitive_filter_reuses_ascii_scratch_and_handles_unicode() {
        let filter = LogFilter {
            level: None,
            category: Some("audio"),
            since_unix_nanos: None,
            until_unix_nanos: None,
            text: Some("Über"),
        };
        let mut scratch = String::with_capacity(128);
        let initial_ptr = scratch.as_ptr();
        let initial_cap = scratch.capacity();

        // ASCII fast path matches case-insensitively and reuses scratch allocation.
        assert!(filter.matches_record(
            "event seq=1 AudioCallback Underrun",
            Some(("info", "audio", 10)),
            Some("underrun"),
            &mut scratch,
        ));
        assert!(!filter.matches_record(
            "event seq=2 AudioCallback Healthy",
            Some(("info", "audio", 11)),
            Some("underrun"),
            &mut scratch,
        ));
        assert_eq!(scratch.as_ptr(), initial_ptr);
        assert_eq!(scratch.capacity(), initial_cap);

        // Non-ASCII Unicode fallback preserves full case-folding semantics.
        assert!(filter.matches_record(
            "event seq=3 Loaded ÜBER_Synth.vst3",
            Some(("info", "audio", 12)),
            Some("über_synth"),
            &mut scratch,
        ));
        assert!(!filter.matches_record(
            "event seq=4 Loaded Straße.vst3",
            Some(("info", "audio", 13)),
            Some("über_synth"),
            &mut scratch,
        ));

        // Non-event context lines remain unfiltered regardless of text query.
        assert!(filter.matches_record("session_started", None, Some("nomatch"), &mut scratch));

        // CompiledLogFilter folds once and reuses caller or internal scratch cleanly.
        let line = LogLine {
            text: "event seq=5 AudioCallback Underrun".into(),
            event: Some(LogEvent {
                unix_nanos: 15,
                elapsed_nanos: 1,
                sequence: 5,
                level: "info".into(),
                category: "audio".into(),
            }),
        };
        let compiled_filter = LogFilter {
            category: Some("audio"),
            text: Some("UNDERRUN"),
            ..LogFilter::default()
        };
        let mut compiled = compiled_filter.compile();
        assert!(compiled.matches(&line));
        assert!(compiled.matches_with_scratch(&line, &mut scratch));
        assert_eq!(scratch.as_ptr(), initial_ptr);
    }
}

#[cfg(test)]
#[path = "reader/filtered_tests.rs"]
mod filtered_tests;

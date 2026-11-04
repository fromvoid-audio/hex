//! Bounded, control-side reading of the writer's rolling session files.
//! Callers run filesystem access and filtering off the UI and audio threads.
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use crate::{MAX_EVENT_BYTES, MAX_SEGMENT_BYTES, RETAINED_SEGMENTS};

// A writer may put one bounded rollover header before a full segment. Older
// unbounded files are clipped rather than causing unbounded viewer allocation.
const SEGMENT_READ_LIMIT: u64 = MAX_SEGMENT_BYTES + (MAX_EVENT_BYTES as u64 * 6 + 1024);
const MAX_LINES: usize = 100_000;

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
    pub category: Option<&'a str>,
    pub text: Option<&'a str>,
    pub since_unix_nanos: Option<u128>,
    pub until_unix_nanos: Option<u128>,
}
impl LogFilter<'_> {
    pub fn matches(&self, line: &LogLine) -> bool {
        let Some(event) = &line.event else {
            return true;
        };
        self.category
            .is_none_or(|category| event.category == category)
            && self
                .text
                .is_none_or(|text| line.text.to_lowercase().contains(&text.to_lowercase()))
            && self
                .since_unix_nanos
                .is_none_or(|since| event.unix_nanos >= since)
            && self
                .until_unix_nanos
                .is_none_or(|until| event.unix_nanos <= until)
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
/// function buffers at most four bounded segment texts, counts their complete
/// lines, then retains only the requested page. Missing archives are normal;
/// missing active files and other I/O failures are returned. A page request
/// beyond the available history is clamped and reported. Concurrent rotation
/// may change the snapshot; segment and sequence discontinuities are reported.
/// Page numbers can shift as a live session grows, so callers should tell users
/// that paging and refresh use a new snapshot.
pub fn read_session_page(active: &Path, requested_page: usize) -> io::Result<LogPage> {
    let mut snapshot = LogSnapshot::default();
    let mut segments = Vec::new();
    for index in (0..RETAINED_SEGMENTS).rev() {
        let path = if index == 0 {
            active.to_owned()
        } else {
            active.with_extension(format!("{index}.log"))
        };
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if index != 0 && error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("{}: {error}", path.display()),
                ));
            }
        };
        let mut bytes = Vec::new();
        file.take(SEGMENT_READ_LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > SEGMENT_READ_LIMIT {
            bytes.truncate(SEGMENT_READ_LIMIT as usize);
            snapshot
                .warnings
                .push(format!("{}: segment exceeds read limit", path.display()));
        }
        if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
            snapshot
                .warnings
                .push(format!("{}: incomplete final line omitted", path.display()));
            bytes.truncate(
                bytes
                    .iter()
                    .rposition(|b| *b == b'\n')
                    .map_or(0, |pos| pos + 1),
            );
        }
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => {
                snapshot
                    .warnings
                    .push(format!("{}: invalid utf-8 replaced", path.display()));
                String::from_utf8_lossy(error.as_bytes()).into_owned()
            }
        };
        segments.push((path, text));
    }
    let total: usize = segments.iter().map(|(_, text)| text.lines().count()).sum();
    let last_page = total.saturating_sub(1) / MAX_LINES;
    let page = requested_page.min(last_page);
    if page != requested_page {
        snapshot
            .warnings
            .push("requested page is no longer available".into());
    }
    let end = total.saturating_sub(page.saturating_mul(MAX_LINES));
    let start = end.saturating_sub(MAX_LINES);
    let mut position = 0usize;
    let mut previous_sequence: Option<u64> = None;
    let mut previous_segment: Option<u64> = None;
    let mut sequence_discontinuity = false;
    let mut truncated = false;
    for (path, text) in segments {
        if snapshot.header.is_none()
            && let Some(line) = text
                .lines()
                .next()
                .filter(|line| line.starts_with("session_started "))
            {
                let clipped: String = line.chars().take(4096).collect();
                snapshot.header = Some(if clipped.len() < line.len() {
                    format!("{clipped} [header clipped]")
                } else {
                    clipped
                });
        }
        if let Some(segment) = text
            .lines()
            .next()
            .filter(|line| line.starts_with("session_started "))
            .and_then(|line| {
                line.split_ascii_whitespace()
                    .find_map(|field| field.strip_prefix("segment="))
            })
            .and_then(|value| value.parse::<u64>().ok())
        {
            if previous_segment.map_or(segment != 0, |previous| {
                previous.checked_add(1) != Some(segment)
            }) {
                snapshot
                    .warnings
                    .push("session segments are missing or changed during reading".into());
            }
            previous_segment = Some(segment);
        }
        let mut malformed = false;
        for line in text.lines() {
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
            if (start..end).contains(&position) {
                snapshot.lines.push(LogLine {
                    text: line.to_owned(),
                    event: event.map(ParsedEvent::into_log_event),
                });
            }
            position += 1;
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
    Ok(LogPage {
        snapshot,
        page,
        has_older: start > 0,
        has_newer: page > 0,
    })
}

struct ParsedEvent<'a> {
    unix_nanos: u128,
    elapsed_nanos: u128,
    sequence: u64,
    level: &'a str,
    category: &'a str,
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

fn parse_event(line: &str) -> Option<ParsedEvent<'_>> {
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
}

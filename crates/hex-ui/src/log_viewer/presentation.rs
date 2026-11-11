//! Pure log excerpt and timestamp presentation.
use hex_diagnostics::reader::LogSnapshot;
use std::path::Path;

pub(super) const COPY_LIMIT: usize = 1024 * 1024;
pub(super) const COPY_CLIPPED: &str = "[excerpt clipped at 1 mib]\n";

pub(super) fn make_excerpt(
    snapshot: &LogSnapshot,
    rows: &[usize],
    path: &Path,
    category: &str,
    time: &str,
    query: &str,
    selection: (usize, usize),
) -> String {
    let mut result = format!(
        "hex log excerpt\nsession: {}\ncategory: {category}; time: {time}; search: {query}\n",
        path.display()
    );
    if let Some(header) = &snapshot.header {
        result.push_str(header);
        result.push('\n');
    } else {
        result.push_str("warning: session header unavailable\n");
    }
    for warning in &snapshot.warnings {
        result.push_str(&format!("warning: {warning}\n"));
    }
    let content_limit = COPY_LIMIT - COPY_CLIPPED.len();
    if result.len() > content_limit {
        let mut end = content_limit;
        while !result.is_char_boundary(end) {
            end -= 1;
        }
        result.truncate(end);
        result.push_str(COPY_CLIPPED);
        return result;
    }
    let (start, end) = selection;
    for index in start.min(end)..=start.max(end) {
        let Some(line) = rows.get(index).and_then(|i| snapshot.lines.get(*i)) else {
            continue;
        };
        if result
            .len()
            .saturating_add(line.text.len())
            .saturating_add(1)
            > content_limit
        {
            result.push_str(COPY_CLIPPED);
            break;
        }
        result.push_str(&line.text);
        result.push('\n');
    }
    result
}

pub(super) fn format_time(unix_nanos: u128) -> String {
    let millis = unix_nanos / 1_000_000;
    let day_millis = millis % 86_400_000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        day_millis / 3_600_000,
        day_millis / 60_000 % 60,
        day_millis / 1000 % 60,
        day_millis % 1000
    )
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_diagnostics::reader::LogLine;

    #[test]
    fn excerpt_copies_selected_visible_range_with_context_and_warning() {
        let snapshot = LogSnapshot {
            lines: vec![
                LogLine {
                    text: "session_started hex_version=1".into(),
                    event: None,
                },
                LogLine {
                    text: "first".into(),
                    event: None,
                },
                LogLine {
                    text: "hidden".into(),
                    event: None,
                },
                LogLine {
                    text: "last".into(),
                    event: None,
                },
            ],
            warnings: vec!["records were lost".into()],
            header: Some("session_started hex_version=1".into()),
        };
        let result = make_excerpt(
            &snapshot,
            &[1, 3],
            Path::new("/tmp/session.log"),
            "audio",
            "last 1 min",
            "error",
            (1, 0),
        );
        assert!(result.contains("session_started hex_version=1"));
        assert!(result.contains("warning: records were lost"));
        assert!(result.contains("first\nlast\n"));
        assert!(!result.contains("hidden"));
    }

    #[test]
    fn excerpt_clips_oversized_lines_and_reports_the_limit() {
        let snapshot = LogSnapshot {
            lines: vec![LogLine {
                text: "x".repeat(COPY_LIMIT + 1),
                event: None,
            }],
            warnings: Vec::new(),
            header: None,
        };
        let result = make_excerpt(
            &snapshot,
            &[0],
            Path::new("session.log"),
            "all",
            "entire session",
            "",
            (0, 0),
        );
        assert!(result.contains("excerpt clipped"));
        assert!(result.len() < COPY_LIMIT);
    }

    #[test]
    fn oversized_multibyte_warning_still_obeys_clipboard_limit() {
        let snapshot = LogSnapshot {
            lines: Vec::new(),
            warnings: vec!["é".repeat(COPY_LIMIT)],
            header: None,
        };
        let result = make_excerpt(
            &snapshot,
            &[],
            Path::new("session.log"),
            "all",
            "entire session",
            "",
            (0, 0),
        );
        assert!(result.len() <= COPY_LIMIT);
        assert!(result.ends_with(COPY_CLIPPED));
    }

    #[test]
    fn timestamp_is_utc_time_of_day() {
        assert_eq!(format_time(3_723_004_000_000), "01:02:03.004");
    }
}

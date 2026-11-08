use super::*;
use std::fs;

fn event(sequence: usize, category: &str, time: u128, message: &str) -> String {
    format!(
        "event level=info category={category} unix_nanos={time} elapsed_nanos=1 sequence={sequence} truncated=false message=\"{message}\"\n"
    )
}

#[test]
fn filtered_snapshot_reaches_matches_outside_the_current_page_in_retained_order() {
    let dir = crate::tests::fixture();
    let path = dir.join("session.log");
    let mut old = "session_started log_schema=3 segment=0 build=fixture\n".to_owned();
    old.push_str(&event(1, "audio", 1, "keep First"));
    for sequence in 2..=55_001 {
        old.push_str(&event(sequence, "ui", 2, "noise"));
    }
    fs::write(path.with_extension("1.log"), old).unwrap();
    let mut current = "session_started log_schema=3 segment=1 build=fixture\n".to_owned();
    for sequence in 55_002..=110_001 {
        current.push_str(&event(sequence, "ui", 3, "noise"));
    }
    current.push_str(&event(110_002, "audio", 4, "Keep last"));
    fs::write(&path, current).unwrap();
    let page = read_session_page(&path, 0).unwrap();
    assert!(page.has_older);
    assert!(
        !page
            .snapshot
            .lines
            .iter()
            .any(|line| line.text.contains("keep First"))
    );
    let result = read_session_filtered(
        &path,
        &LogFilter {
            category: Some("audio"),
            text: Some("KEEP"),
            ..LogFilter::default()
        },
    )
    .unwrap();
    assert_eq!((result.matching_events, result.omitted_lines), (2, 0));
    assert!(result.snapshot.warnings.is_empty());
    let events: Vec<_> = result
        .snapshot
        .lines
        .iter()
        .filter_map(|line| line.event.as_ref())
        .collect();
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        [1, 110_002]
    );
    assert!(
        result
            .snapshot
            .lines
            .iter()
            .any(|line| line.text.contains("keep First"))
    );
    assert!(result.snapshot.header.unwrap().contains("build=fixture"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn inclusive_absolute_window_and_empty_event_result_keep_original_context() {
    let dir = crate::tests::fixture();
    let path = dir.join("session.log");
    let mut text = "session_started log_schema=3 segment=0\n".to_owned();
    for (index, time) in [10, 20, 30, 40].into_iter().enumerate() {
        text.push_str(&event(index + 1, "audio", time, "Take original CASE"));
    }
    text.push_str("diagnostics_loss queue_full=1\nolder untyped context\n");
    fs::write(&path, &text).unwrap();
    let result = read_session_filtered(
        &path,
        &LogFilter {
            category: Some("audio"),
            text: Some("take"),
            since_unix_nanos: Some(20),
            until_unix_nanos: Some(30),
        },
    )
    .unwrap();
    assert_eq!(result.matching_events, 2);
    let times: Vec<_> = result
        .snapshot
        .lines
        .iter()
        .filter_map(|line| line.event.as_ref().map(|event| event.unix_nanos))
        .collect();
    assert_eq!(times, [20, 30]);
    assert!(
        result
            .snapshot
            .lines
            .iter()
            .any(|line| line.text.ends_with("Take original CASE\""))
    );
    let empty = read_session_filtered(
        &path,
        &LogFilter {
            category: Some("plugin"),
            ..LogFilter::default()
        },
    )
    .unwrap();
    assert_eq!((empty.matching_events, empty.omitted_lines), (0, 0));
    assert_eq!(empty.snapshot.lines.len(), 3);
    assert!(
        empty
            .snapshot
            .lines
            .iter()
            .any(|line| line.text.starts_with("diagnostics_loss"))
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        text,
        "reading never changes logs"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn export_limits_report_omissions_and_still_scan_later_loss_and_truncation() {
    let dir = crate::tests::fixture();
    let path = dir.join("session.log");
    let mut text = "session_started log_schema=3 segment=0\n".to_owned();
    for sequence in 1..=300 {
        text.push_str(&event(sequence, "audio", 1, &"x".repeat(4096)));
    }
    text.push_str("event level=warn category=audio unix_nanos=2 elapsed_nanos=1 sequence=302 truncated=true message=\"late truncation\"\npartial final line");
    fs::write(&path, &text).unwrap();
    let result = read_session_filtered(&path, &LogFilter::default()).unwrap();
    assert_eq!(result.matching_events, 301);
    assert!(result.omitted_lines > 0);
    assert!(
        result
            .snapshot
            .lines
            .iter()
            .map(|line| line.text.len() + 1)
            .sum::<usize>()
            <= FILTERED_TEXT_LIMIT
    );
    for reason in ["export limit", "sequence", "truncated", "incomplete"] {
        assert!(
            result
                .snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains(reason)),
            "{reason}: {:?}",
            result.snapshot.warnings
        );
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    fs::write(&path, "\n".repeat(MAX_LINES + 1)).unwrap();
    let lines = read_session_filtered(&path, &LogFilter::default()).unwrap();
    assert_eq!(
        (lines.snapshot.lines.len(), lines.omitted_lines),
        (MAX_LINES, 1)
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unreadable_segments_and_invalid_filters_refuse_without_claiming_complete_export() {
    let dir = crate::tests::fixture();
    let path = dir.join("session.log");
    assert!(read_session_filtered(&path, &LogFilter::default()).is_err());
    fs::write(
        &path,
        b"session_started log_schema=3 segment=0\ninvalid \xff\n",
    )
    .unwrap();
    let result = read_session_filtered(&path, &LogFilter::default()).unwrap();
    assert!(
        result
            .snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("utf-8"))
    );
    fs::create_dir(path.with_extension("1.log")).unwrap();
    assert!(
        read_session_filtered(&path, &LogFilter::default())
            .unwrap_err()
            .to_string()
            .contains("session.1.log")
    );
    let query = "x".repeat(FILTER_QUERY_LIMIT + 1);
    for filter in [
        LogFilter {
            text: Some(&query),
            ..LogFilter::default()
        },
        LogFilter {
            since_unix_nanos: Some(2),
            until_unix_nanos: Some(1),
            ..LogFilter::default()
        },
    ] {
        assert_eq!(
            read_session_filtered(&path, &filter).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
    fs::remove_dir_all(dir).unwrap();
}

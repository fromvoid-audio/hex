use super::*;
#[cfg(feature = "ui-test-support")]
use std::time::Duration;

#[test]
fn filtered_excerpt_reports_empty_limits_and_failed_clipboard_confirmation() {
    let dir = std::env::temp_dir().join(format!("hex-filtered-copy-policy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.log");
    std::fs::write(
        &path,
        "session_started log_schema=3 segment=0 build=fixture\n",
    )
    .unwrap();
    let request = ViewerFilter::new(
        "audio",
        "keep".into(),
        ("last 30 s", Some(30)),
        40_000_000_000,
    );
    let copy = make_filtered_copy(&path, &request).unwrap();
    assert!(copy.text.contains("build=fixture"));
    assert!(
        copy.text
            .contains("inclusive unix_nanos 10000000000..40000000000")
    );
    assert!(copy.text.contains("matching event records: 0"));
    assert!(copy.text.contains("all retained pages"));
    assert!(filtered_copy_status(&copy, Some(&copy.text)).contains("no matching events"));
    for actual in [None, Some("foreign clipboard content")] {
        assert!(filtered_copy_status(&copy, actual).contains("could not verify"));
    }
    std::fs::write(&path, "").unwrap();
    let missing_header = make_filtered_copy(&path, &request).unwrap();
    assert!(missing_header.warnings && missing_header.text.contains("session header unavailable"));
    assert!(
        filtered_copy_status(&missing_header, Some(&missing_header.text)).contains("with warnings")
    );
    std::fs::write(
        &path,
        format!(
            "session_started log_schema=3 segment=0\n{}",
            format!("{}\n", "x".repeat(4096)).repeat(300)
        ),
    )
    .unwrap();
    let limited = make_filtered_copy(&path, &request).unwrap();
    assert!(limited.clipped && limited.warnings);
    assert!(limited.text.len() <= COPY_LIMIT);
    assert!(limited.text.contains("omitted at export limit"));
    assert!(filtered_copy_status(&limited, Some(&limited.text)).contains("incomplete"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(feature = "ui-test-support")]
#[gpui::test]
fn copy_all_uses_frozen_filters_and_real_clipboard_and_read_failures_keep_previous_copy(
    cx: &mut gpui::TestAppContext,
) {
    let dir = std::env::temp_dir().join(format!("hex-filtered-copy-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session-500-1.log");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let second = Duration::from_secs(1).as_nanos();
    let records = [
        ("audio", now - 60 * second, "keep old"),
        ("audio", now - 10 * second, "Keep Recent CASE [redacted]"),
        ("ui", now - 10 * second, "keep wrong category"),
        ("audio", now - 10 * second, "different text"),
        ("audio", now + 60 * second, "keep future"),
    ];
    let mut content = "session_started log_schema=3 segment=0 build=fixture\n".to_owned();
    for (index, (category, time, message)) in records.iter().enumerate() {
        let sequence = index + 1;
        content.push_str(&format!("event level=info category={category} unix_nanos={time} elapsed_nanos={sequence} sequence={sequence} truncated=false message=\"{message}\"\n"));
    }
    std::fs::write(&path, &content).unwrap();
    let handle = cx.add_window(|_, cx| Viewer::new(path.clone(), cx));
    cx.run_until_parked();
    let viewer = handle.root(cx).unwrap();
    viewer.update(cx, |view, cx| {
        view.category = CATEGORIES
            .iter()
            .position(|category| *category == "audio")
            .unwrap();
        view.window = WINDOWS
            .iter()
            .position(|window| window.1 == Some(30))
            .unwrap();
        view.query
            .update(cx, |query, cx| query.set_text("keep", cx));
        view.filter(cx);
    });
    cx.run_until_parked();
    viewer.read_with(cx, |view, _| {
        let snapshot = view.snapshot.as_ref().unwrap();
        let visible: Vec<_> = view
            .rows
            .iter()
            .filter_map(|i| snapshot.lines[*i].event.as_ref())
            .map(|event| event.sequence)
            .collect();
        assert_eq!(
            visible,
            [2],
            "the view uses wall time, not the future page endpoint"
        );
    });
    let visual = &mut gpui::VisualTestContext::from_window(*handle, cx);
    let button = visual
        .debug_bounds("log-copy-all")
        .expect("discoverable copy all action");
    visual.simulate_click(button.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    viewer.read_with(visual, |view, cx| {
        assert!(!view.copying);
        assert!(view.status.contains("all retained matching logs copied"));
        let copied = cx.read_from_clipboard().unwrap().text().unwrap();
        assert!(copied.contains("build=fixture") && copied.contains("category: audio"));
        assert!(
            copied.contains("matching event records: 1")
                && copied.contains("Keep Recent CASE [redacted]")
        );
        for excluded in [
            "keep old",
            "keep wrong category",
            "different text",
            "keep future",
        ] {
            assert!(!copied.contains(excluded));
        }
    });
    viewer.update(visual, |view, cx| {
        view.copy_all(cx);
        view.category = CATEGORIES
            .iter()
            .position(|category| *category == "ui")
            .unwrap();
        view.window = 0;
        view.query
            .update(cx, |query, cx| query.set_text("different request", cx));
        view.filter(cx);
    });
    visual.run_until_parked();
    viewer.read_with(visual, |_, cx| {
        let copied = cx.read_from_clipboard().unwrap().text().unwrap();
        assert!(copied.contains("category: audio") && copied.contains("Keep Recent CASE"));
        assert!(
            !copied.contains("different request"),
            "an in-flight copy keeps its action's filters"
        );
    });
    viewer.update(visual, |view, cx| view.copy_all(cx));
    visual.run_until_parked();
    viewer.read_with(visual, |view, _| {
        assert!(view.status.contains("no matching events"))
    });
    std::fs::remove_file(&path).unwrap();
    viewer.update(visual, |view, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("previous clipboard".into()));
        view.copy_all(cx);
    });
    visual.run_until_parked();
    viewer.read_with(visual, |view, cx| {
        assert!(!view.copying && view.status.contains("could not copy filtered logs"));
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("previous clipboard")
        );
    });
    assert!(
        !path.exists(),
        "copy never recreates or rewrites source logs"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

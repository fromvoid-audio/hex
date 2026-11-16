//! Fixed, owned diagnostic filters for a single viewer action.
use hex_diagnostics::reader::LogFilter;
use std::time::Duration;

/// One action's owned filters, with a fixed inclusive relative-time window.
/// Moving between pages never reanchors a range to that page's last event.
pub(super) struct ViewerFilter {
    pub(super) category: &'static str,
    pub(super) query: String,
    pub(super) window: &'static str,
    pub(super) since: Option<u128>,
    pub(super) until: Option<u128>,
}
impl ViewerFilter {
    pub(super) fn new(
        category: &'static str,
        query: String,
        window: (&'static str, Option<u64>),
        now: u128,
    ) -> Self {
        Self {
            category,
            query,
            window: window.0,
            since: window
                .1
                .map(|seconds| now.saturating_sub(Duration::from_secs(seconds).as_nanos())),
            until: window.1.map(|_| now),
        }
    }
    pub(super) fn borrowed(&self) -> LogFilter<'_> {
        LogFilter {
            category: (self.category != "all").then_some(self.category),
            text: (!self.query.is_empty()).then_some(self.query.as_str()),
            since_unix_nanos: self.since,
            until_unix_nanos: self.until,
        }
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::ViewerFilter;

    #[test]
    fn relative_window_is_fixed_inclusive_and_does_not_follow_page_timestamps() {
        let now = 40_000_000_000;
        let request = ViewerFilter::new("audio", "keep".into(), ("last 30 s", Some(30)), now);
        assert_eq!(request.window, "last 30 s");
        let filter = request.borrowed();
        for (time, expected) in [
            (9_999_999_999, false),
            (10_000_000_000, true),
            (now, true),
            (now + 1, false),
        ] {
            let line = hex_diagnostics::reader::LogLine {
                text: "KEEP original".into(),
                event: Some(hex_diagnostics::reader::LogEvent {
                    unix_nanos: time,
                    elapsed_nanos: 1,
                    sequence: 1,
                    level: "info".into(),
                    category: "audio".into(),
                }),
            };
            assert_eq!(filter.matches(&line), expected, "{time}");
            assert!(
                ViewerFilter::new("all", "".into(), ("entire session", None), now)
                    .borrowed()
                    .matches(&line)
            );
        }
        let early = ViewerFilter::new("all", "".into(), ("last 30 s", Some(30)), 1);
        assert_eq!((early.since, early.until), (Some(0), Some(1)));
    }
}

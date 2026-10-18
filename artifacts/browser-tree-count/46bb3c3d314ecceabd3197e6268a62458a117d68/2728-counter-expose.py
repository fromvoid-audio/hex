from pathlib import Path
p=Path('crates/hex-ui/src/editor/browser/samples.rs');s=p.read_text();old='''    pub(super) fn sample_filters(&self, cx: &mut Context<Self>) -> Div {
        let count = self.browser_visible_entries(cx).len();''';new='''    pub(super) fn sample_results_count_text(&self, cx: &App) -> String {
        let count = self.browser_visible_entries(cx).len();
        format!(
            "{count} samples{}",
            if self.browser.samples.in_flight {
                " · scanning…"
            } else {
                ""
            }
        )
    }

    pub(super) fn sample_filters(&self, cx: &mut Context<Self>) -> Div {''';assert old in s;s=s.replace(old,new)
old='''                        .child(theme::text(format!(
                            "{count} samples{}",
                            if self.browser.samples.in_flight {
                                " · scanning…"
                            } else {
                                ""
                            }
                        )))''';new='''                        .child(theme::text(self.sample_results_count_text(cx)))''';assert old in s;s=s.replace(old,new);p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser_ui_test.rs');p.write_text(p.read_text()+Path('/tmp/2728-counter-test.rs').read_text())

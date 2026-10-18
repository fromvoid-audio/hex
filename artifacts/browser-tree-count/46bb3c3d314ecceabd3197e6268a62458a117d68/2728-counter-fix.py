from pathlib import Path
p=Path('crates/hex-ui/src/editor/browser/samples.rs');s=p.read_text();anchor='struct VisibleCache {'
addition='''/// The Samples listing model shared by row construction and its count label.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ResultsMode {
    Hierarchy,
    Indexed,
}

impl ResultsMode {
    pub(super) fn for_query(query: &str) -> Self {
        if query.is_empty() {
            Self::Hierarchy
        } else {
            Self::Indexed
        }
    }

    fn count_noun(self) -> &'static str {
        match self {
            Self::Hierarchy => "items",
            Self::Indexed => "samples",
        }
    }
}

''';assert anchor in s;s=s.replace(anchor,addition+anchor)
old='''        let count = self.browser_visible_entries(cx).len();
        format!(
            "{count} samples{}",''';new='''        let count = self.browser_visible_entries(cx).len();
        let noun = ResultsMode::for_query(&self.browser.search.read(cx).content).count_noun();
        format!(
            "{count} {noun}{}",''';assert old in s;s=s.replace(old,new);p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser.rs');s=p.read_text();old='''            BrowserSection::Samples if query.is_empty() => {''';new='''            BrowserSection::Samples
                if samples::ResultsMode::for_query(&query) == samples::ResultsMode::Hierarchy =>
            {''';assert old in s;s=s.replace(old,new);p.write_text(s)

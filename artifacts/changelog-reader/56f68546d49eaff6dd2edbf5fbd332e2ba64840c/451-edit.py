from pathlib import Path
p=Path('crates/hex-ui/src/editor/changelog.rs');s=p.read_text()
s=s.replace('    lines: Vec<String>,','    document: Document,\n    #[cfg(all(test, feature = "ui-test-support"))]\n    painted: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, (gpui::Hsla, gpui::SharedString)>>> ,')
a=s.index('fn notes(');b=s.index('\nimpl State {',a)
s=s[:a]+'''#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Category { Features, Improvements, Fixes }
impl Category {
    const ALL: [Self; 3] = [Self::Features, Self::Improvements, Self::Fixes];
    fn label(self) -> &'static str { match self { Self::Features => "features", Self::Improvements => "improvements", Self::Fixes => "fixes" } }
    fn index(self) -> usize { match self { Self::Features => 0, Self::Improvements => 1, Self::Fixes => 2 } }
    fn color(self) -> PaletteRole { match self { Self::Features => PaletteRole::Secondary, Self::Improvements => PaletteRole::Text, Self::Fixes => PaletteRole::Primary } }
}
struct Batch {
    version: String,
    source: String,
    details: String,
    groups: [Vec<String>; 3],
}
struct Document { batches: Vec<Batch>, message: Option<String> }
impl Document {
    fn message(message: impl Into<String>) -> Self { Self { batches: Vec::new(), message: Some(message.into()) } }
}

// Preserve the existing input validation; only the display projection changes.
fn notes(raw: &str) -> Result<Document, &'static str> {
    if raw.is_empty() { return Ok(Document::message("changelog unavailable for this build")); }
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| "invalid changelog data")?;
    if value["format"].as_u64() != Some(1) { return Err("unsupported changelog data"); }
    let batches = value["batches"].as_array().ok_or("invalid changelog history")?;
    let mut result = Vec::new();
    for batch in batches.iter().rev() {
        let version = batch["input"]["version"].as_str().ok_or("missing changelog version")?;
        let source = batch["input"]["source"].as_str()
            .filter(|v| v.len() == 40 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("missing changelog coverage")?;
        let entries = batch["result"]["entries"].as_array().ok_or("invalid changelog entries")?;
        let mut groups: [Vec<String>; 3] = Default::default();
        for entry in entries {
            let category = entry["category"].as_str()
                .and_then(|v| Category::ALL.into_iter().find(|category| category.label() == v))
                .ok_or("invalid changelog category")?;
            let text = entry["text"].as_str().filter(|v| !v.is_empty() && v.chars().count() <= 320
                && v.to_lowercase() == *v && !v.contains(['\\n', '\\r']))
                .ok_or("invalid changelog entry")?;
            groups[category.index()].push(text.to_owned());
        }
        // Keep full original coverage/provenance available without repeating it
        // before each note. These are display details, never regenerated claims.
        let mut details = format!("version {version} · notes through {source}");
        for (label, field) in [("coverage starts at", &batch["input"]["base"]),
            ("generated", &batch["generated_at"]), ("generator", &batch["generator"]),
            ("origin", &batch["execution"]["origin"]), ("evidence", &batch["execution"]["evidence_commit"])] {
            if let Some(value) = field.as_str() { details.push_str(&format!(" · {label} {value}")); }
        }
        result.push(Batch { version: version.to_owned(), source: source.to_owned(), details, groups });
    }
    if result.is_empty() { return Ok(Document::message("changelog unavailable for this build")); }
    Ok(Document { batches: result, message: None })
}
'''+s[b:]
s=s.replace('''        let lines = notes(include_str!(concat!(env!("OUT_DIR"), "/changelog.json")))
            .unwrap_or_else(|error| vec![format!("changelog unavailable · {error}")]);''','''        let document = notes(include_str!(concat!(env!("OUT_DIR"), "/changelog.json")))
            .unwrap_or_else(|error| Document::message(format!("changelog unavailable · {error}")));''').replace('            lines,','            document,\n            #[cfg(all(test, feature = "ui-test-support"))]\n            painted: Default::default(),')
a=s.index('    pub(super) fn changelog_overlay(');b=s.index('\n#[cfg(test)]\nmod tests',a)
s=s[:a]+'''    // Test-only paint observation reads the actual inherited text style during
    // GPUI paint. It neither supplies hover state nor changes production input.
    #[cfg(all(test, feature = "ui-test-support"))]
    fn changelog_paint_probe(&self, key: String) -> impl gpui::IntoElement {
        let painted = std::rc::Rc::clone(&self.changelog.painted);
        gpui::canvas(|_, _, _| {}, move |_, _, window, _| {
            let style = window.text_style();
            painted.borrow_mut().insert(key, (style.color, style.font_family));
        }).absolute().inset_0()
    }

    fn changelog_note(&self, batch: usize, category: Category, index: usize, text: &str) -> Stateful<Div> {
        let key = format!("changelog-note-{batch}-{}-{index}", category.label());
        let selector = key.clone();
        let row = div().id(gpui::SharedString::from(key.clone()))
            .debug_selector(move || selector.clone())
            .relative().w_full().min_w_0().flex_shrink_0()
            .px_2().py_1().text_sm().line_height(gpui::relative(1.4))
            .text_color(rgb(theme::color(PaletteRole::Text)))
            .bg(rgb(theme::color(PaletteRole::Surface)))
            .cursor(gpui::CursorStyle::Arrow)
            .hover(|style| style.text_color(rgb(theme::color(PaletteRole::Secondary)))
                .bg(rgb(theme::color(PaletteRole::Hover))))
            .child(theme::text(text));
        #[cfg(all(test, feature = "ui-test-support"))]
        let row = row.child(self.changelog_paint_probe(key));
        row
    }

    fn changelog_group(&self, batch: usize, category: Category, entries: &[String]) -> Div {
        let key = format!("changelog-heading-{batch}-{}", category.label());
        let selector = key.clone();
        let heading = div().id(gpui::SharedString::from(key.clone()))
            .debug_selector(move || selector.clone())
            .relative().w_full().min_w_0().px_2().py_1().text_xs().font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(theme::color(category.color())))
            .border_b_1().border_color(rgb(theme::color(PaletteRole::Border)))
            .child(theme::text(category.label()));
        #[cfg(all(test, feature = "ui-test-support"))]
        let heading = heading.child(self.changelog_paint_probe(key));
        div().w_full().min_w_0().flex_shrink_0().flex().flex_col().gap(px(2.0))
            .child(heading)
            .children(entries.iter().enumerate().map(|(index, text)| self.changelog_note(batch, category, index, text)))
    }

    pub(super) fn changelog_overlay(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if !self.changelog.open { return None; }
        let mut notes = div().id("changelog-notes")
            .debug_selector(|| "changelog-notes".into())
            .flex_1().min_h_0().min_w_0().overflow_y_scroll().track_scroll(&self.changelog.scroll)
            .flex().flex_col().gap_3();
        if let Some(message) = &self.changelog.document.message {
            notes = notes.child(div().debug_selector(|| "changelog-unavailable".into())
                .text_sm().text_color(rgb(theme::color(PaletteRole::Muted))).child(theme::text(message)));
        }
        for (batch_index, batch) in self.changelog.document.batches.iter().enumerate() {
            let details = theme::text(&batch.details);
            let mut body = div().id(("changelog-batch", batch_index))
                .debug_selector(move || format!("changelog-batch-{batch_index}"))
                .w_full().min_w_0().flex_shrink_0().flex().flex_col().gap_2()
                .child(div().flex().flex_wrap().items_center().gap_2().px_2()
                    .child(div().text_sm().font_weight(gpui::FontWeight::BOLD).child(theme::text(&batch.version)))
                    .child(div().text_xs().text_color(rgb(theme::color(PaletteRole::Muted)))
                        .child(theme::text(format!("notes through {}", &batch.source[..8]))))
                    .tooltip(move |_, cx| cx.new(|_| super::transport::MiniTooltip { description: details.clone() }).into()));
            let mut count = 0;
            for category in Category::ALL {
                let entries = &batch.groups[category.index()];
                count += entries.len();
                if !entries.is_empty() { body = body.child(self.changelog_group(batch_index, category, entries)); }
            }
            if count == 0 {
                body = body.child(div().debug_selector(|| "changelog-empty".into()).px_2().text_sm()
                    .text_color(rgb(theme::color(PaletteRole::Muted)))
                    .child(theme::text("no user-facing changes in this update")));
            }
            notes = notes.child(body);
        }
        Some(div().id("changelog-overlay").debug_selector(|| "changelog-overlay".into())
            .absolute().inset_0().pt(px(super::header::TITLE_HEIGHT)).flex().justify_center()
            .bg(rgb(theme::color(PaletteRole::Background)))
            .track_focus(&self.changelog.focus).key_context("hex_changelog")
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    view.changelog.close_focus.focus(window);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().debug_selector(|| "changelog-reader".into()).w_full().max_w(px(640.0)).min_w_0()
                .h_full().px_3().py_2().flex().flex_col().gap_2()
                .child(div().flex().flex_shrink_0().justify_between().items_center()
                    .child(div().text_sm().font_weight(gpui::FontWeight::BOLD).child(theme::text("changelog")))
                    .child(div().id("close-changelog").debug_selector(|| "close-changelog".into())
                        .track_focus(&self.changelog.close_focus).tab_index(0).key_context("hex_changelog_close")
                        .focus(|style| style.bg(rgb(theme::color(PaletteRole::Hover))))
                        .px_2().py_1().cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::color(PaletteRole::Hover))))
                        .child(theme::text("close · esc"))
                        .on_click(cx.listener(|view, _, window, cx| view.close_changelog(window, cx)))))
                .child(notes)))
    }
}
'''+s[b:]
# Keep the original keyboard/scroll regressions, adapting only their display fixture/assertions.
s=s.replace('''        assert_eq!(notes("").unwrap(), ["changelog unavailable for this build"]);''','''        assert_eq!(notes("").unwrap().message.as_deref(), Some("changelog unavailable for this build"));''')
s=s.replace('''        assert_eq!(parsed[1], "no user-facing changes in this update");
        assert!(parsed[0].contains("notes through 01234567"));''','''        assert!(parsed.batches[0].groups.iter().all(Vec::is_empty));
        assert!(parsed.batches[0].details.contains("notes through 01234567"));''')
s=s.replace('''        assert_eq!(notes(raw).unwrap()[1], "fixes · open projects in one step");''','''        assert_eq!(notes(raw).unwrap().batches[0].groups[2], ["open projects in one step"]);''')
s=s.replace('''            view.changelog.lines = vec![
                "0.1.0 · notes through 01234567".into(),
                "fixes · project opening responds on the first click".into(),
            ];''','''            view.changelog.document = notes(r#"{"format":1,"batches":[{"input":{"version":"0.1.0","source":"0123456789012345678901234567890123456789"},"result":{"entries":[{"category":"fixes","text":"project opening responds on the first click"}]}}]}"#).unwrap();''')
s=s.replace('"changelog-line-1"','"changelog-note-0-features-0"')
a=s.index('            assert!(view.changelog.lines.len() > 1);');b=s.index('\n        });',a)
s=s[:a]+'''            assert!(!view.changelog.document.batches.is_empty());
            assert!(view.changelog.document.batches[0].details.contains("notes through"));
            assert!(view.changelog.document.message.is_none());'''+s[b:]
s=s.replace('''            view.changelog.lines = (0..100)
                .map(|n| format!("fixes · example user-facing fixture note {n}"))
                .collect();''','''            view.changelog.document = notes(include_str!(concat!(env!("OUT_DIR"), "/changelog.json"))).unwrap();
            view.changelog.document.batches[0].groups = [Vec::new(), Vec::new(), (0..100).map(|n| format!("example user-facing fixture note {n}")).collect()];''')
p.write_text(s)

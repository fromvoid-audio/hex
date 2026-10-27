//! Offline, build-embedded release notes. No inference or filesystem work in paint.
use super::EditorView;
use crate::theme::{self, PaletteRole};
use gpui::{
    App, Context, Div, FocusHandle, KeyBinding, Stateful, Window, div, prelude::*, px, rgb,
};

gpui::actions!(hex_changelog, [OpenChangelog, CloseChangelog]);

#[cfg(all(test, feature = "ui-test-support"))]
type PaintSamples = std::rc::Rc<
    std::cell::RefCell<std::collections::HashMap<String, (gpui::Hsla, gpui::SharedString)>>,
>;

pub(super) struct State {
    open: bool,
    focus: FocusHandle,
    close_focus: FocusHandle,
    document: Document,
    #[cfg(all(test, feature = "ui-test-support"))]
    painted: PaintSamples,
    scroll: gpui::ScrollHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Category {
    Features,
    Improvements,
    Fixes,
}
impl Category {
    const ALL: [Self; 3] = [Self::Features, Self::Improvements, Self::Fixes];
    fn label(self) -> &'static str {
        match self {
            Self::Features => "features",
            Self::Improvements => "improvements",
            Self::Fixes => "fixes",
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Features => 0,
            Self::Improvements => 1,
            Self::Fixes => 2,
        }
    }
    fn color(self) -> PaletteRole {
        match self {
            Self::Features => PaletteRole::Secondary,
            Self::Improvements => PaletteRole::Text,
            Self::Fixes => PaletteRole::Primary,
        }
    }
}
struct Batch {
    version: String,
    source: String,
    details: String,
    groups: [Vec<String>; 3],
}
struct Document {
    batches: Vec<Batch>,
    message: Option<String>,
}
impl Document {
    fn message(message: impl Into<String>) -> Self {
        Self {
            batches: Vec::new(),
            message: Some(message.into()),
        }
    }
}

// Preserve the existing input validation; only the display projection changes.
fn notes(raw: &str) -> Result<Document, &'static str> {
    if raw.is_empty() {
        return Ok(Document::message("changelog unavailable for this build"));
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| "invalid changelog data")?;
    if value["format"].as_u64() != Some(1) {
        return Err("unsupported changelog data");
    }
    let batches = value["batches"]
        .as_array()
        .ok_or("invalid changelog history")?;
    let mut result = Vec::new();
    for batch in batches.iter().rev() {
        let version = batch["input"]["version"]
            .as_str()
            .ok_or("missing changelog version")?;
        let source = batch["input"]["source"]
            .as_str()
            .filter(|v| v.len() == 40 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("missing changelog coverage")?;
        let entries = batch["result"]["entries"]
            .as_array()
            .ok_or("invalid changelog entries")?;
        let mut groups: [Vec<String>; 3] = Default::default();
        for entry in entries {
            let category = entry["category"]
                .as_str()
                .and_then(|v| {
                    Category::ALL
                        .into_iter()
                        .find(|category| category.label() == v)
                })
                .ok_or("invalid changelog category")?;
            let text = entry["text"]
                .as_str()
                .filter(|v| {
                    !v.is_empty()
                        && v.chars().count() <= 320
                        && v.to_lowercase() == *v
                        && !v.contains(['\n', '\r'])
                })
                .ok_or("invalid changelog entry")?;
            groups[category.index()].push(text.to_owned());
        }
        // Keep full original coverage/provenance available without repeating it
        // before each note. These are display details, never regenerated claims.
        let mut details = format!("version {version} · notes through {source}");
        for (label, field) in [
            ("coverage starts at", &batch["input"]["base"]),
            ("generated", &batch["generated_at"]),
            ("generator", &batch["generator"]),
            ("origin", &batch["execution"]["origin"]),
            ("evidence", &batch["execution"]["evidence_commit"]),
        ] {
            if let Some(value) = field.as_str() {
                details.push_str(&format!(" · {label} {value}"));
            }
        }
        result.push(Batch {
            version: version.to_owned(),
            source: source.to_owned(),
            details,
            groups,
        });
    }
    if result.is_empty() {
        return Ok(Document::message("changelog unavailable for this build"));
    }
    Ok(Document {
        batches: result,
        message: None,
    })
}

impl State {
    pub(super) fn is_open(&self) -> bool {
        self.open
    }
    pub(super) fn new(cx: &mut Context<EditorView>) -> Self {
        let document = notes(include_str!(concat!(env!("OUT_DIR"), "/changelog.json")))
            .unwrap_or_else(|error| Document::message(format!("changelog unavailable · {error}")));
        Self {
            open: false,
            focus: cx.focus_handle(),
            close_focus: cx.focus_handle(),
            document,
            #[cfg(all(test, feature = "ui-test-support"))]
            painted: Default::default(),
            scroll: gpui::ScrollHandle::new(),
        }
    }
}

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-shift-l", OpenChangelog, None)]);
    cx.bind_keys([KeyBinding::new(
        "enter",
        OpenChangelog,
        Some("hex_changelog_button"),
    )]);
    cx.bind_keys([KeyBinding::new(
        "escape",
        CloseChangelog,
        Some("hex_changelog"),
    )]);
    cx.bind_keys([KeyBinding::new(
        "enter",
        CloseChangelog,
        Some("hex_changelog_close"),
    )]);
    // This reader must not dispatch document-editing keys behind it. Space
    // remains the editor-wide transport shortcut.
    cx.bind_keys(
        [
            "cmd-f",
            "cmd-alt-b",
            "cmd-alt-t",
            "cmd-,",
            "backspace",
            "delete",
            "cmd-z",
            "cmd-shift-z",
            "cmd-x",
            "cmd-v",
            "cmd-d",
            "d",
            "i",
            "cmd-r",
            "h",
            "l",
            "w",
            "0",
            "a",
        ]
        .map(|key| KeyBinding::new(key, gpui::NoAction, Some("hex_changelog"))),
    );
}

impl EditorView {
    pub(super) fn open_changelog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.changelog.open = true;
        self.changelog.focus.focus(window);
        cx.notify();
    }

    pub(super) fn close_changelog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.changelog.open {
            self.changelog.open = false;
            self.focus.focus(window);
            cx.notify();
        }
    }

    // Test-only paint observation reads the actual inherited text style during
    // GPUI paint. It neither supplies hover state nor changes production input.
    #[cfg(all(test, feature = "ui-test-support"))]
    fn changelog_paint_probe(&self, key: String) -> impl gpui::IntoElement {
        let painted = std::rc::Rc::clone(&self.changelog.painted);
        gpui::canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let style = window.text_style();
                painted
                    .borrow_mut()
                    .insert(key, (style.color, style.font_family));
            },
        )
        .absolute()
        .inset_0()
    }

    fn changelog_note(
        &self,
        batch: usize,
        category: Category,
        index: usize,
        text: &str,
    ) -> Stateful<Div> {
        let key = format!("changelog-note-{batch}-{}-{index}", category.label());
        let selector = key.clone();
        let row = div()
            .id(gpui::SharedString::from(key.clone()))
            .debug_selector(move || selector.clone())
            .relative()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .overflow_hidden()
            .px_2()
            .py_1()
            .text_sm()
            .line_height(gpui::relative(1.4))
            .text_color(rgb(theme::color(PaletteRole::Text)))
            .bg(rgb(theme::color(PaletteRole::Surface)))
            .cursor(gpui::CursorStyle::Arrow)
            .hover(|style| {
                style
                    .text_color(rgb(theme::color(PaletteRole::Secondary)))
                    .bg(rgb(theme::color(PaletteRole::Hover)))
            })
            .child(theme::text(text));
        #[cfg(all(test, feature = "ui-test-support"))]
        let row = row.child(self.changelog_paint_probe(key));
        row
    }

    fn changelog_group(&self, batch: usize, category: Category, entries: &[String]) -> Div {
        let key = format!("changelog-heading-{batch}-{}", category.label());
        let selector = key.clone();
        let heading = div()
            .id(gpui::SharedString::from(key.clone()))
            .debug_selector(move || selector.clone())
            .relative()
            .w_full()
            .min_w_0()
            .px_2()
            .py_1()
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(theme::color(category.color())))
            .border_b_1()
            .border_color(rgb(theme::color(PaletteRole::Border)))
            .child(theme::text(category.label()));
        #[cfg(all(test, feature = "ui-test-support"))]
        let heading = heading.child(self.changelog_paint_probe(key));
        div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(heading)
            .children(
                entries
                    .iter()
                    .enumerate()
                    .map(|(index, text)| self.changelog_note(batch, category, index, text)),
            )
    }

    pub(super) fn changelog_overlay(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if !self.changelog.open {
            return None;
        }
        let mut notes = div()
            .id("changelog-notes")
            .debug_selector(|| "changelog-notes".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(&self.changelog.scroll)
            .flex()
            .flex_col()
            .gap_3();
        if let Some(message) = &self.changelog.document.message {
            notes = notes.child(
                div()
                    .debug_selector(|| "changelog-unavailable".into())
                    .text_sm()
                    .text_color(rgb(theme::color(PaletteRole::Muted)))
                    .child(theme::text(message)),
            );
        }
        for (batch_index, batch) in self.changelog.document.batches.iter().enumerate() {
            let details = theme::text(&batch.details);
            let mut body = div()
                .id(("changelog-batch", batch_index))
                .debug_selector(move || format!("changelog-batch-{batch_index}"))
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .id(("changelog-coverage", batch_index))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(theme::text(&batch.version)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme::color(PaletteRole::Muted)))
                                .child(theme::text(format!(
                                    "notes through {}",
                                    &batch.source[..8]
                                ))),
                        )
                        .tooltip(move |_, cx| {
                            cx.new(|_| super::transport::MiniTooltip {
                                kind: crate::tooltips::Kind::Identification,
                                description: details.clone(),
                            })
                            .into()
                        }),
                );
            let mut count = 0;
            for category in Category::ALL {
                let entries = &batch.groups[category.index()];
                count += entries.len();
                if !entries.is_empty() {
                    body = body.child(self.changelog_group(batch_index, category, entries));
                }
            }
            if count == 0 {
                body = body.child(
                    div()
                        .debug_selector(|| "changelog-empty".into())
                        .px_2()
                        .text_sm()
                        .text_color(rgb(theme::color(PaletteRole::Muted)))
                        .child(theme::text("no user-facing changes in this update")),
                );
            }
            notes = notes.child(body);
        }
        Some(
            div()
                .id("changelog-overlay")
                .debug_selector(|| "changelog-overlay".into())
                .absolute()
                .inset_0()
                .pt(px(super::header::TITLE_HEIGHT))
                .flex()
                .justify_center()
                .bg(rgb(theme::color(PaletteRole::Background)))
                .track_focus(&self.changelog.focus)
                .key_context("hex_changelog")
                .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "tab" {
                        view.changelog.close_focus.focus(window);
                        cx.notify();
                    }
                    cx.stop_propagation();
                }))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .debug_selector(|| "changelog-reader".into())
                        .w_full()
                        .max_w(px(640.0))
                        .min_w_0()
                        .h_full()
                        .px_3()
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child(theme::text("changelog")),
                                )
                                .child(
                                    div()
                                        .id("close-changelog")
                                        .debug_selector(|| "close-changelog".into())
                                        .track_focus(&self.changelog.close_focus)
                                        .tab_index(0)
                                        .key_context("hex_changelog_close")
                                        .focus(|style| {
                                            style.bg(rgb(theme::color(PaletteRole::Hover)))
                                        })
                                        .px_2()
                                        .py_1()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(rgb(theme::color(PaletteRole::Hover))))
                                        .child(theme::text("close · esc"))
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.close_changelog(window, cx)
                                        })),
                                ),
                        )
                        .child(notes),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::notes;

    #[test]
    fn unavailable_and_malformed_notes_are_distinct_from_generated_empty_updates() {
        assert_eq!(
            notes("").unwrap().message.as_deref(),
            Some("changelog unavailable for this build")
        );
        assert!(notes("not json").is_err());
        let raw = r#"{"format":1,"batches":[{"input":{"version":"0.1.0","source":"0123456789012345678901234567890123456789"},"result":{"entries":[]}}]}"#;
        let parsed = notes(raw).unwrap();
        assert!(parsed.batches[0].groups.iter().all(Vec::is_empty));
        assert!(parsed.batches[0].details.contains("notes through 01234567"));
    }

    #[test]
    fn notes_show_user_text_and_honest_coverage_not_private_diff_or_commit_references() {
        let raw = r#"{"format":1,"batches":[{"input":{"version":"0.1.0","source":"0123456789012345678901234567890123456789"},"result":{"entries":[{"category":"fixes","text":"open projects in one step","commits":["private-ref"]}]}}]}"#;
        assert_eq!(
            notes(raw).unwrap().batches[0].groups[2],
            ["open projects in one step"]
        );
        assert!(notes(&raw.replace("open projects", "Open projects")).is_err());
        assert!(notes(&raw.replace("fixes", "internal")).is_err());
    }
    #[test]
    fn category_projection_preserves_validation_and_group_order() {
        let raw = r#"{"format":1,"batches":[{"input":{"version":"0.1.0","source":"0123456789012345678901234567890123456789"},"result":{"entries":[{"category":"fixes","text":"first fix"},{"category":"features","text":"first feature"},{"category":"fixes","text":"second fix"}]}}]}"#;
        let parsed = notes(raw).unwrap();
        assert_eq!(parsed.batches[0].groups[0], ["first feature"]);
        assert!(parsed.batches[0].groups[1].is_empty());
        assert_eq!(parsed.batches[0].groups[2], ["first fix", "second fix"]);
        for value in [
            raw.replace("first fix", ""),
            raw.replace("first fix", "Uppercase"),
            raw.replace("first fix", "line\\nbreak"),
            raw.replace("first fix", &"x".repeat(321)),
            raw.replace("fixes", "unknown"),
            raw.replace("0123456789012345678901234567890123456789", "invalid"),
            raw.replace("\"format\":1", "\"format\":2"),
        ] {
            assert!(notes(&value).is_err());
        }
        assert!(notes(&raw.replace("first fix", &"é".repeat(320))).is_ok());
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests {
    use super::*;
    use gpui::{
        AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, size,
    };

    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fixture(cx: &mut TestAppContext) -> (Directory, VisualTestContext, Entity<EditorView>) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-changelog-ui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("samples")).unwrap();
        let directory = Directory(path);
        cx.update(super::super::bind_editor_keys);
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_browser_directory(
                        window,
                        cx,
                        hex_session::service::Service::new(48_000, 32, 0).unwrap(),
                        &directory.0,
                    )
                })
            })
            .unwrap();
        let visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(960.0), px(720.0)));
        visual.run_until_parked();
        (directory, visual, editor)
    }
    fn click(visual: &mut VisualTestContext, selector: &'static str) {
        let bounds = visual.debug_bounds(selector).unwrap();
        let center = point(
            bounds.origin.x + bounds.size.width / 2.0,
            bounds.origin.y + bounds.size.height / 2.0,
        );
        visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
    #[gpui::test]
    fn header_button_opens_offline_notes_next_to_log_and_escape_restores_editor(
        cx: &mut TestAppContext,
    ) {
        let (_directory, mut visual, editor) = fixture(cx);
        let log = visual.debug_bounds("open-session-log").unwrap();
        let button = visual.debug_bounds("open-changelog").unwrap();
        assert!(button.origin.x >= log.origin.x + log.size.width);
        assert!(button.origin.x + button.size.width <= px(960.0));
        editor.update(&mut visual, |view, cx| {
            view.changelog.document = notes(r#"{"format":1,"batches":[{"input":{"version":"0.1.0","source":"0123456789012345678901234567890123456789"},"result":{"entries":[{"category":"fixes","text":"project opening responds on the first click"}]}}]}"#).unwrap();
            cx.notify();
        });
        click(&mut visual, "open-changelog");
        assert!(visual.debug_bounds("changelog-notes").is_some());
        editor.read_with(&visual, |view, _| assert!(view.changelog.open));
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        editor.update_in(&mut visual, |view, window, _cx| {
            assert!(!view.changelog.open);
            assert!(view.focus.is_focused(window));
        });
    }
    #[gpui::test]
    fn shortcut_opens_actual_embedded_generated_notes_without_network_or_files(
        cx: &mut TestAppContext,
    ) {
        let (_directory, mut visual, editor) = fixture(cx);
        editor.update_in(&mut visual, |view, window, _cx| view.focus.focus(window));
        visual.simulate_keystrokes("cmd-shift-l");
        visual.run_until_parked();
        assert!(visual.debug_bounds("changelog-notes").is_some());
        let embedded: serde_json::Value =
            serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/changelog.json")))
                .unwrap();
        editor.read_with(&visual, |view, _| {
            assert_projection(&view.changelog.document, &embedded);
            assert!(view.changelog.open);
            assert!(!view.changelog.document.batches.is_empty());
            assert!(
                view.changelog.document.batches[0]
                    .details
                    .contains("notes through")
            );
            assert!(view.changelog.document.message.is_none());
        });
        visual.simulate_keystrokes("escape");
        editor.read_with(&visual, |view, _| assert!(!view.changelog.open));
    }

    #[gpui::test]
    fn toolbar_keyboard_entry_activates_with_enter_then_escape_restores_focus(
        cx: &mut TestAppContext,
    ) {
        let (_directory, mut visual, editor) = fixture(cx);
        editor.update_in(&mut visual, |view, window, _cx| view.focus.focus(window));
        visual.update(|window, _| window.focus_next());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_background("open-changelog"),
            Some(gpui::Background::from(rgb(theme::color(
                PaletteRole::Hover
            )))),
            "toolbar keyboard focus is visible"
        );
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert!(visual.debug_bounds("changelog-notes").is_some());
        editor.read_with(&visual, |view, _| assert!(view.changelog.open));
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        editor.update_in(&mut visual, |view, window, _cx| {
            assert!(!view.changelog.open);
            assert!(view.focus.is_focused(window));
        });
    }

    #[gpui::test]
    fn overlay_close_is_reachable_by_tab_shift_tab_and_activates_without_document_edits(
        cx: &mut TestAppContext,
    ) {
        let (_directory, mut visual, editor) = fixture(cx);
        let before = editor.read_with(&visual, |view, _| view.worker_requests_pending);
        for (tab, activate) in [("tab", "enter"), ("shift-tab", "enter")] {
            click(&mut visual, "open-changelog");
            visual.simulate_keystrokes(tab);
            visual.run_until_parked();
            assert_eq!(
                visual.rendered_background("close-changelog"),
                Some(gpui::Background::from(rgb(theme::color(
                    PaletteRole::Hover
                )))),
                "close must expose visible keyboard focus within the overlay"
            );
            // The overlay has exactly one interactive control; repeated Tab
            // must remain inside it rather than reach transport/toolbar fields.
            visual.simulate_keystrokes(tab);
            visual.simulate_keystrokes(activate);
            visual.run_until_parked();
            editor.update_in(&mut visual, |view, window, _| {
                assert!(!view.changelog.open);
                assert!(view.focus.is_focused(window));
                assert_eq!(view.worker_requests_pending, before);
            });
        }
    }

    #[gpui::test]
    fn long_notes_scroll_panel_is_bounded_and_close_does_not_edit_project(cx: &mut TestAppContext) {
        let (_directory, mut visual, editor) = fixture(cx);
        editor.update(&mut visual, |view, cx| {
            view.changelog.document = notes(&raw_entries(serde_json::json!(
                (0..100)
                    .map(|n| serde_json::json!({
                        "category":"fixes",
                        "text":format!("example user-facing fixture note {n}")
                    }))
                    .collect::<Vec<_>>()
            )))
            .unwrap();
            cx.notify();
        });
        click(&mut visual, "open-changelog");
        let bounds = visual.debug_bounds("changelog-notes").unwrap();
        assert!(bounds.size.height > px(100.0) && bounds.size.height < px(720.0));
        let before = editor.read_with(&visual, |view, _| {
            (
                view.worker_requests_pending,
                view.pending_requests.len(),
                view.offline_cursor,
            )
        });
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: point(bounds.origin.x + px(20.0), bounds.origin.y + px(30.0)),
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-180.0))),
            ..Default::default()
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.changelog.scroll.offset().y < px(0.0))
        });
        visual.simulate_keystrokes("backspace space cmd-z");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.changelog.open);
            assert_eq!(
                view.last_transport_command,
                Some(hex_engine::PlaybackCommand::PlayFrom(0))
            );
            assert_eq!(
                (
                    view.worker_requests_pending,
                    view.pending_requests.len(),
                    view.offline_cursor
                ),
                before
            );
        });
        click(&mut visual, "close-changelog");
        editor.read_with(&visual, |view, _| assert!(!view.changelog.open));
    }
    fn painted(editor: &Entity<EditorView>, visual: &VisualTestContext, key: &str) -> gpui::Hsla {
        editor.read_with(visual, |view, _| {
            let samples = view.changelog.painted.borrow();
            let (color, family) = samples
                .get(key)
                .unwrap_or_else(|| panic!("no actual paint for {key}"));
            assert_eq!(family.as_ref(), theme::FONT_FAMILY);
            *color
        })
    }

    fn assert_projection(document: &Document, original: &serde_json::Value) {
        let batches = original["batches"].as_array().unwrap();
        assert_eq!(document.batches.len(), batches.len());
        for (batch, input) in document.batches.iter().zip(batches.iter().rev()) {
            assert_eq!(batch.version, input["input"]["version"].as_str().unwrap());
            assert_eq!(batch.source, input["input"]["source"].as_str().unwrap());
            let entries = input["result"]["entries"].as_array().unwrap();
            assert_eq!(
                batch.groups.iter().map(Vec::len).sum::<usize>(),
                entries.len()
            );
            for category in Category::ALL {
                let expected: Vec<_> = entries
                    .iter()
                    .filter(|entry| entry["category"] == category.label())
                    .map(|entry| entry["text"].as_str().unwrap())
                    .collect();
                assert_eq!(batch.groups[category.index()], expected);
            }
            assert!(
                batch
                    .details
                    .contains(&format!("version {}", batch.version))
            );
            assert!(
                batch
                    .details
                    .contains(&format!("notes through {}", batch.source))
            );
            for field in [
                &input["input"]["base"],
                &input["generated_at"],
                &input["generator"],
                &input["execution"]["origin"],
                &input["execution"]["evidence_commit"],
            ] {
                if let Some(value) = field.as_str() {
                    assert!(batch.details.contains(value), "complete original metadata");
                }
            }
        }
    }

    // Synthetic valid reader history, not generated AI evidence. Deliberately
    // unequal counts/category sets expose newest-display vs oldest-storage order.
    fn grouped_history_fixture() -> serde_json::Value {
        serde_json::json!({"format":1,"batches":[
            {"input":{"version":"0.1.0","source":"1111111111111111111111111111111111111111"},
             "result":{"entries":[
                {"category":"fixes","text":"older first fix"},
                {"category":"fixes","text":"older second fix"}]}},
            {"input":{"version":"0.2.0","base":"1111111111111111111111111111111111111111",
                      "source":"2222222222222222222222222222222222222222"},
             "generated_at":"fixture time","generator":"test fixture",
             "result":{"entries":[
                {"category":"fixes","text":"newer complete fix"},
                {"category":"features","text":"newer first feature"},
                {"category":"improvements","text":"newer complete improvement"},
                {"category":"features","text":"newer second feature"}]}}
        ]})
    }

    #[gpui::test]
    fn changelog_grouped_headings_render_once_and_rows_hover_independently(
        cx: &mut TestAppContext,
    ) {
        let (_directory, mut visual, editor) = fixture(cx);
        let original = grouped_history_fixture();
        install(&editor, &mut visual, &original.to_string());
        click(&mut visual, "open-changelog");
        for (selector, role) in [
            ("changelog-heading-0-features", PaletteRole::Secondary),
            ("changelog-heading-0-improvements", PaletteRole::Text),
            ("changelog-heading-0-fixes", PaletteRole::Primary),
        ] {
            assert!(
                visual.debug_bounds(selector).is_some(),
                "one real category heading"
            );
            assert_eq!(
                painted(&editor, &visual, selector),
                rgb(theme::color(role)).into()
            );
        }
        // Match every newest-first displayed batch to its oldest-first input,
        // not just whichever release happened to be embedded when this test began.
        editor.read_with(&visual, |view, _| {
            assert_projection(&view.changelog.document, &original);
        });
        for (selector, present) in [
            ("changelog-heading-1-features", false),
            ("changelog-heading-1-improvements", false),
            ("changelog-heading-1-fixes", true),
            ("changelog-note-1-fixes-0", true),
            ("changelog-note-1-fixes-1", true),
            ("changelog-note-1-fixes-2", false),
            ("changelog-note-0-features-2", false),
            ("changelog-note-0-improvements-1", false),
            ("changelog-note-0-fixes-0", true),
            ("changelog-note-0-fixes-1", false),
        ] {
            assert_eq!(
                visual.debug_bounds(selector).is_some(),
                present,
                "{selector}"
            );
        }
        let first = visual.debug_bounds("changelog-note-0-features-0").unwrap();
        let second = visual.debug_bounds("changelog-note-0-features-1").unwrap();
        let header = visual.debug_bounds("changelog-heading-0-features").unwrap();
        let scroll = editor.read_with(&visual, |view, _| view.changelog.scroll.offset());
        // Hover the padded far edge below the first line, not merely a text glyph.
        visual.simulate_mouse_move(
            point(first.right() - px(2.0), first.bottom() - px(2.0)),
            None,
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert_eq!(
            painted(&editor, &visual, "changelog-note-0-features-0"),
            rgb(theme::color(PaletteRole::Secondary)).into()
        );
        assert_eq!(
            painted(&editor, &visual, "changelog-note-0-features-1"),
            rgb(theme::color(PaletteRole::Text)).into()
        );
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::Arrow)
        );
        assert_eq!(
            visual.rendered_background("changelog-note-0-features-0"),
            Some(rgb(theme::color(PaletteRole::Hover)).into())
        );
        visual.simulate_mouse_move(second.center(), None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            painted(&editor, &visual, "changelog-note-0-features-0"),
            rgb(theme::color(PaletteRole::Text)).into()
        );
        assert_eq!(
            painted(&editor, &visual, "changelog-note-0-features-1"),
            rgb(theme::color(PaletteRole::Secondary)).into()
        );
        visual.simulate_mouse_move(point(px(950.0), px(710.0)), None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            painted(&editor, &visual, "changelog-note-0-features-1"),
            rgb(theme::color(PaletteRole::Text)).into()
        );
        assert_eq!(
            painted(&editor, &visual, "changelog-heading-0-features"),
            rgb(theme::color(PaletteRole::Secondary)).into()
        );
        assert_eq!(
            visual.debug_bounds("changelog-note-0-features-0"),
            Some(first)
        );
        assert_eq!(
            visual.debug_bounds("changelog-note-0-features-1"),
            Some(second)
        );
        assert_eq!(
            visual.debug_bounds("changelog-heading-0-features"),
            Some(header)
        );
        assert_eq!(
            editor.read_with(&visual, |view, _| view.changelog.scroll.offset()),
            scroll
        );
    }
    fn install(editor: &Entity<EditorView>, visual: &mut VisualTestContext, raw: &str) {
        editor.update(visual, |view, cx| {
            view.changelog.document = notes(raw).unwrap_or_else(|error| {
                Document::message(format!("changelog unavailable · {error}"))
            });
            view.changelog.scroll.set_offset(point(px(0.0), px(0.0)));
            cx.notify();
        });
        visual.run_until_parked();
    }
    fn raw_entries(entries: serde_json::Value) -> String {
        serde_json::json!({"format":1,"batches":[{"input":{"version":"0.1.0","source":"0123456789012345678901234567890123456789"},"result":{"entries":entries}}]}).to_string()
    }

    #[gpui::test]
    fn changelog_empty_groups_unavailable_and_malformed_render_truthfully(cx: &mut TestAppContext) {
        let (_directory, mut visual, editor) = fixture(cx);
        click(&mut visual, "open-changelog");
        for raw in [
            "".to_owned(),
            "not json".to_owned(),
            r#"{"format":1,"batches":[]}"#.to_owned(),
        ] {
            install(&editor, &mut visual, &raw);
            assert!(visual.debug_bounds("changelog-unavailable").is_some());
            assert!(
                visual
                    .debug_bounds("changelog-heading-0-features")
                    .is_none()
            );
            assert!(visual.debug_bounds("changelog-empty").is_none());
        }
        install(&editor, &mut visual, &raw_entries(serde_json::json!([])));
        assert!(visual.debug_bounds("changelog-empty").is_some());
        assert!(visual.debug_bounds("changelog-unavailable").is_none());
        install(
            &editor,
            &mut visual,
            &raw_entries(serde_json::json!([{"category":"fixes","text":"one complete fix"}])),
        );
        assert!(visual.debug_bounds("changelog-heading-0-fixes").is_some());
        assert!(
            visual
                .debug_bounds("changelog-heading-0-features")
                .is_none()
        );
        assert!(
            visual
                .debug_bounds("changelog-heading-0-improvements")
                .is_none()
        );
        visual.simulate_keystrokes("escape");
        editor.update_in(&mut visual, |view, window, _| {
            assert!(view.focus.is_focused(window))
        });
    }

    #[gpui::test]
    fn changelog_long_note_wraps_at_compact_and_normal_width_without_hover_reflow(
        cx: &mut TestAppContext,
    ) {
        let (_directory, mut visual, editor) = fixture(cx);
        click(&mut visual, "open-changelog");
        let raw = raw_entries(
            serde_json::json!([{"category":"fixes","text":"a readable long note ".repeat(15)},{"category":"fixes","text":"é".repeat(320)}]),
        );
        install(&editor, &mut visual, &raw);
        for (width, height) in [(480.0, 360.0), (880.0, 540.0), (1280.0, 800.0)] {
            visual.simulate_resize(size(px(width), px(height)));
            visual.run_until_parked();
            let reader = visual.debug_bounds("changelog-reader").unwrap();
            let viewport = visual.debug_bounds("changelog-notes").unwrap();
            let row = visual.debug_bounds("changelog-note-0-fixes-0").unwrap();
            let second = visual.debug_bounds("changelog-note-0-fixes-1").unwrap();
            let close = visual.debug_bounds("close-changelog").unwrap();
            assert!(reader.size.width <= px(640.0) && reader.right() <= px(width));
            assert!(viewport.size.height > px(100.0) && viewport.bottom() <= px(height));
            assert!(row.left() >= viewport.left() && row.right() <= viewport.right());
            assert!(second.left() >= viewport.left() && second.right() <= viewport.right());
            assert!(row.size.height > px(30.0), "long text actually wraps");
            assert!(close.right() <= reader.right() && close.bottom() <= viewport.top());
            visual.simulate_mouse_move(
                row.origin + point(px(4.0), px(4.0)),
                None,
                Modifiers::default(),
            );
            visual.run_until_parked();
            assert_eq!(visual.debug_bounds("changelog-note-0-fixes-0"), Some(row));
            assert_eq!(
                visual.debug_bounds("changelog-note-0-fixes-1"),
                Some(second)
            );
        }
    }

    #[gpui::test]
    fn changelog_scrolling_and_reused_rows_do_not_retain_old_item_hover(cx: &mut TestAppContext) {
        let (_directory, mut visual, editor) = fixture(cx);
        let entries:Vec<_>=(0..60).map(|n|serde_json::json!({"category":"fixes","text":format!("a complete short fixture note {n}")})).collect();
        install(
            &editor,
            &mut visual,
            &raw_entries(serde_json::json!(entries)),
        );
        click(&mut visual, "open-changelog");
        let first = visual.debug_bounds("changelog-note-0-fixes-0").unwrap();
        let pointer = first.center();
        visual.simulate_mouse_move(pointer, None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            painted(&editor, &visual, "changelog-note-0-fixes-0"),
            rgb(theme::color(PaletteRole::Secondary)).into()
        );
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: pointer,
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-150.0))),
            ..Default::default()
        });
        visual.run_until_parked();
        assert!(editor.read_with(&visual, |view, _| view.changelog.scroll.offset().y) < px(0.0));
        let mut hovered = None;
        for n in 1..15 {
            let selector: &'static str =
                Box::leak(format!("changelog-note-0-fixes-{n}").into_boxed_str());
            if visual
                .debug_bounds(selector)
                .is_some_and(|bounds| bounds.contains(&pointer))
            {
                hovered = Some(selector);
                break;
            }
        }
        let hovered = hovered.expect("another real row scrolled below stationary pointer");
        assert_eq!(
            painted(&editor, &visual, hovered),
            rgb(theme::color(PaletteRole::Secondary)).into()
        );
        visual.simulate_mouse_move(point(px(950.0), px(710.0)), None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            painted(&editor, &visual, hovered),
            rgb(theme::color(PaletteRole::Text)).into()
        );
        // Reuse the same row ID for fresh content after leaving the viewport.
        install(
            &editor,
            &mut visual,
            &raw_entries(
                serde_json::json!([{"category":"fixes","text":"replacement content at the same row identity"}]),
            ),
        );
        assert_eq!(
            painted(&editor, &visual, "changelog-note-0-fixes-0"),
            rgb(theme::color(PaletteRole::Text)).into()
        );
        let row = visual.debug_bounds("changelog-note-0-fixes-0").unwrap();
        let before = editor.read_with(&visual, |view, _| {
            (
                view.worker_requests_pending,
                view.pending_requests.len(),
                view.offline_cursor,
            )
        });
        visual.simulate_click(row.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                (
                    view.worker_requests_pending,
                    view.pending_requests.len(),
                    view.offline_cursor
                ),
                before
            )
        });
        visual.simulate_keystrokes("tab enter");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| assert!(!view.changelog.open));
    }
}

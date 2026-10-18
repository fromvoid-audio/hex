//! Offline, build-embedded release notes. No inference or filesystem work in paint.
use super::EditorView;
use crate::theme::{self, PaletteRole};
use gpui::{
    App, Context, Div, FocusHandle, KeyBinding, Stateful, Window, div, prelude::*, px, rgb,
};

gpui::actions!(hex_changelog, [OpenChangelog, CloseChangelog]);

pub(super) struct State {
    open: bool,
    focus: FocusHandle,
    close_focus: FocusHandle,
    lines: Vec<String>,
    scroll: gpui::ScrollHandle,
}

fn notes(raw: &str) -> Result<Vec<String>, &'static str> {
    if raw.is_empty() {
        return Ok(vec!["changelog unavailable for this build".into()]);
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| "invalid changelog data")?;
    if value["format"].as_u64() != Some(1) {
        return Err("unsupported changelog data");
    }
    let batches = value["batches"]
        .as_array()
        .ok_or("invalid changelog history")?;
    let mut lines = Vec::new();
    for batch in batches.iter().rev() {
        let version = batch["input"]["version"]
            .as_str()
            .ok_or("missing changelog version")?;
        let source = batch["input"]["source"]
            .as_str()
            .filter(|v| v.len() == 40 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("missing changelog coverage")?;
        lines.push(format!("{version} · notes through {}", &source[..8]));
        let entries = batch["result"]["entries"]
            .as_array()
            .ok_or("invalid changelog entries")?;
        if entries.is_empty() {
            lines.push("no user-facing changes in this update".into());
        }
        for entry in entries {
            let category = entry["category"]
                .as_str()
                .filter(|v| ["features", "fixes", "improvements"].contains(v))
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
            lines.push(format!("{category} · {text}"));
        }
    }
    if lines.is_empty() {
        lines.push("changelog unavailable for this build".into());
    }
    Ok(lines)
}

impl State {
    pub(super) fn is_open(&self) -> bool {
        self.open
    }
    pub(super) fn new(cx: &mut Context<EditorView>) -> Self {
        let lines = notes(include_str!(concat!(env!("OUT_DIR"), "/changelog.json")))
            .unwrap_or_else(|error| vec![format!("changelog unavailable · {error}")]);
        Self {
            open: false,
            focus: cx.focus_handle(),
            close_focus: cx.focus_handle(),
            lines,
            scroll: gpui::ScrollHandle::new(),
        }
    }
}

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-shift-l", OpenChangelog, None)]);
    cx.bind_keys([
        KeyBinding::new("enter", OpenChangelog, Some("hex_changelog_button")),
        KeyBinding::new("space", OpenChangelog, Some("hex_changelog_button")),
    ]);
    cx.bind_keys([KeyBinding::new(
        "escape",
        CloseChangelog,
        Some("hex_changelog"),
    )]);
    cx.bind_keys([
        KeyBinding::new("enter", CloseChangelog, Some("hex_changelog_close")),
        KeyBinding::new("space", CloseChangelog, Some("hex_changelog_close")),
    ]);
    // This reader must not dispatch document editing/transport keys behind it.
    cx.bind_keys(
        [
            "cmd-f",
            "cmd-alt-b",
            "cmd-alt-t",
            "cmd-,",
            "backspace",
            "delete",
            "space",
            "cmd-z",
            "cmd-shift-z",
            "cmd-x",
            "cmd-v",
            "cmd-d",
            "cmd-r",
            "h",
            "l",
            "0",
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

    pub(super) fn changelog_overlay(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if !self.changelog.open {
            return None;
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
                        // Exactly one control in this reader. Keep either Tab
                        // direction inside the overlay, away from the editor.
                        view.changelog.close_focus.focus(window);
                        cx.notify();
                    }
                    cx.stop_propagation();
                }))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .w_full()
                        .max_w(px(720.0))
                        .h_full()
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .items_center()
                                .child(theme::text("changelog"))
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
                        .child(
                            div()
                                .id("changelog-notes")
                                .debug_selector(|| "changelog-notes".into())
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .track_scroll(&self.changelog.scroll)
                                .flex()
                                .flex_col()
                                .gap_3()
                                .children(self.changelog.lines.iter().enumerate().map(
                                    |(index, line)| {
                                        div()
                                            .id(("changelog-line", index))
                                            .debug_selector(move || {
                                                format!("changelog-line-{index}")
                                            })
                                            .flex_shrink_0()
                                            .text_sm()
                                            .child(theme::text(line))
                                    },
                                )),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::notes;

    #[test]
    fn unavailable_and_malformed_notes_are_distinct_from_generated_empty_updates() {
        assert_eq!(notes("").unwrap(), ["changelog unavailable for this build"]);
        assert!(notes("not json").is_err());
        let raw = r#"{"format":1,"batches":[{"input":{"version":"0.1.0","source":"0123456789012345678901234567890123456789"},"result":{"entries":[]}}]}"#;
        let parsed = notes(raw).unwrap();
        assert_eq!(parsed[1], "no user-facing changes in this update");
        assert!(parsed[0].contains("notes through 01234567"));
    }

    #[test]
    fn notes_show_user_text_and_honest_coverage_not_private_diff_or_commit_references() {
        let raw = r#"{"format":1,"batches":[{"input":{"version":"0.1.0","source":"0123456789012345678901234567890123456789"},"result":{"entries":[{"category":"fixes","text":"open projects in one step","commits":["private-ref"]}]}}]}"#;
        assert_eq!(notes(raw).unwrap()[1], "fixes · open projects in one step");
        assert!(notes(&raw.replace("open projects", "Open projects")).is_err());
        assert!(notes(&raw.replace("fixes", "internal")).is_err());
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
            view.changelog.lines = vec![
                "0.1.0 · notes through 01234567".into(),
                "fixes · project opening responds on the first click".into(),
            ];
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
        assert!(visual.debug_bounds("changelog-line-1").is_some());
        editor.read_with(&visual, |view, _| {
            assert!(view.changelog.open);
            assert!(view.changelog.lines.len() > 1);
            assert!(view.changelog.lines[0].contains("notes through"));
            assert!(
                !view
                    .changelog
                    .lines
                    .iter()
                    .any(|line| line.contains("unavailable"))
            );
        });
        visual.simulate_keystrokes("escape");
        editor.read_with(&visual, |view, _| assert!(!view.changelog.open));
    }

    #[gpui::test]
    fn toolbar_keyboard_entry_activates_with_enter_and_space_then_escape_restores_focus(
        cx: &mut TestAppContext,
    ) {
        let (_directory, mut visual, editor) = fixture(cx);
        for key in ["enter", "space"] {
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
            visual.simulate_keystrokes(key);
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
    }

    #[gpui::test]
    fn overlay_close_is_reachable_by_tab_shift_tab_and_activates_without_document_edits(
        cx: &mut TestAppContext,
    ) {
        let (_directory, mut visual, editor) = fixture(cx);
        let before = editor.read_with(&visual, |view, _| view.worker_requests_pending);
        for (tab, activate) in [("tab", "enter"), ("shift-tab", "space")] {
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
            view.changelog.lines = (0..100)
                .map(|n| format!("fixes · example user-facing fixture note {n}"))
                .collect();
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
    #[gpui::test]
    fn changelog_grouped_headings_render_once_and_rows_hover_independently(cx: &mut TestAppContext) {
        let (_directory, mut visual, _editor) = fixture(cx);
        click(&mut visual, "open-changelog");
        assert!(visual.debug_bounds("changelog-heading-0-features").is_some(), "render one real features heading instead of repeating prefixes in paragraphs");
    }

}

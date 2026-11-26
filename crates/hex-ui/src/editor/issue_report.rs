//! In-app issue & feature request report modal.
//!
//! Freezes `T_report` and captures a read-only `RuntimeSnapshot` immediately
//! when opened (before the user types). All log slicing, redaction, outbox
//! persistence, and network I/O run on the desktop-owned background report
//! worker via `crate::ReportClient`. Never blocks rendering or touches audio.

use super::EditorView;
use crate::{
    ReportClient, ReportCommand, ReportPreviewRequest, ReportRequest, ReportStatus,
    theme::{self, PaletteRole},
};
use gpui::{
    AnyWindowHandle, App, Context, Div, Entity, FocusHandle, Global, KeyBinding, MouseButton,
    ScrollHandle, Stateful, Subscription, WeakEntity, Window, div, prelude::*, px, rgb,
};
use hex_report::{
    AudioRuntimeSnapshot, RedactionContext, ReportEnvelope, ReportKind, RuntimeSnapshot,
    format_redacted_preview, redact_envelope,
};
use hex_widgets::{TextInput, TextInputChanged, TextInputConfig, TextInputKind};
use std::{
    path::PathBuf,
    sync::atomic::Ordering::Acquire,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_SUMMARY_LINES: usize = 12;
const MAX_LINE_BYTES: usize = 512;
const MAX_LOADED_PLUGINS: usize = 64;

gpui::actions!(
    hex_issue_report,
    [OpenIssueReport, CloseIssueReport, SubmitIssueReport]
);

/// Global weak handle to the active editor window so auxiliary windows (such as
/// the diagnostic log viewer) can open the report modal and freeze live state.
#[derive(Clone)]
pub(crate) struct IssueReportOpener {
    editor: WeakEntity<EditorView>,
    window: AnyWindowHandle,
}

impl Global for IssueReportOpener {}

pub(crate) fn register_opener(
    cx: &mut App,
    editor: WeakEntity<EditorView>,
    window: AnyWindowHandle,
) {
    cx.set_global(IssueReportOpener { editor, window });
}

/// Open the report modal on the active editor window from another window (e.g. log viewer).
pub(crate) fn open_from_app(cx: &mut App) -> bool {
    let Some(opener) = cx.try_global::<IssueReportOpener>().cloned() else {
        return false;
    };
    let editor = opener.editor;
    opener
        .window
        .update(cx, |_, window, cx| {
            window.activate_window();
            if let Some(entity) = editor.upgrade() {
                entity.update(cx, |view, cx| {
                    view.open_issue_report(window, cx);
                });
                true
            } else {
                false
            }
        })
        .unwrap_or(false)
}

pub(super) struct State {
    open: bool,
    focus: FocusHandle,
    kind: ReportKind,
    summary_lines: Vec<Entity<TextInput>>,
    _line_subscriptions: Vec<Subscription>,
    active_line: usize,
    include_diagnostics: bool,
    show_preview: bool,
    frozen_snapshot: Option<RuntimeSnapshot>,
    frozen_log_path: Option<PathBuf>,
    preview_text: String,
    preview_loading: bool,
    next_request_id: u64,
    active_preview_id: Option<u64>,
    active_submit_id: Option<u64>,
    status_message: Option<String>,
    status_is_error: bool,
    preview_scroll: ScrollHandle,
}

impl State {
    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn new(cx: &mut Context<EditorView>) -> Self {
        Self {
            open: false,
            focus: cx.focus_handle(),
            kind: ReportKind::Bug,
            summary_lines: Vec::new(),
            _line_subscriptions: Vec::new(),
            active_line: 0,
            include_diagnostics: true,
            show_preview: false,
            frozen_snapshot: None,
            frozen_log_path: None,
            preview_text: String::new(),
            preview_loading: false,
            next_request_id: 0,
            active_preview_id: None,
            active_submit_id: None,
            status_message: None,
            status_is_error: false,
            preview_scroll: ScrollHandle::new(),
        }
    }

    fn next_id(&mut self) -> u64 {
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.next_request_id
    }

    fn summary_text(&self, cx: &App) -> String {
        join_summary_lines(self.summary_lines.iter().map(|line| line.read(cx).text()))
    }
}

fn join_summary_lines<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    lines.collect::<Vec<_>>().join("\n")
}

fn make_summary_line(
    index: usize,
    cx: &mut Context<EditorView>,
) -> (Entity<TextInput>, Subscription) {
    let placeholder = if index == 0 {
        theme::text("describe what happened or what you would like to see…")
    } else {
        theme::text("")
    };
    let entity = cx.new(|cx| {
        TextInput::new(
            cx,
            TextInputConfig {
                id: gpui::SharedString::from(format!("issue-report-line-{index}")),
                kind: TextInputKind::Value,
                style: theme::control_style(),
                action_style: theme::action_style(),
                font_family: theme::FONT_FAMILY.into(),
                placeholder,
                clear_name: theme::text("clear line"),
                clear_glyph: theme::text("×"),
                max_bytes: MAX_LINE_BYTES,
            },
        )
    });
    let sub = cx.subscribe(
        &entity,
        |view: &mut EditorView, _, _: &TextInputChanged, cx| {
            view.refresh_issue_report_preview_local(cx);
            cx.notify();
        },
    );
    (entity, sub)
}

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-shift-r", OpenIssueReport, None),
        KeyBinding::new("escape", CloseIssueReport, Some("hex_issue_report")),
        KeyBinding::new("cmd-enter", SubmitIssueReport, Some("hex_issue_report")),
    ]);
    // Prevent background document-editing shortcuts from firing while the modal is open.
    cx.bind_keys(
        [
            "cmd-f",
            "cmd-alt-b",
            "cmd-alt-t",
            "cmd-,",
            "cmd-z",
            "cmd-shift-z",
            "cmd-d",
            "cmd-r",
            "cmd-s",
            "cmd-o",
            "cmd-n",
            "cmd-i",
            "space",
        ]
        .map(|key| {
            KeyBinding::new(
                key,
                gpui::NoAction,
                Some("hex_issue_report && !hex_text_input"),
            )
        }),
    );
}

/// Parse `HEX_DIAGNOSTIC_SOURCE` (`<sha>:<tree_digest>:<tree_state>`) into `(source_sha, tree_state)`.
fn parse_diagnostic_source(raw: &str) -> (String, String) {
    let mut parts = raw.split(':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(sha), Some(_tree_digest), Some(state), None)
            if !sha.is_empty() && !state.is_empty() =>
        {
            (sha.to_owned(), state.to_owned())
        }
        _ => ("unknown".to_owned(), "unknown".to_owned()),
    }
}

fn current_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos()
}

fn build_local_redacted_preview(
    kind: ReportKind,
    user_summary: &str,
    snapshot: &RuntimeSnapshot,
    include_diagnostics: bool,
) -> String {
    let mut envelope = ReportEnvelope::with_request_id(
        "00000000-0000-4000-8000-000000000000".into(),
        hex_report::DEFAULT_PROJECT_ID,
        kind,
        user_summary.to_owned(),
        snapshot.clone(),
        None,
    );
    if include_diagnostics {
        envelope.digest_text = "(loading redacted diagnostic bundle off-thread…)".into();
    }
    let redacted = redact_envelope(&envelope, &RedactionContext::from_env());
    format_redacted_preview(&redacted)
}

impl EditorView {
    /// Capture a pure read-only runtime snapshot anchored at `reported_at_unix_nanos`.
    /// Never waits on locks, filesystem I/O, or the audio callback thread.
    pub(super) fn capture_runtime_snapshot(
        &self,
        reported_at_unix_nanos: u128,
        cx: &App,
    ) -> RuntimeSnapshot {
        let identity = crate::build_info::current_identity();
        let (source_sha, tree_state) = parse_diagnostic_source(identity.diagnostic_source);

        let (sample_rate, playing, buffering, callback_dropped) = match &self.audio {
            Some(audio) => {
                let (playing, buffering, cb_dropped) = match audio.status() {
                    Some(status) => (
                        status.playing.load(Acquire),
                        status.stream_buffering.load(Acquire),
                        status.audio_diagnostics_dropped(),
                    ),
                    None => (false, false, 0),
                };
                (audio.sample_rate, playing, buffering, cb_dropped)
            }
            None => (0, false, false, 0),
        };

        let degraded_plugins = self
            .device_panel
            .degraded_plugins
            .entries()
            .iter()
            .take(MAX_LOADED_PLUGINS)
            .map(|(id, reason)| format!("device:{id} ({reason})"))
            .collect();

        let mut loaded_plugins = Vec::new();
        if let Some(state) = &self.state {
            for device in state.project.devices().take(MAX_LOADED_PLUGINS) {
                if let Ok(type_id) = device.state().type_id().as_str() {
                    let entry = type_id.to_owned();
                    if !loaded_plugins.contains(&entry) {
                        loaded_plugins.push(entry);
                    }
                }
            }
        }

        RuntimeSnapshot {
            app_version: identity.display_version.to_owned(),
            source_sha,
            tree_state,
            build_label: identity.display_label(),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            reported_at_unix_nanos,
            view_mode: match self.mode {
                super::EditorMode::Arrangement => "arrangement".into(),
                super::EditorMode::Session => "session".into(),
            },
            status: self.status.clone(),
            last_error: self.error.as_ref().map(ToString::to_string),
            audio: AudioRuntimeSnapshot {
                sample_rate,
                mode: format!("{:?}", self.audio_pub.document_audio_mode).to_lowercase(),
                playing,
                buffering,
                callback_dropped,
                logger_dropped: crate::diagnostics::logger_overflow_count(cx),
                degraded_plugins,
            },
            loaded_plugins,
        }
    }

    pub(super) fn open_issue_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Freeze T_report and read-only RuntimeSnapshot BEFORE the user types.
        let reported_at = current_unix_nanos();
        let snapshot = self.capture_runtime_snapshot(reported_at, cx);
        let active_log = crate::diagnostics::log_path(cx);

        let (first_line, first_sub) = make_summary_line(0, cx);
        let first_focus = first_line.read(cx).focus_handle().clone();

        let state = &mut self.settings.issue_report;
        state.open = true;
        state.kind = ReportKind::Bug;
        state.summary_lines = vec![first_line];
        state._line_subscriptions = vec![first_sub];
        state.active_line = 0;
        state.include_diagnostics = true;
        state.show_preview = false;
        state.preview_text = build_local_redacted_preview(ReportKind::Bug, "", &snapshot, true);
        state.preview_loading = false;
        state.frozen_snapshot = Some(snapshot);
        state.frozen_log_path = active_log;
        state.active_preview_id = None;
        state.active_submit_id = None;
        state.status_message = None;
        state.status_is_error = false;

        first_focus.focus(window);
        self.request_issue_report_preview(cx);
        cx.notify();
    }

    pub(super) fn close_issue_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.issue_report.open {
            self.settings.issue_report.open = false;
            self.focus.focus(window);
            cx.notify();
        }
    }

    fn set_issue_report_kind(&mut self, kind: ReportKind, cx: &mut Context<Self>) {
        if self.settings.issue_report.kind == kind {
            return;
        }
        self.settings.issue_report.kind = kind;
        self.refresh_issue_report_preview_local(cx);
        if self.settings.issue_report.show_preview {
            self.request_issue_report_preview(cx);
        }
        cx.notify();
    }

    fn toggle_issue_report_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.settings.issue_report.include_diagnostics =
            !self.settings.issue_report.include_diagnostics;
        self.refresh_issue_report_preview_local(cx);
        self.request_issue_report_preview(cx);
        cx.notify();
    }

    fn toggle_issue_report_preview(&mut self, cx: &mut Context<Self>) {
        self.settings.issue_report.show_preview = !self.settings.issue_report.show_preview;
        if self.settings.issue_report.show_preview {
            self.refresh_issue_report_preview_local(cx);
            self.request_issue_report_preview(cx);
        }
        cx.notify();
    }

    fn refresh_issue_report_preview_local(&mut self, cx: &Context<Self>) {
        let summary = self.settings.issue_report.summary_text(cx);
        let state = &mut self.settings.issue_report;
        if let Some(snapshot) = &state.frozen_snapshot
            && (!state.include_diagnostics || state.active_preview_id.is_none())
        {
            state.preview_text = build_local_redacted_preview(
                state.kind,
                &summary,
                snapshot,
                state.include_diagnostics,
            );
        }
    }

    fn request_issue_report_preview(&mut self, cx: &Context<Self>) {
        let Some(client) = cx.try_global::<ReportClient>().cloned() else {
            return;
        };
        let summary = self.settings.issue_report.summary_text(cx);
        let state = &mut self.settings.issue_report;
        let Some(snapshot) = state.frozen_snapshot.clone() else {
            return;
        };
        let id = state.next_id();
        state.active_preview_id = Some(id);
        state.preview_loading = true;
        let _ = client.send(ReportCommand::Preview(ReportPreviewRequest {
            id,
            kind: state.kind,
            user_summary: summary,
            include_diagnostics: state.include_diagnostics,
            snapshot,
            active_log: state.frozen_log_path.clone(),
        }));
    }

    pub(super) fn submit_issue_report(&mut self, cx: &mut Context<Self>) {
        if self.settings.issue_report.active_submit_id.is_some() {
            return;
        }
        let summary = self.settings.issue_report.summary_text(cx);
        if summary.trim().is_empty() {
            self.settings.issue_report.status_message =
                Some("enter a brief description before submitting".into());
            self.settings.issue_report.status_is_error = true;
            cx.notify();
            return;
        }
        let Some(client) = cx.try_global::<ReportClient>().cloned() else {
            self.settings.issue_report.status_message =
                Some("report worker is unavailable in this build".into());
            self.settings.issue_report.status_is_error = true;
            cx.notify();
            return;
        };
        let state = &mut self.settings.issue_report;
        let Some(snapshot) = state.frozen_snapshot.clone() else {
            state.status_message = Some("report snapshot unavailable · reopen dialog".into());
            state.status_is_error = true;
            cx.notify();
            return;
        };
        let id = state.next_id();
        let request = ReportRequest {
            id,
            kind: state.kind,
            user_summary: summary,
            include_diagnostics: state.include_diagnostics,
            snapshot,
            active_log: state.frozen_log_path.clone(),
        };
        match client.send(ReportCommand::Submit(request)) {
            Ok(()) => {
                state.active_submit_id = Some(id);
                state.status_message = Some("sending report…".into());
                state.status_is_error = false;
            }
            Err(error) => {
                state.status_message = Some(error.into());
                state.status_is_error = true;
            }
        }
        cx.notify();
    }

    fn handle_summary_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        if key == "enter" && mods.platform {
            cx.stop_propagation();
            self.submit_issue_report(cx);
            return;
        }
        if key == "enter" && !mods.control && !mods.alt && !mods.platform {
            if self.settings.issue_report.summary_lines.len() < MAX_SUMMARY_LINES {
                let insert_at = (self.settings.issue_report.active_line + 1)
                    .min(self.settings.issue_report.summary_lines.len());
                let (line, sub) = make_summary_line(insert_at, cx);
                let focus = line.read(cx).focus_handle().clone();
                self.settings
                    .issue_report
                    .summary_lines
                    .insert(insert_at, line);
                self.settings
                    .issue_report
                    ._line_subscriptions
                    .insert(insert_at, sub);
                self.settings.issue_report.active_line = insert_at;
                focus.focus(window);
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if key == "backspace" && !mods.platform && !mods.control && !mods.alt {
            let active = self.settings.issue_report.active_line;
            if active > 0
                && self
                    .settings
                    .issue_report
                    .summary_lines
                    .get(active)
                    .is_some_and(|line| line.read(cx).text().is_empty())
            {
                self.settings.issue_report.summary_lines.remove(active);
                let _ = self
                    .settings
                    .issue_report
                    ._line_subscriptions
                    .remove(active);
                let prev = active - 1;
                self.settings.issue_report.active_line = prev;
                if let Some(prev_line) = self.settings.issue_report.summary_lines.get(prev) {
                    prev_line.read(cx).focus_handle().focus(window);
                }
                self.refresh_issue_report_preview_local(cx);
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        if key == "up" && !mods.platform && !mods.control && !mods.alt {
            let active = self.settings.issue_report.active_line;
            if active > 0 {
                let prev = active - 1;
                self.settings.issue_report.active_line = prev;
                if let Some(line) = self.settings.issue_report.summary_lines.get(prev) {
                    line.read(cx).focus_handle().focus(window);
                }
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if key == "down" && !mods.platform && !mods.control && !mods.alt {
            let active = self.settings.issue_report.active_line;
            if active + 1 < self.settings.issue_report.summary_lines.len() {
                let next = active + 1;
                self.settings.issue_report.active_line = next;
                if let Some(line) = self.settings.issue_report.summary_lines.get(next) {
                    line.read(cx).focus_handle().focus(window);
                }
                cx.stop_propagation();
                cx.notify();
            }
        }
    }

    /// Poll non-blocking status and preview updates from the desktop report worker.
    pub(super) fn poll_report(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(client) = cx.try_global::<ReportClient>().cloned() else {
            return false;
        };
        let mut changed = false;

        if let Some(preview) = client.take_preview() {
            let state = &mut self.settings.issue_report;
            if state.active_preview_id == Some(preview.id) {
                state.preview_text = preview.text;
                state.preview_loading = false;
                changed = true;
            }
        }

        let Some(active_id) = self.settings.issue_report.active_submit_id else {
            return changed;
        };

        match client.status() {
            ReportStatus::Submitting(id) if id == active_id => {}
            ReportStatus::Submitted {
                id,
                report_id,
                deduplicated,
            } if id == active_id => {
                self.settings.issue_report.active_submit_id = None;
                self.settings.issue_report.open = false;
                self.settings.issue_report.status_message = None;
                self.settings.issue_report.status_is_error = false;
                self.status = if deduplicated {
                    format!("sent to pulse triage · report #{report_id} (already received)")
                } else {
                    format!("sent to pulse triage · report #{report_id}")
                };
                self.error = None;
                let _ = client.send(ReportCommand::DismissStatus);
                changed = true;
            }
            ReportStatus::QueuedOffline { id, reason } if id == active_id => {
                self.settings.issue_report.active_submit_id = None;
                self.settings.issue_report.open = false;
                self.settings.issue_report.status_message = None;
                self.settings.issue_report.status_is_error = false;
                self.status = if reason.is_empty() {
                    "saved to offline outbox".into()
                } else {
                    format!("saved to offline outbox · {}", reason.to_lowercase())
                };
                let _ = client.send(ReportCommand::DismissStatus);
                changed = true;
            }
            ReportStatus::Failed { id, error } if id == active_id || id == 0 => {
                self.settings.issue_report.active_submit_id = None;
                let lowered = error.to_lowercase();
                self.settings.issue_report.status_message = Some(lowered.clone());
                self.settings.issue_report.status_is_error = true;
                self.error = Some(lowered.into());
                let _ = client.send(ReportCommand::DismissStatus);
                changed = true;
            }
            _ => {}
        }

        changed
    }

    pub(super) fn issue_report_overlay(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let state = &self.settings.issue_report;
        if !state.open {
            return None;
        }
        let submitting = state.active_submit_id.is_some();
        let selected_kind = state.kind;
        let include_diagnostics = state.include_diagnostics;
        let show_preview = state.show_preview;
        let preview_loading = state.preview_loading;
        let preview_text = gpui::SharedString::from(state.preview_text.clone());

        let mut pills = div().flex().items_center().gap_1();
        for kind in ReportKind::ALL {
            let active = kind == selected_kind;
            let bg = theme::color(if active {
                PaletteRole::Primary
            } else {
                PaletteRole::Surface
            });
            let fg = theme::readable_text(
                bg,
                theme::color(if active {
                    PaletteRole::Background
                } else {
                    PaletteRole::Text
                }),
            );
            pills = pills.child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "issue-report-kind-{}",
                        kind.as_str()
                    )))
                    .debug_selector(move || format!("issue-report-kind-{}", kind.as_str()))
                    .px_2()
                    .py_1()
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(rgb(theme::color(PaletteRole::Border)))
                    .bg(rgb(bg))
                    .text_color(rgb(fg))
                    .text_xs()
                    .cursor_pointer()
                    .child(theme::text(kind.ui_label()))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.set_issue_report_kind(kind, cx);
                    })),
            );
        }

        let mut lines = div()
            .id("issue-report-summary-box")
            .debug_selector(|| "issue-report-summary-box".into())
            .w_full()
            .min_h(px(96.0))
            .max_h(px(220.0))
            .overflow_y_scroll()
            .p_1()
            .rounded(px(3.0))
            .border_1()
            .border_color(rgb(theme::color(PaletteRole::Border)))
            .bg(rgb(theme::color(PaletteRole::Surface)))
            .flex()
            .flex_col()
            .gap(px(2.0));
        for (idx, line) in state.summary_lines.iter().enumerate() {
            lines = lines.child(
                div()
                    .id(("issue-report-line-row", idx))
                    .w_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, _| {
                            view.settings.issue_report.active_line = idx;
                        }),
                    )
                    .child(line.clone()),
            );
        }

        let diag_label = if include_diagnostics {
            "[x] include diagnostic bundle"
        } else {
            "[ ] include diagnostic bundle"
        };
        let preview_toggle_label = if show_preview {
            "hide redacted preview"
        } else {
            "show redacted preview"
        };

        let mut card = div()
            .id("issue-report-card")
            .debug_selector(|| "issue-report-card".into())
            .w_full()
            .max_w(px(620.0))
            .min_w_0()
            .max_h_full()
            .p_3()
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(theme::color(PaletteRole::Border)))
            .bg(rgb(theme::color(PaletteRole::Background)))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(theme::text("report issue / request")),
                    )
                    .child(
                        div()
                            .id("close-issue-report")
                            .debug_selector(|| "close-issue-report".into())
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(theme::color(PaletteRole::Muted)))
                            .hover(|s| s.bg(rgb(theme::color(PaletteRole::Hover))))
                            .child(theme::text("close · esc"))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.close_issue_report(window, cx);
                            })),
                    ),
            )
            .child(pills)
            .child(lines)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::color(PaletteRole::Muted)))
                    .child(theme::text(
                        "enter: new line · cmd+enter: submit · esc: cancel",
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .id("issue-report-toggle-diagnostics")
                            .debug_selector(|| "issue-report-toggle-diagnostics".into())
                            .px_1()
                            .py_1()
                            .cursor_pointer()
                            .text_xs()
                            .hover(|s| s.bg(rgb(theme::color(PaletteRole::Hover))))
                            .child(theme::text(diag_label))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.toggle_issue_report_diagnostics(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("issue-report-toggle-preview")
                            .debug_selector(|| "issue-report-toggle-preview".into())
                            .px_1()
                            .py_1()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(theme::color(PaletteRole::Muted)))
                            .hover(|s| s.bg(rgb(theme::color(PaletteRole::Hover))))
                            .child(theme::text(preview_toggle_label))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.toggle_issue_report_preview(cx);
                            })),
                    ),
            );

        if show_preview {
            card = card.child(
                div()
                    .id("issue-report-preview")
                    .debug_selector(|| "issue-report-preview".into())
                    .w_full()
                    .h(px(180.0))
                    .p_2()
                    .overflow_y_scroll()
                    .track_scroll(&state.preview_scroll)
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(rgb(theme::color(PaletteRole::Border)))
                    .bg(rgb(theme::color(PaletteRole::Surface)))
                    .text_xs()
                    .text_color(rgb(theme::color(PaletteRole::Muted)))
                    .when(preview_loading, |el| {
                        el.child(theme::text("updating redacted preview…"))
                    })
                    .child(preview_text),
            );
        }

        if let Some(message) = &state.status_message {
            let color = theme::color(if state.status_is_error {
                PaletteRole::Primary
            } else {
                PaletteRole::Muted
            });
            card = card.child(
                div()
                    .id("issue-report-status")
                    .debug_selector(|| "issue-report-status".into())
                    .text_xs()
                    .text_color(rgb(color))
                    .child(theme::text(message)),
            );
        }

        let submit_label = if submitting {
            "submitting…"
        } else {
            "submit"
        };
        card = card.child(
            div()
                .flex()
                .justify_end()
                .items_center()
                .gap_2()
                .child(
                    hex_widgets::action(theme::text("cancel"), theme::action_style())
                        .id("issue-report-cancel")
                        .debug_selector(|| "issue-report-cancel".into())
                        .h(px(24.0))
                        .px_2()
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.close_issue_report(window, cx);
                        })),
                )
                .child(
                    hex_widgets::action(theme::text(submit_label), theme::action_style())
                        .id("issue-report-submit")
                        .debug_selector(|| "issue-report-submit".into())
                        .h(px(24.0))
                        .px_2()
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.submit_issue_report(cx);
                        })),
                ),
        );

        Some(
            div()
                .id("issue-report-overlay")
                .debug_selector(|| "issue-report-overlay".into())
                .absolute()
                .inset_0()
                .pt(px(super::header::WINDOW_CONTROL_HEIGHT + 12.0))
                .pb_3()
                .px_3()
                .flex()
                .justify_center()
                .items_start()
                .bg(rgb(theme::color(PaletteRole::Background)))
                .track_focus(&state.focus)
                .key_context("hex_issue_report")
                .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                    view.handle_summary_key_down(event, window, cx);
                }))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(card),
        )
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn parses_diagnostic_source_provenance_cleanly() {
        assert_eq!(
            parse_diagnostic_source("0bf6b787974bc9bf2441b8af88a88a016faf88dd:deadbeef:clean"),
            (
                "0bf6b787974bc9bf2441b8af88a88a016faf88dd".into(),
                "clean".into()
            )
        );
        assert_eq!(
            parse_diagnostic_source("abc12345:tree_digest_unavailable:dirty"),
            ("abc12345".into(), "dirty".into())
        );
        assert_eq!(
            parse_diagnostic_source("unknown"),
            ("unknown".into(), "unknown".into())
        );
    }

    #[test]
    fn multi_line_summary_joins_lines_and_local_preview_redacts_frozen_snapshot() {
        let joined = join_summary_lines(["first line", "second line", "third"].into_iter());
        assert_eq!(joined, "first line\nsecond line\nthird");

        let frozen_ts: u128 = 1_764_170_000_000_000_000;
        let snapshot = RuntimeSnapshot {
            app_version: "0.1.0".into(),
            source_sha: "abc12345".into(),
            tree_state: "clean".into(),
            build_label: "0.1.0 · abc12345".into(),
            platform: "macos-aarch64".into(),
            reported_at_unix_nanos: frozen_ts,
            view_mode: "arrangement".into(),
            status: "ready".into(),
            last_error: Some("token plsr_should_never_appear".into()),
            audio: AudioRuntimeSnapshot {
                sample_rate: 48_000,
                mode: "resident".into(),
                playing: true,
                buffering: false,
                callback_dropped: 2,
                logger_dropped: 0,
                degraded_plugins: vec!["device:5 (vst3 stopped responding)".into()],
            },
            loaded_plugins: vec!["hex.utility".into()],
        };
        let preview =
            build_local_redacted_preview(ReportKind::Performance, &joined, &snapshot, true);
        assert!(preview.contains("kind: performance"));
        assert!(preview.contains(&frozen_ts.to_string()));
        assert!(preview.contains("first line\nsecond line\nthird"));
        assert!(!preview.contains("plsr_should_never_appear"));
        assert!(preview.contains("[redacted-token]"));
    }

    #[test]
    fn report_worker_channel_handoff_preserves_frozen_t_report() {
        let (client, worker) = crate::report_channel();
        let frozen_ts = 999_000_123_456_u128;
        let snapshot = RuntimeSnapshot {
            reported_at_unix_nanos: frozen_ts,
            view_mode: "session".into(),
            ..Default::default()
        };
        client
            .send(ReportCommand::Submit(ReportRequest {
                id: 7,
                kind: ReportKind::Bug,
                user_summary: "clip mute stuck".into(),
                include_diagnostics: true,
                snapshot: snapshot.clone(),
                active_log: Some(PathBuf::from("/tmp/session.log")),
            }))
            .unwrap();

        match worker.commands.recv().unwrap() {
            ReportCommand::Submit(req) => {
                assert_eq!(req.id, 7);
                assert_eq!(req.snapshot.reported_at_unix_nanos, frozen_ts);
                assert_eq!(req.user_summary, "clip mute stuck");
                assert!(req.include_diagnostics);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        worker.publish(ReportStatus::QueuedOffline {
            id: 7,
            reason: "network unreachable".into(),
        });
        assert_eq!(
            client.status(),
            ReportStatus::QueuedOffline {
                id: 7,
                reason: "network unreachable".into()
            }
        );
    }
}

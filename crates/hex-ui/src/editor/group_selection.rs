//! Time-span or whole-clip Arrangement grouping from the focused editor shortcut.
use super::{EditorMode, EditorView};
use gpui::Window;
use hex_session::service::Request;

impl EditorView {
    pub(super) fn group_selected_clips(&mut self, window: &Window) {
        if self.mode != EditorMode::Arrangement
            || !self.focus.is_focused(window)
            || self.midi_focus.is_focused(window)
            || self.audio_event_focus.is_focused(window)
            || self.computer_keyboard.accepts_key("g")
        {
            return;
        }
        if self.close_prompt_open {
            self.error = Some("finish the close action before editing".into());
            return;
        }
        if self.edit_pending() {
            self.error = Some("applying edit · wait before grouping clips".into());
            return;
        }
        if self
            .time_selection
            .is_some_and(|range| range.anchor_frame != range.end_frame)
        {
            if self.time_selection_project_scope && self.create_section_in_selected_gap() {
                return;
            }
            let Some(range) = self.normalized_time_selection() else {
                self.error = Some("time selection no longer belongs to current tracks".into());
                return;
            };
            let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
                return;
            };
            self.prepare_group_selection(true);
            if !self.enqueue(Request::GroupTimeRange {
                project_generation: self.project_generation,
                revision,
                range,
                create_if_empty: !self.time_selection_project_scope,
            }) {
                self.retire_group_selection();
            }
            return;
        }
        let clips = self.selected_clip_ids();
        if clips.len() < 2 {
            // A section span is the only grouping selection: merge it rather
            // than dropping `g` when the stricter section-focus test fails.
            if !self.merge_section_span_selection() {
                self.error =
                    Some("select two or more clips, sections or a time range to group".into());
            }
            return;
        }
        let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
            return;
        };
        self.prepare_group_selection(false);
        if !self.enqueue(Request::GroupClips {
            project_generation: self.project_generation,
            revision,
            clips,
        }) {
            self.retire_group_selection();
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "group_selection_ui_tests.rs"]
mod ui_tests;

// View-only selection receipts follow canonical clip births/removals. They never
// implement project history, and manual selection changes take precedence.
// Canonical history has a maximum of 128 entries, so older receipts can expire.
#[derive(Default)]
pub(super) struct SelectionHistory {
    pending: Option<(super::pending_edit::Receipt, bool, Selection)>,
    receipts: std::collections::VecDeque<(hex_project::ClipId, Selection)>,
}
#[derive(Clone)]
struct Selection {
    track: Option<hex_project::TrackId>,
    primary: Option<hex_project::ClipId>,
    clips: super::clip_selection::ClipSelection,
    ids: Vec<hex_project::ClipId>,
    time: Option<super::time_selection::TimeSelection>,
}
impl Selection {
    fn capture(view: &EditorView) -> Self {
        Self {
            track: view.selected_track,
            primary: view.selected_clip,
            clips: view.clip_selection.clone(),
            ids: view.selected_clip_ids(),
            time: view.time_selection,
        }
    }
    fn matches(&self, view: &EditorView) -> bool {
        self.track == view.selected_track
            && self.primary == view.selected_clip
            && self.time == view.time_selection
            && self.ids == view.selected_clip_ids()
    }
    fn install(self, view: &mut EditorView) {
        view.selected_track = self.track;
        view.selected_clip = self.primary;
        view.clip_selection = self.clips;
        view.time_selection = self.time;
    }
}
impl EditorView {
    fn prepare_group_selection(&mut self, time_range: bool) {
        self.group_selection_history.pending = Some((
            super::pending_edit::Receipt::new(self.project_generation),
            time_range,
            Selection::capture(self),
        ));
    }
    pub(super) fn pending_group_selection_matches(&self, request: &Request) -> bool {
        self.group_selection_history
            .pending
            .as_ref()
            .is_some_and(|(receipt, range, _)| {
                receipt.is_queued()
                    && matches!(
                        (range, request),
                        (true, Request::GroupTimeRange { .. })
                            | (false, Request::GroupClips { .. })
                    )
            })
    }
    pub(super) fn group_selection_submitted(&mut self) {
        if let Some((receipt, _, _)) = &mut self.group_selection_history.pending {
            receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn retire_group_selection(&mut self) {
        self.group_selection_history.pending = None;
    }
    pub(super) fn reconcile_group_selection(&mut self, event: &hex_session::service::Event) {
        if event.project_replaced {
            self.group_selection_history = SelectionHistory::default();
            return;
        }
        if let Some((receipt, _, _)) = &mut self.group_selection_history.pending {
            let disposition = receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            );
            if disposition != super::pending_edit::Disposition::Waiting {
                let Some((_, _, before)) = self.group_selection_history.pending.take() else {
                    return;
                };
                if disposition == super::pending_edit::Disposition::Terminal
                    && event.error.is_none()
                    && let Some(hex_project::Created::Clip(result)) = event.created
                    && !before.ids.contains(&result)
                {
                    let receipts = &mut self.group_selection_history.receipts;
                    if receipts.len() == 128 {
                        receipts.pop_front();
                    }
                    receipts.push_back((result, before));
                }
                return;
            }
        }
        let Some(old) = &self.state else {
            return;
        };
        if old.revision == event.state.revision {
            return;
        }
        let project = &event.state.project;
        let selection =
            self.group_selection_history
                .receipts
                .iter()
                .rev()
                .find_map(|(result, before)| {
                    let has_clip = |project: &hex_project::Project| {
                        project.clip(*result).is_some() || project.midi_clip(*result).is_some()
                    };
                    let was_present = has_clip(&old.project);
                    let present = has_clip(project);
                    if was_present
                        && !present
                        && self.selected_clip == Some(*result)
                        && self.selected_clip_ids() == [*result]
                        && self.time_selection == before.time
                        && before.ids.iter().all(|id| project.clip(*id).is_some())
                    {
                        Some(before.clone())
                    } else if !was_present && present && before.matches(self) {
                        let track = project
                            .tracks()
                            .iter()
                            .find(|t| {
                                t.clips().iter().any(|c| c.id() == *result)
                                    || t.midi_clips().iter().any(|c| c.id() == *result)
                            })?
                            .id();
                        let mut after = before.clone();
                        after.track = Some(track);
                        after.primary = Some(*result);
                        after.clips.only(track, *result);
                        after.ids = vec![*result];
                        Some(after)
                    } else {
                        None
                    }
                });
        if let Some(selection) = selection {
            selection.install(self);
        }
    }
}

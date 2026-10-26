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
            let Some(range) = self.normalized_time_selection() else {
                self.error = Some("time selection no longer belongs to current tracks".into());
                return;
            };
            let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
                return;
            };
            if self
                .enqueue_rendered_audio_group(&hex_project::Command::GroupTimeRange(range.clone()))
            {
                return;
            }
            self.enqueue(Request::GroupTimeRange {
                project_generation: self.project_generation,
                revision,
                range,
            });
            return;
        }
        let clips = self.selected_clip_ids();
        if clips.len() < 2 {
            return;
        }
        let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
            return;
        };
        if self.enqueue_rendered_audio_group(&hex_project::Command::GroupClips(clips.clone())) {
            return;
        }
        self.enqueue(Request::GroupClips {
            project_generation: self.project_generation,
            revision,
            clips,
        });
    }
    /// Use the ordinary cancellable worker bounce lane only when leaf event
    /// flattening cannot preserve an already processed/repeated group.
    fn enqueue_rendered_audio_group(&mut self, command: &hex_project::Command) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        let target = match state.project.audio_group_render_target(command) {
            Ok(Some(target)) => target,
            Ok(None) => return false,
            Err(reason) => {
                self.error = Some(reason.to_string().into());
                return true;
            }
        };
        let before = Selection::capture(self);
        let control = hex_session::service::ExportControl::default();
        if self.enqueue(Request::BounceArrangement {
            project_generation: self.project_generation,
            revision: state.revision,
            target,
            in_place: true,
            control: control.clone(),
        }) {
            self.rendered_group_selection.pending = Some((control.clone(), before));
            self.bounce_control = Some(control);
        }
        true
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
    pending: Option<(hex_session::service::ExportControl, Selection)>,
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
    pub(super) fn reconcile_rendered_group_selection(
        &mut self,
        event: &hex_session::service::Event,
    ) {
        if event.project_replaced {
            self.rendered_group_selection = SelectionHistory::default();
            return;
        }
        if let Some((control, _)) = &self.rendered_group_selection.pending
            && event
                .bounce_complete
                .as_ref()
                .is_some_and(|done| control.same_job(done))
            && let Some((_, before)) = self.rendered_group_selection.pending.take()
            && event.error.is_none()
            && let Some(hex_project::Created::Clip(result)) = event.created
        {
            let receipts = &mut self.rendered_group_selection.receipts;
            if receipts.len() == 128 {
                receipts.pop_front();
            }
            receipts.push_back((result, before));
            return;
        }
        let Some(old) = &self.state else {
            return;
        };
        if old.revision == event.state.revision {
            return;
        }
        let project = &event.state.project;
        let selection = self
            .rendered_group_selection
            .receipts
            .iter()
            .rev()
            .find_map(|(result, before)| {
                let was_present = old.project.clip(*result).is_some();
                let present = project.clip(*result).is_some();
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
                        .find(|t| t.clips().iter().any(|c| c.id() == *result))?
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

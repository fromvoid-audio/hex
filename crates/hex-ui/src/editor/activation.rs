//! Focus-aware activation dispatch. Canonical clip/track state and atomic edits
//! remain in hex-project; prepare/admit/publication remains in hex-session.
//! Focused device cards retain their captured-owner and gesture-barrier adapter.
//! No cached inspector owner may become an activation target by itself.
use super::{
    EditorMode, EditorView,
    pending_edit::{Disposition, Receipt},
    track_name::Target,
};
use gpui::{Context, KeyDownEvent, Modifiers, Window};
use hex_project::{ClipEdit, ClipId, Command, SessionCellContent};
use hex_session::service::{Event, Request};
use std::collections::BTreeSet;

pub(super) struct PendingClipActivation {
    clips: BTreeSet<ClipId>,
    active: bool,
    receipt: Receipt,
}

impl PendingClipActivation {
    pub(super) fn affects_track(&self, track: &hex_project::Track) -> bool {
        track
            .clips()
            .iter()
            .any(|clip| self.clips.contains(&clip.id()))
            || track
                .midi_clips()
                .iter()
                .any(|clip| self.clips.contains(&clip.id()))
    }
}

pub(super) fn is_activation_key(event: &KeyDownEvent) -> bool {
    matches!(event.keystroke.key.as_str(), "c" | "/")
        && event.keystroke.modifiers == Modifiers::default()
        && !event.is_held
}

impl EditorView {
    /// Selected device adapters consume header and parameter shortcuts before this ancestor.
    /// Header focus targets the explicit track selection before retained ranges;
    /// otherwise only the current workspace's explicit selection is eligible. Nested text/plot/piano focus
    /// and the capture-phase musical keyboard retain their existing authority.
    pub(super) fn activation_key(
        &mut self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if (self.automation_view.enabled
            && self.mode == EditorMode::Arrangement
            && self.focus.is_focused(window))
            || !is_activation_key(event)
            || self.sections.naming()
            || self.settings.changelog.is_open()
            || self.track_name.is_editing()
            || self.clip_name_edit.is_some()
            || self.context_menu.is_some()
            || self.settings.theme.open
            || self.settings.tuning.open
            || self.persistence.save_dialog_open
            || self.persistence.export_dialog_open
        {
            return false;
        }
        if self.audio_detail.focus.is_focused(window) {
            let handled = self.event_toggle_active();
            if handled {
                cx.notify();
            }
            return handled;
        }
        let command = match self.activation_command(window) {
            Ok(Some(command)) => command,
            Ok(None) => return false,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return true;
            }
        };
        if self.activation_gesture_blocked() {
            self.error = Some("finish the current edit before changing activation".into());
        } else {
            if let Command::EditClips {
                clips,
                edit: ClipEdit::Active { active },
            } = &command
            {
                self.interaction.pending_clip_activation = Some(PendingClipActivation {
                    clips: clips.iter().copied().collect(),
                    active: *active,
                    receipt: Receipt::new(self.project_generation),
                });
            }
            if self.enqueue(Request::Edit(command)) {
                self.duplicate_intent.cancel_chain();
            } else {
                self.retire_clip_activation();
            }
        }
        cx.notify();
        true
    }

    /// Arrangement paint follows the admitted keypress until the same edit's
    /// terminal installs canonical state or restores it on refusal.
    pub(super) fn displayed_clip_active(&self, clip: ClipId, canonical: bool) -> bool {
        self.interaction
            .pending_clip_activation
            .as_ref()
            .filter(|pending| pending.clips.contains(&clip))
            .map_or(canonical, |pending| pending.active)
    }

    pub(super) fn clip_activation_matches(&self, request: &Request) -> bool {
        let Request::Edit(Command::EditClips {
            clips,
            edit: ClipEdit::Active { active },
        }) = request
        else {
            return false;
        };
        self.interaction
            .pending_clip_activation
            .as_ref()
            .is_some_and(|pending| {
                pending.receipt.is_queued()
                    && pending.active == *active
                    && pending.clips.len() == clips.len()
                    && clips.iter().all(|clip| pending.clips.contains(clip))
            })
    }

    pub(super) fn clip_activation_submitted(&mut self) {
        if let Some(pending) = self.interaction.pending_clip_activation.as_mut() {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }

    pub(super) fn clip_activation_complete(&mut self, event: &Event) {
        if self
            .interaction
            .pending_clip_activation
            .as_mut()
            .is_some_and(|pending| {
                pending.receipt.observe(
                    self.project_generation,
                    event.project_replaced,
                    event.request_complete,
                ) != Disposition::Waiting
            })
        {
            self.retire_clip_activation();
        }
    }

    pub(super) fn retire_clip_activation(&mut self) -> bool {
        self.interaction.pending_clip_activation.take().is_some()
    }

    pub(super) fn retire_abandoned_clip_activation(&mut self) -> bool {
        self.interaction
            .pending_clip_activation
            .as_ref()
            .is_some_and(|pending| pending.receipt.is_queued())
            && !self
                .queued_requests()
                .any(|request| self.clip_activation_matches(request))
            && self.retire_clip_activation()
    }

    pub(super) fn activation_gesture_blocked(&self) -> bool {
        self.edit_pending()
            || self.interaction.arrangement_pan.is_some()
            || self.scrub_active()
            || self.interaction.time_selection_gesture.is_some()
            || self.interaction.time_range_move_gesture.is_some()
            || self.interaction.active_clip_drag.is_some()
            || self.browser.active_audio_drag.is_some()
            || self.device_panel.active_native_drag.is_some()
            || self.interaction.active_trim_drag.is_some()
            || self.midi_editor.active_trim_drag.is_some()
            || self.interaction.active_fade_drag.is_some()
            || self.track_resize_active()
            || self.interaction.loop_rail_gesture.is_some()
            || self.locked_play_start_drag_active()
            || self.sections.held()
            || self.section_install_pending()
            || self.midi_editor.note_gesture.is_some()
            || self.midi_editor.group_gesture.is_some()
            || self.midi_editor.stroke.is_some()
            || self.midi_editor.time_selection.gesture.is_some()
            || self.midi_editor.ruler.held()
            || self.midi_editor.velocity.gesture.is_some()
    }

    fn activation_command(&self, window: &Window) -> Result<Option<Command>, String> {
        let Some(state) = &self.state else {
            return Ok(None);
        };
        if self.midi_editor.focus.is_focused(window) {
            let Some(clip) = self.detail_midi_clip_data() else {
                return Ok(None);
            };
            let selected = self.midi_editor.selection.ids();
            if selected.is_empty() {
                return Ok(None);
            }
            let source = state
                .project
                .midi_source(clip)
                .ok_or("selected midi source is unavailable")?;
            let mut remaining: BTreeSet<_> = selected.iter().copied().collect();
            let mut any_active = false;
            for note in source.notes() {
                if remaining.remove(&note.id()) {
                    any_active |= note.active();
                }
            }
            if !remaining.is_empty() {
                return Err("selected midi note no longer exists".into());
            }
            return Ok(Some(Command::SetMidiNotesActive {
                clip,
                notes: selected.to_vec(),
                active: !any_active,
            }));
        }
        if let Some(target) = self.focused_header_target(window) {
            return match target {
                Target::Track(track) => {
                    let tracks = self.selected_track_ids();
                    if self.selected_track != Some(track)
                        || self.session_grid.focused_return.is_some()
                        || !tracks.contains(&track)
                    {
                        Err("focused track selection changed".into())
                    } else {
                        let tracks = hex_project::TrackSelectionIds::new(&tracks)
                            .map_err(|error| error.to_string())?;
                        Ok(Some(Command::ToggleTracksMute(tracks)))
                    }
                }
                Target::Return(return_track) => state
                    .project
                    .return_tracks()
                    .iter()
                    .find(|track| track.id() == return_track)
                    .filter(|_| self.session_grid.focused_return == Some(return_track))
                    .map(|track| {
                        Some(Command::ReturnMute {
                            return_track,
                            muted: !track.muted(),
                        })
                    })
                    .ok_or_else(|| "focused return selection changed".into()),
            };
        }
        if !self.focus.is_focused(window) {
            return Ok(None);
        }
        match self.mode {
            EditorMode::Arrangement => {
                if self
                    .interaction
                    .time_selection
                    .is_some_and(|range| range.anchor_frame != range.end_frame)
                {
                    let range = self
                        .normalized_time_selection()
                        .ok_or("time selection no longer belongs to current tracks")?;
                    return state
                        .project
                        .time_range_has_active_clips(&range)
                        .map(|active| {
                            active.map(|any_active| Command::SetTimeRangeActive {
                                range,
                                active: !any_active,
                            })
                        })
                        .map_err(|error| error.to_string());
                }
                let mut clips = Vec::new();
                let mut any_active = false;
                let mut remaining: BTreeSet<_> = self.selected_clip_ids().into_iter().collect();
                // Resolve a large multi-selection in one pass rather than
                // scanning the project for every selected identity.
                for track in state.project.tracks() {
                    for clip in track.clips() {
                        if remaining.remove(&clip.id()) {
                            clips.push(clip.id());
                            any_active |= clip.active();
                        }
                    }
                    for clip in track.midi_clips() {
                        if remaining.remove(&clip.id()) {
                            clips.push(clip.id());
                            any_active |= clip.active();
                        }
                    }
                }
                // Mode changes retain a Session selection. It has no
                // Arrangement extent and must not mute its owning track.
                for cell in state.project.session_cells() {
                    match cell.content() {
                        SessionCellContent::Audio(clip) => {
                            remaining.remove(&clip.id());
                        }
                        SessionCellContent::Midi(clip) => {
                            remaining.remove(&clip.id());
                        }
                        SessionCellContent::Stop => (),
                    }
                }
                if !remaining.is_empty() {
                    return Err("selected clip no longer exists".into());
                }
                Ok((!clips.is_empty()).then_some(Command::EditClips {
                    clips,
                    edit: ClipEdit::Active {
                        active: !any_active,
                    },
                }))
            }
            EditorMode::Session => {
                let Some((track, scene)) = self.session_grid.selected_cell else {
                    return Ok(None);
                };
                if state.project.track(track).is_none()
                    || !state.project.scenes().iter().any(|item| item.id() == scene)
                {
                    return Err("selected session cell no longer exists".into());
                }
                let clip = match state.project.session_cell(track, scene) {
                    Some(SessionCellContent::Audio(clip)) => clip.id(),
                    Some(SessionCellContent::Midi(clip)) => clip.id(),
                    Some(SessionCellContent::Stop) | None => return Ok(None),
                };
                let active = state
                    .project
                    .clip_active(clip)
                    .ok_or("selected session clip no longer exists")?;
                Ok(Some(Command::SetClipActive {
                    clip,
                    active: !active,
                }))
            }
        }
    }
}

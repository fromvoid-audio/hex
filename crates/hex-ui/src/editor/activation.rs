//! Focus-aware activation dispatch. Canonical clip/track state and atomic edits
//! remain in hex-project; prepare/admit/publication remains in hex-session.
//! Focused device cards retain their captured-owner and gesture-barrier adapter.
//! No cached inspector owner may become an activation target by itself.
use super::{EditorMode, EditorView, track_name::Target};
use gpui::{Context, KeyDownEvent, Modifiers, Window};
use hex_project::{ClipEdit, Command, SessionCellContent};
use hex_session::service::Request;

pub(super) fn is_activation_key(event: &KeyDownEvent) -> bool {
    matches!(event.keystroke.key.as_str(), "c" | "0")
        && event.keystroke.modifiers == Modifiers::default()
        && !event.is_held
}

impl EditorView {
    /// Device adapters consume their exact-focus keys before this ancestor.
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
            || self.changelog.is_open()
            || self.track_name.is_editing()
            || self.clip_name_edit.is_some()
            || self.context_menu.is_some()
            || self.theme_settings.open
            || self.tuning.open
            || self.save_dialog_open
            || self.export_dialog_open
        {
            return false;
        }
        if self.audio_event_focus.is_focused(window) {
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
        } else if self.enqueue(Request::Edit(command)) {
            self.duplicate_intent.cancel_chain();
        }
        cx.notify();
        true
    }

    pub(super) fn activation_gesture_blocked(&self) -> bool {
        self.edit_pending()
            || self.arrangement_pan.is_some()
            || self.scrub_active()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.track_resize_active()
            || self.loop_rail_gesture.is_some()
            || self.sections.held()
            || self.section_install_pending()
            || self.midi_note_gesture.is_some()
            || self.midi_group_gesture.is_some()
            || self.midi_stroke.is_some()
            || self.midi_time_selection.gesture.is_some()
            || self.midi_ruler.held()
            || self.midi_velocity.gesture.is_some()
    }

    fn activation_command(&self, window: &Window) -> Result<Option<Command>, String> {
        let Some(state) = &self.state else {
            return Ok(None);
        };
        if self.midi_focus.is_focused(window) {
            let Some(clip) = self.detail_midi_clip_data() else {
                return Ok(None);
            };
            let selected = self.midi_selection.ids();
            if selected.is_empty() {
                return Ok(None);
            }
            let source = state
                .project
                .midi_source(clip)
                .ok_or("selected midi source is unavailable")?;
            let mut remaining: std::collections::BTreeSet<_> = selected.iter().copied().collect();
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
                        || self.focused_return.is_some()
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
                    .filter(|_| self.focused_return == Some(return_track))
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
                let mut remaining: std::collections::BTreeSet<_> =
                    self.selected_clip_ids().into_iter().collect();
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
                let Some((track, scene)) = self.selected_cell else {
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

//! Clip-local loop/source display custody. Canonical timing stays in hex-project;
//! a released preview retires only with its ordinary FIFO terminal and state install.
use super::{
    EditorView,
    midi_source_controls::{self, SourceValues},
    pending_edit::{Disposition, Receipt},
};
use hex_project::{AudioViewResizeOperation, Clip, ClipId, Command};
#[path = "audio_clip_loop.rs"]
mod audio;
use hex_session::service::{ArrangementPlayStartAudition, Event, Request};

#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
    play_start_audition_held: bool,
    audio_gesture: Option<audio::Gesture>,
    audio_bounds: Option<gpui::Bounds<gpui::Pixels>>,
}
impl State {
    pub(super) fn retire_deferred_play_start(&mut self) {
        if self.pending.as_ref().is_some_and(|pending| {
            pending.receipt.is_queued()
                && matches!(
                    pending.preview,
                    Preview::Audio {
                        edit: AudioEdit::Resize(AudioViewResizeOperation::PlayStart { .. }),
                        ..
                    }
                )
        }) {
            self.pending = None;
        }
    }
}
struct Pending {
    replayed: bool,
    revision: u64,
    generation: u64,
    clip: ClipId,
    preview: Preview,
    receipt: Receipt,
    start_camera: Option<super::audio_event_editor::PlayStartCamera>,
}
enum Preview {
    Audio {
        clip: Clip,
        edit: AudioEdit,
    },
    Midi {
        values: SourceValues,
        clip: Option<hex_project::MidiClip>,
        edit: MidiEdit,
    },
}
#[derive(Clone, Copy)]
enum MidiEdit {
    Resize(hex_project::MidiViewResizeOperation),
    Source(SourceValues),
    Session(SourceValues),
}
#[derive(Clone, Copy)]
enum AudioEdit {
    Resize(AudioViewResizeOperation),
    Toggle(bool),
}
impl EditorView {
    /// Follow held input and transport changes without sending canonical edits.
    pub(super) fn sync_held_play_start_audition(&mut self) {
        let target = (self.mode == super::EditorMode::Arrangement
            && self.clip_view_open
            && !self.device_panel_active
            && self.recovery.generation().is_none()
            && !self.edit_pending()
            && self
                .audio_status()
                .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Acquire)))
        .then(|| {
            self.held_audio_play_start_audition()
                .or_else(|| self.audio_clip_controls.play_start_audition(self))
                .or_else(|| self.held_midi_play_start_audition())
        })
        .flatten();
        if self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && let Some(target) = &target
            && let hex_session::service::PlayStartEdit::Audio { clip, .. } = target.edit
            && self.has_output_fault_requiring_recovery()
        {
            if self.clip_loop.play_start_audition_held {
                self.set_held_play_start_audition(None);
            }
            if self.pending_restart_request.is_none() && self.claim_audio_play_start_recovery() {
                self.recover_audio();
                if self.recovery.generation().is_some() {
                    self.recovery.bind_clip_play_start(target.context, clip);
                    self.defer_play_until_audio_recovers();
                }
            }
        } else if target.is_some() || self.clip_loop.play_start_audition_held {
            self.set_held_play_start_audition(target);
        }
    }

    fn set_held_play_start_audition(&mut self, target: Option<ArrangementPlayStartAudition>) {
        self.clip_loop.play_start_audition_held = target.is_some();
        self.service.audition_arrangement_play_start(target);
    }

    pub(super) fn finish_held_play_start_audition(&mut self) {
        if std::mem::take(&mut self.clip_loop.play_start_audition_held) {
            self.service.finish_arrangement_play_start_audition();
        }
    }

    pub(super) fn settle_released_play_start_audition(&self, clip: ClipId) {
        if self
            .clip_loop
            .pending
            .as_ref()
            .is_none_or(|p| p.clip != clip)
        {
            self.service.retire_arrangement_play_start_audition();
        }
    }

    pub(super) fn clip_loop_held(&self) -> bool {
        self.clip_loop.audio_gesture.is_some()
    }

    /// Keep the Arrangement overlay eligible through the exact edit receipt.
    pub(super) fn shows_audio_clip_loop_preview(&self) -> bool {
        self.clip_loop_held() || self.pending_audio_clip_loop().is_some()
    }

    /// Keep the Arrangement overlay eligible during held and released MIDI
    /// scope/start edits until their exact terminal.
    pub(super) fn shows_midi_clip_loop_preview(&self) -> bool {
        self.midi_ruler.held() || self.pending_midi_clip_loop_clip().is_some()
    }

    pub(super) fn midi_scope_display_clip<'a>(
        &'a self,
        clip: &'a hex_project::MidiClip,
        state: &super::ViewState,
    ) -> std::borrow::Cow<'a, hex_project::MidiClip> {
        if let Some(preview) = self.midi_ruler_held_preview_clip(clip, state.revision) {
            return std::borrow::Cow::Owned(preview);
        }
        std::borrow::Cow::Borrowed(
            self.pending_midi_clip_loop_clip()
                .filter(|preview| preview.id() == clip.id())
                .unwrap_or(clip),
        )
    }

    /// Consume clip-editor context even when no edit is possible; it must never
    /// fall through to the Arrangement transport-loop action.
    pub(super) fn clip_loop_key(&mut self, window: &gpui::Window) -> bool {
        self.reconcile_audio_loop();
        let midi = self.midi_focus.is_focused(window);
        let audio = self.audio_event_focus.is_focused(window);
        if !midi && !audio {
            return false;
        }
        if self.clip_loop.audio_gesture.is_some() {
            return true;
        }
        if self.edit_pending()
            || self.clip_loop.pending.is_some()
            || self.midi_note_gesture.is_some()
            || self.midi_group_gesture.is_some()
            || self.midi_stroke.is_some()
            || self.midi_ruler.held()
            || self.midi_time_selection.gesture.is_some()
            || self.audio_event_editor.gesture.is_some()
        {
            return true;
        }
        let Some(state) = &self.state else {
            return true;
        };
        let Some(clip) = self.detail_clip_id() else {
            return true;
        };
        if midi {
            let Some(mut values) =
                midi_source_controls::SourceValues::for_clip(&state.project, clip)
            else {
                return true;
            };
            let arrangement = state.project.midi_clip(clip);
            let selected = self
                .midi_time_selection_bounds()
                .filter(|(a, b)| a != b)
                .map(|(a, b)| (a.min(b), a.max(b)));
            if let Some(clip_item) = arrangement {
                let (first, last, next_looping) = if let Some((first, last)) = selected {
                    let unchanged = (first, last) == (values.loop_start, values.loop_end);
                    (first, last, if unchanged { !values.looping } else { true })
                } else {
                    (values.loop_start, values.loop_end, !values.looping)
                };
                let preview = match clip_item.with_scope_range(
                    hex_project::BeatTime::from_raw(first),
                    hex_project::BeatTime::from_raw(last),
                    next_looping,
                ) {
                    Ok(preview) => preview,
                    Err(error) => {
                        self.error = Some(error.to_string().into());
                        return true;
                    }
                };
                let displayed = SourceValues::from_clip(&preview);
                let operation = hex_project::MidiViewResizeOperation::ScopeRange {
                    start: hex_project::BeatTime::from_raw(first),
                    end: hex_project::BeatTime::from_raw(last),
                    looping: next_looping,
                };
                self.commit_midi_clip_command(
                    clip,
                    displayed,
                    Some(preview),
                    MidiEdit::Resize(operation),
                    Command::ResizeMidiClipView { clip, operation },
                );
            } else {
                if let Some((first, last)) = selected {
                    if last > values.source_length {
                        self.error = Some("clip scope selection exceeds its source".into());
                        return true;
                    }
                    values.looping = if (first, last) == (values.loop_start, values.loop_end) {
                        !values.looping
                    } else {
                        true
                    };
                    values.loop_start = first;
                    values.loop_end = last;
                } else {
                    values.looping = !values.looping;
                }
                if values.looping && values.source_start >= values.loop_end {
                    values.source_start = values.loop_start;
                }
                self.commit_midi_clip_loop(clip, values);
            }
        } else if !self.audio_event_editor.source_view {
            if !self.audio_loop_current() {
                return true;
            }
            let Some(source) = state.project.clip(clip) else {
                return true;
            };
            let root = source.audio_event_root();
            let active = source
                .audio_edit()
                .and_then(|e| e.placement)
                .map_or(source.looping(), |p| {
                    p.mode == hex_project::AudioViewMode::Repeat
                });
            let selected = self
                .audio_loop_time_selection()
                .filter(|(a, b)| a != b)
                .map(|(a, b)| (a.min(b), a.max(b)));
            let edit = match selected {
                Some((start, end))
                    if root.placement.is_some()
                        && (start, end) == (root.view_start, root.view_end) =>
                {
                    AudioEdit::Toggle(!active)
                }
                Some((start, end)) => AudioEdit::Resize(AudioViewResizeOperation::ScopeRange {
                    start,
                    end,
                    looping: true,
                }),
                None => AudioEdit::Toggle(!active),
            };
            self.commit_audio_loop_edit(clip, edit);
        }
        true
    }
    pub(super) fn pending_clip_loop_midi(&self, clip: ClipId) -> Option<SourceValues> {
        let p = self.clip_loop.pending.as_ref()?;
        if p.generation != self.project_generation || p.clip != clip {
            return None;
        }
        match &p.preview {
            Preview::Midi { values, .. } => Some(*values),
            _ => None,
        }
    }
    fn pending_midi_clip_loop_clip(&self) -> Option<&hex_project::MidiClip> {
        let p = self.clip_loop.pending.as_ref()?;
        if p.generation != self.project_generation {
            return None;
        }
        match &p.preview {
            Preview::Midi { clip, .. } => clip.as_ref(),
            _ => None,
        }
    }
    pub(super) fn pending_clip_loop_audio(&self, clip: ClipId) -> Option<&Clip> {
        self.pending_audio_clip_loop()
            .filter(|preview| preview.id() == clip)
    }

    fn pending_audio_clip_loop(&self) -> Option<&Clip> {
        let p = self.clip_loop.pending.as_ref()?;
        if p.generation != self.project_generation {
            return None;
        }
        match &p.preview {
            Preview::Audio { clip, .. } => Some(clip),
            _ => None,
        }
    }
    pub(super) fn commit_midi_clip_loop(&mut self, clip: ClipId, values: SourceValues) {
        if self.edit_pending() || self.clip_loop.pending.is_some() {
            return;
        }
        let Some((command, edit, preview_clip)) = self.state.as_ref().and_then(|s| {
            let command = midi_source_controls::edit_command(&s.project, clip, values)?;
            let edit = match &command {
                Command::ResizeMidiClipView { operation, .. } => MidiEdit::Resize(*operation),
                Command::SetMidiClipSource { .. } => MidiEdit::Source(values),
                Command::SetSessionMidiClipSource { .. } => MidiEdit::Session(values),
                _ => return None,
            };
            let preview_clip = s
                .project
                .midi_clip(clip)
                .and_then(|arrangement| match &command {
                    Command::ResizeMidiClipView {
                        operation: hex_project::MidiViewResizeOperation::PlayStart { source_start },
                        ..
                    } => arrangement.with_play_start(*source_start).ok(),
                    Command::ResizeMidiClipView {
                        operation:
                            hex_project::MidiViewResizeOperation::ScopeRange {
                                start,
                                end,
                                looping,
                            },
                        ..
                    } => arrangement.with_scope_range(*start, *end, *looping).ok(),
                    _ => None,
                });
            Some((command, edit, preview_clip))
        }) else {
            return;
        };
        self.commit_midi_clip_command(clip, values, preview_clip, edit, command);
    }
    fn commit_midi_clip_command(
        &mut self,
        clip: ClipId,
        values: SourceValues,
        preview_clip: Option<hex_project::MidiClip>,
        edit: MidiEdit,
        command: Command,
    ) {
        if self.edit_pending() || self.clip_loop.pending.is_some() {
            return;
        }
        self.clip_loop.pending = Some(Pending {
            replayed: false,
            revision: self.state.as_ref().map_or(0, |state| state.revision),
            generation: self.project_generation,
            clip,
            preview: Preview::Midi {
                values,
                clip: preview_clip,
                edit,
            },
            start_camera: None,
            receipt: Receipt::new(self.project_generation),
        });
        if !self.enqueue(Request::Edit(command)) {
            self.retire_pending_clip_loop();
        }
    }
    pub(super) fn commit_audio_clip_loop(
        &mut self,
        clip: ClipId,
        operation: AudioViewResizeOperation,
    ) {
        self.commit_audio_loop_edit(clip, AudioEdit::Resize(operation));
    }
    fn commit_audio_loop_edit(&mut self, clip: ClipId, edit: AudioEdit) {
        let play_start = matches!(
            edit,
            AudioEdit::Resize(AudioViewResizeOperation::PlayStart { .. })
        );
        let recovering = play_start && self.recovering_clip_play_start(clip);
        if (play_start && self.recovery.generation().is_some() && !recovering)
            || (self.edit_pending() && !recovering)
            || self.clip_loop.pending.is_some()
        {
            return;
        }
        let preview = self.state.as_ref().and_then(|s| match edit {
            AudioEdit::Toggle(looping) => Some(s.project.preview_audio_clip_looping(clip, looping)),
            AudioEdit::Resize(operation) => {
                let source = s.project.clip(clip)?;
                let asset = s.project.asset(source.asset())?;
                Some(hex_project::preview_audio_view_resize(
                    source,
                    s.project.tempo_map(),
                    asset.playback_frames(),
                    operation,
                ))
            }
        });
        let preview = match preview {
            Some(Ok(preview)) => preview,
            Some(Err(error)) => {
                self.error = Some(error.to_string().into());
                return;
            }
            None => return,
        };
        let start_camera = match edit {
            AudioEdit::Resize(AudioViewResizeOperation::PlayStart { position }) => {
                self.audio_play_start_camera(clip, position)
            }
            _ => None,
        };
        self.clip_loop.pending = Some(Pending {
            replayed: false,
            revision: self.state.as_ref().map_or(0, |state| state.revision),
            generation: self.project_generation,
            clip,
            start_camera,
            preview: Preview::Audio {
                clip: preview,
                edit,
            },
            receipt: Receipt::new(self.project_generation),
        });
        let command = match edit {
            AudioEdit::Resize(operation) => Command::ResizeAudioClipView { clip, operation },
            AudioEdit::Toggle(looping) => Command::SetClipLooping { clip, looping },
        };
        if !self.enqueue(Request::Edit(command)) {
            self.retire_pending_clip_loop();
        }
    }
    pub(super) fn pending_clip_loop_matches(&self, request: &Request) -> bool {
        let Some(p) = self
            .clip_loop
            .pending
            .as_ref()
            .filter(|p| p.receipt.is_queued())
        else {
            return false;
        };
        match (&p.preview, request) {
            (
                Preview::Audio {
                    edit: AudioEdit::Resize(operation),
                    ..
                },
                Request::Edit(Command::ResizeAudioClipView {
                    clip,
                    operation: actual,
                }),
            ) => *clip == p.clip && operation == actual,
            (
                Preview::Audio {
                    edit: AudioEdit::Toggle(looping),
                    ..
                },
                Request::Edit(Command::SetClipLooping {
                    clip,
                    looping: actual,
                }),
            ) => *clip == p.clip && looping == actual,
            (
                Preview::Midi {
                    edit: MidiEdit::Resize(operation),
                    ..
                },
                Request::Edit(Command::ResizeMidiClipView {
                    clip,
                    operation: actual,
                }),
            ) => *clip == p.clip && operation == actual,
            (
                Preview::Midi {
                    edit: MidiEdit::Source(v),
                    ..
                },
                Request::Edit(Command::SetMidiClipSource {
                    clip,
                    source_length,
                    source_start,
                    looping,
                    length,
                    ..
                }),
            ) => {
                *clip == p.clip
                    && source_length.raw() == v.source_length
                    && source_start.raw() == v.source_start
                    && *looping == v.looping
                    && length.raw() == v.arrangement_length
            }
            (
                Preview::Midi {
                    edit: MidiEdit::Session(v),
                    ..
                },
                Request::Edit(Command::SetSessionMidiClipSource {
                    clip,
                    source_length,
                    source_start,
                    loop_start,
                    loop_end,
                    looping,
                }),
            ) => {
                *clip == p.clip
                    && source_length.raw() == v.source_length
                    && source_start.raw() == v.source_start
                    && loop_start.raw() == v.loop_start
                    && loop_end.raw() == v.loop_end
                    && *looping == v.looping
            }
            _ => false,
        }
    }
    pub(super) fn clip_loop_submitted(&mut self) {
        if let Some(p) = &mut self.clip_loop.pending {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn retire_pending_clip_loop(&mut self) -> bool {
        let retired = self.clip_loop.pending.take().is_some();
        if retired {
            self.service.retire_arrangement_play_start_audition();
        }
        retired
    }
    pub(super) fn clip_loop_request_complete(&mut self, event: &Event) -> Option<Request> {
        if event.project_replaced {
            self.cancel_audio_loop_gesture();
        }
        let disposition = self
            .clip_loop
            .pending
            .as_mut()
            .map_or(Disposition::Waiting, |p| {
                p.receipt.observe(
                    self.project_generation,
                    event.project_replaced,
                    event.request_complete,
                )
            });
        if disposition == Disposition::Terminal
            && let Some(pending) = self.clip_loop.pending.as_mut()
            && !pending.replayed
            && let Preview::Audio {
                edit: AudioEdit::Resize(operation),
                ..
            } = pending.preview
            && event.resident_edit_source_unavailable
                == Some(hex_engine::AudioSourceContext {
                    project_generation: pending.generation,
                    editor_revision: pending.revision,
                })
            && event.state.revision == pending.revision
        {
            pending.receipt = Receipt::new(pending.generation);
            return Some(Request::Edit(Command::ResizeAudioClipView {
                clip: pending.clip,
                operation,
            }));
        }
        if disposition != Disposition::Waiting
            && let Some(pending) = self.clip_loop.pending.take()
        {
            self.service.retire_arrangement_play_start_audition();
            if disposition == Disposition::Terminal
                && let Some(camera) = pending.start_camera
            {
                self.install_audio_play_start_camera(
                    pending.clip,
                    pending.generation,
                    camera,
                    &event.state.project,
                );
            }
        }
        None
    }

    pub(super) fn mark_replayed_clip_loop(&mut self, request: &Request) {
        if self.pending_clip_loop_matches(request)
            && let Some(pending) = &mut self.clip_loop.pending
        {
            pending.replayed = true;
        }
    }
}

//! Explicit piano selection intent, arbitrated through the existing keyboard input owner.
//! Only pointer/keyboard selection actions capture a bank. Paint, ACK and project load
//! never do. Preparation remains a bounded Session request; engine owns the finite gate.
use super::EditorView;
use hex_engine::computer_keyboard::{ComputerKeyboardInput, MAX_NOTE_AUDITION_NOTES};
use hex_project::{ClipId, TrackId};
use hex_session::service::Request;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct MidiAudition {
    pub(super) enabled: bool,
    pending: Option<Selection>,
    active: Option<Active>,
}
#[derive(Clone)]
pub(super) struct Selection {
    generation: u64,
    revision: u64,
    clip: ClipId,
    track: TrackId,
    epoch: u64,
    notes: [(u8, u8); MAX_NOTE_AUDITION_NOTES],
    len: usize,
}
struct Active {
    selection: Selection,
    output: AuditionOutput,
}

enum AuditionOutput {
    Standalone { request: u64, triggered: bool },
    Live(Arc<ComputerKeyboardInput>),
}
impl MidiAudition {
    pub(super) fn clear(&mut self) {
        self.pending = None;
        if let Some(Active {
            output: AuditionOutput::Live(input),
            ..
        }) = self.active.take()
        {
            input.cancel_live_note_audition_on_control();
        }
    }
}
impl Selection {
    fn matches(&self, view: &EditorView) -> bool {
        view.midi_audition.enabled
            && view.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && view.clip_view_open
            && view.project_generation == self.generation
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.revision == self.revision)
            && view.detail_clip_id() == Some(self.clip)
            && view.midi_selection.epoch() == self.epoch
            && !view.opening_project
            && !view.close_prompt_open
    }
}
impl EditorView {
    pub(super) fn cancel_midi_audition(&mut self) {
        self.midi_audition.clear();
        if self
            .computer_keyboard
            .input
            .as_ref()
            .is_some_and(|input| input.is_note_audition())
        {
            self.retire_computer_keyboard_audio();
        }
    }
    /// Capture at most 17 complete notes in one bounded pass over the selected source.
    /// More notes refuse the whole chord; no velocity quantization or truncation.
    pub(super) fn audition_midi_selection(&mut self) {
        let selected = self.midi_selection.shared_ids();
        self.audition_midi_notes(&selected);
    }
    /// The marquee supplies only newly entered, sorted note identities; ordinary
    /// selection actions supply the complete selected bank. Both use one owner.
    pub(super) fn audition_midi_notes(&mut self, ids: &[hex_project::NoteId]) {
        if !self.midi_audition.enabled {
            return;
        }
        self.cancel_midi_audition();
        if self.document_audio_mode != hex_session::DocumentAudioMode::Resident {
            self.error = Some("note audition is unavailable for verified streaming audio".into());
            return;
        }
        let len = ids.len();
        if len == 0 {
            return;
        }
        if len > MAX_NOTE_AUDITION_NOTES {
            self.error = Some("note audition supports at most 17 selected notes".into());
            return;
        }
        let Some(clip) = self.detail_midi_clip_data() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(source) = state.project.midi_source(clip) else {
            return;
        };
        let owner = state.project.tracks().iter()
            .find(|track| track.midi_clips().iter().any(|item| item.id() == clip)).map(|track| track.id())
            .or_else(|| state.project.session_cells().iter().find_map(|cell| {
                matches!(cell.content(), hex_project::SessionCellContent::Midi(item) if item.id() == clip).then_some(cell.track())
            }));
        let Some(track) = owner else { return };
        let mut notes = [(0, 0); MAX_NOTE_AUDITION_NOTES];
        let mut found = 0;
        for note in source.notes() {
            if ids.binary_search(&note.id()).is_ok() {
                let Some(slot) = notes.get_mut(found) else {
                    return;
                };
                *slot = (note.pitch(), note.velocity());
                found += 1;
                if found == len {
                    break;
                }
            }
        }
        if found != len {
            return;
        }
        self.midi_audition.pending = Some(Selection {
            generation: self.project_generation,
            revision: state.revision,
            clip,
            track,
            epoch: self.midi_selection.epoch(),
            notes,
            len,
        });
    }
    pub(super) fn cancel_marquee_audition(
        &mut self,
        context: super::midi_selection::MarqueeContext,
    ) {
        let matches = |selection: &Selection| {
            selection.generation == context.scene.generation
                && selection.clip == context.scene.clip
                && selection.epoch == context.selection_epoch
        };
        if self.midi_audition.pending.as_ref().is_some_and(matches)
            || self
                .midi_audition
                .active
                .as_ref()
                .is_some_and(|active| matches(&active.selection))
        {
            self.cancel_midi_audition();
        }
    }

    /// Standalone audition takes the keyboard slot. With a hardware port
    /// selected, physical preparation runs separately and retains its root.
    pub(super) fn tick_midi_audition(&mut self) -> Option<bool> {
        let invalid = self
            .midi_audition
            .pending
            .as_ref()
            .is_some_and(|s| !s.matches(self))
            || self
                .midi_audition
                .active
                .as_ref()
                .is_some_and(|a| !a.selection.matches(self));
        if invalid {
            self.cancel_midi_audition();
            return Some(true);
        }
        if self.computer_keyboard.physical_notes().next().is_some() {
            return None;
        }
        if let Some(active) = &self.midi_audition.active {
            let failed = self.computer_keyboard.input.as_ref().is_none_or(|input| {
                let same = match &active.output {
                    AuditionOutput::Standalone { .. } => input.is_note_audition(),
                    AuditionOutput::Live(root) => {
                        self.controller_wants_live_notes() && Arc::ptr_eq(input, root)
                    }
                };
                !same
                    || input.is_cancelled()
                    || input.rejected()
                    || (self.worker_owns_keyboard_publication()
                        && input.is_output_bound()
                        && !self.audio.as_ref().is_some_and(|audio| {
                            audio.control.computer_keyboard_input_matches_output(input)
                        }))
            });
            if failed {
                self.cancel_midi_audition();
                return Some(true);
            }
            return Some(false);
        }
        let selection = self.midi_audition.pending.as_ref()?;
        // Selection dragging must keep its existing gesture/command path responsive.
        if self.edit_pending()
            || self.midi_note_gesture.is_some()
            || self.midi_velocity.gesture.is_some()
            || self.midi_group_gesture.is_some()
            || self.midi_creation.is_some()
            || self.midi_stroke.is_some()
            || self.tuning.client.snapshot().is_some()
            || self.tuning.client.has_pending_requests()
            || self.tuning.client.is_recovery_pending()
        {
            return Some(false);
        }
        if self.document_audio_mode != hex_session::DocumentAudioMode::Resident {
            self.error = Some("note audition is unavailable for verified streaming audio".into());
            self.cancel_midi_audition();
            return Some(true);
        }
        let selection = selection.clone();
        let sample_rate = self.state.as_ref()?.project.sample_rate();
        let gate_frames = u64::from(sample_rate / 4).max(1);
        if self.controller_wants_live_notes() {
            // The editor and the hardware share an admitted track root. Wait
            // for that exact route; never send a clip preview to another track.
            if self.selected_track != Some(selection.track) {
                self.error =
                    Some("select the clip’s instrument track to audition its notes".into());
                self.cancel_midi_audition();
                return Some(true);
            }
            if !self.computer_keyboard.prepared
                || self.computer_keyboard.target
                    != Some((selection.generation, selection.revision, selection.track))
            {
                return Some(false);
            }
            let input = Arc::clone(self.computer_keyboard.input.as_ref()?);
            let result = input.trigger_live_note_audition_on_control(
                &selection.notes[..selection.len],
                gate_frames,
            );
            self.midi_audition.pending = None;
            match result {
                Ok(()) => {
                    self.midi_audition.active = Some(Active {
                        selection,
                        output: AuditionOutput::Live(input),
                    })
                }
                Err(error) => self.error = Some(error.to_string().into()),
            }
            return Some(true);
        }
        let fresh = if self.worker_owns_keyboard_publication() {
            self.audio
                .as_ref()
                .ok_or(hex_engine::Invalid("audio output is unavailable"))
                .and_then(|audio| audio.control.new_computer_keyboard_input_on_control())
        } else {
            Ok(ComputerKeyboardInput::default())
        };
        let input = match fresh.and_then(|input| {
            input.with_note_audition(&selection.notes[..selection.len], gate_frames)
        }) {
            Ok(input) => Arc::new(input),
            Err(error) => {
                self.error = Some(error.to_string().into());
                self.cancel_midi_audition();
                return Some(true);
            }
        };
        let Some(request) = self.computer_keyboard.request_id.checked_add(1) else {
            self.error = Some("note audition request identity exhausted".into());
            self.cancel_midi_audition();
            return Some(true);
        };
        self.retire_computer_keyboard_audio();
        self.computer_keyboard.request_id = request;
        self.computer_keyboard.suspended = false;
        self.computer_keyboard.target =
            Some((selection.generation, selection.revision, selection.track));
        self.computer_keyboard.input = Some(Arc::clone(&input));
        self.midi_audition.pending = None;
        self.midi_audition.active = Some(Active {
            selection: selection.clone(),
            output: AuditionOutput::Standalone {
                request,
                triggered: false,
            },
        });
        if !self.enqueue(Request::PrepareComputerKeyboard {
            project_generation: selection.generation,
            revision: selection.revision,
            request_id: request,
            track: selection.track,
            input,
        }) {
            self.cancel_midi_audition();
        }
        Some(true)
    }
    pub(super) fn midi_audition_preparation_matches(&self, request: u64, revision: u64) -> bool {
        self.midi_audition.active.as_ref().is_some_and(|active| {
            matches!(active.output, AuditionOutput::Standalone { request: id, .. } if id == request)
                && active.selection.revision == revision
                && active.selection.matches(self)
        }) && self
            .computer_keyboard
            .input
            .as_ref()
            .is_some_and(|input| input.is_note_audition() && !input.is_cancelled())
    }
    pub(super) fn trigger_prepared_midi_audition(&mut self) -> Result<(), hex_engine::Invalid> {
        let active = self
            .midi_audition
            .active
            .as_mut()
            .ok_or(hex_engine::Invalid("note audition intent expired"))?;
        let AuditionOutput::Standalone { triggered, .. } = &mut active.output else {
            return Err(hex_engine::Invalid(
                "note audition does not own the keyboard input",
            ));
        };
        if !*triggered {
            self.computer_keyboard
                .input
                .as_ref()
                .ok_or(hex_engine::Invalid("note audition input expired"))?
                .trigger_note_audition()?;
            *triggered = true;
        }
        Ok(())
    }
    /// A fresh physical press cancels sounding audition. Only an unplayed intent
    /// is deferred, and is restored after physical ownership has been captured.
    pub(super) fn pause_midi_audition_for_keyboard(&mut self) -> Option<Selection> {
        let pending = self.midi_audition.pending.take().or_else(|| {
            self.midi_audition
                .active
                .as_ref()
                .filter(|active| {
                    matches!(
                        active.output,
                        AuditionOutput::Standalone {
                            triggered: false,
                            ..
                        }
                    )
                })
                .map(|active| active.selection.clone())
        });
        self.cancel_midi_audition();
        pending
    }
    pub(super) fn defer_paused_midi_audition(&mut self, pending: Option<Selection>) {
        if let Some(selection) = pending
            && selection.matches(self)
            && self.computer_keyboard.physical_notes().next().is_some()
        {
            self.midi_audition.pending = Some(selection);
        }
    }
}

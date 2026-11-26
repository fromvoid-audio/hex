//! Explicit piano selection and edit audition through the existing keyboard input owner.
//! When audition is enabled in Resident mode, a single reusable live-audition root is
//! prepared on the clip's instrument track so clicks, drags, creations and nudges trigger
//! synchronously on control without per-note worker roundtrips.
use super::EditorView;
use hex_engine::computer_keyboard::{ComputerKeyboardInput, MAX_NOTE_AUDITION_NOTES};
use hex_project::{ClipId, TrackId};
use hex_session::service::Request;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct MidiAudition {
    pub(super) enabled: bool,
    pending: Option<Intent>,
    active: Option<Active>,
}

#[derive(Clone)]
pub(super) struct Intent {
    generation: u64,
    revision: u64,
    clip: ClipId,
    track: TrackId,
    epoch: Option<u64>,
    notes: [(u8, u8); MAX_NOTE_AUDITION_NOTES],
    len: usize,
}

struct Active {
    trigger: u64,
    intent: Intent,
    output: Arc<ComputerKeyboardInput>,
}

impl MidiAudition {
    pub(super) fn clear(&mut self) {
        self.pending = None;
        if let Some(Active { output, .. }) = self.active.take() {
            output.cancel_live_note_audition_on_control();
        }
    }

    pub(super) fn active_matches_bank(&self, clip: ClipId, notes: &[(u8, u8)]) -> bool {
        self.active.as_ref().is_some_and(|active| {
            active.intent.clip == clip
                && active.intent.len == notes.len()
                && active.intent.notes[..active.intent.len] == *notes
                && !active.output.is_cancelled()
                && active.output.live_audition_id() == active.trigger
        })
    }
}

impl Intent {
    fn matches_pending(&self, view: &EditorView) -> bool {
        view.midi_editor.audition.enabled
            && view.audio_pub.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && view.audio_detail.clip_view_open
            && view.project_generation == self.generation
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.revision == self.revision)
            && view.detail_clip_id() == Some(self.clip)
            && self
                .epoch
                .is_none_or(|epoch| view.midi_editor.selection.epoch() == epoch)
            && !view.persistence.opening_project
            && !view.persistence.close_prompt_open
    }

    fn matches_active(&self, view: &EditorView) -> bool {
        view.midi_editor.audition.enabled
            && view.audio_pub.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && view.audio_detail.clip_view_open
            && view.project_generation == self.generation
            && view.state.as_ref().is_some_and(|state| {
                if self.epoch.is_none() {
                    state.revision >= self.revision
                } else {
                    state.revision == self.revision
                }
            })
            && view.detail_clip_id() == Some(self.clip)
            && self
                .epoch
                .is_none_or(|epoch| view.midi_editor.selection.epoch() == epoch)
            && !view.persistence.opening_project
            && !view.persistence.close_prompt_open
    }
}

impl EditorView {
    pub(super) fn detail_midi_clip_owner_track(&self) -> Option<TrackId> {
        let clip = self.detail_midi_clip_data()?;
        let state = self.state.as_ref()?;
        state
            .project
            .tracks()
            .iter()
            .find(|track| track.midi_clips().iter().any(|item| item.id() == clip))
            .map(|track| track.id())
            .or_else(|| {
                state.project.session_cells().iter().find_map(|cell| {
                    matches!(cell.content(), hex_project::SessionCellContent::Midi(item) if item.id() == clip)
                        .then_some(cell.track())
                })
            })
    }

    fn track_has_enabled_instrument(&self, track: TrackId) -> bool {
        self.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .and_then(hex_project::Track::instrument)
            .is_some_and(|device| !device.bypassed())
    }

    fn is_clip_audition_only_root(&self) -> bool {
        !self.computer_keyboard.enabled
            && !self.controller_wants_live_notes()
            && self
                .computer_keyboard
                .input
                .as_ref()
                .is_some_and(|input| input.has_live_audition() && input.live_notes().is_none())
    }

    fn retire_clip_audition_root_if_unneeded(&mut self) {
        if self.is_clip_audition_only_root() {
            self.retire_computer_keyboard_audio();
        }
    }

    pub(super) fn ready_live_audition_input(
        &self,
        track: TrackId,
    ) -> Option<Arc<ComputerKeyboardInput>> {
        if !self.computer_keyboard.prepared
            || self.computer_keyboard.physical_notes().next().is_some()
        {
            return None;
        }
        let state = self.state.as_ref()?;
        if !self.track_has_enabled_instrument(track) {
            return None;
        }
        let worker_owned = self.worker_owns_keyboard_publication();
        let (generation, target_revision, target_track) = self.computer_keyboard.target?;
        if generation != self.project_generation || target_track != track {
            return None;
        }
        let revision_ok = if worker_owned {
            target_revision <= state.revision
        } else {
            target_revision == state.revision
        };
        if !revision_ok {
            return None;
        }
        let input = self.computer_keyboard.input.as_ref()?;
        if !input.has_live_audition() || input.is_cancelled() || input.rejected() {
            return None;
        }
        if worker_owned
            && input.is_output_bound()
            && !self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.control.computer_keyboard_input_matches_output(input))
        {
            return None;
        }
        if self.controller_wants_live_notes() && self.selected_track != Some(track) {
            return None;
        }
        Some(Arc::clone(input))
    }

    fn live_audition_root_preparing(&self, track: TrackId) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        !self.computer_keyboard.prepared
            && self.computer_keyboard.target
                == Some((self.project_generation, state.revision, track))
            && self
                .computer_keyboard
                .input
                .as_ref()
                .is_some_and(|input| input.has_live_audition() && !input.is_cancelled())
    }

    pub(super) fn cancel_midi_audition(&mut self) {
        self.cancel_pending_midi_edit_audition();
        self.midi_editor.audition.clear();
        if self
            .computer_keyboard
            .input
            .as_ref()
            .is_some_and(|input| input.is_note_audition())
        {
            self.retire_computer_keyboard_audio();
        } else if !self.midi_editor.audition.enabled || !self.audio_detail.clip_view_open {
            self.retire_clip_audition_root_if_unneeded();
        }
    }

    /// Retire the departing focus's physical keys and preserve an explicit
    /// selection/preview captured by the click that just moved focus into the piano roll.
    /// Hardware and reusable live audition keep their admitted route across piano-roll entry.
    pub(super) fn blur_computer_keyboard(&mut self, window: &gpui::Window) {
        let entering_piano_roll = self.midi_editor.focus.is_focused(window);
        if !entering_piano_roll {
            self.cancel_pending_midi_edit_audition();
        }
        if entering_piano_roll {
            let keep_live = (self.controller_wants_live_notes()
                || self.midi_editor.audition.enabled)
                && self.computer_keyboard.input.as_ref().is_some_and(|input| {
                    (input.live_notes().is_some() || input.has_live_audition())
                        && !input.is_cancelled()
                });
            if keep_live {
                self.clear_physical_keys_preserving_audition();
                return;
            }
            let pending = self.midi_editor.audition.pending.take();
            self.cancel_computer_keyboard();
            if !self.midi_editor.audition.enabled {
                self.computer_keyboard.suspended = true;
            }
            self.midi_editor.audition.pending = pending;
            return;
        }
        if self.controller_wants_live_notes()
            && let Some(input) = self
                .computer_keyboard
                .input
                .as_ref()
                .filter(|input| input.live_notes().is_some() && !input.is_cancelled())
        {
            input.clear_keys_on_control();
            self.computer_keyboard.release_all();
            self.midi_editor.audition.clear();
        } else {
            self.cancel_computer_keyboard();
            self.computer_keyboard.suspended = true;
        }
    }

    /// Capture at most 17 complete notes in one bounded pass over the selected source.
    /// More notes refuse the whole chord; no velocity quantization or truncation.
    pub(super) fn audition_midi_selection(&mut self) -> bool {
        let selected = self.midi_editor.selection.shared_ids();
        self.audition_midi_notes(&selected)
    }

    /// The marquee supplies only newly entered, sorted note identities; ordinary
    /// selection actions supply the complete selected bank. Both use one owner.
    pub(super) fn audition_midi_notes(&mut self, ids: &[hex_project::NoteId]) -> bool {
        if !self.midi_editor.audition.enabled {
            return false;
        }
        self.cancel_midi_audition();
        if self.audio_pub.document_audio_mode != hex_session::DocumentAudioMode::Resident {
            self.error = Some("note audition is unavailable for verified streaming audio".into());
            return false;
        }
        let len = ids.len();
        if len == 0 {
            return false;
        }
        if len > MAX_NOTE_AUDITION_NOTES {
            self.error = Some("note audition supports at most 17 selected notes".into());
            return false;
        }
        let Some(clip) = self.detail_midi_clip_data() else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(source) = state.project.midi_source(clip) else {
            return false;
        };
        let Some(track) = self.detail_midi_clip_owner_track() else {
            return false;
        };
        let mut notes = [(0, 0); MAX_NOTE_AUDITION_NOTES];
        let mut found = 0;
        for note in source.notes() {
            if ids.binary_search(&note.id()).is_ok() {
                let Some(slot) = notes.get_mut(found) else {
                    return false;
                };
                *slot = (note.pitch(), note.velocity());
                found += 1;
                if found == len {
                    break;
                }
            }
        }
        if found != len {
            return false;
        }
        self.queue_and_trigger_midi_audition(Intent {
            generation: self.project_generation,
            revision: state.revision,
            clip,
            track,
            epoch: Some(self.midi_editor.selection.epoch()),
            notes,
            len,
        })
    }

    /// Immediately audition preview notes during held drags, creation, draw strokes
    /// or keyboard nudges without waiting for worker edit roundtrips.
    pub(super) fn audition_midi_preview_bank(
        &mut self,
        clip: ClipId,
        preview: &[(u8, u8)],
    ) -> bool {
        if !self.midi_editor.audition.enabled {
            return false;
        }
        if self.audio_pub.document_audio_mode != hex_session::DocumentAudioMode::Resident {
            self.error = Some("note audition is unavailable for verified streaming audio".into());
            self.cancel_midi_audition();
            return false;
        }
        let len = preview.len();
        if len == 0 {
            return false;
        }
        if len > MAX_NOTE_AUDITION_NOTES {
            self.error = Some("note audition supports at most 17 selected notes".into());
            self.cancel_midi_audition();
            return false;
        }
        if self.detail_midi_clip_data() != Some(clip) {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(track) = self.detail_midi_clip_owner_track() else {
            return false;
        };
        let mut notes = [(0, 0); MAX_NOTE_AUDITION_NOTES];
        notes[..len].copy_from_slice(preview);
        self.midi_editor.audition.clear();
        self.queue_and_trigger_midi_audition(Intent {
            generation: self.project_generation,
            revision: state.revision,
            clip,
            track,
            epoch: None,
            notes,
            len,
        })
    }

    fn queue_and_trigger_midi_audition(&mut self, intent: Intent) -> bool {
        self.midi_editor.audition.pending = Some(intent);
        if self.try_trigger_midi_audition_now() {
            return true;
        }
        self.wake_poll();
        false
    }

    pub(super) fn try_trigger_midi_audition_now(&mut self) -> bool {
        let Some(intent) = self.midi_editor.audition.pending.as_ref() else {
            return false;
        };
        if !intent.matches_pending(self) {
            self.midi_editor.audition.pending = None;
            return false;
        }
        if self.computer_keyboard.physical_notes().next().is_some() {
            return false;
        }
        if self.controller_wants_live_notes() && self.selected_track != Some(intent.track) {
            self.error = Some("select the clip’s instrument track to audition its notes".into());
            self.cancel_midi_audition();
            return false;
        }
        let Some(input) = self.ready_live_audition_input(intent.track) else {
            return false;
        };
        let Some(sample_rate) = self.state.as_ref().map(|state| state.project.sample_rate()) else {
            return false;
        };
        let gate_frames = u64::from(sample_rate / 4).max(1);
        let Some(intent) = self.midi_editor.audition.pending.take() else {
            return false;
        };
        if self.worker_owns_keyboard_publication()
            && let Some((_, revision, _)) = self.computer_keyboard.target.as_mut()
            && let Some(state) = self.state.as_ref()
        {
            *revision = state.revision;
        }
        match input.trigger_live_note_audition_on_control(&intent.notes[..intent.len], gate_frames)
        {
            Ok(trigger) => {
                self.midi_editor.audition.active = Some(Active {
                    trigger,
                    intent,
                    output: input,
                });
                true
            }
            Err(error) => {
                self.error = Some(error.to_string().into());
                false
            }
        }
    }

    pub(super) fn cancel_marquee_audition(
        &mut self,
        context: super::midi_selection::MarqueeContext,
    ) {
        let matches = |intent: &Intent| {
            intent.generation == context.scene.generation
                && intent.clip == context.scene.clip
                && intent.epoch == Some(context.selection_epoch)
        };
        if self
            .midi_editor
            .audition
            .pending
            .as_ref()
            .is_some_and(matches)
            || self
                .midi_editor
                .audition
                .active
                .as_ref()
                .is_some_and(|active| matches(&active.intent))
        {
            self.cancel_midi_audition();
        }
    }

    /// Maintain the reusable live-audition root while audition is enabled on a clip
    /// track, and flush any deferred/pending note bank as soon as its root is ready.
    /// Returns `Some(changed)` when clip audition handled the keyboard slot.
    pub(super) fn tick_midi_audition(&mut self) -> Option<bool> {
        if self.is_clip_audition_only_root()
            && self
                .computer_keyboard
                .target
                .is_some_and(|(generation, _, _)| generation != self.project_generation)
        {
            self.retire_computer_keyboard_audio();
        }

        let invalid_pending = self
            .midi_editor
            .audition
            .pending
            .as_ref()
            .is_some_and(|intent| !intent.matches_pending(self));
        let invalid_active = self
            .midi_editor
            .audition
            .active
            .as_ref()
            .is_some_and(|active| !active.intent.matches_active(self));
        if invalid_pending || invalid_active {
            self.cancel_midi_audition();
            return Some(true);
        }

        if let Some(active) = &self.midi_editor.audition.active {
            if active.output.live_note_audition_refused(active.trigger) {
                self.midi_editor.audition.active = None;
                self.error = Some("instrument could not play the complete audition chord".into());
                return Some(true);
            }
            let failed = self.computer_keyboard.input.as_ref().is_none_or(|input| {
                !Arc::ptr_eq(input, &active.output)
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
        }

        if !self.midi_editor.audition.enabled
            || !self.audio_detail.clip_view_open
            || self.persistence.opening_project
            || self.persistence.close_prompt_open
            || self.audio_pub.document_audio_mode != hex_session::DocumentAudioMode::Resident
        {
            if self.is_clip_audition_only_root() {
                self.retire_computer_keyboard_audio();
                return Some(true);
            }
            return None;
        }

        if self.computer_keyboard.physical_notes().next().is_some() {
            return None;
        }

        if self.try_trigger_midi_audition_now() {
            return Some(true);
        }

        if self.controller_wants_live_notes() {
            if self.midi_editor.audition.pending.is_some()
                && self
                    .midi_editor
                    .audition
                    .pending
                    .as_ref()
                    .is_some_and(|intent| self.selected_track != Some(intent.track))
            {
                self.error =
                    Some("select the clip’s instrument track to audition its notes".into());
                self.cancel_midi_audition();
                return Some(true);
            }
            return None;
        }

        let Some(track) = self.detail_midi_clip_owner_track() else {
            if self.is_clip_audition_only_root() {
                self.retire_computer_keyboard_audio();
                return Some(true);
            }
            return None;
        };

        if !self.track_has_enabled_instrument(track) {
            if self.midi_editor.audition.pending.is_some() {
                self.midi_editor.audition.pending = None;
            }
            if self.is_clip_audition_only_root() {
                self.retire_computer_keyboard_audio();
                return Some(true);
            }
            return None;
        }

        // If physical QWERTY keyboard is also enabled on a different track and there
        // is no active clip audition request, let physical keyboard keep its slot.
        if self.computer_keyboard.enabled
            && !self.computer_keyboard.suspended
            && self.selected_track != Some(track)
            && self.midi_editor.audition.pending.is_none()
            && self.midi_editor.audition.active.is_none()
        {
            return None;
        }

        if self.ready_live_audition_input(track).is_some() {
            if self.worker_owns_keyboard_publication()
                && let Some((_, revision, _)) = self.computer_keyboard.target.as_mut()
                && let Some(state) = self.state.as_ref()
                && *revision != state.revision
            {
                *revision = state.revision;
            }
            return Some(false);
        }

        if self.live_audition_root_preparing(track) {
            return Some(false);
        }

        // Never tear down a currently sounding live-audition root just to re-prepare
        // a silent root across a non-worker-owned revision advance.
        if self.midi_editor.audition.pending.is_none()
            && self
                .midi_editor
                .audition
                .active
                .as_ref()
                .is_some_and(|active| {
                    active.intent.track == track
                        && !active.output.is_cancelled()
                        && active.output.live_audition_id() == active.trigger
                })
        {
            return Some(false);
        }

        // Do not proactively spin up a fresh root while suspended (after focus loss
        // to Arrangement or Stop) until an explicit piano-roll audition intent arrives.
        if self.computer_keyboard.suspended && self.midi_editor.audition.pending.is_none() {
            return Some(false);
        }

        // Only root preparation (which enqueues PrepareComputerKeyboard) waits for
        // idle worker/pointer state; once prepared, triggers are lock-free and immediate.
        if self.edit_pending()
            || self.midi_editor.note_gesture.is_some()
            || self.midi_editor.velocity.gesture.is_some()
            || self.midi_editor.group_gesture.is_some()
            || self.midi_editor.creation.is_some()
            || self.midi_editor.stroke.is_some()
            || self.device_gesture_pending()
            || self.audio_pub.prepared_seek.is_some()
            || (self.audio_pub.stopped_playback_restore.is_some()
                && !self.worker_owns_keyboard_publication())
            || self.settings.tuning.client.snapshot().is_some()
            || self.settings.tuning.client.has_pending_requests()
            || self.settings.tuning.client.is_recovery_pending()
        {
            return Some(false);
        }

        let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
            return Some(false);
        };
        let fresh = if self.worker_owns_keyboard_publication() {
            self.audio
                .as_ref()
                .ok_or(hex_engine::Invalid("audio output is unavailable"))
                .and_then(|audio| audio.control.new_computer_keyboard_input_on_control())
        } else {
            Ok(ComputerKeyboardInput::default())
        };
        let input = match fresh.and_then(ComputerKeyboardInput::with_live_audition) {
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
        self.computer_keyboard.target = Some((self.project_generation, revision, track));
        self.computer_keyboard.input = Some(Arc::clone(&input));
        if !self.enqueue(Request::PrepareComputerKeyboard {
            project_generation: self.project_generation,
            revision,
            request_id: request,
            track,
            input,
        }) {
            self.retire_computer_keyboard_audio();
            self.midi_editor.audition.clear();
        }
        Some(true)
    }

    pub(super) fn midi_audition_preparation_matches(&self, request: u64, revision: u64) -> bool {
        self.midi_editor.audition.enabled
            && self.audio_detail.clip_view_open
            && !self.persistence.opening_project
            && !self.persistence.close_prompt_open
            && self.audio_pub.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && request == self.computer_keyboard.request_id
            && self.computer_keyboard.target
                == self
                    .detail_midi_clip_owner_track()
                    .map(|track| (self.project_generation, revision, track))
            && self.computer_keyboard.input.as_ref().is_some_and(|input| {
                input.has_live_audition() && input.live_notes().is_none() && !input.is_cancelled()
            })
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == revision)
    }

    /// A fresh physical press cancels sounding audition. Only an unplayed intent
    /// is deferred, and is restored after physical ownership has been captured.
    pub(super) fn pause_midi_audition_for_keyboard(&mut self) -> Option<Intent> {
        let pending = self.midi_editor.audition.pending.take();
        self.midi_editor.audition.clear();
        pending
    }

    pub(super) fn defer_paused_midi_audition(&mut self, pending: Option<Intent>) {
        if let Some(intent) = pending
            && intent.matches_pending(self)
            && self.computer_keyboard.physical_notes().next().is_some()
        {
            self.midi_editor.audition.pending = Some(intent);
        }
    }
}

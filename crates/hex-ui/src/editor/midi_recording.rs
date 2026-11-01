//! Transient take custody. The audio endpoint supplies delivered events; Session
//! pairs them and owns the source-bound edit. Only its exact terminal retires notes.
use super::*;
use hex_audio::midi_recording::{Receipt, Target};
use hex_engine::midi_capture::MidiCaptureBinding;
use hex_session::midi_recording::{MidiTake, MidiTakeBinding, RecordedMidiNote};
use std::sync::{Arc, atomic::Ordering::Acquire};

#[cfg(feature = "ui-test-support")]
#[path = "midi_recording_pressure_fixture.rs"]
pub(crate) mod pressure_fixture;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Mode {
    #[default]
    Idle,
    Preparing,
    Capturing,
    Finishing,
    Ready,
    Interrupted,
    Committing,
}
#[derive(Default)]
pub(super) struct Recorder {
    mode: Mode,
    serial: u64,
    commit_serial: u64,
    pending_commit: Option<u64>,
    output_identity: Option<u64>,
    prepared_audio_revision: Option<u64>,
    binding: Option<Arc<MidiTakeBinding>>,
    generation: u64,
    revision: u64,
    input: Option<Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>>,
    keys: Option<hex_engine::computer_keyboard::MidiKeyActivation>,
    target: Option<Target>,
    take: Option<MidiTake>,
    notes: Option<Arc<[RecordedMidiNote]>>,
    stop_frame: Option<u64>,
}
impl Recorder {
    pub(super) fn blocks_edits(&self) -> bool {
        self.mode != Mode::Idle
    }
    pub(super) fn accepts_commit(&self, id: u64) -> bool {
        self.mode == Mode::Ready && self.pending_commit == Some(id)
    }
    pub(super) fn preparing(&self) -> bool {
        self.mode == Mode::Preparing
    }
    pub(super) fn label(&self) -> &'static str {
        match self.mode {
            Mode::Idle => {
                "record midi into selected arrangement clip · start playback inside the clip first"
            }
            Mode::Preparing => "preparing midi recording · click to cancel",
            Mode::Capturing | Mode::Finishing => "finish midi recording",
            Mode::Ready => "retry committing retained midi take",
            Mode::Interrupted => {
                "interrupted midi take retained · discard explicitly before retrying"
            }
            Mode::Committing => "committing midi take · waiting for acknowledgment",
        }
    }
    fn seal(&self) {
        if let Some((input, keys)) = self.input.as_ref().zip(self.keys) {
            let _ = input.seal_ordered_keys_on_control(keys);
        }
    }
    fn clear(&mut self) {
        self.seal();
        let serial = self.serial;
        let commit_serial = self.commit_serial;
        *self = Self {
            serial,
            commit_serial,
            ..Self::default()
        };
    }
}
impl EditorView {
    pub(super) fn midi_recording_published(
        &mut self,
        context: hex_engine::AudioSourceContext,
        request_id: Option<u64>,
        audio_revision: u64,
    ) {
        if self.midi_recorder.preparing()
            && request_id == Some(self.computer_keyboard.request_id)
            && context.project_generation == self.midi_recorder.generation
            && context.editor_revision == self.midi_recorder.revision
            && self
                .audio
                .as_ref()
                .and_then(|audio| audio.control.output_identity())
                == self.midi_recorder.output_identity
        {
            self.midi_recorder.prepared_audio_revision = Some(audio_revision);
        }
    }
    pub(super) fn midi_take_not_submitted(&mut self, request: &Request) {
        if let Request::CommitMidiTake { request_id, .. } = request
            && self.midi_recorder.pending_commit == Some(*request_id)
        {
            self.midi_recorder.mode = Mode::Ready;
            self.midi_recorder.pending_commit = None;
        }
    }
    pub(super) fn midi_service_disconnected(&mut self) {
        if self.midi_recorder.mode == Mode::Committing {
            self.status = "midi commit outcome unknown · take retained".into();
        }
        if matches!(
            self.midi_recorder.mode,
            Mode::Preparing | Mode::Capturing | Mode::Finishing | Mode::Committing
        ) {
            self.midi_recorder.seal();
            if let Some(audio) = &self.audio {
                audio.control.midi_cancel();
            }
            if let Some(take) = &mut self.midi_recorder.take {
                take.interrupt();
            }
            self.midi_recorder.mode = Mode::Interrupted;
        }
    }
    pub(super) fn midi_recording_preparation_failed(&mut self) {
        if self.midi_recorder.preparing() {
            self.midi_recorder.mode = Mode::Interrupted;
        }
    }
    /// Decide before priority Stop cancels its own input prefix. A known
    /// disconnect/refusal must stay interrupted, even if no callback has yet
    /// observed it. Cancel wins over the endpoint's subsequent normal Stop.
    pub(super) fn finish_midi_recording(&mut self) {
        if self.midi_recorder.preparing() {
            self.midi_recorder.clear();
            return;
        }
        if self.midi_recorder.mode != Mode::Capturing {
            return;
        }
        let valid = self
            .midi_recorder
            .input
            .as_ref()
            .zip(self.midi_recorder.target)
            .is_some_and(|(input, target)| {
                input.recording_input_current_on_control(
                    target.capture.keys,
                    target.capture.output_epoch,
                    target.capture.hardware,
                )
            });
        if let Some(audio) = &self.audio {
            if valid {
                audio.control.midi_finish();
            } else {
                audio.control.midi_cancel();
            }
        }
        self.midi_recorder.mode = Mode::Finishing;
    }
    pub(super) fn toggle_midi_recording(&mut self) {
        match self.midi_recorder.mode {
            Mode::Capturing => {
                self.finish_midi_recording();
                return;
            }
            Mode::Preparing => {
                self.midi_recorder.clear();
                return;
            }
            Mode::Ready => {
                self.commit_midi_take();
                return;
            }
            Mode::Idle => {}
            _ => return,
        }
        if self.edit_pending()
            || self.gain_recorder.blocks_edits()
            || self.midi_step.enabled
            || self.pending_playback.is_some()
            || !self.playback_candidates.is_empty()
            || self.mode != EditorMode::Arrangement
            || self.document_audio_mode != hex_session::DocumentAudioMode::Resident
            || !self.worker_owns_keyboard_publication()
        {
            self.error = Some(
                "finish pending edits and use resident arrangement playback before recording midi"
                    .into(),
            );
            return;
        }
        if !self.computer_keyboard.enabled && !self.controller_wants_live_notes() {
            self.error = Some(
                "enable the computer midi keyboard or configure a hardware input before recording"
                    .into(),
            );
            return;
        }
        let Some((track, clip)) = self.selected_track.zip(self.selected_clip) else {
            self.error = Some("select an arrangement midi clip before recording".into());
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let grid = match self.resolve_input_grid() {
            Ok(grid) => grid,
            Err(reason) => {
                self.error = Some(reason.into());
                return;
            }
        };
        let binding = match MidiTakeBinding::capture_with_grid(&state.project, track, clip, grid) {
            Ok(binding) => Arc::new(binding),
            Err(reason) => {
                self.error = Some(reason.into());
                return;
            }
        };
        if !self.audio_status().is_some_and(|s| {
            s.playing.load(Acquire)
                && (binding.first_frame()..binding.end_frame()).contains(&s.position.load(Acquire))
        }) {
            self.error =
                Some("start playback inside the selected midi clip before recording".into());
            return;
        }
        self.midi_recorder.generation = self.project_generation;
        self.midi_recorder.revision = state.revision;
        self.midi_recorder.binding = Some(binding);
        self.midi_recorder.output_identity = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity());
        self.midi_recorder.mode = Mode::Preparing;
        self.midi_audition.clear();
        self.reprepare_controller_for_recording();
        self.cancel_computer_keyboard();
        self.computer_keyboard.suspended = false;
        self.tick_computer_keyboard();
        self.status = "preparing midi recording · waiting for admitted input".into();
    }
    pub(super) fn discard_midi_take(&mut self) {
        if matches!(
            self.midi_recorder.mode,
            Mode::Capturing | Mode::Finishing | Mode::Committing
        ) {
            self.error = Some("finish the active recording or commit before discarding".into());
            return;
        }
        if let Some(audio) = &self.audio
            && audio.control.midi_target() == self.midi_recorder.target
            && self.midi_recorder.target.is_some()
        {
            audio.control.midi_cancel();
        }
        self.midi_recorder.clear();
        self.status = "midi take discarded".into();
    }
    pub(super) fn observe_midi_take_completion(
        &mut self,
        completion: Option<hex_session::service::MidiTakeCompletion>,
    ) {
        let Some(completion) =
            completion.filter(|c| Some(c.request_id) == self.midi_recorder.pending_commit)
        else {
            return;
        };
        if completion.accepted {
            self.midi_recorder.clear();
        } else {
            self.midi_recorder.mode = Mode::Ready;
            self.midi_recorder.pending_commit = None;
        }
    }
    fn commit_midi_take(&mut self) {
        if self.midi_recorder.mode != Mode::Ready {
            return;
        }
        let Some(request_id) = self.midi_recorder.commit_serial.checked_add(1) else {
            self.error = Some("midi commit identity exhausted".into());
            return;
        };
        let Some((binding, notes, stop_frame)) = self
            .midi_recorder
            .binding
            .as_ref()
            .zip(self.midi_recorder.notes.as_ref())
            .zip(self.midi_recorder.stop_frame)
            .map(|((binding, notes), end)| (Arc::clone(binding), Arc::clone(notes), end))
        else {
            return;
        };
        // Keep immutable notes and binding until this request's own terminal,
        // including queue refusal and delayed service preparation.
        let request = Request::CommitMidiTake {
            request_id,
            project_generation: self.midi_recorder.generation,
            revision: self.midi_recorder.revision,
            binding,
            notes,
            stop_frame,
        };
        self.midi_recorder.commit_serial = request_id;
        self.midi_recorder.pending_commit = Some(request_id);
        if self.enqueue(request) {
            self.midi_recorder.mode = Mode::Committing;
        } else {
            self.midi_recorder.pending_commit = None;
        }
    }
    fn arm_prepared_midi_take(&mut self) -> Result<bool, &'static str> {
        if !self.computer_keyboard.prepared || !self.controller_capture_ready() {
            return Ok(false);
        }
        let binding = self
            .midi_recorder
            .binding
            .as_ref()
            .ok_or("midi take binding is missing")?;
        let input = self
            .computer_keyboard
            .input
            .as_ref()
            .ok_or("midi recording input was cancelled")?;
        let hardware = self.controller_capture_activation(input)?;
        let audio = self.audio.as_mut().ok_or("midi recording output closed")?;
        let status = audio
            .control
            .status()
            .ok_or("midi recording output closed")?;
        let Some(source) = status.recording_source().filter(|s| {
            s.context.project_generation == self.midi_recorder.generation
                && s.context.editor_revision == self.midi_recorder.revision
                && Some(s.output_identity) == self.midi_recorder.output_identity
                && self
                    .midi_recorder
                    .prepared_audio_revision
                    .is_some_and(|revision| s.audio_revision >= revision)
        }) else {
            return Ok(false);
        };
        let Some(take_id) = self
            .midi_recorder
            .serial
            .checked_add(1)
            .filter(|id| *id <= u64::MAX >> 2)
        else {
            return Err("midi take identity exhausted");
        };
        let output_epoch = input
            .output_epoch_on_control()
            .ok_or("midi recording output activation is unavailable")?;
        let keys = input
            .begin_ordered_keys_on_control()
            .map_err(|_| "midi recording input could not be activated")?;
        let target = Target {
            capture: MidiCaptureBinding {
                take_id,
                track: binding.track(),
                device: binding.instrument(),
                sample_rate: binding.sample_rate(),
                output_epoch,
                keys,
                hardware,
                first_frame: binding.first_frame(),
                end_frame: binding.end_frame(),
            },
            project_generation: self.midi_recorder.generation,
            canonical_revision: self.midi_recorder.revision,
            audio_revision: source.audio_revision,
            output_identity: source.output_identity,
            transport_epoch: status.transport_epoch.load(Acquire),
        };
        if let Err(reason) = audio.control.midi_begin(target) {
            let _ = input.seal_ordered_keys_on_control(keys);
            return Err(reason);
        }
        self.midi_recorder.serial = take_id;
        self.midi_recorder.keys = Some(keys);
        self.midi_recorder.input = Some(Arc::clone(input));
        self.midi_recorder.target = Some(target);
        self.midi_recorder.mode = Mode::Capturing;
        self.status = "midi recording requested · waiting for output".into();
        Ok(true)
    }
    pub(super) fn tick_midi_recording(&mut self) -> bool {
        if !self.midi_recorder.blocks_edits() {
            // An explicit discard can precede a late terminal on a faulted
            // output. Drain that cancelled take so admission can be released.
            let mut changed = false;
            for _ in 0..1026 {
                if self
                    .audio
                    .as_mut()
                    .and_then(|audio| audio.control.midi_receipt())
                    .is_none()
                {
                    break;
                }
                changed = true;
            }
            return changed;
        }
        if matches!(
            self.midi_recorder.mode,
            Mode::Preparing | Mode::Capturing | Mode::Finishing
        ) && self.midi_recorder.output_identity.is_some()
            && self.audio.as_ref().is_none_or(|audio| {
                audio.control.output_identity() != self.midi_recorder.output_identity
                    || audio.control.status().is_none()
            })
        {
            self.midi_recorder.seal();
            if let Some(take) = &mut self.midi_recorder.take {
                take.interrupt();
            }
            self.midi_recorder.mode = Mode::Interrupted;
            self.error = Some("midi output changed · incomplete take retained".into());
            return true;
        }
        if self.midi_recorder.preparing() {
            if self.computer_keyboard.suspended {
                self.midi_recorder.mode = Mode::Interrupted;
                self.error =
                    Some("midi recording input preparation refused · discard to retry".into());
                return true;
            }
            match self.arm_prepared_midi_take() {
                Ok(changed) => {
                    if changed {
                        return true;
                    }
                }
                Err(reason) => {
                    self.midi_recorder.mode = Mode::Interrupted;
                    self.error = Some(reason.into());
                    return true;
                }
            }
        }
        let mut changed = false;
        // The callback's bounded ring is drained on control, without taking any
        // project/audio owner into GPUI. Terminal has independent queue capacity.
        for _ in 0..1026 {
            let receipt = self
                .audio
                .as_mut()
                .and_then(|audio| audio.control.midi_receipt());
            let Some(receipt) = receipt else {
                break;
            };
            if Some(receipt.target()) != self.midi_recorder.target {
                continue;
            }
            changed = true;
            if self.midi_recorder.mode == Mode::Interrupted {
                if matches!(receipt, Receipt::Finished { .. } | Receipt::Aborted { .. }) {
                    self.midi_recorder.seal();
                }
                continue;
            }
            match receipt {
                Receipt::Armed { target, clock } => {
                    match MidiTake::new(target.capture.first_frame, target.capture.end_frame, clock)
                    {
                        Ok(take) => {
                            self.midi_recorder.take = Some(take);
                            self.status = "midi recording · next-block input timing".into();
                        }
                        Err(reason) => {
                            self.error = Some(reason.into());
                            if let Some(audio) = &self.audio {
                                audio.control.midi_cancel();
                            }
                            self.midi_recorder.mode = Mode::Finishing;
                        }
                    }
                }
                Receipt::Note {
                    note,
                    content_frame,
                    ..
                } => {
                    if let Some(take) = &mut self.midi_recorder.take
                        && let Err(reason) = take.push(note, content_frame)
                    {
                        self.error = Some(reason.into());
                        if let Some(audio) = &self.audio {
                            audio.control.midi_cancel();
                        }
                        self.midi_recorder.mode = Mode::Finishing;
                    }
                }
                Receipt::Finished { clock, .. } => {
                    self.midi_recorder.seal();
                    if clock.is_none() && self.midi_recorder.take.is_none() {
                        self.midi_recorder.clear();
                        continue;
                    }
                    let result = clock
                        .zip(self.midi_recorder.take.as_mut())
                        .ok_or("midi take completion is missing delivered notes")
                        .and_then(|(clock, take)| {
                            take.finish(clock)?;
                            Ok(clock.content_frame)
                        });
                    match result {
                        Ok(end) => {
                            self.midi_recorder.stop_frame = Some(end);
                            self.midi_recorder.notes = self
                                .midi_recorder
                                .take
                                .as_ref()
                                .map(|take| Arc::from(take.notes()));
                            self.midi_recorder.mode = Mode::Ready;
                            self.commit_midi_take();
                        }
                        Err(reason) => {
                            self.midi_recorder.mode = Mode::Interrupted;
                            self.error = Some(reason.into());
                        }
                    }
                }
                Receipt::Aborted { reason, .. } => {
                    self.status = "midi recording interrupted · take retained".into();
                    self.midi_recorder.seal();
                    if let Some(take) = &mut self.midi_recorder.take {
                        take.interrupt();
                    }
                    self.midi_recorder.mode = Mode::Interrupted;
                    self.error = Some(
                        format!(
                            "midi recording interrupted · {} · take retained; discard explicitly",
                            super::user_errors::midi_abort(reason)
                        )
                        .into(),
                    );
                }
            }
        }
        changed
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::super::midi_canvas::creation_tests::{fixture, pump};
    use super::*;
    use std::time::Instant;

    #[gpui::test]
    fn midi_take_survives_delayed_commit_and_refusal_while_document_actions_are_blocked(
        cx: &mut gpui::TestAppContext,
    ) {
        for substitute_source in [false, true] {
            let (mut visual, editor, _environment, clip) = fixture(cx);
            editor.update(&mut visual, |view, _| {
                let track = view.selected_track.unwrap();
                assert!(view.enqueue(Request::Edit(Command::InsertInstrument {
                    track,
                    state: hex_project::default_instrument_state().unwrap()
                })));
            });
            pump(&editor, &mut visual);
            let retained = editor.update(&mut visual, |view, _| {
                let state = view.state.as_ref().unwrap();
                let project = if substitute_source {
                    hex_project::Project::from_snapshot(state.project.snapshot()).unwrap()
                } else {
                    (*state.project).clone()
                };
                let binding = Arc::new(
                    MidiTakeBinding::capture(&project, view.selected_track.unwrap(), clip).unwrap(),
                );
                let notes: Arc<[RecordedMidiNote]> = Arc::from([RecordedMidiNote {
                    start_frame: 4_800,
                    end_frame: 9_600,
                    pitch: 60,
                    channel: 2,
                    velocity: 93,
                }]);
                view.midi_recorder.binding = Some(binding);
                view.midi_recorder.notes = Some(Arc::clone(&notes));
                view.midi_recorder.stop_frame = Some(12_000);
                view.midi_recorder.generation = view.project_generation;
                view.midi_recorder.revision = state.revision;
                view.midi_recorder.mode = Mode::Ready;
                assert!(!view.enqueue(Request::Edit(Command::AddMidiTrack {
                    name: "blocked".into()
                })));
                view.choose_project(
                    std::path::PathBuf::from("/tmp/midi-take-guard.hex"),
                    hex_session::DocumentAudioMode::Resident,
                );
                assert!(view.pending_open_path.is_none());
                view.commit_midi_take();
                assert!(view.midi_recorder.mode == Mode::Committing);
                assert!(Arc::ptr_eq(
                    view.midi_recorder.notes.as_ref().unwrap(),
                    &notes
                ));
                notes
            });
            // Complete repaints while the worker response is deliberately not
            // installed. Notes stay owned by this exact take, independent of UI focus.
            visual.run_until_parked();
            editor.update(&mut visual, |view, cx| {
                assert!(view.midi_recorder.blocks_edits());
                assert!(Arc::ptr_eq(
                    view.midi_recorder.notes.as_ref().unwrap(),
                    &retained
                ));
                cx.notify();
            });
            visual.run_until_parked();
            let deadline = crate::test_wait::hang_deadline();
            let event = loop {
                if let Ok(event) = editor.update(&mut visual, |view, _| view.service.try_event()) {
                    break event;
                }
                assert!(Instant::now() < deadline, "take worker did not respond");
                std::thread::yield_now();
            };
            editor.update(&mut visual, |view, _| {
                assert_eq!(
                    event.midi_take_complete.unwrap().accepted,
                    !substitute_source
                );
                view.receive(event);
                if substitute_source {
                    assert!(view.midi_recorder.mode == Mode::Ready);
                    assert!(Arc::ptr_eq(
                        view.midi_recorder.notes.as_ref().unwrap(),
                        &retained
                    ));
                    assert!(
                        view.state
                            .as_ref()
                            .unwrap()
                            .project
                            .midi_clip(clip)
                            .unwrap()
                            .notes()
                            .is_empty()
                    );
                    view.discard_midi_take();
                } else {
                    assert!(!view.midi_recorder.blocks_edits());
                    assert_eq!(
                        view.state
                            .as_ref()
                            .unwrap()
                            .project
                            .midi_clip(clip)
                            .unwrap()
                            .notes()
                            .len(),
                        1
                    );
                }
                assert!(!view.midi_recorder.blocks_edits());
            });
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod lifecycle_tests {
    use super::super::midi_canvas::creation_tests::{fixture, pump};
    use super::*;
    use std::time::Instant;

    fn open_output() -> (
        hex_audio::WorkerPreparedOutput,
        hex_audio::worker_fixture::HeadlessBackend,
    ) {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            match hex_audio::worker_fixture::open(48_000) {
                Ok((mut output, backend)) => {
                    output
                        .publication
                        .control()
                        .unwrap()
                        .configure_device_resource_publication_on_control(
                            hex_engine::AudioStorageLedgerReservation {
                                data_rows: 512,
                                handle_rows: 256,
                                root_rows: 128,
                                max_retained_bytes: 20_000_000,
                                max_construction_peak_bytes: 20_000_000,
                            },
                        )
                        .unwrap();
                    return (output, backend);
                }
                Err(error)
                    if error == "audio output is open or still shutting down"
                        && Instant::now() < deadline =>
                {
                    std::thread::yield_now()
                }
                Err(error) => panic!("{error}"),
            }
        }
    }
    fn audio(client: hex_audio::prepared::Client) -> crate::EditorAudio {
        crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "headless".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        }
    }
    #[gpui::test]
    fn midi_recording_captures_input_quantization_before_worker_preparation(
        cx: &mut gpui::TestAppContext,
    ) {
        let _output_fixture_guard = crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
        let (mut visual, editor, _environment, clip) = fixture(cx);
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::InsertInstrument {
                track: view.selected_track.unwrap(),
                state: hex_project::default_instrument_state().unwrap(),
            })));
        });
        pump(&editor, &mut visual);
        let (project, generation, revision) = editor.read_with(&visual, |view, _| {
            (
                Arc::clone(&view.state.as_ref().unwrap().project),
                view.project_generation,
                view.state.as_ref().unwrap().revision,
            )
        });
        let prepared = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &Default::default(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let (mut output, mut backend) = open_output();
        let client = output.take_client().unwrap();
        let mut playback =
            hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap();
        playback.apply(hex_engine::playback::PlaybackCommand::Play);
        output
            .publication
            .control()
            .unwrap()
            .publish_with_admission_and_source_on_control(
                playback,
                hex_audio::prepared::PublicationPolicy::InheritRuntime,
                hex_engine::AudioSourceContext {
                    project_generation: generation,
                    editor_revision: revision,
                },
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        backend
            .render_direct(&mut [[0.0; 2]; 64], &mut [0.0; 128])
            .unwrap();
        editor.update(&mut visual, |view, _| {
            view.audio = Some(audio(client));
            view.computer_keyboard.enabled = true;
            view.set_input_quantization(hex_project::InputQuantization::Eighth);
            // This fixture authoring happened before it had an audio output.
            // Its offline candidate is redundant with the same canonical source
            // actually installed above; retire it on control, not as a fake ACK.
            if let Some(pending) = view.pending_playback.take() {
                assert_eq!(pending.project_revision, revision);
                assert_eq!(pending.project_generation, generation);
            }
            assert!(view.worker_owns_keyboard_publication());
            assert!(!view.edit_pending());
            view.toggle_midi_recording();
            assert!(
                view.midi_recorder.mode == Mode::Preparing,
                "{:?}",
                view.error
            );
            view.set_input_quantization(hex_project::InputQuantization::Quarter);
            assert_eq!(
                view.input_quantization,
                hex_project::InputQuantization::Eighth
            );
            let binding = Arc::clone(view.midi_recorder.binding.as_ref().unwrap());
            // Later display state cannot reinterpret the retained admission.
            view.input_quantization = hex_project::InputQuantization::Off;
            view.snap_grid = super::super::musical_time::SnapGrid::Off;
            let command = hex_session::midi_recording::finish_take(
                &project,
                &binding,
                &[RecordedMidiNote {
                    start_frame: 13_000,
                    end_frame: 14_000,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                }],
                14_000,
            )
            .unwrap()
            .unwrap();
            let Command::AddMidiNotes {
                clip: target,
                notes,
            } = command
            else {
                panic!("take")
            };
            assert_eq!(target, clip);
            assert_eq!(
                notes[0].start,
                hex_project::BeatTime::from_ratio(1, 2).unwrap()
            );
            view.finish_midi_recording(); // cancel preparation, without a completed take
            view.cancel_computer_keyboard();
            view.audio.as_mut().unwrap().control.close();
            view.audio = None;
        });
        backend.close();
    }

    fn exercise_midi_recording_terminal(cx: &mut gpui::TestAppContext, case: u8) {
        let _output_fixture_guard = crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
        let (mut visual, editor, _environment, clip) = fixture(cx);
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::InsertInstrument {
                track: view.selected_track.unwrap(),
                state: hex_project::default_instrument_state().unwrap()
            })));
        });
        pump(&editor, &mut visual);
        let (project, revision, generation, track) = editor.read_with(&visual, |view, _| {
            (
                Arc::clone(&view.state.as_ref().unwrap().project),
                view.state.as_ref().unwrap().revision,
                view.project_generation,
                view.selected_track.unwrap(),
            )
        });
        let context = hex_engine::AudioSourceContext {
            project_generation: generation,
            editor_revision: revision,
        };
        let prepared = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &Default::default(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let (mut output, mut backend) = open_output();
        let client = output.take_client().unwrap();
        let old_input = Arc::new(client.new_computer_keyboard_input_on_control().unwrap());
        let mut old =
            hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&prepared))
                .unwrap();
        old.prepare_computer_keyboard(&project, track, old_input)
            .unwrap();
        let old_revision = output
            .publication
            .control()
            .unwrap()
            .publish_with_admission_and_source_on_control(
                old,
                hex_audio::prepared::PublicationPolicy::InheritRuntime,
                context,
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        let mut frames = [[0.0; 2]; 64];
        let mut host = [0.0; 128];
        backend.render_direct(&mut frames, &mut host).unwrap();
        let input = Arc::new(
            client
                .new_computer_keyboard_input_on_control()
                .unwrap()
                .with_recording_support()
                .unwrap(),
        );
        let mut next =
            hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap();
        next.prepare_computer_keyboard(&project, track, Arc::clone(&input))
            .unwrap();
        let new_revision = output
            .publication
            .control()
            .unwrap()
            .publish_with_admission_and_source_on_control(
                next,
                hex_audio::prepared::PublicationPolicy::InheritRuntime,
                context,
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        editor.update(&mut visual, |view, _| {
            view.audio = Some(audio(client));
            view.computer_keyboard.enabled = true;
            view.computer_keyboard.prepared = true; // worker submission has completed
            view.computer_keyboard.request_id = 42;
            view.computer_keyboard.input = Some(Arc::clone(&input));
            view.midi_recorder.mode = Mode::Preparing;
            view.midi_recorder.binding = Some(Arc::new(
                MidiTakeBinding::capture(&project, track, clip).unwrap(),
            ));
            view.midi_recorder.generation = generation;
            view.midi_recorder.revision = revision;
            view.midi_recorder.output_identity =
                view.audio.as_ref().unwrap().control.output_identity();
            view.midi_recording_published(context, Some(42), new_revision);
            assert_eq!(
                view.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .recording_source()
                    .unwrap()
                    .audio_revision,
                old_revision
            );
            assert!(!view.arm_prepared_midi_take().unwrap());
            assert!(view.audio.as_ref().unwrap().control.midi_target().is_none());
        });
        backend.render_direct(&mut frames, &mut host).unwrap();
        editor.update(&mut visual, |view, _| {
            view.audio
                .as_mut()
                .unwrap()
                .control
                .command(hex_engine::PlaybackCommand::Play)
                .unwrap();
        });
        backend.render_direct(&mut frames, &mut host).unwrap();
        editor.update(&mut visual, |view, _| {
            assert!(view.arm_prepared_midi_take().unwrap());
        });
        input.set_key(0, Some((60, 100, 1))).unwrap();
        input.set_key(0, None).unwrap();
        backend.render_direct(&mut frames, &mut host).unwrap();
        let captured = editor.update(&mut visual, |view, _| {
            assert!(view.tick_midi_recording());
            let notes = view.midi_recorder.take.as_ref().unwrap().notes().to_vec();
            assert_eq!(notes.len(), 1);
            // Close and replace without any tick seeing an absent endpoint.
            if case == 0 {
                view.audio.as_mut().unwrap().control.close();
            }
            notes
        });
        if case != 0 {
            if case == 1 {
                input.cancel();
            } // disconnect before Stop, without a callback
            editor.update(&mut visual, |view, _| {
                view.stop();
                if case == 2 {
                    view.stop();
                }
            });
            backend.render_direct(&mut frames, &mut host).unwrap();
            editor.update(&mut visual, |view, _| {
                if case == 1 {
                    let before = view.state.as_ref().unwrap().project.snapshot();
                    let revision = view.state.as_ref().unwrap().revision;
                    assert!(view.tick_midi_recording());
                    assert!(view.midi_recorder.mode == Mode::Interrupted);
                    assert!(view.midi_recorder.blocks_edits());
                    assert!(view.midi_recorder.pending_commit.is_none());
                    assert!(view.error.as_ref().unwrap().contains("cancelled"));
                    assert_eq!(view.state.as_ref().unwrap().revision, revision);
                    assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
                } else {
                    let terminal = view.audio.as_mut().unwrap().control.midi_receipt().unwrap();
                    assert!(matches!(terminal, Receipt::Finished { clock: Some(_), .. }));
                }
                assert!(
                    view.audio
                        .as_mut()
                        .unwrap()
                        .control
                        .midi_receipt()
                        .is_none()
                );
                assert_eq!(view.midi_recorder.take.as_ref().unwrap().notes(), captured);
                assert!(
                    view.state
                        .as_ref()
                        .unwrap()
                        .project
                        .midi_clip(clip)
                        .unwrap()
                        .notes()
                        .is_empty()
                );
                if case == 1 {
                    view.discard_midi_take();
                    assert!(!view.midi_recorder.blocks_edits());
                } else {
                    view.midi_recorder.clear();
                }
                view.audio.as_mut().unwrap().control.close();
                view.audio = None;
            });
            backend.close();
            drop(output);
            return;
        }
        backend.close();
        drop(output);
        let (mut replacement, mut replacement_backend) = open_output();
        let replacement_client = replacement.take_client().unwrap();
        editor.update(&mut visual, |view, _| {
            view.audio = Some(audio(replacement_client));
            assert!(view.audio.as_ref().unwrap().status().is_some());
            assert!(view.tick_midi_recording());
            assert!(view.midi_recorder.mode == Mode::Interrupted);
            assert_eq!(view.midi_recorder.take.as_ref().unwrap().notes(), captured);
            view.discard_midi_take();
            assert!(!view.midi_recorder.blocks_edits());
            view.audio.as_mut().unwrap().control.close();
            view.audio = None;
        });
        replacement_backend.close();
    }

    #[gpui::test]
    fn midi_recording_retains_take_on_same_tick_output_replacement(cx: &mut gpui::TestAppContext) {
        exercise_midi_recording_terminal(cx, 0);
    }

    #[gpui::test]
    fn midi_recording_retains_take_when_input_disconnect_races_stop(cx: &mut gpui::TestAppContext) {
        exercise_midi_recording_terminal(cx, 1);
    }

    #[gpui::test]
    fn midi_recording_stop_twice_finishes_once(cx: &mut gpui::TestAppContext) {
        exercise_midi_recording_terminal(cx, 2);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[gpui::test]
fn midi_recording_worker_closure_distinguishes_unsubmitted_from_unknown_commit(
    cx: &mut gpui::TestAppContext,
) {
    use super::midi_canvas::creation_tests::{fixture, pump};
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::InsertInstrument {
            track: view.selected_track.unwrap(),
            state: hex_project::default_instrument_state().unwrap()
        })));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        let binding = Arc::new(
            MidiTakeBinding::capture(&state.project, view.selected_track.unwrap(), clip).unwrap(),
        );
        let notes: Arc<[RecordedMidiNote]> = Arc::from([RecordedMidiNote {
            start_frame: 1,
            end_frame: 10,
            pitch: 60,
            channel: 0,
            velocity: 100,
        }]);
        view.midi_recorder.notes = Some(Arc::clone(&notes));
        view.midi_recorder.binding = Some(Arc::clone(&binding));
        view.midi_recorder.mode = Mode::Committing;
        view.midi_recorder.pending_commit = Some(77);
        let request = Request::CommitMidiTake {
            request_id: 77,
            project_generation: view.project_generation,
            revision: state.revision,
            binding,
            notes: Arc::clone(&notes),
            stop_frame: 11,
        };
        view.midi_service_disconnected();
        assert!(view.midi_recorder.mode == Mode::Interrupted);
        assert!(view.status.contains("outcome unknown"));
        assert!(Arc::ptr_eq(
            view.midi_recorder.notes.as_ref().unwrap(),
            &notes
        ));
        // A retained Closed(request) subsequently proves this particular queued
        // attempt was never submitted. Only that evidence makes Retry safe.
        view.midi_take_not_submitted(&request);
        assert!(view.midi_recorder.mode == Mode::Ready);
        assert!(Arc::ptr_eq(
            view.midi_recorder.notes.as_ref().unwrap(),
            &notes
        ));
        view.midi_recorder.mode = Mode::Committing;
        view.midi_recorder.pending_commit = Some(78);
        view.midi_service_disconnected();
        view.midi_take_not_submitted(&request); // wrong attempt cannot authorize retry
        assert!(view.midi_recorder.mode == Mode::Interrupted);
        view.toggle_midi_recording();
        assert!(view.midi_recorder.mode == Mode::Interrupted);
        view.discard_midi_take();
        assert!(!view.midi_recorder.blocks_edits());
    });
}

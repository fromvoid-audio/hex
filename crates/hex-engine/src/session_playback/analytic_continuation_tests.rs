//! Original coefficient provider fixture: independent of Operator and mutable
//! voice admission. Tests exercise the normal prepared revision/handoff paths.
use super::*;
use hex_lattice::{
    AnalyticContinuation, AnalyticContinuationError as Error, AnalyticContinuationFormat,
    AnalyticContinuationId as Id, AnalyticParameterFrame, PreparedAnalyticContinuation,
    PreparedNativeInstrument, PreparedSharedAnalyticContext, PreparedSharedAnalyticNote,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use std::any::Any;

const TYPE: DeviceTypeId = DeviceTypeId::from_static("test.instrument.coefficient.continuation");
const RELEASE: u64 = 512;

struct Definition;

impl NativeDeviceMetadata for Definition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("coefficient fixture")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(TYPE, 1, vec![1, 0, 0])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != TYPE
            || state.state_version() != 1
            || state.payload().len() != 3
            || state.payload()[0] > 1
            || state.payload()[2] > 2
        {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
        _: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}

impl hex_lattice::NativeInstrumentDefinition for Definition {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        _context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let state = self.migrate_state(state)?;
        Ok(Box::new(Context {
            gain: f64::from(state.payload()[0]),
            law: state.payload()[1],
            refusal: state.payload()[2],
        }))
    }
}

#[derive(Clone)]
struct Context {
    gain: f64,
    law: u8,
    refusal: u8,
}

// Deliberately includes onset age and gate so resets cannot pass by rendering
// a constant. It is a fixture law, not an oscillator or reference-device claim.
fn signal(age: u64, gate: u64, gain: f64) -> [f32; 2] {
    if age >= gate.saturating_add(RELEASE) {
        return [0.0; 2];
    }
    let envelope = if age < gate {
        1.0
    } else {
        (gate + RELEASE - age) as f64 / RELEASE as f64
    };
    let value = (gain * envelope * (1.0 + (age % 17) as f64 / 100.0)) as f32;
    [value; 2]
}

impl PreparedNativeInstrument for Context {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_shared_analytic_context(
        &self,
    ) -> Result<Option<Box<dyn PreparedSharedAnalyticContext>>, DeviceContractError> {
        Ok(Some(Box::new(self.clone())))
    }
}

impl PreparedSharedAnalyticContext for Context {
    fn analytic_continuation(&self) -> Option<&dyn PreparedAnalyticContinuation> {
        Some(self)
    }
    fn declared_context_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedSharedAnalyticNote, DeviceContractError> {
        let mut payload = [0; 8];
        payload[0] = u64::from(pitch);
        payload[1] = u64::from(velocity);
        Ok(PreparedSharedAnalyticNote::new(payload, RELEASE))
    }
    fn sample(
        &self,
        _: &PreparedSharedAnalyticNote,
        age: u64,
        gate: u64,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> [f32; 2] {
        signal(age, gate, self.gain)
    }
}

impl Context {
    fn frame(target: Id, gain: f64) -> AnalyticParameterFrame {
        let mut words = [0; 8];
        words[0] = gain.to_bits();
        AnalyticParameterFrame::new(target, words)
    }
    fn gain(&self, target: Id, frame: &AnalyticParameterFrame) -> Result<f64, Error> {
        if frame.target() != target {
            return Err(Error::WrongTarget);
        }
        let gain = f64::from_bits(frame.words()[0]);
        if !gain.is_finite()
            || !(0.0..=1.0).contains(&gain)
            || frame.words()[1..].iter().any(|word| *word != 0)
        {
            return Err(Error::InvalidParameters);
        }
        Ok(gain)
    }
}

impl PreparedAnalyticContinuation for Context {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn format(&self) -> AnalyticContinuationFormat {
        AnalyticContinuationFormat::new(TYPE, 1).unwrap()
    }
    fn endpoint(&self, target: Id) -> AnalyticParameterFrame {
        Self::frame(target, self.gain)
    }
    fn prepare_start(
        &self,
        target: Id,
        source: &dyn PreparedAnalyticContinuation,
        source_target: Id,
        boundary: &AnalyticParameterFrame,
    ) -> Result<AnalyticParameterFrame, Error> {
        if self.refusal == 1 {
            return Err(Error::InvalidParameters);
        }
        let source = source
            .as_any()
            .downcast_ref::<Self>()
            .filter(|source| source.law == self.law)
            .ok_or(Error::IncompatibleSource)?;
        Ok(Self::frame(target, source.gain(source_target, boundary)?))
    }
    fn validate_note(&self, note: &PreparedSharedAnalyticNote) -> Result<(), Error> {
        if note.release_frames() != RELEASE
            || note.payload()[0] > 127
            || !(1..=127).contains(&note.payload()[1])
            || note.payload()[2..].iter().any(|word| *word != 0)
        {
            return Err(Error::InvalidNote);
        }
        Ok(())
    }
    fn parameters_at(
        &self,
        target: Id,
        transition: &AnalyticContinuation,
        frame: u64,
    ) -> Result<AnalyticParameterFrame, Error> {
        let start = self.gain(target, transition.start())?;
        let elapsed = transition.elapsed_at(frame)?;
        let gain = if elapsed == transition.duration() {
            self.gain
        } else {
            start + (self.gain - start) * f64::from(elapsed) / f64::from(transition.duration())
        };
        Ok(Self::frame(target, gain))
    }
    fn sample_with_parameters(
        &self,
        target: Id,
        parameters: &AnalyticParameterFrame,
        note: &PreparedSharedAnalyticNote,
        age: u64,
        gate: u64,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<[f32; 2], Error> {
        self.validate_note(note)?;
        if self.refusal == 2 && age.is_multiple_of(17) {
            return Err(Error::InvalidParameters);
        }
        Ok(signal(age, gate, self.gain(target, parameters)?))
    }
}

fn fixture(gain: u8, law: u8, refusal: u8, duration: u64) -> Project {
    let mut snapshot = project(1.0, 0, duration).snapshot();
    snapshot.tracks[0].instrument.as_mut().unwrap().state =
        NativeDeviceState::new(TYPE, 1, vec![gain, law, refusal]).unwrap();
    Project::from_legacy_snapshot(snapshot).unwrap()
}

fn edited(project: &Project, gain: u8, law: u8, refusal: u8) -> Project {
    let device = project.tracks()[0].instrument().unwrap().id();
    let mut editor = Editor::new(project.clone(), 4).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: NativeDeviceState::new(TYPE, 1, vec![gain, law, refusal]).unwrap(),
        })
        .unwrap();
    editor.project().clone()
}

fn prepared(project: &Project) -> Arc<PreparedRevision> {
    let definition = Definition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    revision_with_instrument_registry(project, Arc::new(hex_devices::SineTable::new()), &registry)
}

fn playing(project: &Project, session: bool) -> SessionPlayback {
    let mut runtime = SessionPlayback::new(prepared(project)).unwrap();
    if session {
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: runtime.revision.session().midi_clips()[0].id,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
    }
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime
}

fn handoff(runtime: &mut SessionPlayback, project: &Project) -> SessionPlayback {
    let mut candidate = SessionPlayback::new(prepared(project)).unwrap();
    candidate.try_inherit_state_from(runtime).unwrap();
    candidate
}

#[test]
fn compatible_held_and_released_notes_use_actual_boundary_and_partition_independent_clock() {
    for session in [false, true] {
        for releasing in [false, true] {
            // One beat at 120 bpm is 24000 frames; small exact gate for release.
            let duration = if releasing {
                (64_u64 << 32) / 24_000
            } else {
                8 << 32
            };
            let on = fixture(1, 0, 0, duration);
            let off = edited(&on, 0, 0, 0);
            let mut old = playing(&on, session);
            render_checkpoint_prefix(&mut old, 96);
            let mut disabled = handoff(&mut old, &off);
            assert_eq!(
                disabled.release_voice_count(),
                0,
                "compatible edits keep the ordinary active source path"
            );
            assert!(disabled.mutable_instruments.iter().all(Option::is_none));
            let actual = render_checkpoint_partitioned(&mut disabled, &[1, 17, 64], 120);
            let gate = if releasing { 64 } else { 192_000 };
            for (offset, frame) in actual.iter().enumerate() {
                assert_eq!(
                    *frame,
                    signal(96 + offset as u64, gate, 1.0 - offset as f64 / 240.0)
                );
            }
            // Undo-equivalent reversal occurs halfway through the applied ramp.
            let mut enabled = handoff(&mut disabled, &on);
            let actual = render_checkpoint_partitioned(&mut enabled, &[31, 1, 64], 240);
            for (offset, frame) in actual.iter().enumerate() {
                let gain = 0.5 + 0.5 * offset as f64 / 240.0;
                assert_eq!(*frame, signal(216 + offset as u64, gate, gain));
            }
            assert!(
                enabled.analytic_continuations.iter().all(Option::is_none),
                "completed ramps must retire at their committed endpoint",
            );
            let endpoint = render_checkpoint_partitioned(&mut enabled, &[1, 17], 127);
            for (offset, frame) in endpoint.iter().enumerate() {
                assert_eq!(*frame, signal(456 + offset as u64, gate, 1.0));
            }
        }
    }
}

#[test]
fn detached_scene_tail_snapshots_ramp_and_keeps_context_lease_across_later_enable_edit() {
    for session in [false, true] {
        let on = fixture(1, 0, 0, 8 << 32);
        let off = edited(&on, 0, 0, 0);
        let mut old = playing(&on, session);
        render_checkpoint_prefix(&mut old, 96);
        let mut disabled = handoff(&mut old, &off);
        render_checkpoint_prefix(&mut disabled, 80);
        let track = disabled.routes[0].id;
        disabled
            .apply(SessionPlaybackCommand::StopTrack {
                track,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        // The pending cut is committed by this sample, age/gate are unchanged.
        render_checkpoint_prefix(&mut disabled, 1);
        assert_eq!(disabled.release_voice_count(), 1);
        let detached_id = disabled.releases[0].voice.sampler().context_id();
        assert!(!disabled.releases[0].voice.sampler().is_current());
        let mut enabled = handoff(&mut disabled, &on);
        assert_eq!(
            enabled.releases[0].voice.sampler().context_id(),
            detached_id
        );
        assert!(enabled.retained_contexts.iter().any(Option::is_some));
        let actual = render_checkpoint_partitioned(&mut enabled, &[17, 64], 520);
        for (offset, frame) in actual.iter().enumerate() {
            let absolute = 177 + offset as u64;
            let gain = (1.0 - (absolute - 96) as f64 / 240.0).max(0.0);
            assert_eq!(*frame, signal(absolute, 176, gain));
        }
        assert_eq!(enabled.release_voice_count(), 0);
    }
}

#[test]
fn incompatible_law_keeps_old_held_sampler_and_refused_handoff_keeps_both_owners() {
    for session in [false, true] {
        let on = fixture(1, 0, 0, 8 << 32);
        let mut old = playing(&on, session);
        render_checkpoint_prefix(&mut old, 96);
        let bad = edited(&on, 0, 0, 1);
        let mut candidate = SessionPlayback::new(prepared(&bad)).unwrap();
        assert_eq!(
            candidate.try_inherit_state_from(&mut old),
            Err(SessionPlaybackError::SharedAnalyticContext)
        );
        assert_eq!(old.performance_frame(), 96);
        assert_eq!(candidate.performance_frame(), 0);
        assert!(candidate.analytic_continuations.iter().all(Option::is_none));
        assert!(candidate.retained_contexts.iter().all(Option::is_none));
        let incompatible = edited(&on, 0, 1, 0);
        let mut changed = handoff(&mut old, &incompatible);
        assert!(changed.analytic_continuations.iter().all(Option::is_none));
        let actual = render_checkpoint_prefix(&mut changed, 16);
        for (offset, frame) in actual.iter().enumerate() {
            assert_eq!(*frame, signal(96 + offset as u64, 192_000, 1.0));
        }
    }
}

#[test]
fn provider_render_refusal_rolls_back_absolute_clock_and_seek_resets_transition() {
    for session in [false, true] {
        let on = fixture(1, 0, 0, 8 << 32);
        let off = edited(&on, 0, 0, 2);
        let mut old = playing(&on, session);
        render_checkpoint_prefix(&mut old, 96);
        let mut disabled = handoff(&mut old, &off);
        let routes = disabled.routes.clone();
        let before = disabled.analytic_continuations.clone();
        let mut output = [[9.0; 2]; 512];
        if session {
            assert!(disabled.render(&mut output).is_err());
        } else {
            let plan = disabled
                .output_capture_plan(&[OutputCaptureId::Track(disabled.routes[0].id)], 512)
                .unwrap();
            let mut captured = [[9.0; 2]; 512];
            let mut buffers: [&mut [Frame]; 1] = [&mut captured];
            let mut capture = OutputCapture::new(&plan, &mut buffers);
            assert!(
                disabled
                    .render_with_capture(&mut output, &mut capture)
                    .is_err()
            );
            assert_eq!(capture.valid_frames(), 0);
            assert_eq!(captured, [[0.0; 2]; 512]);
        }
        assert_eq!(output, [[0.0; 2]; 512]);
        assert_eq!(disabled.performance_frame(), 96);
        assert_eq!(disabled.routes, routes);
        assert_eq!(disabled.analytic_continuations, before);
        disabled.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        assert!(disabled.analytic_continuations.iter().all(Option::is_none));
        disabled.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 512]);
    }
}

#[test]
fn continuation_storage_preserves_sampler_footprint_and_preroll_cap() {
    let mut runtime = playing(&fixture(1, 0, 0, 8 << 32), false);
    assert_eq!(
        std::mem::size_of::<crate::midi_note_bank::PreparedMidiNote>(),
        352
    );
    assert_eq!(runtime.analytic_continuations.len(), MAX_TRACKS);
    assert_eq!(runtime.analytic_continuations.capacity(), MAX_TRACKS);
    assert_eq!(runtime.analytic_continuation_scratch.len(), MAX_TRACKS);
    assert_eq!(runtime.analytic_continuation_scratch.capacity(), MAX_TRACKS);
    let actual = runtime.accounted_runtime_storage_bytes().unwrap();
    let seeds = std::mem::take(&mut runtime.analytic_import.seeds);
    let proofs = std::mem::take(&mut runtime.analytic_import.proofs);
    let without_import = runtime.accounted_runtime_storage_bytes().unwrap();
    assert_eq!(
        actual - without_import,
        seeds.capacity() * std::mem::size_of::<hex_lattice::NativeInstrumentAnalyticSeed>()
            + proofs.capacity()
                * std::mem::size_of::<Option<crate::mutable_instrument::MutableOccurrence>>()
    );
    runtime.analytic_import.seeds = seeds;
    runtime.analytic_import.proofs = proofs;
    let admitted =
        super::super::preroll::arrangement_runtime_storage_preflight_bytes(&runtime.revision)
            .unwrap();
    eprintln!(
        "enable storage: sampler={} continuation_slot={} current_capacity={} scratch_capacity={} table_payload_bytes={} actual={} admitted={} cap={}",
        std::mem::size_of::<crate::midi_note_bank::PreparedMidiNote>(),
        std::mem::size_of::<Option<hex_lattice::AnalyticContinuation>>(),
        runtime.analytic_continuations.capacity(),
        runtime.analytic_continuation_scratch.capacity(),
        (runtime.analytic_continuations.capacity()
            + runtime.analytic_continuation_scratch.capacity())
            * std::mem::size_of::<Option<hex_lattice::AnalyticContinuation>>(),
        actual,
        admitted,
        super::super::preroll::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
    );
    assert!(actual <= admitted);
    assert!(admitted <= super::super::preroll::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
}

#[test]
fn unrelated_recreated_context_keeps_original_clock_and_skipped_desired_target_is_not_applied() {
    let on = fixture(1, 0, 0, 8 << 32);
    let off = edited(&on, 0, 0, 0);
    let mut old = playing(&on, false);
    render_checkpoint_prefix(&mut old, 96);
    // This control-prepared desired target is never published to audio.
    let _skipped = prepared(&off);
    let mut same = handoff(&mut old, &on);
    assert_eq!(
        render_checkpoint_prefix(&mut same, 1),
        [signal(96, 192_000, 1.0)]
    );
    let mut disabled = handoff(&mut same, &off);
    render_checkpoint_prefix(&mut disabled, 80);
    let before = disabled
        .analytic_continuations
        .iter()
        .flatten()
        .next()
        .copied()
        .unwrap();
    let mut unrelated = handoff(&mut disabled, &off);
    let after = unrelated
        .analytic_continuations
        .iter()
        .flatten()
        .next()
        .copied()
        .unwrap();
    assert_eq!(after.start_frame(), before.start_frame());
    assert_eq!(after.duration(), before.duration());
    let actual = render_checkpoint_prefix(&mut unrelated, 161);
    for (offset, frame) in actual.iter().enumerate() {
        let elapsed = 80 + offset;
        assert_eq!(
            *frame,
            signal(
                177 + offset as u64,
                192_000,
                (1.0 - elapsed as f64 / 240.0).max(0.0)
            )
        );
    }
}

#[test]
fn new_notes_and_exact_publication_note_off_share_the_current_owner_boundary() {
    for session in [false, true] {
        let mut on = fixture(1, 0, 0, (96_u64 << 32) / 24_000).snapshot();
        let start = (96_u64 << 32) / 24_000;
        let duration = 8 << 32;
        let next = on.next_id;
        if let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut on.tracks[0].content {
            midi_clips[0].notes.push(MidiNoteSnapshot {
                active: true,
                id: next,
                start_raw: start,
                duration_raw: duration,
                pitch: 67,
                channel: 0,
                velocity: 100,
            });
        }
        let SessionCellContentSnapshot::Midi(clip) = &mut on.session.cells[0].content else {
            panic!("fixture midi")
        };
        clip.source.notes.push(MidiNoteSnapshot {
            active: true,
            id: next + 1,
            start_raw: start,
            duration_raw: duration,
            pitch: 67,
            channel: 0,
            velocity: 100,
        });
        on.next_id += 2;
        let on = Project::from_legacy_snapshot(on).unwrap();
        let off = edited(&on, 0, 0, 0);
        let mut old = playing(&on, session);
        render_checkpoint_prefix(&mut old, 96);
        let mut disabled = handoff(&mut old, &off);
        let actual = render_checkpoint_prefix(&mut disabled, 241);
        for (offset, frame) in actual.iter().enumerate() {
            let gain = (1.0 - offset as f64 / 240.0).max(0.0);
            let first = signal(96 + offset as u64, 96, gain);
            let second = signal(offset as u64, 192_000, gain);
            assert_eq!(*frame, [first[0] + second[0], first[1] + second[1]]);
        }
    }
}

#[test]
fn deleted_owner_mid_ramp_retains_target_context_until_original_release_expires() {
    for session in [false, true] {
        let on = fixture(1, 0, 0, 8 << 32);
        let off = edited(&on, 0, 0, 0);
        let mut old = playing(&on, session);
        render_checkpoint_prefix(&mut old, 96);
        let mut disabled = handoff(&mut old, &off);
        render_checkpoint_prefix(&mut disabled, 80);
        let mut removed = off.snapshot();
        removed.tracks.clear();
        removed.session.cells.clear();
        let mut deleted = handoff(
            &mut disabled,
            &Project::from_legacy_snapshot(removed).unwrap(),
        );
        assert_eq!(deleted.release_voice_count(), 1);
        assert!(!deleted.releases[0].voice.sampler().is_current());
        assert!(deleted.retained_contexts.iter().any(Option::is_some));
        let actual = render_checkpoint_prefix(&mut deleted, 520);
        for (offset, frame) in actual.iter().enumerate() {
            let absolute = 176 + offset as u64;
            let gain = (1.0 - (absolute - 96) as f64 / 240.0).max(0.0);
            assert_eq!(*frame, signal(absolute, 176, gain));
        }
        assert_eq!(deleted.release_voice_count(), 0);
    }
}

#[test]
fn analytic_polyphony_above_mutable_limit_and_capture_single_sample_order_are_preserved() {
    let mut on = fixture(1, 0, 0, 8 << 32).snapshot();
    if let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut on.tracks[0].content {
        for offset in 0..24 {
            midi_clips[0].notes.push(MidiNoteSnapshot {
                active: true,
                id: on.next_id + offset,
                start_raw: 0,
                duration_raw: 8 << 32,
                pitch: 61 + offset as u8,
                channel: 0,
                velocity: 100,
            });
        }
    }
    on.next_id += 24;
    let on = Project::from_legacy_snapshot(on).unwrap();
    let off = edited(&on, 0, 0, 0);
    let mut old = playing(&on, false);
    render_checkpoint_prefix(&mut old, 96);
    let mut disabled = handoff(&mut old, &off);
    assert!(disabled.mutable_instruments.iter().all(Option::is_none));
    let track = disabled.routes[0].id;
    let plan = disabled
        .output_capture_plan(&[OutputCaptureId::Track(track)], 241)
        .unwrap();
    let mut captured = [[0.0; 2]; 241];
    let mut buffers: [&mut [Frame]; 1] = [&mut captured];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    let mut actual = [[0.0; 2]; 241];
    disabled
        .render_with_capture(&mut actual, &mut capture)
        .unwrap();
    assert_eq!(capture.valid_frames(), 241);
    assert_eq!(actual, captured);
    for (offset, frame) in actual.iter().enumerate() {
        let contribution = signal(
            96 + offset as u64,
            192_000,
            (1.0 - offset as f64 / 240.0).max(0.0),
        );
        let mut expected = [0.0; 2];
        for _ in 0..25 {
            expected[0] += contribution[0];
            expected[1] += contribution[1];
        }
        assert_eq!(*frame, expected);
    }
}

#[test]
fn reconstructed_matching_session_sources_keep_current_ownership_through_historical_bridge() {
    let on = fixture(1, 0, 0, 8 << 32);
    let off = fixture(0, 0, 0, 8 << 32);
    let mut old = playing(&on, true);
    render_checkpoint_prefix(&mut old, 96);
    let mut disabled = handoff(&mut old, &off);
    assert_eq!(
        disabled.release_voice_count(),
        1,
        "fresh source identity needs the established historical bridge"
    );
    assert!(disabled.releases[0].voice.sampler().is_current());
    let actual = render_checkpoint_prefix(&mut disabled, 80);
    for (offset, frame) in actual.iter().enumerate() {
        assert_eq!(
            *frame,
            signal(96 + offset as u64, 192_000, 1.0 - offset as f64 / 240.0)
        );
    }
    let mut enabled = handoff(&mut disabled, &on);
    let actual = render_checkpoint_prefix(&mut enabled, 241);
    let boundary = 1.0 - 80.0 / 240.0;
    for (offset, frame) in actual.iter().enumerate() {
        let gain = if offset >= 240 {
            1.0
        } else {
            boundary + (1.0 - boundary) * offset as f64 / 240.0
        };
        assert_eq!(*frame, signal(176 + offset as u64, 192_000, gain));
    }
}

#[test]
fn stopped_enable_edits_start_at_target_and_paused_notes_release_without_retrigger() {
    for session in [false, true] {
        for paused in [false, true] {
            let on = fixture(1, 0, 0, 8 << 32);
            let off = edited(&on, 0, 0, 0);
            let mut old = playing(&on, session);
            render_checkpoint_prefix(&mut old, 96);
            old.apply(if paused {
                SessionPlaybackCommand::Pause
            } else {
                SessionPlaybackCommand::Stop
            })
            .unwrap();
            let mut disabled = handoff(&mut old, &off);
            // Pause releases continue at the captured source age while the
            // canonical transport and parameter ramp clock stay frozen.
            let monitored = render_checkpoint_prefix(&mut disabled, 16);
            for (offset, actual) in monitored.iter().enumerate() {
                let expected = if paused {
                    signal(96 + offset as u64, 96, 1.0)
                } else {
                    [0.0; 2]
                };
                assert_eq!(
                    *actual, expected,
                    "monitor session={session}, paused={paused}, offset={offset}"
                );
            }
            assert_eq!(disabled.performance_frame(), if paused { 96 } else { 0 });
            disabled.apply(SessionPlaybackCommand::Play).unwrap();
            let actual = render_checkpoint_prefix(&mut disabled, 241);
            for (offset, frame) in actual.iter().enumerate() {
                let expected = if paused {
                    signal(
                        112 + offset as u64,
                        96,
                        (1.0 - offset as f64 / 240.0).max(0.0),
                    )
                } else {
                    [0.0; 2]
                };
                assert_eq!(
                    *frame, expected,
                    "session={session}, paused={paused}, offset={offset}"
                );
            }
        }
    }
}

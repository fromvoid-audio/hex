//! Original delayed processors enter through the normal registry/compiler seam.
use super::*;
use crate::routed_midi_fixture;
use crate::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, SineTable,
};
use hex_lattice::{
    NativeDeviceDefinition, NativeDeviceRegistry, NativeProcessorRuntime, OutputTail,
    PreparedNativeProcessor, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{Command, Created, Editor};
use std::any::Any;

#[path = "tests/limiter_latency.rs"]
mod limiter_latency;

#[path = "tests/eq_automation.rs"]
mod eq_automation;
#[path = "tests/shared_legacy.rs"]
mod shared_legacy;
#[path = "tests/shared_sources.rs"]
mod shared_sources;

const TYPE: DeviceTypeId = DeviceTypeId::from_static("test.compensation.delay");
pub(crate) struct Definition;
impl NativeDeviceMetadata for Definition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        state(0)
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != TYPE || state.state_version() != 1 || state.payload().len() != 4 {
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
pub(crate) fn state(delay: u32) -> Result<NativeDeviceState, DeviceContractError> {
    NativeDeviceState::new(TYPE, 1, delay.to_le_bytes().to_vec())
}
impl NativeDeviceDefinition for Definition {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        self.migrate_state(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        _context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        self.migrate_state(state)?;
        let delay = u32::from_le_bytes(
            state
                .payload()
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        );
        Ok(Some(Box::new(Prepared {
            delay: (delay != u32::MAX).then_some(delay),
        })))
    }
}
struct Prepared {
    delay: Option<u32>,
}
impl PreparedNativeProcessor for Prepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        self.delay
    }
    fn declared_runtime_bytes(&self) -> usize {
        size_of::<Runtime>() + self.delay.unwrap_or(0) as usize * size_of::<Frame>()
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(Runtime {
            ring: vec![[0.0; 2]; self.delay.unwrap_or(0) as usize],
            cursor: 0,
        })
    }
}
struct Runtime {
    ring: Vec<Frame>,
    cursor: usize,
}
impl NativeProcessorRuntime for Runtime {
    fn process_block(
        &mut self,
        frames: &mut [Frame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            if !self.ring.is_empty() {
                std::mem::swap(frame, &mut self.ring[self.cursor]);
                self.cursor = (self.cursor + 1) % self.ring.len();
            }
        }
    }
    fn reset(&mut self) {
        self.ring.fill([0.0; 2]);
        self.cursor = 0;
    }
    fn is_quiescent(&self) -> bool {
        self.ring.iter().all(|frame| *frame == [0.0; 2])
    }
    fn can_copy_history_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        old.as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| old.ring.len() == self.ring.len())
    }
    fn copy_history_from(&mut self, old: &dyn NativeProcessorRuntime) {
        if let Some(old) = old.as_any().downcast_ref::<Self>() {
            self.ring.copy_from_slice(&old.ring);
            self.cursor = old.cursor;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
pub(crate) struct Fixture {
    pub(crate) editor: Editor,
    pub(crate) tracks: Vec<TrackId>,
    pub(crate) media: BTreeMap<hex_project::AssetId, Audio>,
    pub(crate) samples: Vec<Frame>,
}
pub(crate) fn fixture(delays: &[&[u32]], frames: usize) -> Fixture {
    fixture_at_rate(delays, frames, 48000)
}
fn fixture_at_rate(delays: &[&[u32]], frames: usize, rate: u32) -> Fixture {
    let mut editor = Editor::new(hex_project::Project::new(rate).unwrap(), 32).unwrap();
    let mut samples = vec![[0.0; 2]; frames];
    samples[0] = [0.25, -0.5];
    samples[frames - 1] = [-0.125, 0.375];
    let mut media = BTreeMap::new();
    let mut tracks = Vec::new();
    for chain in delays {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "source".into(),
                path: "memory.wav".into(),
                source_sample_rate: rate,
                source_frames: frames as u64,
                playback_frames: frames as u64,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import")
        };
        tracks.push(track);
        media.insert(asset, Audio::new(rate, samples.clone()).unwrap());
        for &delay in *chain {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: state(delay).unwrap(),
                })
                .unwrap();
        }
    }
    Fixture {
        editor,
        tracks,
        media,
        samples,
    }
}
pub(crate) fn prepare(f: &Fixture) -> Result<Arc<PreparedRevision>, Invalid> {
    let definition = Definition;
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let effects = NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    PreparedRevision::prepare_with_registries(
        f.editor.project(),
        &f.media,
        Arc::new(SineTable::new()),
        None,
        &effects,
        &instruments,
    )
    .map(Arc::new)
}
fn session(revision: Arc<PreparedRevision>, frames: usize, partition: usize) -> Vec<Frame> {
    let mut runtime = SessionPlayback::new(revision).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = vec![[0.0; 2]; frames];
    for block in output.chunks_mut(partition) {
        runtime.render(block).unwrap();
    }
    output
}
#[test]
fn actual_dry_and_seven_frame_paths_align_capture_and_both_renderers_across_partitions() {
    let f = fixture(&[&[], &[7]], 300);
    let revision = prepare(&f).unwrap();
    assert_eq!(revision.arrangement().main_latency_frames(), 7);
    let mut expected = vec![[0.0; 2]; 307];
    for (to, source) in expected[7..].iter_mut().zip(&f.samples) {
        *to = [source[0] * 2.0, source[1] * 2.0];
    }
    for partition in [1, 63, 256, 513] {
        assert_eq!(session(Arc::clone(&revision), 307, partition), expected);
    }
    let mut renderer = Renderer::new(revision.arrangement());
    renderer.play();
    let mut legacy = expected.clone();
    renderer.render(&mut legacy).unwrap();
    assert_eq!(legacy, expected);
    let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let ids: Vec<_> = f
        .tracks
        .iter()
        .copied()
        .map(crate::OutputCaptureId::Track)
        .collect();
    let plan = runtime.output_capture_plan(&ids, 307).unwrap();
    let mut main = expected.clone();
    let mut a = main.clone();
    let mut b = main.clone();
    let mut views = [&mut a[..], &mut b[..]];
    let mut capture = crate::OutputCapture::new(&plan, &mut views);
    runtime
        .render_with_capture(&mut main, &mut capture)
        .unwrap();
    assert_eq!(main, expected);
    for index in 0..307 {
        let source = if index < 7 {
            [0.0; 2]
        } else {
            f.samples[index - 7]
        };
        assert_eq!(capture.output(0).unwrap()[index], source);
        assert_eq!(capture.output(1).unwrap()[index], source);
    }
}

#[test]
fn compensated_continuous_audio_matches_single_frame_rendering_across_loop_wraps() {
    let mut f = fixture(&[&[], &[7], &[3]], 173);
    for (track, audio) in f.media.values_mut().enumerate() {
        let samples = (0..173)
            .map(|frame| {
                let value = ((frame * (track + 1)) % 29) as f32 / 64.0;
                [value, -value * 0.5]
            })
            .collect();
        *audio = Audio::new(48000, samples).unwrap();
    }
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 173,
            enabled: true,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let expected = session(Arc::clone(&revision), 1200, 1);
    assert!(expected.iter().any(|frame| frame[0] != 0.0));
    for partition in [63, 256, 513] {
        assert_eq!(session(Arc::clone(&revision), 1200, partition), expected);
    }
}
#[test]
fn serial_three_plus_five_matches_eight_and_return_feed_cumulative_delay() {
    let mut f = fixture(&[&[3, 5], &[8]], 32);
    let revision = prepare(&f).unwrap();
    let pcm = session(revision, 40, 63);
    assert_eq!(pcm[8], [0.5, -1.0]);
    assert_eq!(pcm[39], [-0.25, 0.75]);
    let mut returns = Vec::new();
    for delay in [3, 5] {
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        f.editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: state(delay).unwrap(),
            })
            .unwrap();
        returns.push(bus);
    }
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: returns[0],
            gain: 1.0,
        })
        .unwrap();
    f.editor
        .execute(Command::SetReturnFeed {
            source: returns[0],
            destination: returns[1],
            gain: 1.0,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    assert_eq!(revision.arrangement().main_latency_frames(), 16);
    let pcm = session(revision, 48, 1);
    assert!(pcm[..16].iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(pcm[16], [1.0, -2.0]);
    assert_eq!(pcm[47], [-0.5, 1.5]);
}
#[test]
fn gain_uses_delayed_content_frame_and_loop_history_survives_wrap() {
    let mut f = fixture(&[&[7]], 8);
    let beat = f.editor.project().tempo_map().frame_to_beat(3).unwrap();
    f.editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track: f.tracks[0],
            points: vec![
                hex_project::GainPoint {
                    incoming_gain: None,
                    beat: hex_project::BeatTime::ZERO,
                    gain: 0.5,
                    outgoing: hex_project::GainInterpolation::Step,
                },
                hex_project::GainPoint {
                    incoming_gain: None,
                    beat,
                    gain: 1.0,
                    outgoing: hex_project::GainInterpolation::Step,
                },
            ],
        })
        .unwrap();
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 8,
            enabled: true,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let whole = session(Arc::clone(&revision), 40, 40);
    assert_eq!(whole, session(revision, 40, 1));
    for (index, frame) in whole.iter().enumerate() {
        let expected = if index < 7 {
            [0.0; 2]
        } else {
            let origin = (index - 7) % 8;
            let gain = if origin < 3 { 0.5 } else { 1.0 };
            [f.samples[origin][0] * gain, f.samples[origin][1] * gain]
        };
        assert_eq!(*frame, expected, "frame {index}");
    }
}
#[test]
fn compatible_gain_handoff_preserves_inflight_impulse_and_changed_latency_refuses() {
    let mut f = fixture(&[&[], &[7]], 32);
    let revision = prepare(&f).unwrap();
    let mut old = SessionPlayback::new(revision).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 3]).unwrap();
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.5,
        })
        .unwrap();
    let mut candidate = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    let mut output = [[0.0; 2]; 8];
    candidate.render(&mut output).unwrap();
    assert_eq!(output[4], [0.5, -1.0]);
    let device = f.editor.project().track(f.tracks[1]).unwrap().devices()[0].id();
    f.editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state(8).unwrap(),
        })
        .unwrap();
    let mut changed = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    assert!(changed.try_inherit_state_from(&mut candidate).is_err());
    candidate.apply(SessionPlaybackCommand::Stop).unwrap();
    changed.try_inherit_state_from(&mut candidate).unwrap();
}
#[test]
fn unknown_latency_and_cumulative_overflow_refuse_actual_preparation() {
    assert!(prepare(&fixture(&[&[u32::MAX]], 8)).is_err());
    assert!(prepare(&fixture(&[&[40000, 40000]], 8)).is_err());
}

thread_local! { static CONTEXT_PREPARATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
thread_local! { static INSTRUMENT_PREPARATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
struct InstrumentDefinition;
const INSTRUMENT: DeviceTypeId = DeviceTypeId::from_static("test.compensation.generator");
impl NativeDeviceMetadata for InstrumentDefinition {
    fn type_id(&self) -> DeviceTypeId {
        INSTRUMENT
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(INSTRUMENT, 1, vec![])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != INSTRUMENT
            || state.state_version() != 1
            || !state.payload().is_empty()
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
struct Instrument;
impl hex_lattice::NativeInstrumentDefinition for InstrumentDefinition {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        _context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn hex_lattice::PreparedNativeInstrument>, DeviceContractError> {
        self.migrate_state(state)?;
        INSTRUMENT_PREPARATIONS.with(|count| count.set(count.get() + 1));
        Ok(Box::new(Instrument))
    }
}
impl hex_lattice::PreparedNativeInstrument for Instrument {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(5)
    }
    fn prepare_shared_analytic_context(
        &self,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedSharedAnalyticContext>>, DeviceContractError>
    {
        CONTEXT_PREPARATIONS.with(|count| count.set(count.get() + 1));
        Ok(Some(Box::new(Instrument)))
    }
    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedAnalyticNote, DeviceContractError> {
        Ok(hex_lattice::PreparedAnalyticNote::new(
            [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
            5,
            |_, age, _, _, _| if age == 5 { [0.25, -0.5] } else { [0.0; 2] },
        ))
    }
}
impl hex_lattice::PreparedSharedAnalyticContext for Instrument {
    fn declared_context_bytes(&self) -> usize {
        1
    }
    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedSharedAnalyticNote, DeviceContractError> {
        Ok(hex_lattice::PreparedSharedAnalyticNote::new([0; 8], 5))
    }
    fn sample(
        &self,
        _: &hex_lattice::PreparedSharedAnalyticNote,
        age: u64,
        _: u64,
        _: &dyn hex_lattice::DeviceResources,
    ) -> Frame {
        if age == 5 { [0.25, -0.5] } else { [0.0; 2] }
    }
}
#[test]
fn independent_generator_five_and_audio_prefix_three_align_before_sum() {
    let mut f = fixture(&[&[3]], 32);
    let mut snapshot = f.editor.project().snapshot();
    let next = snapshot.next_id;
    snapshot.next_id += 3;
    let owner = &mut snapshot.tracks[0];
    owner.instrument = Some(hex_project::DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: next,
        bypassed: false,
        state: InstrumentDefinition.default_state().unwrap(),
    });
    owner.instrument_position = Some(1);
    let audio_clips = match &owner.content {
        hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips.clone(),
        _ => panic!("hybrid"),
    };
    owner.content = hex_project::TrackContentSnapshot::Hybrid {
        audio_clips,
        midi_clips: vec![hex_project::MidiClipSnapshot {
            content_view: None,
            active: true,
            playback_offset_raw: 0,
            one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
            volume: hex_project::MidiClipVolume::default(),
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            id: next + 1,
            start_raw: 0,
            length_raw: hex_project::BeatTime::ONE.raw(),
            source_length_raw: hex_project::BeatTime::ONE.raw(),
            source_start_raw: 0,
            loop_start_raw: 0,
            loop_end_raw: hex_project::BeatTime::ONE.raw(),
            looping: false,
            notes: vec![hex_project::MidiNoteSnapshot {
                active: true,
                id: next + 2,
                start_raw: 0,
                duration_raw: hex_project::BeatTime::ONE.raw(),
                pitch: 69,
                velocity: 127,
                channel: 0,
            }],
        }],
    };
    routed_midi_fixture::separate_midi(&mut snapshot, 0);
    f.editor = Editor::new(
        hex_project::Project::from_legacy_snapshot(snapshot).unwrap(),
        8,
    )
    .unwrap();
    let effects: [&dyn NativeDeviceDefinition; 1] = [&Definition];
    let effects = NativeDeviceRegistry::new(&effects).unwrap();
    let instruments: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&InstrumentDefinition];
    let instruments = hex_lattice::NativeInstrumentRegistry::new(&instruments).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_registries(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            &effects,
            &instruments,
        )
        .unwrap(),
    );
    assert_eq!(revision.arrangement().main_latency_frames(), 5);
    // The analytic generator emits only one impulse at its declared five-frame
    // latency; the 3-frame audio path is aligned to that same output frame.
    let mut expected = vec![[0.0; 2]; 37];
    expected[5] = [0.5, -1.0];
    expected[36] = f.samples[31];
    for block in [1, 3, 7, 17, 64] {
        assert_eq!(
            session(Arc::clone(&revision), 37, block),
            expected,
            "Main block {block}"
        );
        let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let plan = runtime
            .output_capture_plan(&[crate::OutputCaptureId::Track(f.tracks[0])], block)
            .unwrap();
        let mut output = vec![[0.0; 2]; 37];
        let mut tap = output.clone();
        for (main, track) in output.chunks_mut(block).zip(tap.chunks_mut(block)) {
            let mut captures = [track];
            runtime
                .render_with_capture(main, &mut crate::OutputCapture::new(&plan, &mut captures))
                .unwrap();
        }
        assert_eq!(output, expected, "captured Main block {block}");
        assert_eq!(tap, expected, "track tap block {block}");
    }
    let mut legacy = Renderer::new(revision.arrangement());
    legacy.play();
    let mut output = expected.clone();
    legacy.render(&mut output).unwrap();
    assert_eq!(output, expected);
}

#[test]
fn actual_delay_storage_limit_refuses_before_runtime_allocation() {
    let mut chains = vec![vec![]; 256];
    chains[0] = vec![500; 9];
    let refs: Vec<_> = chains.iter().map(Vec::as_slice).collect();
    let f = fixture(&refs, 8);
    assert!(prepare(&f).is_err());
}

#[test]
fn typed_audio_track_inputs_align_before_prefix_and_preserve_main_capture() {
    let mut f = fixture(&[&[3], &[7]], 17);
    f.editor
        .execute(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.tracks[1]),
            destination: f.tracks[0],
            gain: 0.5,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    assert_eq!(revision.arrangement().main_latency_frames(), 10);
    let mut expected = vec![[0.0; 2]; 27];
    for (to, source) in expected[10..].iter_mut().zip(&f.samples) {
        *to = [source[0] * 2.5, source[1] * 2.5];
    }
    for partition in [1, 63, 256, 513] {
        assert_eq!(session(Arc::clone(&revision), 27, partition), expected);
    }
    let mut renderer = Renderer::new(revision.arrangement());
    renderer.play();
    let mut actual = expected.clone();
    renderer.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let plan = runtime
        .output_capture_plan(
            &[
                crate::OutputCaptureId::Track(f.tracks[0]),
                crate::OutputCaptureId::Track(f.tracks[1]),
            ],
            27,
        )
        .unwrap();
    let mut a = vec![[0.0; 2]; 27];
    let mut b = a.clone();
    let mut outputs = [&mut a[..], &mut b[..]];
    runtime
        .render_with_capture(
            &mut actual,
            &mut crate::OutputCapture::new(&plan, &mut outputs),
        )
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(a[10], [0.375, -0.75]);
    assert_eq!(b[10], f.samples[0]);

    // The direct source and its 0.5 feed meet before the three-frame
    // prefix; both the fader tap and Main must retain the aligned transient.
    let mut expected_a = vec![[0.0; 2]; 27];
    let mut expected_b = expected_a.clone();
    for (index, source) in f.samples.iter().enumerate() {
        expected_a[index + 10] = [source[0] * 1.5, source[1] * 1.5];
        expected_b[index + 10] = *source;
    }
    for block in [1, 3, 7, 17] {
        let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let plan = runtime
            .output_capture_plan(
                &[
                    crate::OutputCaptureId::Track(f.tracks[0]),
                    crate::OutputCaptureId::Track(f.tracks[1]),
                ],
                block,
            )
            .unwrap();
        let mut actual_main = vec![[0.0; 2]; 27];
        let mut actual_a = actual_main.clone();
        let mut actual_b = actual_main.clone();
        for ((main, a), b) in actual_main
            .chunks_mut(block)
            .zip(actual_a.chunks_mut(block))
            .zip(actual_b.chunks_mut(block))
        {
            let mut outputs = [a, b];
            runtime
                .render_with_capture(main, &mut crate::OutputCapture::new(&plan, &mut outputs))
                .unwrap();
        }
        assert_eq!(actual_main, expected, "Main, block {block}");
        assert_eq!(actual_a, expected_a, "track A, block {block}");
        assert_eq!(actual_b, expected_b, "track B, block {block}");
    }
}

#[test]
fn return_to_track_inputs_follow_combined_topology_with_zero_and_positive_latency() {
    for latency in [0, 5] {
        let mut f = fixture(&[&[], &[]], 17);
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "feed".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        f.editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: state(latency).unwrap(),
            })
            .unwrap();
        f.editor
            .execute(Command::SetReturnSend {
                track: f.tracks[1],
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        f.editor
            .execute(Command::SetAudioFeed {
                source: hex_project::RoutingOwner::Return(bus),
                destination: f.tracks[0],
                gain: 0.5,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let mut expected = vec![[0.0; 2]; 17 + latency as usize];
        for (to, sample) in expected[latency as usize..].iter_mut().zip(&f.samples) {
            *to = [sample[0] * 2.75, sample[1] * 2.75];
        }
        assert_eq!(session(Arc::clone(&revision), expected.len(), 63), expected);
        let mut renderer = Renderer::new(revision.arrangement());
        renderer.play();
        let mut actual = expected.clone();
        renderer.render(&mut actual).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn audio_feed_gain_only_handoff_preserves_pending_samples_and_updates_new_source_gain() {
    let mut f = fixture(&[&[3], &[7]], 17);
    f.editor
        .execute(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.tracks[1]),
            destination: f.tracks[0],
            gain: 0.5,
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 4]).unwrap();
    f.editor
        .execute(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.tracks[1]),
            destination: f.tracks[0],
            gain: 1.0,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut pcm = [[0.0; 2]; 23];
    next.render(&mut pcm).unwrap();
    // The source impulse is still upstream of the feed boundary when the edit lands.
    assert_eq!(pcm[6], [0.75, -1.5]);
    assert_eq!(pcm[22], [-0.375, 1.125]);
}

/// Control-owned construction and buffers stay outside the timed render scope.
/// This throughput benchmark does not measure native callback deadlines.
#[test]
#[ignore = "release throughput evidence; run explicitly with --release --ignored --nocapture"]
fn profile_real_compensated_and_routed_graph() {
    use std::time::Instant;
    for (label, delays, routed, grouped) in [
        ("dry", [&[][..], &[][..]], false, false),
        ("compensated", [&[][..], &[7][..]], false, false),
        ("routed", [&[3][..], &[7][..]], true, false),
        ("grouped", [&[][..], &[][..]], false, true),
    ] {
        let mut f = fixture(&delays, 48000);
        // Keep authored source traversal and nonzero input active for the whole
        // warmup and measured interval, including the last timing sample.
        for (index, sample) in f.samples.iter_mut().enumerate() {
            *sample = [
                (index as f32 * 0.013).sin() * 0.1,
                (index as f32 * 0.017).cos() * 0.1,
            ];
        }
        for audio in f.media.values_mut() {
            *audio = Audio::new(48000, f.samples.clone()).unwrap();
        }
        for &track in &f.tracks {
            let clip = f.editor.project().track(track).unwrap().clips()[0].id();
            f.editor
                .execute(Command::SetClipLooping {
                    clip,
                    looping: true,
                })
                .unwrap();
            f.editor
                .execute(Command::TrimClip {
                    clip,
                    start: 0,
                    duration: 500000,
                    source_offset: 0,
                    source_length: 48000,
                })
                .unwrap();
        }
        if routed {
            f.editor
                .execute(Command::SetAudioFeed {
                    source: hex_project::RoutingOwner::Track(f.tracks[1]),
                    destination: f.tracks[0],
                    gain: 0.5,
                })
                .unwrap();
        }
        if grouped {
            f.editor
                .execute(Command::GroupTracks {
                    tracks: hex_project::TrackSelectionIds::new(&f.tracks).unwrap(),
                    name: "group".into(),
                })
                .unwrap();
        }
        let mut runtime = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut block = [[0.0; 2]; 256];
        for _ in 0..32 {
            runtime.render(&mut block).unwrap();
        }
        let mut samples = [0u128; 101];
        for sample in &mut samples {
            let start = Instant::now();
            for _ in 0..16 {
                runtime.render(&mut block).unwrap();
                std::hint::black_box(&block);
            }
            *sample = start.elapsed().as_nanos();
        }
        samples.sort_unstable();
        let frames = 16.0 * 256.0;
        println!(
            "{label}: rate=48000 tracks={} block=256 warmup=8192 samples=101 median_ns_per_frame={:.2} p99_ns_per_frame={:.2} median_frames_per_second={:.0}",
            if grouped { 3 } else { 2 },
            samples[50] as f64 / frames,
            samples[99] as f64 / frames,
            frames * 1e9 / samples[50] as f64
        );
    }
}

#[test]
fn compensated_stopped_and_live_handoffs_reject_rate_change_without_touching_donor() {
    for playing in [false, true] {
        let f = fixture_at_rate(&[&[7]], 17, 44100);
        let other = fixture(&[&[7]], 17);
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        if playing {
            old.apply(SessionPlaybackCommand::Play).unwrap();
            reference.apply(SessionPlaybackCommand::Play).unwrap();
            old.render(&mut [[0.0; 2]; 4]).unwrap();
            reference.render(&mut [[0.0; 2]; 4]).unwrap();
        }
        let mut candidate = SessionPlayback::new(prepare(&other).unwrap()).unwrap();
        assert_eq!(
            candidate.try_inherit_state_from(&mut old),
            Err(crate::SessionPlaybackError::RevisionSampleRateMismatch)
        );
        assert_eq!(old.performance_frame(), if playing { 4 } else { 0 });
        old.apply(SessionPlaybackCommand::Play).unwrap();
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        let mut actual = [[0.0; 2]; 24];
        let mut expected = actual;
        old.render(&mut actual).unwrap();
        reference.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn zero_latency_audio_feed_and_device_insertion_adopt_fresh_empty_delay_layouts_live() {
    let mut f = fixture(&[&[], &[]], 17);
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 4]).unwrap();
    f.editor
        .execute(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.tracks[1]),
            destination: f.tracks[0],
            gain: 0.5,
        })
        .unwrap();
    let mut routed = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    routed.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(routed.performance_frame(), 4);
    assert_eq!(
        routed
            .prepared_revision()
            .arrangement()
            .main_latency_frames(),
        0
    );
    routed.render(&mut [[0.0; 2]; 4]).unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: state(0).unwrap(),
        })
        .unwrap();
    let mut inserted = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    inserted.try_inherit_state_from(&mut routed).unwrap();
    let mut actual = [[0.0; 2]; 9];
    inserted.render(&mut actual).unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let sample = f.samples[8 + offset];
        assert_eq!(*frame, [sample[0] * 2.5, sample[1] * 2.5]);
    }
}

#[test]
fn zero_latency_routed_tempo_edit_preserves_pcm_while_positive_history_refuses_atomically() {
    for latency in [0, 7] {
        let mut f = fixture(&[&[latency], &[]], 17);
        f.editor
            .execute(Command::SetAudioFeed {
                source: hex_project::RoutingOwner::Track(f.tracks[1]),
                destination: f.tracks[0],
                gain: 0.5,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 4]).unwrap();
        }
        f.editor
            .execute(Command::SetTempoPoint {
                beat: hex_project::BeatTime::ZERO,
                tempo: hex_project::Tempo::new(90_000_000).unwrap(),
            })
            .unwrap();
        let mut candidate = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        if latency == 0 {
            candidate.try_inherit_state_from(&mut old).unwrap();
        } else {
            assert!(candidate.try_inherit_state_from(&mut old).is_err());
            assert_eq!(old.performance_frame(), 4);
        }
        let mut actual = [[0.0; 2]; 24];
        let mut expected = actual;
        if latency == 0 {
            candidate.render(&mut actual).unwrap();
        } else {
            old.render(&mut actual).unwrap();
        }
        reference.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn delayed_origin_clock_reports_source_performance_and_transport_without_guesses() {
    use hex_lattice::NativeInstrumentClock;
    let unknown = OriginClock(None);
    assert_eq!(unknown.performance_frame_at_offset(0), None);
    assert_eq!(unknown.transport_is_running(), None);
    let source = TransportStamp {
        tempo_micro_bpm: Some(123_000_000),
        performance_frame: 900,
        transport_is_running: true,
        arrangement_frame: 12,
        loop_pass: 7,
        performance_beat_raw: 5 << 32,
        arrangement_beat_raw: Some(1 << 32),
    };
    let clock = OriginClock(Some(source));
    assert_eq!(clock.tempo_micro_bpm_at_offset(0), Some(123_000_000));
    assert_eq!(clock.tempo_micro_bpm_at_offset(1), None);
    assert_eq!(unknown.tempo_micro_bpm_at_offset(0), None);
    assert_eq!(clock.performance_frame_at_offset(0), Some(900));
    assert_eq!(clock.arrangement_frame_at_offset(0), Some(12));
    assert_eq!(clock.loop_pass_at_offset(0), Some(7));
    assert_eq!(clock.beat_at_offset(0), Some(5 << 32));
    assert_eq!(clock.arrangement_beat_at_offset(0), Some(1 << 32));
    assert_eq!(clock.transport_is_running(), Some(true));
    assert_eq!(clock.performance_frame_at_offset(1), None);
    let unavailable_beat = OriginClock(Some(TransportStamp {
        arrangement_beat_raw: None,
        ..source
    }));
    assert_eq!(unavailable_beat.arrangement_beat_at_offset(0), None);
    assert_eq!(unavailable_beat.arrangement_frame_at_offset(0), Some(12));
    assert_eq!(unavailable_beat.beat_at_offset(0), Some(5 << 32));
    let stopped = OriginClock(Some(TransportStamp {
        transport_is_running: false,
        ..source
    }));
    assert_eq!(stopped.transport_is_running(), Some(false));
    assert_eq!(stopped.performance_frame_at_offset(0), None);
    assert_eq!(stopped.beat_at_offset(0), Some(5 << 32));
}

#[test]
fn main_review_serial_latency_reports_final_output_without_moving_pre_main_capture() {
    let mut f = fixture(&[&[], &[7]], 300);
    for delay in [3, 5] {
        f.editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: state(delay).unwrap(),
            })
            .unwrap();
    }
    f.editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let revision = prepare(&f).unwrap();
    eprintln!(
        "actual Project layout {} bytes; pre-Main 7 + Main 3 + Main 5 = final 15",
        size_of::<Project>()
    );
    assert_eq!(
        revision
            .arrangement()
            .signal_graph
            .as_ref()
            .unwrap()
            .main_latency(),
        7,
        "upstream alignment stays at pre-Main merge"
    );
    assert_eq!(
        revision.arrangement().main_latency_frames(),
        15,
        "causal report includes serial Main processing"
    );
    let mut expected = vec![[0.0; 2]; 315];
    expected[15..].copy_from_slice(&f.samples);
    for partition in [1, 63, 256] {
        assert_eq!(session(Arc::clone(&revision), 315, partition), expected);
    }
    let mut renderer = Renderer::new(revision.arrangement());
    renderer.play();
    let mut rendered = vec![[0.0; 2]; 315];
    renderer.render(&mut rendered).unwrap();
    assert_eq!(rendered, expected, "Main serial DSP/master executes once");
    let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let ids: Vec<_> = f
        .tracks
        .iter()
        .copied()
        .map(crate::OutputCaptureId::Track)
        .collect();
    let plan = runtime.output_capture_plan(&ids, 315).unwrap();
    let mut main = vec![[0.0; 2]; 315];
    let mut a = main.clone();
    let mut b = main.clone();
    let mut views = [&mut a[..], &mut b[..]];
    let mut capture = crate::OutputCapture::new(&plan, &mut views);
    runtime
        .render_with_capture(&mut main, &mut capture)
        .unwrap();
    assert_eq!(main, expected);
    for output in 0..2 {
        for index in 0..315 {
            assert_eq!(
                capture.output(output).unwrap()[index],
                if (7..307).contains(&index) {
                    f.samples[index - 7]
                } else {
                    [0.0; 2]
                },
                "Track taps/stems retain pre-Main seven-frame alignment"
            );
        }
    }
}

#[test]
fn main_review_unknown_latency_refuses_before_candidate_publication() {
    let mut f = fixture(&[&[]], 8);
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(u32::MAX).unwrap(),
        })
        .unwrap();
    let before = f.editor.project().clone();
    assert!(
        prepare(&f).is_err(),
        "enabled unknown Main latency must refuse"
    );
    assert_eq!(f.editor.project(), &before);
    let id = before.main_devices()[0].id();
    f.editor
        .execute(Command::SetDeviceBypass {
            device: id,
            bypassed: true,
        })
        .unwrap();
    let bypassed = prepare(&f).unwrap();
    assert_eq!(bypassed.arrangement().main_latency_frames(), 0);
    assert!(
        bypassed
            .arrangement()
            .signal_graph
            .as_ref()
            .is_some_and(|graph| !graph.needs_runtime())
    );
    assert_eq!(session(bypassed, 8, 3), f.samples);
}

#[test]
fn main_review_cumulative_latency_limit_includes_upstream_and_serial_main() {
    let mut f = fixture(&[&[1]], 8);
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(MAX_COMPENSATION_FRAMES).unwrap(),
        })
        .unwrap();
    let before = f.editor.project().clone();
    assert!(
        prepare(&f).is_err(),
        "valid separate paths must refuse when final cumulative arrival exceeds cap"
    );
    assert_eq!(f.editor.project(), &before);
}

#[test]
fn main_review_main_only_delay_retains_fast_pre_main_path_and_bypass() {
    let mut f = fixture(&[&[]], 8);
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    assert!(
        revision
            .arrangement()
            .signal_graph
            .as_ref()
            .unwrap()
            .main_latency()
            == 0,
        "Main clock history must not add upstream alignment delay"
    );
    assert_eq!(revision.arrangement().main_latency_frames(), 3);
    let mut expected = vec![[0.0; 2]; 11];
    expected[3..].copy_from_slice(&f.samples);
    assert_eq!(session(revision, 11, 2), expected);
    let id = f.editor.project().main_devices()[0].id();
    f.editor
        .execute(Command::SetDeviceBypass {
            device: id,
            bypassed: true,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    assert_eq!(revision.arrangement().main_latency_frames(), 0);
    assert_eq!(session(revision, 8, 3), f.samples);
    assert_eq!(
        Project::new(48_000, vec![]).unwrap().main_latency_frames(),
        0
    );
}

#[test]
fn main_review_known_latency_handoff_preserves_history_and_refuses_changed_runtime_layout() {
    let mut f = fixture(&[&[]], 32);
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut oracle = SessionPlayback::new(revision).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 2]).unwrap();
    oracle.render(&mut [[0.0; 2]; 2]).unwrap();
    f.editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let revision = prepare(&f).unwrap();
    assert_eq!(revision.arrangement().main_latency_frames(), 3);
    let mut candidate = SessionPlayback::new(revision).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    let mut expected = [[0.0; 2]; 6];
    let mut actual = expected;
    oracle.render(&mut expected).unwrap();
    candidate.render(&mut actual).unwrap();
    assert_eq!(
        actual,
        expected.map(|f| [f[0] * 0.5, f[1] * 0.5]),
        "Main delay history remains warm"
    );
    let device = f.editor.project().main_devices()[0].id();
    f.editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state(5).unwrap(),
        })
        .unwrap();
    let mut changed = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    let positions = (
        candidate.performance_frame(),
        candidate.arrangement_frame(),
        candidate.playing(),
    );
    assert!(
        changed.try_inherit_state_from(&mut candidate).is_err(),
        "existing native history preflight refuses incompatible delay storage"
    );
    assert_eq!(
        (
            candidate.performance_frame(),
            candidate.arrangement_frame(),
            candidate.playing()
        ),
        positions,
        "refused handoff preserves old transport"
    );
}

#[test]
fn nested_default_outputs_sum_once_through_each_group() {
    let mut f = fixture(&[&[], &[]], 8);
    let Created::Track(inner) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&f.tracks).unwrap(),
            name: "inner".into(),
        })
        .unwrap()
    else {
        panic!("inner group")
    };
    let Created::Track(outer) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[inner]).unwrap(),
            name: "outer".into(),
        })
        .unwrap()
    else {
        panic!("outer group")
    };
    let canonical = f.editor.project();
    let mut tracks = Vec::new();
    for row in canonical.tracks() {
        let clips = if f.tracks.contains(&row.id()) {
            vec![
                crate::Clip::new(Audio::new(48000, f.samples.clone()).unwrap(), 0, 8, false)
                    .unwrap(),
            ]
        } else {
            Vec::new()
        };
        let gain = if row.id() == inner {
            0.5
        } else if row.id() == outer {
            0.25
        } else {
            1.0
        };
        let mut track = Track::new(clips, gain, false).unwrap();
        track.track_id = Some(row.id());
        tracks.push(track);
    }
    let mut project = Project::new(48000, tracks).unwrap();
    project.signal_graph = SignalGraph::prepare(&project, &[], canonical).unwrap();
    project.tempo_map = Some(canonical.tempo_map().clone());
    let mut renderer = Renderer::new(&project);
    renderer.play();
    let mut actual = [[0.0; 2]; 8];
    renderer.render(&mut actual).unwrap();
    for (actual, source) in actual.iter().zip(&f.samples) {
        assert_eq!(*actual, [source[0] * 0.25, source[1] * 0.25]);
    }
    drop(renderer);
    f.editor
        .execute(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.tracks[0]),
            destination: inner,
            gain: 0.5,
        })
        .unwrap();
    project.signal_graph = SignalGraph::prepare(&project, &[], f.editor.project()).unwrap();
    let mut renderer = Renderer::new(&project);
    renderer.play();
    renderer.render(&mut actual).unwrap();
    for (actual, source) in actual.iter().zip(&f.samples) {
        assert_eq!(*actual, [source[0] * 0.3125, source[1] * 0.3125]);
    }
}

#[test]
fn compiled_nested_groups_process_shared_effect_gain_and_captures_once() {
    let mut f = fixture(&[&[], &[]], 8);
    let Created::Track(inner) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&f.tracks).unwrap(),
            name: "inner".into(),
        })
        .unwrap()
    else {
        panic!("inner group")
    };
    let Created::Track(outer) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[inner]).unwrap(),
            name: "outer".into(),
        })
        .unwrap()
    else {
        panic!("outer group")
    };
    f.editor
        .execute(Command::Gain {
            track: inner,
            gain: 0.5,
        })
        .unwrap();
    f.editor
        .execute(Command::Gain {
            track: outer,
            gain: 0.25,
        })
        .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: inner,
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut expected = vec![[0.0; 2]; 11];
    for (out, source) in expected[3..].iter_mut().zip(&f.samples) {
        *out = [source[0] * 0.25, source[1] * 0.25];
    }
    for partition in [1, 3, 11] {
        assert_eq!(session(Arc::clone(&revision), 11, partition), expected);
    }
    let mut renderer = Renderer::new(revision.arrangement());
    renderer.play();
    let mut output = [[0.0; 2]; 11];
    renderer.render(&mut output).unwrap();
    assert_eq!(output.as_slice(), expected);
    let mut runtime = SessionPlayback::new(revision).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let plan = runtime
        .output_capture_plan(
            &[
                crate::OutputCaptureId::Track(inner),
                crate::OutputCaptureId::Track(outer),
            ],
            11,
        )
        .unwrap();
    let mut main = [[0.0; 2]; 11];
    let mut group_capture = main;
    let mut outer_capture = main;
    let mut buffers: [&mut [Frame]; 2] = [&mut group_capture, &mut outer_capture];
    let mut capture = crate::OutputCapture::new(&plan, &mut buffers);
    runtime
        .render_with_capture(&mut main, &mut capture)
        .unwrap();
    for (index, frame) in expected.iter().enumerate() {
        assert_eq!(capture.output(0).unwrap()[index], frame.map(|x| x * 4.0));
        assert_eq!(capture.output(1).unwrap()[index], *frame);
        assert_eq!(main[index], *frame);
    }
}

#[test]
fn compiled_group_default_and_additive_feed_preserve_independent_return_send() {
    let mut f = fixture(&[&[], &[]], 8);
    let Created::Track(group) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&f.tracks).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    f.editor
        .execute(Command::Gain {
            track: group,
            gain: 0.5,
        })
        .unwrap();
    f.editor
        .execute(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.tracks[0]),
            destination: group,
            gain: 0.5,
        })
        .unwrap();
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let output = session(revision, 8, 3);
    for (actual, source) in output.iter().zip(&f.samples) {
        // Two children plus the explicit half feed through the group, and a
        // separate half send to Return: 2.5 * 0.5 + 0.5 = 1.75.
        assert_eq!(*actual, [source[0] * 1.75, source[1] * 1.75]);
    }
    f.editor
        .execute(Command::Balance {
            track: group,
            balance: 0.5,
        })
        .unwrap();
    let output = session(prepare(&f).unwrap(), 8, 3);
    for (actual, source) in output.iter().zip(&f.samples) {
        assert_eq!(*actual, [source[0] * 1.125, source[1] * 1.75]);
    }
    f.editor
        .execute(Command::Mute {
            track: group,
            muted: true,
        })
        .unwrap();
    let output = session(prepare(&f).unwrap(), 8, 3);
    for (actual, source) in output.iter().zip(&f.samples) {
        assert_eq!(*actual, [source[0] * 0.5, source[1] * 0.5]);
    }
}

#[test]
fn compiled_group_solo_selects_descendants_and_ancestors_without_changing_flags() {
    let mut f = fixture(&[&[], &[], &[]], 8);
    let Created::Track(group) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&f.tracks[..2]).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    f.editor
        .execute(Command::ToggleSoloTracks {
            tracks: hex_project::TrackSelectionIds::new(&[group]).unwrap(),
            exclusive: true,
        })
        .unwrap();
    let output = session(prepare(&f).unwrap(), 8, 3);
    for (actual, source) in output.iter().zip(&f.samples) {
        assert_eq!(*actual, [source[0] * 2.0, source[1] * 2.0]);
    }
    assert!(!f.editor.project().track(f.tracks[0]).unwrap().soloed());
    assert!(!f.editor.project().track(f.tracks[1]).unwrap().soloed());
    f.editor
        .execute(Command::ToggleSoloTracks {
            tracks: hex_project::TrackSelectionIds::new(&[f.tracks[0]]).unwrap(),
            exclusive: true,
        })
        .unwrap();
    let output = session(prepare(&f).unwrap(), 8, 3);
    assert_eq!(output, f.samples);
    f.editor
        .execute(Command::Mute {
            track: group,
            muted: true,
        })
        .unwrap();
    assert_eq!(session(prepare(&f).unwrap(), 8, 3), [[0.0; 2]; 8]);
}

#[test]
fn group_gain_automation_tracks_delayed_content_across_render_partitions() {
    let mut f = fixture(&[&[]], 8);
    let Created::Track(group) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&f.tracks).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    f.editor
        .execute(Command::InsertDevice {
            track: group,
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap();
    let beat = f.editor.project().tempo_map().frame_to_beat(3).unwrap();
    f.editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track: group,
            points: vec![
                hex_project::GainPoint {
                    incoming_gain: None,
                    beat: hex_project::BeatTime::ZERO,
                    gain: 0.5,
                    outgoing: hex_project::GainInterpolation::Step,
                },
                hex_project::GainPoint {
                    incoming_gain: None,
                    beat,
                    gain: 1.0,
                    outgoing: hex_project::GainInterpolation::Step,
                },
            ],
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let whole = session(Arc::clone(&revision), 11, 11);
    assert_eq!(whole, session(revision, 11, 1));
    assert_eq!(whole[3], f.samples[0].map(|value| value * 0.5));
    assert_eq!(whole[10], f.samples[7]);
}

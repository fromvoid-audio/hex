#![cfg(feature = "instrument-test-support")]

use hex_engine::{
    Audio, LaunchQuantization, NativeDeviceClass, PreparedRevision, SessionPlayback,
    SessionPlaybackCommand, SessionPlaybackError, SineTable, default_native_device_state_for_class,
    native_device_catalog_for_class, patch_native_device_parameter_for_class,
};
use hex_lattice::{
    MutableInstrumentRuntime, NativeInstrumentDefinition, NativeInstrumentEvent,
    NativeInstrumentRegistry, NativeInstrumentRuntimeError, PreparedAnalyticNote,
    PreparedMutableInstrumentRuntime, PreparedNativeInstrument, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{AssetId, BeatTime, Command, Created, Editor, IDENTITY_CONVERSION, Project};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const RATE: u32 = 48_000;
const FIXTURE_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.export.mutable");
const LEVEL: f32 = 0.125;
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hex-export-prepared-mutable-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct FixtureDefinition;
struct FixturePrepared {
    noise_floor: bool,
    echo_frame: Option<u64>,
    stuck: bool,
    release_frames: u32,
}
struct FixtureFactory {
    noise_floor: bool,
    echo_frame: Option<u64>,
    stuck: bool,
    release_frames: u32,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    id: u64,
    active: bool,
    gated: bool,
    release_left: u32,
}

struct Runtime {
    rendered_frames: u64,
    noise_floor: bool,
    echo_frame: Option<u64>,
    stuck: bool,
    release_frames: u32,
    voices: [Voice; 64],
}

impl NativeDeviceMetadata for FixtureDefinition {
    fn type_id(&self) -> DeviceTypeId {
        FIXTURE_TYPE
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("export mutable fixture")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(FIXTURE_TYPE, 1, vec![1])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() == FIXTURE_TYPE
            && state.state_version() == 1
            && matches!(state.payload(), [1] | [2] | [3] | [4] | [5])
        {
            Ok(state.clone())
        } else {
            Err(DeviceContractError::MalformedState)
        }
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

impl NativeInstrumentDefinition for FixtureDefinition {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        if !context.tuning.is_neutral() {
            return Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass);
        }
        if !(8_000..=192_000).contains(&sample_rate) || self.migrate_state(state).is_err() {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(Box::new(FixturePrepared {
            noise_floor: matches!(state.payload(), [4] | [5]),
            echo_frame: (state.payload() == [5]).then_some(u64::from(sample_rate) * 12),
            stuck: state.payload() == [2],
            release_frames: if state.payload() == [3] {
                sample_rate * 12
            } else {
                3
            },
        }))
    }
}

impl PreparedNativeInstrument for FixturePrepared {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(Some(Box::new(FixtureFactory {
            noise_floor: self.noise_floor,
            echo_frame: self.echo_frame,
            stuck: self.stuck,
            release_frames: self.release_frames,
        })))
    }
}

impl PreparedMutableInstrumentRuntime for FixtureFactory {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        256
    }
    fn max_active_occurrences(&self) -> usize {
        64
    }
    fn max_release_tail_frames(&self) -> u64 {
        u64::from(self.release_frames)
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime {
            rendered_frames: 0,
            noise_floor: self.noise_floor,
            echo_frame: self.echo_frame,
            stuck: self.stuck,
            release_frames: self.release_frames,
            voices: [Voice::default(); 64],
        })
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl MutableInstrumentRuntime for Runtime {
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.len() > 256 {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
        }
        let mut offset = 0;
        for event in events {
            if matches!(event, NativeInstrumentEvent::Expression { .. }) {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            let current = event.frame_offset();
            if current < offset || current >= frame_count {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            offset = current;
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        let mut event_index = 0;
        for (frame_offset, frame) in frames.iter_mut().enumerate() {
            while let Some(event) = events.get(event_index).copied() {
                let offset = event.frame_offset();
                if offset != frame_offset {
                    break;
                }
                match event {
                    // Unsupported by this fixture's read-only preflight.
                    NativeInstrumentEvent::Expression { .. } => {}
                    NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                        if let Some(voice) = self.voices.iter_mut().find(|voice| !voice.active) {
                            *voice = Voice {
                                id: occurrence_id,
                                active: true,
                                gated: true,
                                release_left: 0,
                            };
                        }
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        if let Some(voice) = self
                            .voices
                            .iter_mut()
                            .find(|voice| voice.active && voice.id == occurrence_id)
                        {
                            voice.gated = false;
                            voice.release_left = self.release_frames;
                        }
                    }
                }
                event_index += 1;
            }
            let signal = self.voices.iter_mut().fold(0.0, |sum, voice| {
                if !voice.active {
                    return sum;
                }
                let level = if voice.gated {
                    LEVEL
                } else if voice.release_left > 0 {
                    LEVEL * (voice.release_left as f32 / (self.release_frames + 1) as f32)
                } else {
                    voice.active = false;
                    return sum;
                };
                if !voice.gated && !self.stuck {
                    voice.release_left -= 1;
                }
                sum + level
            });
            let noise = if self.noise_floor {
                if self.rendered_frames % 2 == 0 {
                    1e-5
                } else {
                    -1e-5
                }
            } else {
                0.0
            };
            frame[0] += signal + noise;
            frame[1] += signal - noise;
            if self.echo_frame == Some(self.rendered_frames) {
                frame[1] += 0.00022;
            }
            self.rendered_frames += 1;
        }
    }
    fn reset(&mut self) {
        self.rendered_frames = 0;
        self.voices.fill(Voice::default());
    }
    fn is_quiescent(&self) -> bool {
        !self.noise_floor && self.voices.iter().all(|voice| !voice.active)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|_| true)
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.voices = previous.voices;
            self.rendered_frames = previous.rendered_frames;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn mutable_state() -> NativeDeviceState {
    FixtureDefinition.default_state().expect("fixture state")
}

fn gain_state(gain: f32) -> NativeDeviceState {
    let type_id = native_device_catalog_for_class(NativeDeviceClass::Effect)
        .expect("effect catalog")
        .into_iter()
        .find(|entry| entry.name == "utility")
        .expect("utility catalog entry")
        .type_id;
    let defaults = default_native_device_state_for_class(NativeDeviceClass::Effect, type_id)
        .expect("utility defaults");
    patch_native_device_parameter_for_class(
        NativeDeviceClass::Effect,
        &defaults,
        ParameterId::from_static(1),
        ParameterValue::Float(gain),
    )
    .expect("utility gain state")
}

fn prepared_hybrid(
    transport_loop: bool,
    session_loop: bool,
    arrangement_loop: bool,
) -> (Arc<PreparedRevision>, AssetId) {
    prepared_hybrid_using(transport_loop, session_loop, arrangement_loop, false)
}
fn prepared_hybrid_using(
    transport_loop: bool,
    session_loop: bool,
    arrangement_loop: bool,
    stuck: bool,
) -> (Arc<PreparedRevision>, AssetId) {
    prepared_hybrid_with_state(
        transport_loop,
        session_loop,
        arrangement_loop,
        if stuck {
            NativeDeviceState::new(FIXTURE_TYPE, 1, vec![2]).unwrap()
        } else {
            mutable_state()
        },
    )
}
fn prepared_hybrid_with_state(
    transport_loop: bool,
    session_loop: bool,
    arrangement_loop: bool,
    state: NativeDeviceState,
) -> (Arc<PreparedRevision>, AssetId) {
    prepared_hybrid_with_state_at_rate(transport_loop, session_loop, arrangement_loop, state, RATE)
}
fn prepared_hybrid_with_state_at_rate(
    transport_loop: bool,
    session_loop: bool,
    arrangement_loop: bool,
    state: NativeDeviceState,
    rate: u32,
) -> (Arc<PreparedRevision>, AssetId) {
    let mut editor = Editor::new(Project::new(rate).expect("project"), 8).expect("editor");
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "hybrid".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("track")
    {
        Created::Track(track) => track,
        other => panic!("unexpected track result: {other:?}"),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .expect("default instrument")
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state,
        })
        .expect("mutable instrument");
    // Keep the authored MIDI on a silent input track. The hybrid owner retains
    // its audio prefix, mutable instrument gap and suffix without stacked clips.
    let Created::Track(source) = editor
        .execute(Command::AddTrack {
            name: "silent midi source".into(),
        })
        .expect("midi source")
    else {
        panic!("source track")
    };
    editor
        .execute(Command::SetMidiSource {
            destination: track,
            source: Some(source),
        })
        .expect("route authored midi to hybrid owner");
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track: source,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .expect("midi clip")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip result: {other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(17, 1_600).expect("note duration"),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("midi note");

    let _prefix = match editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: gain_state(2.0),
        })
        .expect("prefix effect")
    {
        Created::Device(device) => device,
        other => panic!("unexpected effect result: {other:?}"),
    };
    let suffix = match editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: gain_state(0.5),
        })
        .expect("suffix effect")
    {
        Created::Device(device) => device,
        other => panic!("unexpected effect result: {other:?}"),
    };
    editor
        .execute(Command::MoveTrackDevice {
            device: instrument,
            track,
            before: Some(suffix),
        })
        .expect("position instrument between effects");
    assert_eq!(
        editor.project().track(track).unwrap().instrument_position(),
        Some(1)
    );
    let audio_frames = (0..512)
        .map(|index| [index as f32 / 2048.0, -(index as f32) / 4096.0])
        .collect();
    let audio = Audio::new(rate, audio_frames).expect("audio");
    let imported = editor
        .execute(Command::ImportPreparedAudioClip {
            track,
            start: 0,
            path: PathBuf::from("/fixture/hybrid.wav"),
            source_sample_rate: rate,
            source_frames: 512,
            playback_frames: 512,
            conversion_version: IDENTITY_CONVERSION,
        })
        .expect("import clip");
    let asset = match imported {
        Created::Imported { asset, .. } => asset,
        other => panic!("unexpected imported result: {other:?}"),
    };
    let mut media = BTreeMap::new();
    media.insert(asset, audio);

    if transport_loop {
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 100,
                enabled: true,
            })
            .expect("transport loop");
    }
    if session_loop {
        let scene = match editor
            .execute(Command::AddScene {
                name: "unused".into(),
            })
            .expect("scene")
        {
            Created::Scene(scene) => scene,
            other => panic!("unexpected scene result: {other:?}"),
        };
        let session_clip = match editor
            .execute(Command::CreateSessionMidiClip {
                track: source,
                scene,
                name: "unused loop".into(),
                source_length: BeatTime::ONE,
            })
            .expect("session clip")
        {
            Created::SessionClip(clip) => clip,
            other => panic!("unexpected session clip result: {other:?}"),
        };
        editor
            .execute(Command::SetSessionMidiClipSource {
                clip: session_clip,
                source_length: BeatTime::ONE,
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: BeatTime::ONE,
                looping: true,
            })
            .expect("session source loop");
    }
    if arrangement_loop {
        editor
            .execute(Command::SetMidiClipSource {
                clip,
                source_length: BeatTime::ONE,
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: BeatTime::ONE,
                looping: true,
                length: BeatTime::ONE,
            })
            .expect("arrangement source loop");
    }
    let project = editor.project().clone();
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&FixtureDefinition];
    let instruments = NativeInstrumentRegistry::new(&definitions).expect("instrument registry");
    let revision = PreparedRevision::prepare_with_test_instrument_registry(
        &project,
        &media,
        Arc::new(SineTable::new()),
        None,
        &instruments,
    )
    .expect("prepared revision");
    (Arc::new(revision), asset)
}

fn pcm16(bytes: &[u8]) -> Vec<i16> {
    assert_eq!(bytes.len() % 2, 0);
    bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

/// PCM16 export applies ±1 LSB TPDF dither: within one LSB of rounding.
fn assert_near(actual: i16, sample: f32) {
    let rounded = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i32;
    assert!(
        (i32::from(actual) - rounded).abs() <= 1,
        "{actual} not within one lsb of {rounded}"
    );
}

#[test]
fn prepared_mutable_hybrid_export_matches_independent_ordered_pcm_oracle() {
    let dir = TestDir::new();
    let path = dir.0.join("hybrid.wav");
    let (revision, _) = prepared_hybrid(true, true, false);
    let report = hex_export::render_prepared_new(&path, Arc::clone(&revision), 0, 300)
        .expect("linear mutable hybrid export");
    let bytes = fs::read(path).expect("read wav");
    assert_eq!(report.frames, 300);
    assert_eq!(report.data_bytes, 1_200);

    // Audio is doubled by the prefix, mutable MIDI is added at the persisted
    // gap, then the suffix halves the sum. The independent mutable law gates
    // for 255 frames, then emits three linearly declining release frames across
    // the exporter’s 256-frame block boundary.
    let samples = pcm16(&bytes[44..]);
    assert_eq!(samples.len(), 600);
    for (index, frame) in samples.chunks_exact(2).enumerate() {
        let mutable = match index {
            0..=254 => LEVEL,
            255 => LEVEL * 0.75,
            256 => LEVEL * 0.5,
            257 => LEVEL * 0.25,
            _ => 0.0,
        };
        assert_near(frame[0], index as f32 / 2048.0 + mutable * 0.5);
        assert_near(frame[1], -(index as f32) / 4096.0 + mutable * 0.5);
    }
    let repeated = dir.0.join("hybrid-repeat.wav");
    hex_export::render_prepared_new(&repeated, Arc::clone(&revision), 0, 300)
        .expect("repeat export");
    assert_eq!(fs::read(repeated).expect("read repeat"), bytes);
    let truncated = dir.0.join("hybrid-truncated.wav");
    let report = hex_export::render_prepared_new(&truncated, Arc::clone(&revision), 0, 257)
        .expect("truncate during release");
    let truncated_bytes = fs::read(truncated).expect("read truncated export");
    assert_eq!(report.frames, 257);
    assert_eq!(truncated_bytes.len(), 44 + 257 * 4);
    assert_eq!(&truncated_bytes[44..], &bytes[44..44 + 257 * 4]);
}

#[test]
fn linear_export_ignores_transport_and_unused_session_loops_but_refuses_arrangement_loop() {
    let dir = TestDir::new();
    let path = dir.0.join("linear.wav");
    let (revision, _) = prepared_hybrid(true, true, false);
    hex_export::render_prepared_new(&path, Arc::clone(&revision), 0, 300)
        .expect("transport and Session loops do not wrap an Arrangement export");
    let samples = pcm16(&fs::read(path).expect("read linear wav")[44..]);
    assert_near(samples[150 * 2], 150.0 / 2048.0 + LEVEL * 0.5);

    let refused_path = dir.0.join("authored-loop.wav");
    let (revision, _) = prepared_hybrid(false, false, true);
    assert!(matches!(
        hex_export::render_prepared_new(&refused_path, revision, 0, 16),
        Err(hex_export::ExportError::SessionRender(
            SessionPlaybackError::MutableInstrumentLoopUnsupported
        ))
    ));
    assert!(!refused_path.exists());
    assert_eq!(fs::read_dir(&dir.0).expect("list files").count(), 1);
}

#[test]
fn export_nonzero_mutable_range_refuses_before_sounding_destination_seek() {
    let dir = TestDir::new();
    let path = dir.0.join("chase.wav");
    let (revision, _) = prepared_hybrid(false, false, false);
    assert!(matches!(
        hex_export::render_prepared_new(&path, revision, 1, 16),
        Err(hex_export::ExportError::Invalid(
            "nonzero range start with a mutable arrangement instrument is unsupported"
        ))
    ));
    assert!(!path.exists());
    assert_eq!(fs::read_dir(&dir.0).expect("list files").count(), 0);
}

#[test]
fn mutable_arrangement_export_refuses_nonzero_range_even_after_release() {
    let dir = TestDir::new();
    let path = dir.0.join("range.wav");
    let (revision, _) = prepared_hybrid(false, false, false);
    assert!(revision.has_arrangement_mutable_instruments());
    assert!(matches!(
        hex_export::render_prepared_new(&path, revision, 258, 257),
        Err(hex_export::ExportError::Invalid(
            "nonzero range start with a mutable arrangement instrument is unsupported"
        ))
    ));
    assert!(!path.exists());
    assert_eq!(fs::read_dir(&dir.0).expect("list files").count(), 0);
}

#[test]
fn nonmutable_prepared_export_keeps_nonzero_range_support() {
    let dir = TestDir::new();
    let full_path = dir.0.join("full.wav");
    let range_path = dir.0.join("range.wav");
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 8).expect("editor");
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "analytic".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("track")
    {
        Created::Track(track) => track,
        other => panic!("unexpected track result: {other:?}"),
    };
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .expect("midi clip")
    {
        Created::Clip(clip) => clip,
        other => panic!("unexpected clip result: {other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 10).expect("note duration"),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("midi note");
    let project = editor.project().clone();
    let revision = Arc::new(
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .expect("prepared revision"),
    );
    assert!(!revision.has_arrangement_mutable_instruments());
    hex_export::render_prepared_new(&full_path, Arc::clone(&revision), 0, 64)
        .expect("full analytic export");
    let report = hex_export::render_prepared_new(&range_path, revision, 10, 16)
        .expect("nonmutable range remains supported");
    assert_eq!(report.frames, 16);
    let full = fs::read(full_path).expect("read full wav");
    assert!(
        full[44..].iter().any(|sample_byte| *sample_byte != 0),
        "analytic range control must retain its audible instrument workload"
    );
    let range = fs::read(range_path).expect("read range wav");
    assert_eq!(range.len(), 44 + 16 * 4);
    assert_eq!(&range[44..], &full[44 + 10 * 4..44 + 26 * 4]);
}

#[test]
fn linear_export_owner_rejects_session_routes_and_state_inheritance_both_ways() {
    let (revision, _) = prepared_hybrid(false, false, false);
    let mut export = SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision))
        .expect("export owner");
    assert_eq!(
        export.apply(SessionPlaybackCommand::StopAllClips {
            quantization: LaunchQuantization::Immediate,
        }),
        Err(SessionPlaybackError::InvalidCommand)
    );
    let mut interactive = SessionPlayback::new(Arc::clone(&revision)).expect("interactive owner");
    interactive
        .apply(SessionPlaybackCommand::Play)
        .expect("play interactive");
    let export_frame = export.performance_frame();
    let interactive_frame = interactive.performance_frame();
    assert!(matches!(
        export.try_inherit_state_from(&mut interactive),
        Err(SessionPlaybackError::InvalidCommand)
    ));
    assert_eq!(export.performance_frame(), export_frame);
    assert_eq!(interactive.performance_frame(), interactive_frame);

    let mut fresh_export =
        SessionPlayback::new_linear_arrangement_export(revision).expect("fresh export owner");
    let export_frame = fresh_export.performance_frame();
    assert!(matches!(
        interactive.try_inherit_state_from(&mut fresh_export),
        Err(SessionPlaybackError::InvalidCommand)
    ));
    assert_eq!(interactive.performance_frame(), interactive_frame);
    assert_eq!(fresh_export.performance_frame(), export_frame);
}

#[test]
fn arrangement_bounce_caps_actual_nonquiescent_tail_and_preserves_destinations() {
    const TAIL_RATE: u32 = 8_000;
    let dir = TestDir::new();
    let path = dir.0.join("capped-tail.wav");
    let (revision, _) = prepared_hybrid_with_state_at_rate(
        false,
        false,
        false,
        NativeDeviceState::new(FIXTURE_TYPE, 1, vec![2]).unwrap(),
        TAIL_RATE,
    );
    let report = hex_export::render_arrangement_bounce_new(
        &path,
        revision,
        None,
        0,
        320,
        true,
        &hex_export::ExportControl::default(),
    )
    .expect("unfinished tail is a successful capped bounce");
    assert_eq!(report.frames, 320 + u64::from(TAIL_RATE) * 30);
    assert_eq!(report.data_bytes, report.frames as u32 * 8);
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        bytes.len(),
        hex_export::FLOAT32_WAV_HEADER_BYTES as usize + report.frames as usize * 8
    );
    assert_eq!(
        u16::from_le_bytes(bytes[20..22].try_into().unwrap()),
        3,
        "float WAV"
    );
    assert_eq!(
        u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
        TAIL_RATE
    );
    assert!(
        bytes[bytes.len() - 8..]
            .chunks_exact(4)
            .all(|sample| f32::from_le_bytes(sample.try_into().unwrap()).abs() > 0.0),
        "the tail is still sounding at the exact cap"
    );
    let original = bytes;
    let (revision, _) = prepared_hybrid(false, false, false);
    assert!(
        hex_export::render_arrangement_bounce_new(
            &path,
            revision,
            None,
            0,
            320,
            false,
            &hex_export::ExportControl::default(),
        )
        .is_err()
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        original,
        "create-new preserves existing media"
    );
    assert_eq!(
        fs::read_dir(&dir.0).unwrap().count(),
        1,
        "no temporary output remains"
    );
}

#[test]
fn arrangement_bounce_keeps_a_naturally_ending_tail_beyond_ten_seconds() {
    const TAIL_RATE: u32 = 8_000;
    let dir = TestDir::new();
    let path = dir.0.join("natural-tail.wav");
    let (revision, _) = prepared_hybrid_with_state_at_rate(
        false,
        false,
        false,
        NativeDeviceState::new(FIXTURE_TYPE, 1, vec![3]).unwrap(),
        TAIL_RATE,
    );
    let report = hex_export::render_arrangement_bounce_new(
        &path,
        revision,
        None,
        0,
        320,
        true,
        &hex_export::ExportControl::default(),
    )
    .expect("a twelve-second release finishes naturally");
    assert!(report.frames > u64::from(TAIL_RATE) * 12);
    assert!(report.frames < u64::from(TAIL_RATE) * 13);
    let bytes = fs::read(&path).unwrap();
    let late = hex_export::FLOAT32_WAV_HEADER_BYTES as usize + TAIL_RATE as usize * 11 * 8;
    assert!(
        bytes[late..late + 8]
            .chunks_exact(4)
            .any(|sample| f32::from_le_bytes(sample.try_into().unwrap()).abs() > 0.0)
    );
    assert_eq!(
        &bytes[bytes.len() - 8..],
        &[0; 8],
        "natural release reaches silence before publication"
    );
}

#[test]
fn arrangement_bounce_ignores_quiet_noise_without_trimming_selected_content() {
    const TAIL_RATE: u32 = 8_000;
    let dir = TestDir::new();
    let (revision, _) = prepared_hybrid_with_state_at_rate(
        false,
        false,
        false,
        NativeDeviceState::new(FIXTURE_TYPE, 1, vec![4]).unwrap(),
        TAIL_RATE,
    );
    let path = dir.0.join("quiet-tail.wav");
    let report = hex_export::render_arrangement_bounce_new(
        &path,
        Arc::clone(&revision),
        None,
        0,
        1024,
        true,
        &hex_export::ExportControl::default(),
    )
    .unwrap();
    assert_eq!(
        report.frames, 1024,
        "quiet background must not extend the selected span"
    );
    let exact_path = dir.0.join("exact.wav");
    let exact = hex_export::render_arrangement_bounce_new(
        &exact_path,
        revision,
        None,
        0,
        1024,
        false,
        &hex_export::ExportControl::default(),
    )
    .unwrap();
    assert_eq!(exact.frames, 1024);
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        &bytes[hex_export::FLOAT32_WAV_HEADER_BYTES as usize..],
        &fs::read(exact_path).unwrap()[hex_export::FLOAT32_WAV_HEADER_BYTES as usize..],
        "retained PCM is unchanged"
    );
    assert!(
        bytes[bytes.len() - 8..]
            .chunks_exact(4)
            .all(|v| f32::from_le_bytes(v.try_into().unwrap()).abs() > 0.0),
        "the retained source noise is not zeroed or gated"
    );
}

#[test]
fn arrangement_bounce_keeps_a_late_one_channel_echo_across_quiet_noise() {
    const TAIL_RATE: u32 = 8_000;
    let dir = TestDir::new();
    let (revision, _) = prepared_hybrid_with_state_at_rate(
        false,
        false,
        false,
        NativeDeviceState::new(FIXTURE_TYPE, 1, vec![5]).unwrap(),
        TAIL_RATE,
    );
    let path = dir.0.join("late-echo.wav");
    let report = hex_export::render_arrangement_bounce_new(
        &path,
        revision,
        None,
        0,
        1024,
        true,
        &hex_export::ExportControl::default(),
    )
    .unwrap();
    let echo_frame = u64::from(TAIL_RATE) * 12;
    assert_eq!(
        report.frames,
        echo_frame + 1 + u64::from(TAIL_RATE) / 20,
        "retain fifty milliseconds after the late echo"
    );
    let bytes = fs::read(path).unwrap();
    let sample_offset = hex_export::FLOAT32_WAV_HEADER_BYTES as usize + echo_frame as usize * 8;
    let left = f32::from_le_bytes(bytes[sample_offset..sample_offset + 4].try_into().unwrap());
    let right = f32::from_le_bytes(
        bytes[sample_offset + 4..sample_offset + 8]
            .try_into()
            .unwrap(),
    );
    assert!(left.abs() < 0.0001);
    assert!(
        right.abs() > 0.0001,
        "late right-channel echo survives the long quiet gap"
    );
}

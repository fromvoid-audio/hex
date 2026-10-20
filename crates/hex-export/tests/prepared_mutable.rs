#![cfg(feature = "instrument-test-support")]

use hex_engine::{
    Audio, LaunchQuantization, NativeDeviceClass, PreparedRevision, SessionPlayback,
    SessionPlaybackCommand, SessionPlaybackError, default_native_device_state_for_class,
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
struct FixturePrepared;
struct FixtureFactory;

#[derive(Clone, Copy, Default)]
struct Voice {
    id: u64,
    active: bool,
    gated: bool,
    release_left: u8,
}

struct Runtime {
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
        if state.type_id() == FIXTURE_TYPE && state.state_version() == 1 && state.payload() == [1] {
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
        Ok(Box::new(FixturePrepared))
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
        Ok(Some(Box::new(FixtureFactory)))
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
        3
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime {
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
                            voice.release_left = 3;
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
                    LEVEL * (f32::from(voice.release_left) / 4.0)
                } else {
                    voice.active = false;
                    return sum;
                };
                if !voice.gated {
                    voice.release_left -= 1;
                }
                sum + level
            });
            frame[0] += signal;
            frame[1] += signal;
        }
    }
    fn reset(&mut self) {
        self.voices.fill(Voice::default());
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|voice| !voice.active)
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
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 8).expect("editor");
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
            state: mutable_state(),
        })
        .expect("mutable instrument");
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
    let audio = Audio::new(RATE, audio_frames).expect("audio");
    let imported = editor
        .execute(Command::ImportPreparedAudioClip {
            track,
            start: 0,
            path: PathBuf::from("/fixture/hybrid.wav"),
            source_sample_rate: RATE,
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
                track,
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
        hex_engine::SharedSineTable::new_on_control(),
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

fn quantize(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
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
        assert_eq!(
            frame,
            [
                quantize(index as f32 / 2048.0 + mutable * 0.5),
                quantize(-(index as f32) / 4096.0 + mutable * 0.5),
            ]
        );
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
    assert_eq!(samples[150 * 2], quantize(150.0 / 2048.0 + LEVEL * 0.5));

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
        PreparedRevision::prepare(
            &project,
            &BTreeMap::new(),
            hex_engine::SharedSineTable::new_on_control(),
            None,
        )
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

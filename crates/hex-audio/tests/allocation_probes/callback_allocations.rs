//! Instrumented headless callback allocation regression; no device is opened.

#[path = "callback_allocations/compressor.rs"]
mod compressor;
#[path = "callback_allocations/cursor_loop.rs"]
mod cursor_loop;
#[path = "callback_allocations/finite_bounce.rs"]
mod finite_bounce;
#[path = "callback_allocations/gate.rs"]
mod gate;
#[path = "callback_allocations/output_capture.rs"]
mod output_capture;
#[path = "callback_allocations/overdrive.rs"]
mod overdrive;
#[path = "callback_allocations/probe.rs"]
pub(super) mod probe;
#[path = "callback_allocations/project_tuning.rs"]
mod project_tuning;
#[path = "callback_allocations/reverb_outputs.rs"]
mod reverb_outputs;
#[path = "../../../hex-engine/examples/support/session_audio_fixture.rs"]
#[allow(dead_code)]
mod session_audio_fixture;
#[path = "callback_allocations/transport_fade.rs"]
mod transport_fade;
use probe::{compare_output, measured_block, measured_shutdown};

use allocation_counter::{AllocationInfo, measure};
use hex_audio::prepared::{self, PublicationPolicy, SessionRouteOwner};
use hex_engine::{
    Audio, Frame, LaunchQuantization, PreparedRevision, Renderer, SessionPlaybackCommand,
    SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState, ParameterId,
    ParameterValue,
};
use hex_project::{
    AssetId, BeatTime, Command, Created, Editor, Project, SceneId, Tempo, TrackContentSnapshot,
    TrackId,
};
use session_audio_fixture as fixture;
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::ThreadId,
};

const RATE: u32 = fixture::RATE;
const BLOCK: usize = 1_024;
type Candidate = (Arc<PreparedRevision>, Project);

const MUTABLE_FIXTURE_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.audio.mutable");
const SHARED_CONTEXT_FIXTURE_TYPE: DeviceTypeId =
    DeviceTypeId::from_static("test.audio.shared-context");

struct SharedContextDropProbe {
    control_thread: ThreadId,
    old_context_drops: AtomicUsize,
    wrong_thread_drop: AtomicBool,
}

struct SharedContextFixtureDefinition(Arc<SharedContextDropProbe>);
struct SharedContextFixturePrepared {
    level: f32,
    probe: Arc<SharedContextDropProbe>,
}
struct SharedContextFixture {
    level: f32,
    probe: Arc<SharedContextDropProbe>,
}

impl Drop for SharedContextFixture {
    fn drop(&mut self) {
        if std::thread::current().id() != self.probe.control_thread {
            self.probe.wrong_thread_drop.store(true, Ordering::Relaxed);
        }
        if self.level == 0.25 {
            self.probe.old_context_drops.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl NativeDeviceMetadata for SharedContextFixtureDefinition {
    fn type_id(&self) -> DeviceTypeId {
        SHARED_CONTEXT_FIXTURE_TYPE
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("shared context callback fixture")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [hex_lattice_contract::ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(SHARED_CONTEXT_FIXTURE_TYPE, 1, vec![1])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() == SHARED_CONTEXT_FIXTURE_TYPE
            && state.state_version() == 1
            && matches!(state.payload(), [1..=4])
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

impl hex_lattice::NativeInstrumentDefinition for SharedContextFixtureDefinition {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        _context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn hex_lattice::PreparedNativeInstrument>, DeviceContractError> {
        let state = self.migrate_state(state)?;
        Ok(Box::new(SharedContextFixturePrepared {
            level: match state.payload()[0] {
                1 => 0.25,
                2 => 0.5,
                3 => 0.75,
                _ => 1.0,
            },
            probe: Arc::clone(&self.0),
        }))
    }
}

impl hex_lattice::PreparedNativeInstrument for SharedContextFixturePrepared {
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
    ) -> Result<Option<Box<dyn hex_lattice::PreparedSharedAnalyticContext>>, DeviceContractError>
    {
        Ok(Some(Box::new(SharedContextFixture {
            level: self.level,
            probe: Arc::clone(&self.probe),
        })))
    }
}

impl hex_lattice::PreparedSharedAnalyticContext for SharedContextFixture {
    fn declared_context_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + std::mem::size_of::<SharedContextDropProbe>()
    }
    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedSharedAnalyticNote, DeviceContractError> {
        let mut payload = [0; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[0] = u64::from(self.level.to_bits());
        Ok(hex_lattice::PreparedSharedAnalyticNote::new(payload, 8))
    }
    fn sample(
        &self,
        note: &hex_lattice::PreparedSharedAnalyticNote,
        age: u64,
        gate: u64,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Frame {
        if age < gate.saturating_add(note.release_frames()) {
            [f32::from_bits(note.payload()[0] as u32); 2]
        } else {
            [0.0; 2]
        }
    }
}

struct MutableFixtureDefinition;
struct MutableFixturePrepared {
    level: f32,
}
struct MutableFixtureFactory {
    level: f32,
}
#[derive(Clone, Copy, Default)]
struct MutableFixtureVoice {
    id: u64,
    active: bool,
    gated: bool,
    release: u8,
}
struct MutableFixtureRuntime {
    key: u32,
    level: f32,
    voices: [MutableFixtureVoice; 64],
    // Real inline state above the former 4 KiB instrument cap. All mutable
    // publication/render/rollback/tail probes below exercise its full copy.
    history: [u8; 8192],
}

impl hex_lattice_contract::NativeDeviceMetadata for MutableFixtureDefinition {
    fn type_id(&self) -> DeviceTypeId {
        MUTABLE_FIXTURE_TYPE
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("mutable callback fixture")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [hex_lattice_contract::ParameterDescriptor] {
        &[]
    }
    fn default_state(
        &self,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        hex_lattice_contract::NativeDeviceState::new(MUTABLE_FIXTURE_TYPE, 1, vec![1])
    }
    fn migrate_state(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        if state.type_id() == MUTABLE_FIXTURE_TYPE
            && state.state_version() == 1
            && state.payload().len() == 1
            && (1..=2).contains(&state.payload()[0])
        {
            Ok(state.clone())
        } else {
            Err(hex_lattice_contract::DeviceContractError::MalformedState)
        }
    }
    fn read_parameter(
        &self,
        _: &hex_lattice_contract::NativeDeviceState,
        _: ParameterId,
    ) -> Result<hex_lattice_contract::ParameterValue, hex_lattice_contract::DeviceContractError>
    {
        Err(hex_lattice_contract::DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _: &hex_lattice_contract::NativeDeviceState,
        _: ParameterId,
        _: hex_lattice_contract::ParameterValue,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        Err(hex_lattice_contract::DeviceContractError::UnknownParameter)
    }
}

impl hex_lattice::NativeInstrumentDefinition for MutableFixtureDefinition {
    fn prepare_instrument(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
        _context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<
        Box<dyn hex_lattice::PreparedNativeInstrument>,
        hex_lattice_contract::DeviceContractError,
    > {
        let state = self.migrate_state(state)?;
        Ok(Box::new(MutableFixturePrepared {
            level: f32::from(state.payload()[0]) * 0.0001,
        }))
    }
}

impl hex_lattice::PreparedNativeInstrument for MutableFixturePrepared {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<
        Option<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>>,
        hex_lattice_contract::DeviceContractError,
    > {
        Ok(Some(Box::new(MutableFixtureFactory { level: self.level })))
    }
}

impl hex_lattice::PreparedMutableInstrumentRuntime for MutableFixtureFactory {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<MutableFixtureRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        256
    }
    fn max_active_occurrences(&self) -> usize {
        64
    }
    fn max_release_tail_frames(&self) -> u64 {
        8
    }
    fn create_runtime(&self) -> Box<dyn hex_lattice::MutableInstrumentRuntime> {
        Box::new(MutableFixtureRuntime {
            key: self.level.to_bits(),
            level: self.level,
            voices: [MutableFixtureVoice::default(); 64],
            history: [0; 8192],
        })
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl hex_lattice::MutableInstrumentRuntime for MutableFixtureRuntime {
    fn preflight_events(
        &self,
        events: &[hex_lattice::NativeInstrumentEvent],
        frame_count: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), hex_lattice::NativeInstrumentRuntimeError> {
        if output_capture::refuse_gateoff(events) {
            return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        for event in events {
            let (offset, id, on) = match *event {
                hex_lattice::NativeInstrumentEvent::Expression { .. } => {
                    return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                hex_lattice::NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    ..
                } => (frame_offset, occurrence_id, true),
                hex_lattice::NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id,
                } => (frame_offset, occurrence_id, false),
            };
            if offset >= frame_count
                || (on
                    && self
                        .voices
                        .iter()
                        .any(|voice| voice.active && voice.id == id))
                || (!on
                    && !self
                        .voices
                        .iter()
                        .any(|voice| voice.active && voice.gated && voice.id == id))
            {
                return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
            }
        }
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[hex_lattice::NativeInstrumentEvent],
        frames: &mut [[f32; 2]],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        let mut event_index = 0;
        for (frame_index, frame) in frames.iter_mut().enumerate() {
            while events
                .get(event_index)
                .is_some_and(|event| event.frame_offset() == frame_index)
            {
                match events[event_index] {
                    // Unsupported by this fixture's read-only preflight.
                    hex_lattice::NativeInstrumentEvent::Expression { .. } => {}
                    hex_lattice::NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                        if let Some(voice) = self.voices.iter_mut().find(|voice| !voice.active) {
                            *voice = MutableFixtureVoice {
                                id: occurrence_id,
                                active: true,
                                gated: true,
                                release: 8,
                            };
                        }
                    }
                    hex_lattice::NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        if let Some(voice) = self
                            .voices
                            .iter_mut()
                            .find(|voice| voice.active && voice.id == occurrence_id)
                        {
                            voice.gated = false;
                        }
                    }
                }
                event_index += 1;
            }
            let active = self.voices.iter().any(|voice| voice.active);
            if active {
                frame[0] += self.level;
                frame[1] += self.level;
            }
            for voice in &mut self.voices {
                if voice.active && !voice.gated {
                    voice.release = voice.release.saturating_sub(1);
                    if voice.release == 0 {
                        *voice = MutableFixtureVoice::default();
                    }
                }
            }
        }
    }
    fn reset(&mut self) {
        self.voices = [MutableFixtureVoice::default(); 64];
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|voice| !voice.active)
    }
    fn can_copy_history_from(&self, previous: &dyn hex_lattice::MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| self.key == old.key)
    }
    fn copy_history_from(&mut self, previous: &dyn hex_lattice::MutableInstrumentRuntime) {
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.voices = old.voices;
            self.history = old.history;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[test]
fn reused_candidate_rejection_is_allocation_free_and_keeps_the_active_graph()
-> Result<(), Box<dyn Error>> {
    let project = Project::new(RATE)?;
    let revision = Arc::new(PreparedRevision::prepare(
        &project,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    let active_revision = control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(
            &revision,
        ))?)
        .map_err(|_| "active publication failed")?;
    let mut output = [[0.0; 2]; BLOCK];
    measured_block(&mut callback, &mut output);
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        active_revision
    );

    let mut used_candidate = Playback::try_from_prepared_with_session(revision)?;
    used_candidate.apply(PlaybackCommand::Play);
    let rejected_revision = control
        .publish(used_candidate)
        .map_err(|_| "used candidate publication failed")?;
    measured_block(&mut callback, &mut output);

    assert_eq!(
        control.status.rejected_revision(),
        Some((
            rejected_revision,
            hex_engine::PlaybackHandoffRejection::SessionRuntime
        ))
    );
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        active_revision,
        "the previously accepted graph remains active"
    );
    assert_eq!(
        control.reclaim(),
        1,
        "the rejected candidate returns to control"
    );
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn conditional_fresh_runtime_publication_paths_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    let project = Project::new(RATE)?;
    let revision = Arc::new(PreparedRevision::prepare(
        &project,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let playback = || {
        Playback::try_from_prepared_with_session(Arc::clone(&revision))
            .map_err(|error| -> Box<dyn Error> { Box::new(error) })
    };
    let mut output = [[0.0; 2]; BLOCK];

    // Exact match, including the empty audio owner revision-zero case.
    let (mut control, mut callback) = prepared::channel(RATE)?;
    let fence = control
        .capture_fresh_runtime_fence()
        .ok_or("empty route snapshot unavailable")?;
    let accepted = control
        .publish_with_policy(
            playback()?,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .map_err(|_| "conditional fresh publication failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        accepted
    );
    assert_eq!(control.status.rejected_revision(), None);
    assert_eq!(control.reclaim(), 0);

    // A command admitted after capture permanently rejects the candidate.
    let stale = control
        .capture_fresh_runtime_fence()
        .ok_or("active route snapshot unavailable")?;
    control
        .command_with_ticket(PlaybackCommand::Seek(29))
        .map_err(|_| "stale-fence command admission failed")?;
    let rejected = control
        .publish_with_policy(
            playback()?,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .map_err(|_| "stale conditional fresh publication failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        accepted
    );
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            rejected,
            hex_engine::PlaybackHandoffRejection::RuntimeFenceChanged
        ))
    );
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    measured_block(&mut callback, &mut output);
    assert_eq!(control.reclaim(), 1);

    // Retirement backpressure leaves the candidate queued; the same stale
    // ticket is rechecked once the control owner frees a slot.
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback()?)
        .map_err(|_| "backpressure fixture publication failed")?;
    measured_block(&mut callback, &mut output);
    for _ in 0..2 {
        control
            .publish_with_policy(playback()?, PublicationPolicy::InstallFreshRuntime)
            .map_err(|_| "backpressure fill publication failed")?;
        measured_block(&mut callback, &mut output);
    }
    let stale = control
        .capture_fresh_runtime_fence()
        .ok_or("backpressure route snapshot unavailable")?;
    let rejected = control
        .publish_with_policy(
            playback()?,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .map_err(|_| "backpressured conditional publication failed")?;
    control
        .command_with_ticket(PlaybackCommand::Seek(31))
        .map_err(|_| "backpressure stale command admission failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(control.reclaim(), 2);
    measured_block(&mut callback, &mut output);
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            rejected,
            hex_engine::PlaybackHandoffRejection::RuntimeFenceChanged
        ))
    );
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    measured_block(&mut callback, &mut output);
    assert_eq!(control.reclaim(), 1);
    Ok(())
}

fn assert_session_owner(
    control: &prepared::Control,
    routes: &mut [prepared::SessionRouteEntry],
    track_count: usize,
    clip: u64,
) {
    let snapshot = control
        .status
        .session_routes(routes)
        .expect("route snapshot is available");
    assert_eq!(snapshot.count, track_count);
    assert_eq!(
        routes[0].owner,
        SessionRouteOwner::Session { clip_id: clip }
    );
}

fn launch_scene(control: &mut prepared::Control, scene: SceneId) -> Result<(), &'static str> {
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .map(|_| ())
        .map_err(|_| "scene launch admission failed")
}

fn prepare_fixture(
    workload: &fixture::Fixture,
) -> Result<Arc<PreparedRevision>, hex_engine::Invalid> {
    Ok(Arc::new(PreparedRevision::prepare(
        &workload.project,
        &workload.media,
        Arc::new(SineTable::new()),
        None,
    )?))
}

fn assert_scene_audio_oracle(
    output: &[Frame],
    workload: &fixture::Fixture,
    scene: SceneId,
    elapsed: u64,
) -> Result<(), Box<dyn Error>> {
    let expected = fixture::finite_session_oracle(
        &workload.project,
        &workload.media,
        scene,
        elapsed,
        output.len(),
    )?;
    compare_output(output, &expected);
    Ok(())
}

fn assert_project_audio_oracle(
    actual: &[Frame],
    project: &Project,
    media: &BTreeMap<AssetId, Audio>,
    scene: SceneId,
    elapsed: u64,
) -> Result<(), Box<dyn Error>> {
    let expected = fixture::finite_session_oracle(project, media, scene, elapsed, actual.len())?;
    compare_output(actual, &expected);
    Ok(())
}

#[test]
fn injected_mutable_instrument_process_handoff_and_retirement_are_allocation_free()
-> Result<(), Box<dyn Error>> {
    let definition = MutableFixtureDefinition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions)?;
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "mutable".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected MIDI track".into()),
    };
    let scene = match editor.execute(Command::AddScene { name: "a".into() })? {
        Created::Scene(scene) => scene,
        _ => return Err("expected Session scene".into()),
    };
    let clip = match editor.execute(Command::CreateSessionMidiClip {
        track,
        scene,
        name: "source".into(),
        source_length: BeatTime::from_quarters(4)?,
    })? {
        Created::SessionClip(clip) => clip,
        _ => return Err("expected MIDI clip".into()),
    };
    let _note = match editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(1)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })? {
        Created::Note(note) => note,
        _ => return Err("expected created MIDI note".into()),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected default instrument slot")?
        .id();
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: hex_lattice_contract::NativeDeviceState::new(MUTABLE_FIXTURE_TYPE, 1, vec![1])?,
    })?;
    let first_project = editor.project().clone();
    let media = BTreeMap::new();
    let table = Arc::new(SineTable::new());
    let first = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        &first_project,
        &media,
        Arc::clone(&table),
        None,
        &registry,
    )?);
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&first))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "mutable scene launch failed")?;
    let mut output = [[0.0; 2]; 128];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| *sample > 0.0));

    let original_source = editor
        .project()
        .track(track)
        .and_then(|track| track.source_id())
        .ok_or("source missing")?;
    // A provider without typed state retarget cannot replace its active kernel
    // in place. Explicit device replacement creates a distinct retained source
    // and exercises the finite historical-tail ownership transfer instead.
    editor.execute(Command::RemoveInstrument { track })?;
    editor.execute(Command::InsertInstrument {
        track,
        state: hex_lattice_contract::NativeDeviceState::new(MUTABLE_FIXTURE_TYPE, 1, vec![2])?,
    })?;
    assert_ne!(
        editor
            .project()
            .track(track)
            .and_then(|track| track.source_id()),
        Some(original_source)
    );
    assert_eq!(
        editor
            .project()
            .native_source(original_source)
            .ok_or("retained source missing")?
            .instrument()
            .id(),
        instrument
    );
    let second = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        editor.project(),
        &media,
        Arc::clone(&table),
        Some(first.session()),
        &registry,
    )?);
    let replacement = Playback::try_from_prepared_with_session(Arc::clone(&second))?;
    control
        .publish(replacement)
        .map_err(|_| "stateful handoff publication failed")?;
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| *sample > 0.0));

    // The old source's scoped NoteOff drains its declared finite tail. The
    // replacement must not replay the pre-cut authored occurrence. Continue
    // beyond its original one-quarter gate too, checking terminal cleanup.
    for _ in 0..190 {
        measured_block(&mut callback, &mut output);
    }
    assert!(output.iter().flatten().all(|sample| *sample == 0.0));
    assert!(control.status.rejected_revision().is_none());

    let final_playback = Playback::try_from_prepared_with_session(Arc::clone(&first))?;
    control
        .publish(final_playback)
        .map_err(|_| "retirement publication failed")?;
    measured_block(&mut callback, &mut output);
    let mut reclaimed = 0;
    let reclaimed_allocations = measure(|| reclaimed = control.reclaim());
    assert!(reclaimed > 0, "retired graphs should reclaim on control");
    assert_eq!(reclaimed_allocations.count_total, 0);
    assert!(reclaimed_allocations.count_current < 0);
    assert!(reclaimed_allocations.bytes_current < 0);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn shared_analytic_context_release_survives_handoff_and_drops_on_control()
-> Result<(), Box<dyn Error>> {
    let probe = Arc::new(SharedContextDropProbe {
        control_thread: std::thread::current().id(),
        old_context_drops: AtomicUsize::new(0),
        wrong_thread_drop: AtomicBool::new(false),
    });
    let definition = SharedContextFixtureDefinition(Arc::clone(&probe));
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions)?;
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "shared context".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected MIDI track".into()),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected default instrument slot")?
        .id();
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: NativeDeviceState::new(SHARED_CONTEXT_FIXTURE_TYPE, 1, vec![1])?,
    })?;
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected Arrangement MIDI clip".into()),
    };
    let _note = match editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(1)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })? {
        Created::Note(note) => note,
        _ => return Err("expected Arrangement note".into()),
    };

    let media = BTreeMap::new();
    let table = Arc::new(SineTable::new());
    let mut first = Some(Arc::new(
        PreparedRevision::prepare_with_test_instrument_registry(
            editor.project(),
            &media,
            Arc::clone(&table),
            None,
            &registry,
        )?,
    ));
    let oracle_probe = Arc::new(SharedContextDropProbe {
        control_thread: std::thread::current().id(),
        old_context_drops: AtomicUsize::new(0),
        wrong_thread_drop: AtomicBool::new(false),
    });
    let oracle_definition = SharedContextFixtureDefinition(Arc::clone(&oracle_probe));
    let oracle_definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] =
        [&oracle_definition];
    let oracle_registry = hex_lattice::NativeInstrumentRegistry::new(&oracle_definitions)?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::new(
        PreparedRevision::prepare_with_test_instrument_registry(
            editor.project(),
            &media,
            Arc::clone(&table),
            None,
            &oracle_registry,
        )?,
    ))?;
    oracle.apply(PlaybackCommand::Play);

    let playback = Playback::try_from_prepared_with_session(Arc::clone(
        first
            .as_ref()
            .ok_or("initial shared-context revision disappeared")?,
    ))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial shared-context publication failed")?;
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| *sample == 0.25));
    render_partitioned_shared_oracle(&mut oracle, &mut expected)?;
    compare_output(&output, &expected);

    let mut previous = Arc::clone(
        first
            .as_ref()
            .ok_or("initial shared-context revision disappeared")?,
    );
    for state in [2, 3, 4] {
        editor.execute(Command::ReplaceDeviceState {
            device: instrument,
            state: NativeDeviceState::new(SHARED_CONTEXT_FIXTURE_TYPE, 1, vec![state])?,
        })?;
        let candidate = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
            editor.project(),
            &media,
            Arc::clone(&table),
            Some(previous.session()),
            &registry,
        )?);
        let replacement = Playback::try_from_prepared_with_session(Arc::clone(&candidate))?;
        control
            .publish(replacement)
            .map_err(|_| "shared-context handoff publication failed")?;
        measured_block(&mut callback, &mut output);
        render_partitioned_shared_oracle(&mut oracle, &mut expected)?;
        compare_output(&output, &expected);
        assert!(output.iter().flatten().any(|sample| *sample == 0.25));
        drop(previous);
        previous = candidate;
        control.reclaim();
        if state == 2 {
            drop(first.take());
            control.reclaim();
        }
        assert_eq!(
            probe.old_context_drops.load(Ordering::Relaxed),
            0,
            "the carried release lease keeps the original context alive"
        );
    }

    // The active candidate now has no current shared context. Its copied old
    // release must still resolve through retained-context provenance.
    editor.execute(Command::RemoveInstrument { track })?;
    let silent_slot = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        editor.project(),
        &media,
        Arc::clone(&table),
        Some(previous.session()),
        &registry,
    )?);
    assert!(
        editor
            .project()
            .track(track)
            .is_some_and(|track| track.instrument().is_none())
    );
    let replacement = Playback::try_from_prepared_with_session(Arc::clone(&silent_slot))?;
    control
        .publish(replacement)
        .map_err(|_| "shared-context removal handoff failed")?;
    measured_block(&mut callback, &mut output);
    render_partitioned_shared_oracle(&mut oracle, &mut expected)?;
    compare_output(&output, &expected);
    assert!(output.iter().flatten().any(|sample| *sample == 0.25));
    drop(previous);
    control.reclaim();
    assert_eq!(probe.old_context_drops.load(Ordering::Relaxed), 0);

    // Finish the original one-quarter note and its eight-frame release. Its
    // old law remains exact across the callback's fixed blocks and the oracle's
    // irregular partitions.
    for _ in 0..190 {
        measured_block(&mut callback, &mut output);
        render_partitioned_shared_oracle(&mut oracle, &mut expected)?;
        compare_output(&output, &expected);
    }
    assert!(output.iter().flatten().all(|sample| *sample == 0.0));
    assert_eq!(probe.old_context_drops.load(Ordering::Relaxed), 0);

    let final_playback = Playback::try_from_prepared_with_session(Arc::clone(&silent_slot))?;
    control
        .publish(final_playback)
        .map_err(|_| "shared-context lease retirement publication failed")?;
    measured_block(&mut callback, &mut output);
    drop(silent_slot);
    drop(oracle);
    control.reclaim();
    assert_eq!(
        probe.old_context_drops.load(Ordering::Relaxed),
        1,
        "the final old-context owner is reclaimed on the control thread"
    );
    assert!(!probe.wrong_thread_drop.load(Ordering::Relaxed));

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

fn render_partitioned_shared_oracle(
    playback: &mut Playback,
    output: &mut [Frame; 128],
) -> Result<(), Box<dyn Error>> {
    let mut offset = 0;
    for count in [17, 31, 80] {
        let end = offset + count;
        playback.render(&mut output[offset..end])?;
        offset = end;
    }
    Ok(())
}

#[test]
fn unchanged_mutable_arrangement_handoff_and_chase_restart_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    let definition = MutableFixtureDefinition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions)?;
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "mutable arrangement".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected MIDI track".into()),
    };
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected Arrangement MIDI clip".into()),
    };
    let note = match editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })? {
        Created::Note(note) => note,
        _ => return Err("expected Arrangement MIDI note".into()),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected instrument slot")?
        .id();
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: hex_lattice_contract::NativeDeviceState::new(MUTABLE_FIXTURE_TYPE, 1, vec![1])?,
    })?;
    let initial_project = editor.project().clone();
    let media = BTreeMap::new();
    let table = Arc::new(SineTable::new());
    let initial_revision = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        &initial_project,
        &media,
        Arc::clone(&table),
        None,
        &registry,
    )?);
    let initial_playback = Playback::try_from_prepared_with_session(Arc::clone(&initial_revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(initial_playback)
        .map_err(|_| "initial mutable Arrangement publication failed")?;
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 128];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| *sample > 0.0));

    // Renaming leaves the exact MIDI source, instrument, topology and timing
    // unchanged, so callback handoff copies the live mutable pair and ledger.
    editor.execute(Command::RenameTrack {
        track,
        name: "renamed mutable Arrangement".into(),
    })?;
    let renamed_revision = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        editor.project(),
        &media,
        Arc::clone(&table),
        Some(initial_revision.session()),
        &registry,
    )?);
    let renamed_playback = Playback::try_from_prepared_with_session(Arc::clone(&renamed_revision))?;
    control
        .publish(renamed_playback)
        .map_err(|_| "unchanged mutable handoff publication failed")?;
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| *sample > 0.0));
    assert!(control.status.rejected_revision().is_none());

    let mut reclaimed = 0;
    let retirement = measure(|| reclaimed = control.reclaim());
    assert!(reclaimed > 0, "the replaced graph retires on control");
    assert_eq!(
        retirement.count_total, 0,
        "control retirement allocates nothing"
    );
    assert!(
        retirement.count_current < 0,
        "retirement drops old runtime state on control"
    );
    assert!(retirement.bytes_current < 0);

    // A source edit changes the exact descriptor while the old occurrence is
    // still held. The callback cannot prove the chase, so it restarts audio at
    // the cursor (Pulse #3770) instead of rejecting, still without allocation.
    editor.execute(Command::UpdateMidiNote {
        note,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 61,
        channel: 0,
        velocity: 100,
    })?;
    let changed_revision = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        editor.project(),
        &media,
        Arc::clone(&table),
        Some(renamed_revision.session()),
        &registry,
    )?);
    let changed_playback = Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    control
        .publish(changed_playback)
        .map_err(|_| "changed-source candidate publication failed")?;
    measured_block(&mut callback, &mut output);
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| *sample > 0.0));
    assert!(control.status.rejected_revision().is_none());
    let restarted = measure(|| reclaimed = control.reclaim());
    assert!(
        reclaimed > 0,
        "the replaced graph returns for control retirement"
    );
    assert_eq!(restarted.count_total, 0);
    assert!(restarted.count_current < 0);
    assert!(restarted.bytes_current < 0);

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn disabling_held_midi_note_handoff_allocates_and_frees_nothing_in_callback()
-> Result<(), Box<dyn Error>> {
    let definition = MutableFixtureDefinition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions)?;
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "notes".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
    })?
    else {
        return Err("expected track".into());
    };
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })?
    else {
        return Err("expected clip".into());
    };
    let Created::Note(note) = editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?
    else {
        return Err("expected note".into());
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected instrument")?
        .id();
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: NativeDeviceState::new(MUTABLE_FIXTURE_TYPE, 1, vec![1])?,
    })?;
    let media = BTreeMap::new();
    let table = Arc::new(SineTable::new());
    let initial = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        editor.project(),
        &media,
        Arc::clone(&table),
        None,
        &registry,
    )?);
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&initial))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial publication failed")?;
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 128];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| *sample > 0.0));
    editor.execute(Command::SetMidiNotesActive {
        clip,
        notes: vec![note],
        active: false,
    })?;
    let muted = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        editor.project(),
        &media,
        table,
        Some(initial.session()),
        &registry,
    )?);
    let playback = Playback::try_from_prepared_with_session(muted)?;
    control
        .publish(playback)
        .map_err(|_| "note activation publication failed")?;
    measured_block(&mut callback, &mut output);
    assert!(control.status.rejected_revision().is_none());
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().all(|sample| *sample == 0.0));
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn hybrid_arrangement_audio_and_midi_callback_is_allocation_free_and_exact()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "hybrid".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected hybrid track".into()),
    };
    let asset = match editor.execute(Command::AddAsset {
        path: "memory/hybrid.wav".into(),
        sample_rate: RATE,
        frames: BLOCK as u64,
    })? {
        Created::Asset(asset) => asset,
        _ => return Err("expected audio asset".into()),
    };
    editor.execute(Command::PlaceClip {
        track,
        asset,
        start: BLOCK as u64,
        duration: BLOCK as u64,
        source_offset: 0,
        source_length: BLOCK as u64,
        looping: false,
    })?;
    let midi_clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: editor.project().tempo_map().frame_to_beat(BLOCK as u64)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected arrangement midi clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip: midi_clip,
        start: BeatTime::ZERO,
        duration: editor.project().tempo_map().frame_to_beat(BLOCK as u64)?,
        pitch: 64,
        channel: 0,
        velocity: 100,
    })?;
    editor.execute(Command::Gain { track, gain: 0.5 })?;
    editor.execute(Command::Balance {
        track,
        balance: -0.25,
    })?;
    let utility_type = DeviceTypeId::new("hex.utility")?;
    let utility_state = hex_engine::default_native_device_state(utility_type)?;
    let utility_state = hex_engine::patch_native_device_parameter(
        &utility_state,
        ParameterId::new(6)?,
        ParameterValue::Bool(true),
    )?;
    editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: utility_state,
    })?;
    let project = editor.project().clone();
    let media = BTreeMap::from([(
        asset,
        Audio::new(
            RATE,
            (0..BLOCK)
                .map(|frame| {
                    let sample = (frame % 31) as f32 / 64.0 - 0.2;
                    [sample, sample]
                })
                .collect(),
        )?,
    )]);
    let revision = Arc::new(PreparedRevision::prepare(
        &project,
        &media,
        Arc::new(SineTable::new()),
        None,
    )?);

    // Render each source independently without devices or track coefficients,
    // then sum at the device input. The independent original Utility inversion
    // and gain/balance equations are applied once after that sum, matching the
    // explicit host signal boundary rather than relying on f32 distributivity.
    let mut audio_project_snapshot = project.snapshot();
    let TrackContentSnapshot::Hybrid {
        audio_clips,
        midi_clips,
    } = &audio_project_snapshot.tracks[0].content
    else {
        return Err("expected hybrid snapshot".into());
    };
    let audio_clips = audio_clips.clone();
    let midi_clips = midi_clips.clone();
    audio_project_snapshot.tracks[0].content = TrackContentSnapshot::Hybrid {
        audio_clips,
        midi_clips: Vec::new(),
    };
    audio_project_snapshot.tracks[0].devices.clear();
    audio_project_snapshot.tracks[0].gain = 1.0;
    audio_project_snapshot.tracks[0].balance = 0.0;
    let audio_project = Project::from_snapshot(audio_project_snapshot)?;
    let mut midi_project_snapshot = project.snapshot();
    midi_project_snapshot.tracks[0].content = TrackContentSnapshot::Hybrid {
        audio_clips: Vec::new(),
        midi_clips,
    };
    midi_project_snapshot.tracks[0].devices.clear();
    midi_project_snapshot.tracks[0].gain = 1.0;
    midi_project_snapshot.tracks[0].balance = 0.0;
    let midi_project = Project::from_snapshot(midi_project_snapshot)?;
    let audio_revision =
        PreparedRevision::prepare(&audio_project, &media, Arc::new(SineTable::new()), None)?;
    let midi_revision =
        PreparedRevision::prepare(&midi_project, &media, Arc::new(SineTable::new()), None)?;
    let mut audio_oracle = [[0.0; 2]; BLOCK];
    let mut midi_oracle = [[0.0; 2]; BLOCK];
    let mut audio_renderer = Renderer::new(audio_revision.arrangement());
    audio_renderer.play();
    audio_renderer.render(&mut audio_oracle)?;
    audio_renderer.render(&mut audio_oracle)?;
    let mut midi_renderer = Renderer::new(midi_revision.arrangement());
    midi_renderer.play();
    midi_renderer.render(&mut midi_oracle)?;
    midi_renderer.render(&mut midi_oracle)?;
    assert!(
        audio_oracle
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6)
    );
    assert!(
        midi_oracle
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6)
    );
    let expected: Vec<Frame> = audio_oracle
        .iter()
        .zip(midi_oracle.iter())
        .map(|(audio, midi)| {
            let input = [audio[0] + midi[0], audio[1] + midi[1]];
            [input[0] * 0.5, -input[1] * 0.375]
        })
        .collect();

    let playback = Playback::try_from_prepared_with_session(revision)?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "Play command admission failed")?;
    let mut actual = [[0.0; 2]; BLOCK];
    measured_block(&mut callback, &mut actual);
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn reverb_automation_publication_and_render_are_allocation_free() -> Result<(), Box<dyn Error>> {
    use hex_devices::reverb::{REVERB_TYPE_ID, parameter};
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, DeviceAutomationTarget,
        DeviceAutomationTouchSpan, DeviceAutomationTouchStep,
    };
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported { track, asset, .. } =
        editor.execute(Command::ImportPreparedAudioTrack {
            name: "automated room".into(),
            path: "memory/automated-room.wav".into(),
            source_sample_rate: RATE,
            source_frames: (BLOCK * 3) as u64,
            playback_frames: (BLOCK * 3) as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })?
    else {
        return Err("expected imported audio".into());
    };
    let Created::Device(device) = editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state(REVERB_TYPE_ID)?,
    })?
    else {
        return Err("expected reverb device".into());
    };
    let lane = |value| -> Result<Command, hex_project::Invalid> {
        Ok(Command::ReplaceDeviceAutomation {
            device,
            parameter: parameter::DRY_WET,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(value)?,
                outgoing: AutomationInterpolation::Step,
            }],
        })
    };
    editor.execute(lane(0.0)?)?;
    let dry = [0.125, -0.0625];
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![dry; BLOCK * 3])?)]);
    let table = Arc::new(SineTable::new());
    let first = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::clone(&table),
        None,
    )?);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(
            &first,
        ))?)
        .map_err(|_| "initial playback publication failed")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "Play admission failed")?;
    let mut actual = [[0.0; 2]; BLOCK];
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &[dry; BLOCK]);

    editor.execute(lane(0.75)?)?;
    let map = editor.project().tempo_map();
    let start = map.frame_to_beat((BLOCK + 5) as u64)?;
    let end = map.frame_to_beat((BLOCK + 19) as u64)?;
    editor.execute(Command::SpliceDeviceAutomationTouch {
        vst3_class: None,
        device,
        target: DeviceAutomationTarget::Parameter(parameter::DRY_WET),
        spans: vec![DeviceAutomationTouchSpan {
            start,
            end,
            steps: vec![DeviceAutomationTouchStep {
                beat: start,
                value: AutomationValue::new(0.25)?,
            }],
        }],
    })?;
    let path =
        std::env::temp_dir().join(format!("hex-reverb-automation-{}.hex", std::process::id()));
    hex_storage::save_new(&path, editor.project())?;
    let reopened = hex_storage::load(&path);
    std::fs::remove_file(path)?;
    let reopened = reopened?;
    assert_eq!(editor.project(), &reopened);
    let second = Arc::new(PreparedRevision::prepare(
        &reopened,
        &media,
        table,
        Some(first.session()),
    )?);
    let mut oracle = Renderer::new(second.arrangement());
    oracle.play();
    let mut expected = [[0.0; 2]; BLOCK];
    oracle.render(&mut expected)?;
    oracle.render(&mut expected)?;
    control
        .publish(Playback::try_from_prepared_with_session(second)?)
        .map_err(|_| "automation publication failed")?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn auto_pan_prepared_callback_and_parameter_handoff_are_allocation_free()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let track = match editor.execute(Command::AddTrack {
        name: "auto pan allocation probe".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected audio track".into()),
    };
    let asset = match editor.execute(Command::AddAsset {
        path: "memory/auto-pan-callback.wav".into(),
        sample_rate: RATE,
        frames: (BLOCK * 3) as u64,
    })? {
        Created::Asset(asset) => asset,
        _ => return Err("expected audio asset".into()),
    };
    editor.execute(Command::PlaceClip {
        track,
        asset,
        start: 0,
        duration: (BLOCK * 3) as u64,
        source_offset: 0,
        source_length: (BLOCK * 3) as u64,
        looping: false,
    })?;
    let type_id = DeviceTypeId::from_static("hex.auto-pan");
    let state = hex_engine::default_native_device_state(type_id)?;
    let device = match editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: state.clone(),
    })? {
        Created::Device(device) => device,
        _ => return Err("expected Auto Pan device".into()),
    };
    let media = BTreeMap::from([(
        asset,
        Audio::new(
            RATE,
            (0..BLOCK * 3)
                .map(|frame| {
                    let sample = (frame % 47) as f32 / 64.0 - 0.25;
                    [sample, sample * -0.75]
                })
                .collect(),
        )?,
    )]);
    let first_revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::new(SineTable::new()),
        None,
    )?);
    let mut oracle = Renderer::new(first_revision.arrangement());
    oracle.play();
    let mut expected = [[0.0; 2]; BLOCK];
    oracle.render(&mut expected)?;

    let playback = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "Play command admission failed")?;
    let mut actual = [[0.0; 2]; BLOCK];
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);

    let changed = hex_engine::patch_native_device_parameter(
        &state,
        ParameterId::from_static(1),
        ParameterValue::Float(0.8),
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: changed,
    })?;
    let second_revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::new(SineTable::new()),
        Some(first_revision.session()),
    )?);
    let replacement = Playback::try_from_prepared_with_session(Arc::clone(&second_revision))?;
    control
        .publish(replacement)
        .map_err(|_| "edited playback publication failed")?;
    measured_block(&mut callback, &mut actual);
    assert!(control.status.rejected_revision().is_none());
    let ramp_frames = (f64::from(RATE) * 0.005).ceil() as usize;
    let amount_target = f64::from(0.8_f32);
    for (index, actual_frame) in actual.iter().enumerate() {
        let amount =
            0.5 + (amount_target - 0.5) * (index + 1).min(ramp_frames) as f64 / ramp_frames as f64;
        let phase = (BLOCK + index) as f64 / f64::from(RATE);
        let left_lfo = (1.0 + (std::f64::consts::TAU * phase).sin()) * 0.5;
        let right_lfo = (1.0 + (std::f64::consts::TAU * (phase + 0.5).rem_euclid(1.0)).sin()) * 0.5;
        let base_gain = 1.0 - amount;
        let source_index = BLOCK + index;
        let source = (source_index % 47) as f32 / 64.0 - 0.25;
        let expected = [
            (f64::from(source) * (base_gain + amount * left_lfo)) as f32,
            (f64::from(source * -0.75) * (base_gain + amount * right_lfo)) as f32,
        ];
        assert_eq!(*actual_frame, expected, "edited frame {index}");
    }
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 1.0e-6));

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn bypassed_limiter_and_delay_block_paths_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let track = match editor.execute(Command::AddTrack {
        name: "bypassed limiter delay allocation probe".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected audio track".into()),
    };
    let asset = match editor.execute(Command::AddAsset {
        path: "memory/bypassed-limiter-delay.wav".into(),
        sample_rate: RATE,
        frames: (BLOCK * 2) as u64,
    })? {
        Created::Asset(asset) => asset,
        _ => return Err("expected audio asset".into()),
    };
    editor.execute(Command::PlaceClip {
        track,
        asset,
        start: 0,
        duration: (BLOCK * 2) as u64,
        source_offset: 0,
        source_length: (BLOCK * 2) as u64,
        looping: false,
    })?;
    // BLOCK exceeds the Limiter's bounded bypass scratch, so the chunked path runs.
    for type_id in ["hex.limiter", "hex.delay"] {
        let state = hex_engine::default_native_device_state(DeviceTypeId::new(type_id)?)?;
        let Created::Device(device) = editor.execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })?
        else {
            return Err("expected native device".into());
        };
        editor.execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })?;
    }
    let media = BTreeMap::from([(
        asset,
        Audio::new(
            RATE,
            (0..BLOCK * 2)
                .map(|frame| {
                    let sample = (frame % 53) as f32 / 64.0 - 0.4;
                    [sample, sample * -0.5]
                })
                .collect(),
        )?,
    )]);
    let revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::new(SineTable::new()),
        None,
    )?);
    let mut oracle = Renderer::new(revision.arrangement());
    oracle.play();
    let mut expected = [[0.0; 2]; BLOCK];
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(
            &revision,
        ))?)
        .map_err(|_| "initial playback publication failed")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "Play command admission failed")?;
    let mut actual = [[0.0; 2]; BLOCK];
    for _ in 0..2 {
        oracle.render(&mut expected)?;
        measured_block(&mut callback, &mut actual);
        compare_output(&actual, &expected);
    }
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn play_from_callback_is_allocation_free_and_renders_from_requested_frame()
-> Result<(), Box<dyn Error>> {
    let workload = fixture::fixture(false)?;
    let revision = prepare_fixture(&workload)?;
    let playback = Playback::try_from_prepared_with_session(revision)?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    let start = 1_024;
    let ticket = control
        .command_with_ticket(PlaybackCommand::PlayFrom(start))
        .map_err(|_| "PlayFrom command admission failed")?;
    let mut actual = [[0.0; 2]; BLOCK];
    measured_block(&mut callback, &mut actual);
    let expected =
        fixture::finite_arrangement_oracle(&workload.project, &workload.media, start, BLOCK)?;
    compare_output(&actual, &expected);
    assert_eq!(
        control
            .status
            .consumed_command
            .load(std::sync::atomic::Ordering::Acquire),
        ticket
    );
    assert_eq!(
        control
            .status
            .position
            .load(std::sync::atomic::Ordering::Acquire),
        start + BLOCK as u64
    );

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn shared_and_distinct_session_audio_callback_blocks_are_allocation_free()
-> Result<(), Box<dyn Error>> {
    for distinct in [false, true] {
        let workload = fixture::fixture(distinct)?;
        let revision = prepare_fixture(&workload)?;
        let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
        let (mut control, mut callback) = prepared::channel(RATE)?;
        control
            .publish(playback)
            .map_err(|_| "initial playback publication failed")?;

        // Thirty pre-cut seeks plus a post-cut scene launch and Play fill the
        // bounded queue. The additional request is refused before measurement.
        for _ in 0..30 {
            control
                .command(PlaybackCommand::Seek(0))
                .map_err(|_| "queue full early")?;
        }
        control.stop();
        control
            .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
                scene: workload.scene_a,
                quantization: LaunchQuantization::Immediate,
            })
            .map_err(|_| "post-stop scene launch admission failed")?;
        control
            .command(PlaybackCommand::Play)
            .map_err(|_| "post-stop Play admission failed")?;
        assert!(control.command(PlaybackCommand::Pause).is_err());

        let mut output = [[0.0; 2]; BLOCK];
        measured_block(&mut callback, &mut output[..128]);
        assert_scene_audio_oracle(&output[..128], &workload, workload.scene_a, 0)?;

        let partitions = [1_usize, 127, 257, 511, 1_024, 1_024, 1_024];
        let mut elapsed = 128_u64;
        for frames in partitions {
            measured_block(&mut callback, &mut output[..frames]);
            assert_scene_audio_oracle(&output[..frames], &workload, workload.scene_a, elapsed)?;
            elapsed += u64::try_from(frames)?;
        }
        assert_eq!(elapsed, 4_096);

        measured_block(&mut callback, &mut output[..128]);
        assert!(output[..128].iter().all(|frame| *frame == [0.0; 2]));

        // A scene relaunch re-anchors every one-shot to its source start.
        launch_scene(&mut control, workload.scene_a)?;
        measured_block(&mut callback, &mut output[..257]);
        assert_scene_audio_oracle(&output[..257], &workload, workload.scene_a, 0)?;

        // Execute a full 32-action burst; the final alternating scene is B.
        for index in 0..32 {
            let scene = if index % 2 == 0 {
                workload.scene_a
            } else {
                workload.scene_b
            };
            control
                .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
                    scene,
                    quantization: LaunchQuantization::Immediate,
                })
                .map_err(|_| "32-action burst admission failed")?;
        }
        assert!(control.command(PlaybackCommand::Pause).is_err());
        measured_block(&mut callback, &mut output[..257]);
        // The scene A instance that had played 257 frames fades out over the
        // shared 128-frame declick; the burst's later same-frame launches
        // never sounded and leave no release.
        let incoming = fixture::finite_session_oracle(
            &workload.project,
            &workload.media,
            workload.scene_b,
            0,
            257,
        )?;
        let outgoing = fixture::finite_session_oracle(
            &workload.project,
            &workload.media,
            workload.scene_a,
            257,
            128,
        )?;
        for (offset, (actual, incoming)) in output[..257].iter().zip(&incoming).enumerate() {
            let gain = 128_usize.saturating_sub(offset) as f32 / 128.0;
            let released = outgoing.get(offset).copied().unwrap_or([0.0; 2]);
            for channel in 0..2 {
                let expected = incoming[channel] + released[channel] * gain;
                assert!(
                    (actual[channel] - expected).abs() <= 1e-5 * expected.abs().max(1.0),
                    "burst frame {offset}: {actual:?} != {expected}"
                );
            }
        }

        control.request_shutdown();
        measured_shutdown(&mut callback, &mut output[..128]);
        control.reclaim();
    }
    Ok(())
}

fn edited_candidates(workload: &fixture::Fixture) -> Result<Vec<Candidate>, Box<dyn Error>> {
    let table = Arc::new(SineTable::new());
    let initial = prepare_fixture(workload)?;
    let mut editor = Editor::new(workload.project.clone(), 8)?;
    let mut candidates = vec![(Arc::clone(&initial), workload.project.clone())];

    editor.execute(Command::Gain {
        track: workload.tracks[0],
        gain: 0.125,
    })?;
    let gain = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &workload.media,
        Arc::clone(&table),
        Some(initial.session()),
    )?);
    candidates.push((Arc::clone(&gain), editor.project().clone()));

    let clip = workload.scene_a_clips[0];
    let source = editor
        .project()
        .session_audio_clip(clip)
        .ok_or("missing Session audio clip")?;
    editor.execute(Command::SetSessionAudioClipSource {
        clip,
        asset: source.asset(),
        source_start: source.source_start() + 1,
        source_end: source.source_end(),
    })?;
    let source_revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &workload.media,
        Arc::clone(&table),
        Some(gain.session()),
    )?);
    candidates.push((Arc::clone(&source_revision), editor.project().clone()));

    editor.execute(Command::SetTempoPoint {
        beat: BeatTime::ZERO,
        tempo: Tempo::new(60_000_000)?,
    })?;
    let tempo = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &workload.media,
        table,
        Some(source_revision.session()),
    )?);
    candidates.push((tempo, editor.project().clone()));
    Ok(candidates)
}

#[test]
fn prepared_edits_inherit_through_a_full_retirement_ring_without_callback_allocation()
-> Result<(), Box<dyn Error>> {
    let workload = fixture::fixture(false)?;
    let candidates = edited_candidates(&workload)?;
    let initial = Playback::try_from_prepared_with_session(Arc::clone(&candidates[0].0))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(initial)
        .map_err(|_| "initial playback publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene: workload.scene_a,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "initial launch admission failed")?;

    let mut output = [[0.0; 2]; 128];
    let expected_initial = fixture::finite_session_oracle(
        &candidates[0].1,
        &workload.media,
        workload.scene_a,
        0,
        output.len(),
    )?;
    measured_block(&mut callback, &mut output);
    compare_output(&output, &expected_initial);
    assert_eq!(
        control
            .status
            .performance_frame
            .load(std::sync::atomic::Ordering::Acquire),
        128
    );
    let mut routes = [prepared::SessionRouteEntry::default(); prepared::SESSION_ROUTE_CAPACITY];
    let first_clip = workload.scene_a_clips[0].value();
    assert_session_owner(&control, &mut routes, workload.tracks.len(), first_clip);
    for (revision, (candidate, project)) in candidates.iter().enumerate().skip(1).take(2) {
        let playback = Playback::try_from_prepared_with_session(Arc::clone(candidate))?;
        assert_eq!(
            control
                .publish(playback)
                .map_err(|_| "candidate publication failed")?,
            revision as u64 + 1
        );
        measured_block(&mut callback, &mut output);
        // Only the first track's gain changes. Interpolate two independent
        // static project renders to account for its ongoing 5 ms ramp, including
        // the following source edit while that ramp is still in progress.
        let mut original_gain = Editor::new(project.clone(), 1)?;
        original_gain.execute(Command::Gain {
            track: workload.tracks[0],
            gain: workload.project.snapshot().tracks[0].gain,
        })?;
        let elapsed = 128 * revision as u64;
        let before = fixture::finite_session_oracle(
            original_gain.project(),
            &workload.media,
            workload.scene_a,
            elapsed,
            output.len(),
        )?;
        let after = fixture::finite_session_oracle(
            project,
            &workload.media,
            workload.scene_a,
            elapsed,
            output.len(),
        )?;
        let ramp_frames = RATE.div_ceil(200) as usize;
        let expected: Vec<Frame> = before
            .iter()
            .zip(&after)
            .enumerate()
            .map(|(offset, (a, b))| {
                let progress = ((revision - 1) * output.len() + offset + 1).min(ramp_frames) as f32
                    / ramp_frames as f32;
                [
                    a[0] + (b[0] - a[0]) * progress,
                    a[1] + (b[1] - a[1]) * progress,
                ]
            })
            .collect();
        for (frame, (actual, expected)) in output.iter().zip(&expected).enumerate() {
            for channel in 0..2 {
                assert!(
                    (actual[channel] - expected[channel]).abs() < 1e-5,
                    "frame {frame}, channel {channel}: {actual:?} != {expected:?}"
                );
            }
        }
        assert_eq!(
            control
                .status
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire),
            revision as u64 + 1
        );
        assert_eq!(
            control
                .status
                .performance_frame
                .load(std::sync::atomic::Ordering::Acquire),
            128 * (revision as u64 + 1)
        );
        assert_session_owner(&control, &mut routes, workload.tracks.len(), first_clip);
    }

    let pending = Playback::try_from_prepared_with_session(Arc::clone(&candidates[3].0))?;
    assert_eq!(
        control
            .publish(pending)
            .map_err(|_| "tempo publication failed")?,
        4
    );
    measured_block(&mut callback, &mut output);
    assert_project_audio_oracle(
        &output,
        &candidates[2].1,
        &workload.media,
        workload.scene_a,
        384,
    )?;
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        3
    );
    assert_eq!(
        control
            .status
            .performance_frame
            .load(std::sync::atomic::Ordering::Acquire),
        512
    );
    assert_session_owner(&control, &mut routes, workload.tracks.len(), first_clip);
    assert_eq!(control.reclaim(), 2);
    measured_block(&mut callback, &mut output);
    assert_project_audio_oracle(
        &output,
        &candidates[3].1,
        &workload.media,
        workload.scene_a,
        512,
    )?;
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        4
    );
    assert_eq!(
        control
            .status
            .performance_frame
            .load(std::sync::atomic::Ordering::Acquire),
        640
    );
    assert_session_owner(&control, &mut routes, workload.tracks.len(), first_clip);
    assert_eq!(control.reclaim(), 1);

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    // Avoid keeping candidate revisions alive beyond the callback assertions.
    drop(candidates);
    Ok(())
}

#[test]
fn fresh_project_and_shutdown_reclaim_graph_and_pcm_only_on_control() -> Result<(), Box<dyn Error>>
{
    let workload = fixture::fixture(false)?;
    let revision = prepare_fixture(&workload)?;
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    let weak_revision = Arc::downgrade(
        playback
            .prepared_revision()
            .ok_or("prepared revision missing from playback")?,
    );
    drop(revision);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene: workload.scene_a,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "initial launch admission failed")?;
    let mut output = [[0.0; 2]; 128];
    measured_block(&mut callback, &mut output);
    assert!(weak_revision.upgrade().is_some());

    let fresh_project = Project::new(RATE)?;
    let fresh_revision = Arc::new(PreparedRevision::prepare(
        &fresh_project,
        &BTreeMap::<AssetId, Audio>::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let fresh_playback = Playback::try_from_prepared_with_session(Arc::clone(&fresh_revision))?;
    let weak_fresh_revision = Arc::downgrade(&fresh_revision);
    drop(fresh_revision);
    control
        .publish_with_policy(fresh_playback, PublicationPolicy::InstallFreshRuntime)
        .map_err(|_| "fresh playback publication failed")?;
    drop(workload);
    measured_block(&mut callback, &mut output);
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
    assert!(weak_fresh_revision.upgrade().is_some());
    assert!(
        weak_revision.upgrade().is_some(),
        "retired graph stays alive before reclaim"
    );

    let mut reclaimed = 0;
    let released = measure(|| reclaimed = control.reclaim());
    assert_eq!(reclaimed, 1);
    assert_eq!(released.count_total, 0);
    assert!(released.count_current < 0);
    assert!(
        released.bytes_current <= -((fixture::SOURCE_FRAMES * std::mem::size_of::<Frame>()) as i64)
    );
    assert!(
        weak_revision.upgrade().is_none(),
        "old graph drops only after control reclaim"
    );

    let mut routes = [prepared::SessionRouteEntry::default(); prepared::SESSION_ROUTE_CAPACITY];
    let fresh_routes = control
        .status
        .session_routes(&mut routes)
        .ok_or("fresh route snapshot")?;
    assert_eq!(fresh_routes.count, 0);

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    assert!(weak_fresh_revision.upgrade().is_some());
    let mut reclaimed_fresh = 0;
    let fresh_release = measure(|| reclaimed_fresh = control.reclaim());
    assert_eq!(reclaimed_fresh, 1);
    assert_eq!(fresh_release.count_total, 0);
    assert!(fresh_release.count_current < 0);
    assert!(weak_fresh_revision.upgrade().is_none());
    Ok(())
}

#[test]
fn priority_stop_is_allocation_free_under_command_and_retirement_pressure()
-> Result<(), Box<dyn Error>> {
    let workload = fixture::fixture(false)?;
    let candidates = edited_candidates(&workload)?;
    for frames in [1, 17, 128] {
        let (mut control, mut callback) = prepared::channel(RATE)?;
        let initial = Playback::try_from_prepared_with_session(Arc::clone(&candidates[0].0))?;
        control
            .publish(initial)
            .map_err(|_| "initial playback publication failed")?;
        control
            .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
                scene: workload.scene_a,
                quantization: LaunchQuantization::Immediate,
            })
            .map_err(|_| "initial launch admission failed")?;

        let mut output = [[0.0; 2]; 128];
        measured_block(&mut callback, &mut output[..frames]);
        for candidate in candidates.iter().skip(1).take(2) {
            let playback = Playback::try_from_prepared_with_session(Arc::clone(&candidate.0))?;
            control
                .publish(playback)
                .map_err(|_| "retirement-filling publication failed")?;
            measured_block(&mut callback, &mut output[..frames]);
        }
        let pending = Playback::try_from_prepared_with_session(Arc::clone(&candidates[3].0))?;
        control
            .publish(pending)
            .map_err(|_| "pending candidate admission failed")?;
        let mut last_ticket = 0;
        for _ in 0..32 {
            last_ticket = control
                .command_with_ticket(PlaybackCommand::Play)
                .map_err(|_| "command queue filled early")?;
        }
        assert!(control.command(PlaybackCommand::Play).is_err());

        // Stop must suppress the entire full command prefix even when graph
        // replacement is held behind a full retirement ring.
        control.stop();
        measured_block(&mut callback, &mut output[..frames]);
        assert!(output[..frames].iter().all(|frame| *frame == [0.0; 2]));
        assert!(
            !control
                .status
                .playing
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert_eq!(
            control
                .status
                .consumed_command
                .load(std::sync::atomic::Ordering::Acquire),
            last_ticket
        );

        assert_eq!(control.reclaim(), 2);
        measured_block(&mut callback, &mut output[..frames]);
        assert!(output[..frames].iter().all(|frame| *frame == [0.0; 2]));
        control.request_shutdown();
        measured_shutdown(&mut callback, &mut output[..frames]);
        control.reclaim();
    }
    drop(candidates);
    drop(workload);
    Ok(())
}

fn midi_session_project() -> Result<(Project, TrackId, SceneId, SceneId), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "keys".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected instrument track".into()),
    };
    let scene_a = match editor.execute(Command::AddScene { name: "a".into() })? {
        Created::Scene(scene) => scene,
        _ => return Err("expected Session scene".into()),
    };
    let scene_b = match editor.execute(Command::AddScene { name: "b".into() })? {
        Created::Scene(scene) => scene,
        _ => return Err("expected Session scene".into()),
    };
    for (scene, name, pitch) in [(scene_a, "source a", 60), (scene_b, "source b", 67)] {
        let clip = match editor.execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: name.into(),
            source_length: BeatTime::from_quarters(4)?,
        })? {
            Created::SessionClip(clip) => clip,
            _ => return Err("expected Session MIDI clip".into()),
        };
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(2)?,
            pitch,
            channel: 0,
            velocity: 120,
        })?;
    }
    Ok((editor.project().clone(), track, scene_a, scene_b))
}

fn insert_stateful_effects_at(
    editor: &mut Editor,
    track: TrackId,
    instrument_position: usize,
) -> Result<(), Box<dyn Error>> {
    let utility_type = DeviceTypeId::new("hex.utility")?;
    let utility = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(utility_type)?,
        ParameterId::new(11)?,
        ParameterValue::Bool(true),
    )?;
    editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: utility.clone(),
    })?;
    let suffix = match editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: utility,
    })? {
        Created::Device(id) => id,
        _ => return Err("expected suffix device".into()),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected instrument slot")?;
    let instrument = instrument.id();
    if instrument_position == 1 {
        editor.execute(Command::MoveTrackDevice {
            device: instrument,
            track,
            before: Some(suffix),
        })?;
    } else if instrument_position != 0 {
        return Err("unsupported instrument position in allocation fixture".into());
    }
    Ok(())
}

#[test]
fn sounding_midi_scene_switches_use_the_prepared_release_bank_without_allocations()
-> Result<(), Box<dyn Error>> {
    for instrument_position in [0, 1] {
        sounding_midi_scene_switches_at_position(instrument_position)?;
    }
    Ok(())
}

fn sounding_midi_scene_switches_at_position(
    instrument_position: usize,
) -> Result<(), Box<dyn Error>> {
    let (project, track, scene_a, scene_b) = midi_session_project()?;
    let mut editor = Editor::new(project, 16)?;
    insert_stateful_effects_at(&mut editor, track, instrument_position)?;
    let project = editor.project().clone();
    let revision = Arc::new(PreparedRevision::prepare(
        &project,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let mut oracle = hex_engine::SessionPlayback::new(Arc::clone(&revision))?;
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene: scene_a,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "MIDI launch admission failed")?;
    let mut output = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    oracle.apply(SessionPlaybackCommand::LaunchScene {
        scene: scene_a,
        quantization: LaunchQuantization::Immediate,
    })?;
    oracle.apply(SessionPlaybackCommand::Play)?;
    oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, expected);
    assert!(control.status.rejected_action().is_none());
    assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));

    for scene in [scene_b, scene_a, scene_b, scene_a, scene_b, scene_a] {
        control
            .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            })
            .map_err(|_| "MIDI scene switch admission failed")?;
        oracle.apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })?;
        oracle.apply(SessionPlaybackCommand::Play)?;
        oracle.render(&mut expected)?;
        measured_block(&mut callback, &mut output);
        assert_eq!(output, expected);
        assert!(control.status.rejected_action().is_none());
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
    }
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn stateful_utility_processing_and_revision_handoff_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    for instrument_position in [0, 1] {
        stateful_utility_handoff_at_position(instrument_position, false)?;
        stateful_utility_handoff_at_position(instrument_position, true)?;
    }
    Ok(())
}

#[test]
fn stateful_utility_return_render_and_retirement_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    stateful_return_handoff_allocations(false)
}

#[test]
fn stateful_eq8_return_parameter_handoffs_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    stateful_return_handoff_allocations(true)
}

fn stateful_return_handoff_allocations(eq8_enabled: bool) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let asset = match editor.execute(Command::AddAsset {
        path: "memory/return-callback.wav".into(),
        sample_rate: RATE,
        frames: 256,
    })? {
        Created::Asset(asset) => asset,
        _ => return Err("expected audio asset".into()),
    };
    let track = match editor.execute(Command::AddTrack {
        name: "source".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected audio track".into()),
    };
    editor.execute(Command::PlaceClip {
        track,
        asset,
        start: 0,
        duration: 1_000_000,
        source_offset: 0,
        source_length: 256,
        looping: true,
    })?;
    editor.execute(Command::SetLoopRegion {
        start: 0,
        end: 256,
        enabled: true,
    })?;
    let return_track = match editor.execute(Command::AddReturnTrack {
        name: "filter return".into(),
    })? {
        Created::ReturnTrack(return_track) => return_track,
        _ => return Err("expected return track".into()),
    };
    editor.execute(Command::SetReturnSend {
        track,
        return_track,
        gain: 0.5,
    })?;
    let effect = hex_engine::NativeDeviceClass::Effect;
    let device_state = if eq8_enabled {
        let mut state = hex_engine::default_native_device_state_for_class(
            effect,
            DeviceTypeId::from_static("hex.eq8"),
        )?;
        for (parameter, value) in [
            (1, ParameterValue::Bool(true)),
            (2, ParameterValue::Choice(0)),
            (3, ParameterValue::Float(1_200.0)),
            (4, ParameterValue::Float(3.0)),
            (5, ParameterValue::Float(1.2)),
        ] {
            state = hex_engine::patch_native_device_parameter_for_class(
                effect,
                &state,
                ParameterId::new(parameter)?,
                value,
            )?;
        }
        state
    } else {
        hex_engine::patch_native_device_parameter(
            &hex_engine::default_native_device_state(DeviceTypeId::new("hex.utility")?)?,
            ParameterId::new(11)?,
            ParameterValue::Bool(true),
        )?
    };
    editor.execute(Command::InsertReturnDevice {
        return_track,
        before: None,
        state: device_state,
    })?;
    let media = BTreeMap::from([(
        asset,
        Audio::new(
            RATE,
            (0..256)
                .map(|index| {
                    let phase = index as f32 * 0.137;
                    [0.3 + phase.sin() * 0.2, -0.25 + (phase * 0.73).cos() * 0.31]
                })
                .collect(),
        )?,
    )]);
    let table = Arc::new(SineTable::new());
    let mut revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::clone(&table),
        None,
    )?);
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    oracle.apply(PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; 256];
    oracle.render(&mut expected)?;

    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(
            &revision,
        ))?)
        .map_err(|_| "initial Return graph publication failed")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "Play command admission failed")?;
    let mut actual = [[0.0; 2]; 256];
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);

    let changed_return = editor
        .project()
        .return_tracks()
        .iter()
        .find(|item| item.id() == return_track)
        .ok_or("expected return track")?
        .devices()[0]
        .id();
    let edits: &[(u32, ParameterValue)] = if eq8_enabled {
        &[
            (4, ParameterValue::Float(10.0)),
            (5, ParameterValue::Float(2.4)),
            (2, ParameterValue::Choice(1)),
        ]
    } else {
        &[
            (8, ParameterValue::Bool(true)),
            (8, ParameterValue::Bool(false)),
        ]
    };
    for &(parameter, value) in edits {
        let current = editor
            .project()
            .return_tracks()
            .iter()
            .find(|item| item.id() == return_track)
            .ok_or("expected return track")?
            .devices()[0]
            .state();
        let state = hex_engine::patch_native_device_parameter_for_class(
            effect,
            current,
            ParameterId::new(parameter)?,
            value,
        )?;
        editor.execute(Command::ReplaceDeviceState {
            device: changed_return,
            state,
        })?;
        let next_revision = Arc::new(PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::clone(&table),
            Some(revision.session()),
        )?);
        let mut replacement_oracle =
            Playback::try_from_prepared_with_session(Arc::clone(&next_revision))?;
        replacement_oracle
            .try_inherit_runtime_from(&mut oracle)
            .map_err(|_| "compatible Return handoff was refused")?;
        let mut expected_handoff = [[0.0; 2]; 256];
        replacement_oracle.render(&mut expected_handoff)?;
        let accepted_revision = control
            .publish(Playback::try_from_prepared_with_session(Arc::clone(
                &next_revision,
            ))?)
            .map_err(|_| "Return replacement publication failed")?;
        measured_block(&mut callback, &mut actual);
        assert_eq!(
            control.status.applied_revision.load(Ordering::Acquire),
            accepted_revision
        );
        compare_output(&actual, &expected_handoff);
        control.reclaim();
        oracle = replacement_oracle;
        revision = next_revision;
    }
    if !eq8_enabled {
        // Keep the original bypass/retired-tail coverage after measuring actual
        // mute and unmute parameter handoffs above.
        editor.execute(Command::SetDeviceBypass {
            device: changed_return,
            bypassed: true,
        })?;
        let next_revision = Arc::new(PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::clone(&table),
            Some(revision.session()),
        )?);
        let mut replacement_oracle =
            Playback::try_from_prepared_with_session(Arc::clone(&next_revision))?;
        replacement_oracle
            .try_inherit_runtime_from(&mut oracle)
            .map_err(|_| "compatible Utility Return handoff was refused")?;
        let mut expected_handoff = [[0.0; 2]; 256];
        replacement_oracle.render(&mut expected_handoff)?;
        let accepted_revision = control
            .publish(Playback::try_from_prepared_with_session(Arc::clone(
                &next_revision,
            ))?)
            .map_err(|_| "Utility Return replacement publication failed")?;
        measured_block(&mut callback, &mut actual);
        assert_eq!(
            control.status.applied_revision.load(Ordering::Acquire),
            accepted_revision
        );
        compare_output(&actual, &expected_handoff);
        control.reclaim();
    }

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    let mut reclaimed = 0;
    let retired = measure(|| reclaimed = control.reclaim());
    assert!(reclaimed >= 1);
    assert_eq!(retired.count_total, 0);
    assert!(retired.count_current < 0);
    assert!(retired.bytes_current < 0);
    control.reclaim();
    Ok(())
}

fn stateful_utility_handoff_at_position(
    instrument_position: usize,
    bypass_instrument: bool,
) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "keys".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected instrument track".into()),
    };
    let scene = match editor.execute(Command::AddScene { name: "a".into() })? {
        Created::Scene(scene) => scene,
        _ => return Err("expected Session scene".into()),
    };
    let clip = match editor.execute(Command::CreateSessionMidiClip {
        track,
        scene,
        name: "source".into(),
        source_length: BeatTime::from_quarters(4)?,
    })? {
        Created::SessionClip(clip) => clip,
        _ => return Err("expected Session MIDI clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(1)?,
        pitch: 60,
        channel: 0,
        velocity: 120,
    })?;
    insert_stateful_effects_at(&mut editor, track, instrument_position)?;
    let first_project = editor.project().clone();
    editor.execute(Command::SetTempoPoint {
        beat: BeatTime::ZERO,
        tempo: Tempo::new(60_000_000)?,
    })?;
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected instrument slot")?
        .id();
    editor.execute(Command::SetDeviceBypass {
        device: instrument,
        bypassed: bypass_instrument,
    })?;
    let second_project = editor.project().clone();
    let media = BTreeMap::new();
    let table = Arc::new(SineTable::new());
    let first_revision = Arc::new(PreparedRevision::prepare(
        &first_project,
        &media,
        Arc::clone(&table),
        None,
    )?);
    let second_revision = Arc::new(PreparedRevision::prepare(
        &second_project,
        &media,
        Arc::clone(&table),
        Some(first_revision.session()),
    )?);

    let playback = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "Session launch admission failed")?;
    let mut output = [[0.0; 2]; 128];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));

    let replacement = Playback::try_from_prepared_with_session(Arc::clone(&second_revision))?;
    control
        .publish(replacement)
        .map_err(|_| "stateful revision publication failed")?;
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
    for _ in 0..256 {
        measured_block(&mut callback, &mut output);
    }
    assert!(control.status.rejected_revision().is_none());

    if bypass_instrument {
        editor.execute(Command::SetDeviceBypass {
            device: instrument,
            bypassed: false,
        })?;
        let third_project = editor.project().clone();
        let third_revision = Arc::new(PreparedRevision::prepare(
            &third_project,
            &media,
            Arc::clone(&table),
            Some(second_revision.session()),
        )?);
        let reenabled = Playback::try_from_prepared_with_session(Arc::clone(&third_revision))?;
        control
            .publish(reenabled)
            .map_err(|_| "instrument re-enable publication failed")?;
        measured_block(&mut callback, &mut output);
        assert!(control.status.rejected_revision().is_none());
    }

    // The prior graph retains completed stateful tail owners until a later
    // graph publication retires it. Callback transfer stays allocation-free;
    // control reclamation performs only the expected deallocation.
    let final_playback = Playback::try_from_prepared_with_session(Arc::clone(&second_revision))?;
    control
        .publish(final_playback)
        .map_err(|_| "final stateful publication failed")?;
    measured_block(&mut callback, &mut output);
    let mut reclaimed = 0;
    let retired = measure(|| reclaimed = control.reclaim());
    assert_eq!(reclaimed, 2);
    assert_eq!(retired.count_total, 0);
    assert!(retired.count_current < 0);
    assert!(retired.bytes_current < 0);

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn arrangement_instrument_bypass_and_reenable_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    for instrument_position in [0, 1] {
        arrangement_instrument_bypass_at_position(instrument_position)?;
    }
    Ok(())
}

#[test]
fn builtin_fm4_render_state_handoff_and_retirement_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "fm4 arrangement".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected FM4 instrument track".into()),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("default instrument slot missing")?
        .id();
    let fm4_type = DeviceTypeId::from_static("hex.instrument.fm4");
    let fm4_default = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        fm4_type,
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: fm4_default,
    })?;
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected Arrangement MIDI clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 120,
    })?;
    let first_project = editor.project().clone();
    let media = BTreeMap::new();
    let table = Arc::new(SineTable::new());
    let first_revision = Arc::new(PreparedRevision::prepare(
        &first_project,
        &media,
        Arc::clone(&table),
        None,
    )?);
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial FM4 graph publication failed")?;
    control.command(PlaybackCommand::Play).unwrap();
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    oracle.apply(PlaybackCommand::Play);
    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 1.0e-6));

    let changed_state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        editor
            .project()
            .device(instrument)
            .ok_or("FM4 disappeared")?
            .state(),
        ParameterId::from_static(29),
        ParameterValue::Float(0.5),
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: changed_state,
    })?;
    let changed_project = editor.project().clone();
    let changed_revision = Arc::new(PreparedRevision::prepare(
        &changed_project,
        &media,
        Arc::clone(&table),
        Some(first_revision.session()),
    )?);
    let mut changed_oracle =
        Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    changed_oracle
        .try_inherit_runtime_from(&mut oracle)
        .map_err(|_| "FM4 state-change oracle handoff failed")?;
    let replacement = Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    control
        .publish(replacement)
        .map_err(|_| "FM4 state-change publication failed")?;
    changed_oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(control.status.rejected_revision().is_none());

    let mut completed = 0;
    let retired = measure(|| completed = control.reclaim());
    assert!(completed > 0, "old FM4 graph retires on the control side");
    assert_eq!(retired.count_total, 0, "retirement does not allocate");
    assert!(
        retired.count_current < 0,
        "retirement performs its drops on control"
    );
    assert!(retired.bytes_current < 0);

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    drop((changed_revision, first_revision, changed_oracle, oracle));
    Ok(())
}

#[test]
fn builtin_operator_render_state_handoff_and_retirement_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    check_operator_callback_lifecycle(false)
}

#[test]
fn mixed_wave_operator_handoff_and_retirement_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    check_operator_callback_lifecycle(true)
}

fn check_operator_callback_lifecycle(mixed_waveforms: bool) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "operator arrangement".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected Operator track".into()),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("default instrument slot missing")?
        .id();
    let operator_type = DeviceTypeId::from_static("hex.instrument.operator");
    let mut state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        operator_type,
    )?;
    for (parameter, value) in [
        (29, 1.0),
        (33, 1.0),
        (34, 1.0),
        (35, 1.0),
        (36, 0.0),
        (37, 0.001),
        (78, 0.25),
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(parameter),
            ParameterValue::Float(value),
        )?;
    }
    if mixed_waveforms {
        for (parameter, choice) in [(57, 1), (58, 2), (59, 3)] {
            state = hex_engine::patch_native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                &state,
                ParameterId::from_static(parameter),
                ParameterValue::Choice(choice),
            )?;
        }
    }
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state,
    })?;
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected Arrangement MIDI clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 120,
    })?;

    let media = BTreeMap::new();
    let table = Arc::new(SineTable::new());
    let first_revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::clone(&table),
        None,
    )?);
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial Operator graph publication failed")?;
    control.command(PlaybackCommand::Play).unwrap();
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    oracle.apply(PlaybackCommand::Play);
    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 1.0e-6));

    let mut changed_state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        editor
            .project()
            .device(instrument)
            .ok_or("Operator disappeared")?
            .state(),
        ParameterId::from_static(29),
        ParameterValue::Float(0.6),
    )?;
    changed_state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &changed_state,
        ParameterId::from_static(78),
        ParameterValue::Float(0.65),
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: changed_state,
    })?;
    let second_revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::clone(&table),
        Some(first_revision.session()),
    )?);
    let mut changed_oracle =
        Playback::try_from_prepared_with_session(Arc::clone(&second_revision))?;
    changed_oracle
        .try_inherit_runtime_from(&mut oracle)
        .map_err(|_| "Operator state-change oracle handoff failed")?;
    let replacement = Playback::try_from_prepared_with_session(Arc::clone(&second_revision))?;
    control
        .publish(replacement)
        .map_err(|_| "Operator state-change publication failed")?;
    changed_oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(control.status.rejected_revision().is_none());

    let mut completed = 0;
    let retired = measure(|| completed = control.reclaim());
    assert!(completed > 0, "old Operator graph retires on control");
    assert_eq!(retired.count_total, 0, "retirement does not allocate");
    assert!(retired.count_current < 0, "retirement drops on control");
    assert!(retired.bytes_current < 0);

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    drop((second_revision, first_revision, changed_oracle, oracle));
    Ok(())
}

fn arrangement_instrument_bypass_at_position(
    instrument_position: usize,
) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "arrangement keys".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected instrument track".into()),
    };
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected Arrangement MIDI clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 120,
    })?;
    insert_stateful_effects_at(&mut editor, track, instrument_position)?;
    let first_project = editor.project().clone();

    let mut changed_editor = Editor::new(first_project.clone(), 16)?;
    changed_editor.execute(Command::RemoveInstrument { track })?;
    changed_editor.execute(Command::InsertInstrument {
        track,
        state: hex_project::default_instrument_state()?,
    })?;
    let changed_instrument = changed_editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("replacement instrument slot is missing")?
        .id();
    if instrument_position == 1 {
        let before = changed_editor
            .project()
            .track(track)
            .and_then(|track| track.devices().get(1))
            .map(|device| device.id())
            .ok_or("instrument suffix anchor is missing")?;
        changed_editor.execute(Command::MoveTrackDevice {
            device: changed_instrument,
            track,
            before: Some(before),
        })?;
    }
    let changed_project = changed_editor.project().clone();
    let mut bypassed_editor = Editor::new(changed_project.clone(), 16)?;
    bypassed_editor.execute(Command::SetDeviceBypass {
        device: changed_instrument,
        bypassed: true,
    })?;
    let bypassed_project = bypassed_editor.project().clone();
    let table = Arc::new(SineTable::new());
    let first_revision = Arc::new(PreparedRevision::prepare(
        &first_project,
        &BTreeMap::new(),
        Arc::clone(&table),
        None,
    )?);
    let changed_revision = Arc::new(PreparedRevision::prepare(
        &changed_project,
        &BTreeMap::new(),
        Arc::clone(&table),
        Some(first_revision.session()),
    )?);
    let bypassed_revision = Arc::new(PreparedRevision::prepare(
        &bypassed_project,
        &BTreeMap::new(),
        Arc::clone(&table),
        Some(changed_revision.session()),
    )?);

    let initial = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(initial)
        .map_err(|_| "initial Arrangement publication failed")?;
    control.command(PlaybackCommand::Play).unwrap();
    oracle.apply(PlaybackCommand::Play);

    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 1.0e-6));

    let mut changed_oracle =
        Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    changed_oracle
        .try_inherit_runtime_from(&mut oracle)
        .map_err(|_| "Arrangement instrument identity handoff was refused")?;
    let identity_replacement =
        Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    control
        .publish(identity_replacement)
        .map_err(|_| "Arrangement instrument identity publication failed")?;
    changed_oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(control.status.rejected_revision().is_none());

    let mut bypassed_oracle =
        Playback::try_from_prepared_with_session(Arc::clone(&bypassed_revision))?;
    bypassed_oracle
        .try_inherit_runtime_from(&mut changed_oracle)
        .map_err(|_| "Arrangement bypass oracle handoff was refused")?;
    let replacement = Playback::try_from_prepared_with_session(Arc::clone(&bypassed_revision))?;
    control
        .publish(replacement)
        .map_err(|_| "Arrangement bypass publication failed")?;
    bypassed_oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(control.status.rejected_revision().is_none());

    let mut reenabled_oracle =
        Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    reenabled_oracle
        .try_inherit_runtime_from(&mut bypassed_oracle)
        .map_err(|_| "Arrangement re-enable oracle handoff was refused")?;
    let reenabled = Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    control
        .publish(reenabled)
        .map_err(|_| "Arrangement re-enable publication failed")?;
    reenabled_oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(control.status.rejected_revision().is_none());

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn legacy_fallback_owns_persistent_stateful_renderer_off_callback() -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let asset = match editor.execute(Command::AddAsset {
        path: "memory/fallback-stateful.wav".into(),
        sample_rate: RATE,
        frames: 256,
    })? {
        Created::Asset(asset) => asset,
        _ => return Err("expected audio asset".into()),
    };
    let track = match editor.execute(Command::AddTrack {
        name: "audio".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected audio track".into()),
    };
    editor.execute(Command::PlaceClip {
        track,
        asset,
        start: 0,
        duration: 1_000_000,
        source_offset: 0,
        source_length: 256,
        looping: true,
    })?;
    editor.execute(Command::SetLoopRegion {
        start: 0,
        end: 256,
        enabled: true,
    })?;
    let utility = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::new("hex.utility")?)?,
        ParameterId::new(11)?,
        ParameterValue::Bool(true),
    )?;
    let device = match editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: utility,
    })? {
        Created::Device(device) => device,
        _ => return Err("expected Utility device".into()),
    };
    let media = BTreeMap::from([(
        asset,
        Audio::new(
            RATE,
            (0..256)
                .map(|index| {
                    let phase = index as f32 * 0.137;
                    [0.3 + phase.sin() * 0.2, -0.25 + (phase * 0.73).cos() * 0.31]
                })
                .collect(),
        )?,
    )]);
    let compiled = Arc::new(hex_engine::compile::project(editor.project(), &media)?);

    let mut oracle = Playback::from_shared(Arc::clone(&compiled));
    oracle.apply(PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; 512];
    oracle.render(&mut expected)?;

    let mut partitioned = Playback::from_shared(Arc::clone(&compiled));
    partitioned.apply(PlaybackCommand::Play);
    let mut partitioned_output = [[0.0; 2]; 512];
    for chunk in partitioned_output.chunks_exact_mut(128) {
        partitioned.render(chunk)?;
    }
    assert_eq!(partitioned_output, expected);

    let callback_playback = Playback::from_shared(Arc::clone(&compiled));
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(callback_playback)
        .map_err(|_| "legacy fallback publication failed")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "legacy fallback Play admission failed")?;
    let mut output = [[0.0; 2]; 128];
    for block in expected.chunks_exact(128) {
        measured_block(&mut callback, &mut output);
        assert_eq!(output, block);
    }

    let changed_state = hex_engine::patch_native_device_parameter(
        editor.project().track(track).unwrap().devices()[0].state(),
        ParameterId::new(12)?,
        ParameterValue::Bool(true),
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: changed_state,
    })?;
    let changed_project = Arc::new(hex_engine::compile::project(editor.project(), &media)?);
    let mut inherited = Playback::from_shared(Arc::clone(&changed_project));
    inherited
        .try_inherit_runtime_from(&mut oracle)
        .map_err(|_| "legacy fallback compatible handoff was refused")?;
    let mut expected_handoff = [[0.0; 2]; 128];
    inherited.render(&mut expected_handoff)?;
    control
        .publish(Playback::from_shared(Arc::clone(&changed_project)))
        .map_err(|_| "legacy fallback compatible replacement failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, expected_handoff);

    oracle = inherited;
    // The callback declicks a queued Seek while playing (#4140).
    oracle
        .try_apply_declicked_action(hex_engine::playback::PlaybackAction::Transport(
            PlaybackCommand::Seek(0),
        ))
        .map_err(|_| "oracle Seek refused")?;
    let mut expected_seek = [[0.0; 2]; 128];
    oracle.render(&mut expected_seek)?;
    control
        .command(PlaybackCommand::Seek(0))
        .map_err(|_| "legacy fallback seek admission failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, expected_seek);

    // The callback declicks a queued Stop over this 128-frame block (#4151).
    oracle
        .try_apply_declicked_action(hex_engine::playback::PlaybackAction::Transport(
            PlaybackCommand::Stop,
        ))
        .map_err(|_| "oracle Stop refused")?;
    let mut expected_stop = [[1.0; 2]; 128];
    oracle.render(&mut expected_stop)?;
    control
        .command(PlaybackCommand::Stop)
        .map_err(|_| "legacy fallback Stop admission failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, expected_stop);
    oracle.apply(PlaybackCommand::Play);
    let mut expected_restart = [[0.0; 2]; 128];
    oracle.render(&mut expected_restart)?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "legacy fallback resumed Play admission failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, expected_restart);

    let empty = Arc::new(hex_engine::compile::project(
        &Project::new(RATE)?,
        &BTreeMap::new(),
    )?);
    control
        .publish(Playback::from_shared(empty))
        .map_err(|_| "legacy fallback retirement publication failed")?;
    measured_block(&mut callback, &mut output);
    let mut reclaimed = 0;
    let retired = measure(|| reclaimed = control.reclaim());
    assert_eq!(reclaimed, 2);
    assert_eq!(retired.count_total, 0);
    assert!(retired.count_current < 0);
    assert!(retired.bytes_current < 0);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn mutable_session_source_loop_callback_is_allocation_and_deallocation_free()
-> Result<(), Box<dyn Error>> {
    let definition = MutableFixtureDefinition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions)?;
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "looping mutable".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected midi track".into()),
    };
    let scene = match editor.execute(Command::AddScene {
        name: "loop".into(),
    })? {
        Created::Scene(scene) => scene,
        _ => return Err("expected session scene".into()),
    };
    let clip = match editor.execute(Command::CreateSessionMidiClip {
        track,
        scene,
        name: "source loop".into(),
        source_length: BeatTime::from_quarters(4)?,
    })? {
        Created::SessionClip(clip) => clip,
        _ => return Err("expected session midi clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::from_quarters(1)?,
        duration: BeatTime::from_quarters(3)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;
    editor.execute(Command::SetSessionMidiClipSource {
        clip,
        source_length: BeatTime::from_quarters(4)?,
        source_start: BeatTime::ZERO,
        loop_start: BeatTime::from_quarters(1)?,
        loop_end: BeatTime::from_quarters(2)?,
        looping: true,
    })?;
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected default instrument slot")?
        .id();
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: hex_lattice_contract::NativeDeviceState::new(MUTABLE_FIXTURE_TYPE, 1, vec![1])?,
    })?;
    let graph = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &registry,
    )?);
    let playback = Playback::try_from_prepared_with_session(graph)?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "looping mutable clip launch failed")?;

    let mut output = [[0.0; 2]; 128];
    let mut heard_note = false;
    // Cross three source-loop seams while measuring the real prepared callback.
    for _ in 0..600 {
        measured_block(&mut callback, &mut output);
        heard_note |= output.iter().any(|frame| frame[0] > 0.0);
    }
    assert!(heard_note);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn mutable_arrangement_transport_loop_callback_is_allocation_and_deallocation_free()
-> Result<(), Box<dyn Error>> {
    let definition = MutableFixtureDefinition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions)?;
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "transport loop".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected MIDI track".into()),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected default instrument slot")?
        .id();
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: NativeDeviceState::new(MUTABLE_FIXTURE_TYPE, 1, vec![1])?,
    })?;
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(4)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected Arrangement MIDI clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_ratio(3, 2)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;
    editor.execute(Command::SetLoopRegion {
        start: (RATE / 2) as u64,
        end: RATE as u64,
        enabled: true,
    })?;

    let graph = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &registry,
    )?);
    let playback = Playback::try_from_prepared_with_session(graph)?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial transport-loop publication failed")?;
    control
        .session_command_with_ticket(SessionPlaybackCommand::Play)
        .map_err(|_| "transport-loop Play admission failed")?;

    let mut output = [[0.0; 2]; 128];
    let mut heard_note = false;
    // Cross the loop start, then the first actual wrap and repeated chase.
    for _ in 0..800 {
        measured_block(&mut callback, &mut output);
        heard_note |= output.iter().any(|frame| frame[0] != 0.0);
    }
    assert!(
        heard_note,
        "Arrangement loop fixture rendered no instrument audio"
    );
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn mutable_arrangement_source_and_transport_loops_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    let definition = MutableFixtureDefinition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions)?;
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "combined loop".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected MIDI track".into()),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected default instrument slot")?
        .id();
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: NativeDeviceState::new(MUTABLE_FIXTURE_TYPE, 1, vec![1])?,
    })?;
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(5)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected Arrangement MIDI clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::from_ratio(3, 2)?,
        duration: BeatTime::from_quarters(2)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;
    editor.execute(Command::SetMidiClipSource {
        clip,
        source_length: BeatTime::from_quarters(5)?,
        source_start: BeatTime::from_ratio(1, 2)?,
        loop_start: BeatTime::from_quarters(1)?,
        loop_end: BeatTime::from_quarters(2)?,
        looping: true,
        length: BeatTime::from_quarters(5)?,
    })?;
    editor.execute(Command::SetLoopRegion {
        start: 30_000,
        end: 78_000,
        enabled: true,
    })?;

    let graph = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &registry,
    )?);
    let playback = Playback::try_from_prepared_with_session(graph)?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial combined-loop publication failed")?;
    control
        .session_command_with_ticket(SessionPlaybackCommand::Play)
        .map_err(|_| "combined-loop Play admission failed")?;

    let mut output = [[0.0; 2]; 128];
    let mut heard_note = false;
    // 210,000 frames cross the initial pass and three transport-loop wraps.
    for _ in 0..1_641 {
        measured_block(&mut callback, &mut output);
        heard_note |= output.iter().any(|frame| frame[0] != 0.0);
    }
    assert!(
        heard_note,
        "combined loop fixture rendered no instrument audio"
    );
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

mod delay_mix;
mod eq8_gain;

mod stage_clocks;

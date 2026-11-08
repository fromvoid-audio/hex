//! Release comparison for inline versus shared-context analytic MIDI sampling.
//!
//! This is an injected deterministic provider, not a built-in device or an audio
//! deadline test. Run with:
//! `cargo run --release -p hex-engine --features instrument-test-support --example profile_shared_context`
use hex_engine::{
    PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice::{
    NativeInstrumentDefinition, PreparedAnalyticNote, PreparedNativeInstrument,
    PreparedSharedAnalyticContext, PreparedSharedAnalyticNote, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{
    BeatTime, DeviceSnapshot, Meter, MeterPoint, MidiClipSnapshot, MidiNoteSnapshot, Project,
    Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

type ProfileFixture = (Project, Vec<(u8, u8)>);

const RATE: u32 = 48_000;
const TRACKS: usize = 100;
const NOTES_PER_TRACK: usize = 16;
const BLOCK: usize = 128;
const WARMUP_BLOCKS: usize = 100;
const MEASURED_BLOCKS: usize = 2_000;
const ATTACK_FRAMES: u64 = 16;
const RELEASE_FRAMES: u64 = 8;
const INLINE_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.profile.inline");
const SHARED_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.profile.shared");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceMode {
    Inline,
    Shared,
}

struct ProfileDefinition {
    id: DeviceTypeId,
    mode: SourceMode,
}

struct ProfilePrepared {
    mode: SourceMode,
    sample_rate: u32,
}

struct ProfileContext {
    sample_rate: u32,
}

impl NativeDeviceMetadata for ProfileDefinition {
    fn type_id(&self) -> DeviceTypeId {
        self.id
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("shared context profile fixture")
    }

    fn state_version(&self) -> u32 {
        1
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(self.id, 1, Vec::new())
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() == self.id && state.state_version() == 1 && state.payload().is_empty() {
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

impl NativeInstrumentDefinition for ProfileDefinition {
    fn prepare_instrument(
        &self,
        _: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        if !context.tuning.is_neutral() {
            return Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass);
        }
        if sample_rate != RATE {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Box::new(ProfilePrepared {
            mode: self.mode,
            sample_rate,
        }))
    }
}

impl PreparedNativeInstrument for ProfilePrepared {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedAnalyticNote, DeviceContractError> {
        let mut payload = [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[0] = u64::from(amplitude(velocity).to_bits());
        payload[1] = u64::from(pitch);
        Ok(PreparedAnalyticNote::new(
            payload,
            RELEASE_FRAMES,
            sample_inline,
        ))
    }

    fn prepare_shared_analytic_context(
        &self,
    ) -> Result<Option<Box<dyn PreparedSharedAnalyticContext>>, DeviceContractError> {
        if self.mode == SourceMode::Shared {
            Ok(Some(Box::new(ProfileContext {
                sample_rate: self.sample_rate,
            })))
        } else {
            Ok(None)
        }
    }
}

impl PreparedSharedAnalyticContext for ProfileContext {
    fn declared_context_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedSharedAnalyticNote, DeviceContractError> {
        if self.sample_rate != RATE {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut payload = [0; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[0] = u64::from(amplitude(velocity).to_bits());
        payload[1] = u64::from(pitch);
        Ok(PreparedSharedAnalyticNote::new(payload, RELEASE_FRAMES))
    }

    fn sample(
        &self,
        note: &PreparedSharedAnalyticNote,
        age: u64,
        gate: u64,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> StereoFrame {
        sample_payload(
            note.payload()[0],
            note.payload()[1] as u8,
            age,
            gate,
            RELEASE_FRAMES,
        )
    }
}

fn amplitude(velocity: u8) -> f32 {
    (f32::from(velocity) / 127.0) * 0.000_05
}

fn envelope(age: u64, gate: u64, release: u64) -> f32 {
    if age < ATTACK_FRAMES {
        (age as f32 + 1.0) / ATTACK_FRAMES as f32
    } else if age < gate {
        1.0
    } else {
        let release_age = age - gate;
        if release_age >= release {
            0.0
        } else {
            (release - release_age) as f32 / release as f32
        }
    }
}

fn sample_payload(
    amplitude_bits: u64,
    pitch: u8,
    age: u64,
    gate: u64,
    release: u64,
) -> StereoFrame {
    let phase = age.wrapping_add(u64::from(pitch) * 3) % 64;
    let polarity = if phase < 32 { 1.0 } else { -1.0 };
    let mono = f32::from_bits(amplitude_bits as u32) * envelope(age, gate, release) * polarity;
    [mono, mono * 0.75]
}

fn sample_inline(
    payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    age: u64,
    gate: u64,
    release: u64,
    _resources: &dyn hex_lattice::DeviceResources,
) -> StereoFrame {
    sample_payload(payload[0], payload[1] as u8, age, gate, release)
}

fn fixture(instrument_type: DeviceTypeId) -> Result<ProfileFixture, Box<dyn Error>> {
    let mut next_id = 1_u64;
    let mut tracks = Vec::with_capacity(TRACKS);
    let mut notes_for_oracle = Vec::with_capacity(TRACKS * NOTES_PER_TRACK);
    let clip_length = BeatTime::from_quarters(128)?.raw();
    let note_duration = BeatTime::from_quarters(120)?.raw();

    for track_index in 0..TRACKS {
        let track_id = next_id;
        next_id += 1;
        let instrument_id = next_id;
        next_id += 1;
        let clip_id = next_id;
        next_id += 1;
        let mut notes = Vec::with_capacity(NOTES_PER_TRACK);
        for note_index in 0..NOTES_PER_TRACK {
            let pitch = 36 + ((track_index * 3 + note_index * 5) % 48) as u8;
            let velocity = 64 + ((note_index * 3) % 64) as u8;
            notes_for_oracle.push((pitch, velocity));
            notes.push(MidiNoteSnapshot {
                active: true,
                id: next_id,
                start_raw: 0,
                duration_raw: note_duration,
                pitch,
                channel: 0,
                velocity,
            });
            next_id += 1;
        }
        tracks.push(TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track_id,
            name: format!("context profile {track_index:03}"),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: Some(DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: instrument_id,
                bypassed: false,
                state: NativeDeviceState::new(instrument_type, 1, Vec::new())?,
            }),
            instrument_position: Some(0),
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                content_view: None,
                active: true,
                playback_offset_raw: 0,
                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                volume: hex_project::MidiClipVolume::default(),
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                id: clip_id,
                start_raw: 0,
                length_raw: clip_length,
                source_length_raw: clip_length,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: clip_length,
                looping: false,
                notes,
            }]),
            sends: Vec::new(),
        });
    }

    let project = Project::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        import_provenance: None,
        sample_rate: RATE,
        sample_rate_mode: Default::default(),
        next_id,
        assets: Vec::new(),
        tracks,
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })?;
    Ok((project, notes_for_oracle))
}

fn prepare(
    project: &Project,
    definitions: &[&dyn NativeInstrumentDefinition],
) -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
    let registry = hex_lattice::NativeInstrumentRegistry::new(definitions)?;
    let started = Instant::now();
    let revision = Arc::new(PreparedRevision::prepare_with_test_instrument_registry(
        project,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &registry,
    )?);
    println!(
        "revision_prepare_elapsed={:.6}s",
        started.elapsed().as_secs_f64()
    );
    Ok(revision)
}

fn playback(revision: Arc<PreparedRevision>) -> Result<Playback, Box<dyn Error>> {
    let mut playback = Playback::try_from_prepared_with_session(revision)?;
    playback.apply(PlaybackCommand::Play);
    Ok(playback)
}

fn expected_frame(notes: &[(u8, u8)], age: u64, gate: u64) -> StereoFrame {
    let mut total = [0.0_f32; 2];
    // Empty effect chains accumulate each contribution directly into the mix.
    // Preserve that documented f32 accumulation order in the scalar oracle.
    for &(pitch, velocity) in notes {
        let frame = sample_payload(
            u64::from(amplitude(velocity).to_bits()),
            pitch,
            age,
            gate,
            RELEASE_FRAMES,
        );
        total[0] += frame[0];
        total[1] += frame[1];
    }
    total
}

fn verify_pcm(
    inline_revision: Arc<PreparedRevision>,
    shared_revision: Arc<PreparedRevision>,
    notes: &[(u8, u8)],
) -> Result<u64, Box<dyn Error>> {
    const CHECK_FRAMES: usize = 4_096;
    let mut inline = playback(inline_revision)?;
    let mut shared = playback(shared_revision)?;
    let mut inline_pcm = vec![[0.0; 2]; CHECK_FRAMES];
    let mut shared_pcm = vec![[0.0; 2]; CHECK_FRAMES];
    for (inline_chunk, shared_chunk) in inline_pcm.chunks_mut(97).zip(shared_pcm.chunks_mut(97)) {
        inline.render(inline_chunk)?;
        shared.render(shared_chunk)?;
    }
    if inline_pcm != shared_pcm {
        return Err("inline and shared-context PCM differ".into());
    }
    for (frame_index, frame) in inline_pcm.iter().enumerate() {
        let expected = expected_frame(notes, frame_index as u64, u64::MAX);
        if *frame != expected {
            return Err(
                format!("independent aggregation oracle mismatch at frame {frame_index}: {frame:?} != {expected:?}").into(),
            );
        }
    }
    Ok(digest(&inline_pcm))
}

fn digest(frames: &[[f32; 2]]) -> u64 {
    frames
        .iter()
        .flatten()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, sample| {
            (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

struct ProfileResult {
    p50_us: f64,
    p99_us: f64,
    max_us: f64,
    digest: u64,
    pcm: Vec<StereoFrame>,
}

fn measure(revision: Arc<PreparedRevision>) -> Result<ProfileResult, Box<dyn Error>> {
    let construction_started = Instant::now();
    let mut playback = playback(revision)?;
    let playback_construction = construction_started.elapsed();
    let mut output = [[0.0; 2]; BLOCK];
    for _ in 0..WARMUP_BLOCKS {
        playback.render(&mut output)?;
    }

    let frame_capacity = BLOCK
        .checked_mul(MEASURED_BLOCKS)
        .ok_or("profile frame count overflow")?;
    let mut pcm = Vec::with_capacity(frame_capacity);
    let mut timings = Vec::with_capacity(MEASURED_BLOCKS);
    for _ in 0..MEASURED_BLOCKS {
        let started = Instant::now();
        playback.render(black_box(&mut output))?;
        timings.push(started.elapsed().as_nanos());
        pcm.extend_from_slice(&output);
        black_box(&output);
    }
    timings.sort_unstable();
    println!(
        "playback_construction_elapsed={:.6}s",
        playback_construction.as_secs_f64()
    );
    Ok(ProfileResult {
        p50_us: timings[MEASURED_BLOCKS / 2] as f64 / 1_000.0,
        p99_us: timings[(MEASURED_BLOCKS * 99).div_ceil(100) - 1] as f64 / 1_000.0,
        max_us: timings[MEASURED_BLOCKS - 1] as f64 / 1_000.0,
        digest: digest(&pcm),
        pcm,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let inline_definition = ProfileDefinition {
        id: INLINE_TYPE,
        mode: SourceMode::Inline,
    };
    let shared_definition = ProfileDefinition {
        id: SHARED_TYPE,
        mode: SourceMode::Shared,
    };
    let inline_definitions: [&dyn NativeInstrumentDefinition; 1] = [&inline_definition];
    let shared_definitions: [&dyn NativeInstrumentDefinition; 1] = [&shared_definition];
    let (inline_project, notes) = fixture(INLINE_TYPE)?;
    let (shared_project, _) = fixture(SHARED_TYPE)?;

    let inline_revision = prepare(&inline_project, &inline_definitions)?;
    let shared_revision = prepare(&shared_project, &shared_definitions)?;
    let oracle_digest = verify_pcm(
        Arc::clone(&inline_revision),
        Arc::clone(&shared_revision),
        &notes,
    )?;
    let inline = measure(Arc::clone(&inline_revision))?;
    let shared = measure(Arc::clone(&shared_revision))?;
    if inline.pcm != shared.pcm {
        return Err("inline/shared full-run PCM differs".into());
    }

    println!(
        "shared-context analytic profile: tracks={TRACKS} notes_per_track={NOTES_PER_TRACK} concurrent_notes={} rate={RATE} block={BLOCK} warmup_blocks={WARMUP_BLOCKS} measured_blocks={MEASURED_BLOCKS} frame_count={} gate_beats=120 source=Arrangement-through-SessionPlayback",
        TRACKS * NOTES_PER_TRACK,
        BLOCK * MEASURED_BLOCKS,
    );
    println!(
        "inline: p50_us={:.3} p99_us={:.3} max_us={:.3} full_pcm_digest={:016x}",
        inline.p50_us, inline.p99_us, inline.max_us, inline.digest
    );
    println!(
        "shared: p50_us={:.3} p99_us={:.3} max_us={:.3} full_pcm_digest={:016x}",
        shared.p50_us, shared.p99_us, shared.max_us, shared.digest
    );
    println!(
        "independent_aggregation_first_4096_frame_oracle_digest={oracle_digest:016x}; PCM checks, preparation and playback construction are outside render timings; no device, UI, callback deadline or handoff timing"
    );
    Ok(())
}

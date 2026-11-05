//! Canonical project fixture for the registered Operator feedback profiler.
use hex_devices::operator_state::{OPERATOR_METADATA, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState, ParameterValue};
use hex_project::{
    BeatTime, Command, Created, DeviceId, DeviceSnapshot, Editor, Meter, MeterPoint,
    MidiClipSnapshot, MidiNoteSnapshot, Project, Snapshot, Tempo, TempoPoint, TrackContentSnapshot,
    TrackSnapshot,
};
use std::error::Error;

pub const SAMPLE_RATE: u32 = 48_000;
/// Notes hold for 120 beats at the fixture's default 120 BPM tempo.
pub const NOTE_HOLD_FRAMES: usize = SAMPLE_RATE as usize * 60;

#[derive(Debug)]
pub struct Fixture {
    pub project: Project,
    pub instrument_ids: Vec<DeviceId>,
    pub source_count: usize,
    pub contributor_count: usize,
    pub authored_notes: usize,
    pub peak_authored_voices: usize,
}

pub fn project(
    tracks: usize,
    notes_per_track: usize,
    feedback: f32,
    level: f32,
) -> Result<Fixture, Box<dyn Error>> {
    project_with_operators(tracks, notes_per_track, feedback, level, false)
}

pub fn project_with_operators(
    tracks: usize,
    notes_per_track: usize,
    feedback: f32,
    level: f32,
    all_operators: bool,
) -> Result<Fixture, Box<dyn Error>> {
    if tracks == 0 || tracks > hex_project::MAX_TRACKS || notes_per_track == 0 {
        return Err("tracks and notes must be nonzero and within project limits".into());
    }
    let authored_notes = tracks
        .checked_mul(notes_per_track)
        .ok_or("operator fixture note count overflow")?;
    let state = state_with_operators(feedback, level, all_operators)?;
    let mut next_id = 1_u64;
    let mut track_snapshots = Vec::with_capacity(tracks);
    let mut instrument_ids = Vec::with_capacity(tracks);
    let note_duration = BeatTime::from_ratio(120, 1)?.raw();
    let clip_length = BeatTime::from_ratio(128, 1)?.raw();

    for track_index in 0..tracks {
        let track_id = take_id(&mut next_id)?;
        let instrument_id = take_id(&mut next_id)?;
        let clip_id = take_id(&mut next_id)?;
        let mut notes = Vec::with_capacity(notes_per_track);
        for note_index in 0..notes_per_track {
            notes.push(MidiNoteSnapshot {
                active: true,
                id: take_id(&mut next_id)?,
                start_raw: 0,
                duration_raw: note_duration,
                pitch: 48 + ((track_index + note_index) % 36) as u8,
                channel: 0,
                velocity: 100,
            });
        }
        track_snapshots.push(TrackSnapshot {
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track_id,
            name: format!("operator {track_index:03}"),
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
                state: state.clone(),
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
        sample_rate: SAMPLE_RATE,
        next_id,
        assets: Vec::new(),
        tracks: track_snapshots,
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })?;
    instrument_ids.extend(
        project
            .tracks()
            .iter()
            .filter_map(|track| track.instrument().map(|instrument| instrument.id())),
    );
    Ok(Fixture {
        source_count: project.native_sources().len(),
        contributor_count: tracks,
        authored_notes,
        peak_authored_voices: authored_notes,
        project,
        instrument_ids,
    })
}

/// Shared-source workload with one registered Operator instance and all tracks
/// contributing MIDI to its single receiver. Notes use the independent fixture's
/// pitches, simultaneous onset and 120-beat hold so steady measurements cover
/// the same sustained workload. Larger workloads exceed the source voice bound.
pub fn shared_source_project_with_operators(
    tracks: usize,
    notes_per_track: usize,
    feedback: f32,
    level: f32,
    all_operators: bool,
) -> Result<Fixture, Box<dyn Error>> {
    if tracks == 0 || tracks > hex_project::MAX_TRACKS || notes_per_track == 0 {
        return Err("tracks and notes must be nonzero and within project limits".into());
    }
    let authored_notes = tracks
        .checked_mul(notes_per_track)
        .ok_or("shared fixture note count overflow")?;
    let source_voice_capacity = hex_engine::midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK;
    if authored_notes > source_voice_capacity {
        return Err(format!(
            "shared Operator fixture needs {authored_notes} source voice slots, beyond the current {source_voice_capacity}-voice shared-source capacity"
        )
        .into());
    }
    let state = state_with_operators(feedback, level, all_operators)?;
    let mut editor = Editor::new(Project::new(SAMPLE_RATE)?, 1)?;
    let Created::NativeSource(source) = editor.execute(Command::CreateNativeSource {
        name: "shared operator".into(),
        devices: vec![state],
        instrument_index: 0,
    })?
    else {
        return Err("shared source fixture did not create a native source".into());
    };
    let instrument = editor
        .project()
        .native_source(source)
        .ok_or("shared source fixture lost its native source")?
        .instrument()
        .id();
    let mut contributors = Vec::with_capacity(tracks);
    let clip_length = BeatTime::from_quarters(128)?;
    for track_index in 0..tracks {
        let Created::Track(track) = editor.execute(Command::AddMidiTrack {
            name: format!("operator contributor {track_index:03}"),
        })?
        else {
            return Err("shared source fixture did not create a MIDI track".into());
        };
        editor.execute(Command::BindTrackSource {
            track,
            source: Some(source),
        })?;
        let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: clip_length,
        })?
        else {
            return Err("shared source fixture did not create a MIDI clip".into());
        };
        for note_index in 0..notes_per_track {
            editor.execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(120)?,
                pitch: 48 + ((track_index + note_index) % 36) as u8,
                channel: 0,
                velocity: 100,
            })?;
        }
        contributors.push(track);
    }
    let project = editor.project().clone();
    if project.native_sources().len() != 1
        || project.source_contributors(source).count() != tracks
        || project
            .native_source(source)
            .and_then(|source| source.receiver())
            != contributors.first().copied()
        || project
            .source_contributors(source)
            .any(|track| track.instrument().map(|device| device.id()) != Some(instrument))
    {
        return Err("shared source fixture did not preserve one source and receiver".into());
    }
    Ok(Fixture {
        project,
        instrument_ids: vec![instrument],
        source_count: 1,
        contributor_count: tracks,
        authored_notes,
        peak_authored_voices: authored_notes,
    })
}

pub fn state(feedback: f32, level: f32) -> Result<NativeDeviceState, Box<dyn Error>> {
    let state = OPERATOR_METADATA.default_state()?;
    let state = OPERATOR_METADATA.patch_parameter(
        &state,
        parameter::feedback(1),
        ParameterValue::Float(feedback),
    )?;
    Ok(OPERATOR_METADATA.patch_parameter(
        &state,
        parameter::oscillator_level(1),
        ParameterValue::Float(level),
    )?)
}

/// Optional four-active-carrier control workload; defaults remain unchanged.
pub fn state_with_operators(
    feedback: f32,
    level: f32,
    all_operators: bool,
) -> Result<NativeDeviceState, Box<dyn Error>> {
    let mut state = state(feedback, level)?;
    if all_operators {
        for operator in 2..=4 {
            state = OPERATOR_METADATA.patch_parameter(
                &state,
                parameter::oscillator_level(operator),
                ParameterValue::Float(0.3 + operator as f32 * 0.05),
            )?;
            state = OPERATOR_METADATA.patch_parameter(
                &state,
                parameter::carrier(operator),
                ParameterValue::Float(0.25),
            )?;
        }
    }
    Ok(state)
}

fn take_id(next_id: &mut u64) -> Result<u64, Box<dyn Error>> {
    let id = *next_id;
    *next_id = next_id.checked_add(1).ok_or("fixture id overflow")?;
    Ok(id)
}

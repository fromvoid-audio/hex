//! Translating a MIDI source origin must preserve every unchanged audible note.
//! An uninterrupted independent playback is the PCM/note-off/tail oracle.
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{
    BeatTime, Command, Created, Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint,
    MidiNoteUpdate, Project,
};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering},
};

#[test]
fn source_prefix_preserves_uninterrupted_held_voice_gate_and_tail_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    for mutable in [false, true] {
        for block_size in [1, 7, 64, 512] {
            compare(mutable, block_size, false)?;
        }
        compare(mutable, 64, true)?;
    }
    Ok(())
}
fn compare(mutable: bool, block_size: usize, looping: bool) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(48_000)?, 16)?;
    let state = if mutable {
        let mut state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
        )?;
        for (id, value) in [
            (29, 1.0),
            (33, 1.0),
            (34, 1.0),
            (35, 1.0),
            (36, 0.0),
            (37, 0.001),
            (38, 0.005),
            (78, 0.4),
        ] {
            state = hex_engine::patch_native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                &state,
                hex_lattice_contract::ParameterId::from_static(id),
                hex_lattice_contract::ParameterValue::Float(value),
            )?;
        }
        state
    } else {
        hex_project::default_instrument_state()?
    };
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "translated held note".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state,
    })?
    else {
        return Err("track".into());
    };
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(4)?,
    })?
    else {
        return Err("clip".into());
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(3)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;
    editor.execute(Command::SetMidiClipSource {
        clip,
        source_length: BeatTime::from_quarters(8)?,
        source_start: BeatTime::ZERO,
        loop_start: BeatTime::from_quarters(1)?,
        loop_end: BeatTime::from_ratio(5, 2)?,
        looping,
        length: BeatTime::from_quarters(4)?,
    })?;
    let Created::Note(moved) = editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::from_quarters(6)?,
        duration: BeatTime::from_ratio(1, 4)?,
        pitch: 72,
        channel: 2,
        velocity: 71,
    })?
    else {
        return Err("note".into());
    };
    editor.execute(Command::SetTempoPoint {
        beat: BeatTime::from_quarters(2)?,
        tempo: hex_project::Tempo::new(93_500_000)?,
    })?;
    editor.execute(Command::ReplaceMidiClipVolume {
        clip,
        volume: MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.25)?),
                MidiClipVolumePoint::linear(BeatTime::from_quarters(4)?, MidiClipGain::new(0.75)?),
            ],
            BeatTime::from_quarters(8)?,
        )?,
    })?;
    let prepare = |project: &Project| -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
        Ok(Arc::new(PreparedRevision::prepare(
            project,
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )?))
    };
    let edges = hex_engine::compile::midi_events(editor.project())?;
    let gate = edges
        .iter()
        .find(|edge| edge.kind == hex_engine::midi_compile::MidiEventKind::NoteOff)
        .ok_or("gate")?
        .frame;
    let initial = prepare(editor.project())?;
    let mut oracle = Playback::try_from_prepared_with_session(initial.clone())?;
    oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = prepared::channel(48_000)?;
    control
        .publish_with_policy_and_context_generation(
            Playback::try_from_prepared_with_session(initial)?,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|_| "initial publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut actual = vec![[0.0; 2]; 512];
    let mut expected = actual.clone();
    let mut position = 0_u64;
    let mut heard = false;
    for limit in [30_000, 128_000] {
        while position < limit {
            // Exercise tiny blocks at publication and musical boundaries; broad
            // silent/tail regions use 512 frames rather than repeating expensive
            // fixed callback setup for every sample of the whole project.
            let boundary = [30_000, gate, 128_000]
                .into_iter()
                .find(|edge| position < *edge);
            let edge = boundary.unwrap_or(limit).min(limit);
            let near_edge = edge - position <= 1_024
                || (position >= 30_000 && position - 30_000 < 1_024)
                || (position >= gate && position - gate < 1_024);
            let count = (if near_edge { block_size } else { 512 }).min((edge - position) as usize);
            let mut result = None;
            let measured = allocation_counter::measure(|| {
                callback.begin_block();
                result = Some(callback.render(&mut actual[..count]));
            });
            result.ok_or("render result")??;
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            oracle.render(&mut expected[..count])?;
            assert_eq!(
                actual[..count],
                expected[..count],
                "mutable={mutable} looping={looping} block={block_size} frame={position}"
            );
            if position >= 127_000 {
                assert!(
                    actual[..count]
                        .iter()
                        .flatten()
                        .all(|sample| sample.abs() < 1e-7),
                    "held gate and release tail must have ended"
                );
            }
            heard |= actual[..count]
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 1e-5);
            position += count as u64;
            assert_eq!(control.status.position.load(Ordering::Acquire), position);
            control.reclaim();
        }
        if limit == 30_000 {
            editor.execute(Command::UpdateMidiNotesWithSourcePrefix {
                clip,
                prefix: BeatTime::from_ratio(5, 4)?,
                notes: vec![MidiNoteUpdate {
                    note: moved,
                    start: BeatTime::ZERO,
                    duration: BeatTime::from_ratio(1, 4)?,
                    pitch: 72,
                    channel: 2,
                    velocity: 71,
                }],
            })?;
            assert_eq!(
                hex_engine::compile::midi_events(editor.project())?,
                edges,
                "unmoved note IDs and absolute gate deadlines remain identical"
            );
            control
                .publish_with_policy_and_context_generation(
                    Playback::try_from_prepared_with_session(prepare(editor.project())?)?,
                    PublicationPolicy::InheritRuntime,
                    1,
                )
                .map_err(|_| "source prefix publication")?;
        }
    }
    assert!(heard);
    Ok(())
}

//! Canonical tag edits must be inaudible through ordinary callback publication.
//! An uninterrupted independent playback is the PCM/note-off/tail oracle.
#![cfg(feature = "allocation-probe")]
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{BeatTime, Command, Created, Editor, Project, TrackTag};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering},
};

#[test]
fn tag_publication_preserves_uninterrupted_held_voice_note_off_and_tail_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    for mutable in [false, true] {
        for block_size in [64, 512] {
            compare(mutable, block_size)?;
        }
    }
    Ok(())
}
fn compare(mutable: bool, block_size: usize) -> Result<(), Box<dyn Error>> {
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
        name: "tagged held note".into(),
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
        length: BeatTime::from_quarters(8)?,
    })?
    else {
        return Err("clip".into());
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;
    let prepare = |project: &Project| -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
        Ok(Arc::new(PreparedRevision::prepare(
            project,
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )?))
    };
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
    let mut actual = vec![[0.0; 2]; block_size];
    let mut expected = actual.clone();
    let mut position = 0;
    let mut heard = false;
    for limit in [30_000, 108_000] {
        while position < limit {
            let count = block_size.min((limit - position) as usize);
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
                "mutable={mutable} block={block_size} frame={position}"
            );
            heard |= actual[..count]
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 1e-5);
            position += count as u64;
            assert_eq!(control.status.position.load(Ordering::Acquire), position);
            control.reclaim();
        }
        if limit == 30_000 {
            editor.execute(Command::SetTrackTags {
                track,
                tags: vec![TrackTag::new("drums")?, TrackTag::new("lowend")?],
            })?;
            control
                .publish_with_policy_and_context_generation(
                    Playback::try_from_prepared_with_session(prepare(editor.project())?)?,
                    PublicationPolicy::InheritRuntime,
                    1,
                )
                .map_err(|_| "tag publication")?;
        }
    }
    assert!(heard);
    Ok(())
}

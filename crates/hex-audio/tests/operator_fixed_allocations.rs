//! Fixed tuning and velocity sensitivity prepare before admission; callback sampling and seek stay bounded.
#![cfg(feature = "allocation-probe")]
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, sync::Arc};

#[test]
fn fixed_and_velocity_operator_render_seek_and_shutdown_allocate_nothing_on_callback()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(48_000)?, 16)?;
    let Created::Track(track) = editor.execute(Command::AddMidiTrack {
        name: "fixed operator".into(),
    })?
    else {
        return Err("track missing".into());
    };
    let device = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("instrument missing")?
        .id();
    let class = hex_engine::NativeDeviceClass::Instrument;
    let mut state = hex_engine::default_native_device_state_for_class(
        class,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )?;
    for (id, value) in [
        (29, ParameterValue::Float(1.0)),
        (33, ParameterValue::Float(1.0)),
        (34, ParameterValue::Float(1.0)),
        (35, ParameterValue::Float(1.0)),
        (36, ParameterValue::Float(0.0)),
        (37, ParameterValue::Float(0.0)),
        (38, ParameterValue::Float(0.0)),
        (63, ParameterValue::Float(220.0)),
        (64, ParameterValue::Float(2.0)),
        (62, ParameterValue::Bool(true)),
        (57, ParameterValue::Choice(1)),
        (74, ParameterValue::Float(0.75)),
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            class,
            &state,
            ParameterId::from_static(id),
            value,
        )?;
    }
    editor.execute(Command::ReplaceDeviceState { device, state })?;
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })?
    else {
        return Err("clip missing".into());
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 64,
    })?;
    let graph = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&graph))?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&graph))?;
    let (mut control, mut callback) = prepared::channel(48_000)?;
    control
        .publish(playback)
        .map_err(|_| "publication refused")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play refused")?;
    oracle.apply(PlaybackCommand::Play);
    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    for destination in [None, Some(24_000), Some(0)] {
        if let Some(frame) = destination {
            control
                .command(PlaybackCommand::PlayFrom(frame))
                .map_err(|_| "seek refused")?;
            oracle.apply(PlaybackCommand::PlayFrom(frame));
        }
        oracle.render(&mut expected)?;
        let mut rendered = None;
        let counts = measure(|| {
            callback.begin_block();
            rendered = Some(callback.render(&mut actual));
        });
        rendered.ok_or("render not called")??;
        assert_eq!(counts.count_total, 0);
        assert_eq!(counts.bytes_total, 0);
        assert_eq!(counts.count_current, 0);
        assert_eq!(counts.bytes_current, 0);
        assert_eq!(actual, expected);
        assert!(actual.iter().flatten().any(|value| value.abs() > 1.0e-6));
    }
    control.request_shutdown();
    let counts = measure(|| {
        callback.begin_block();
        callback.render(&mut actual).expect("shutdown render");
    });
    assert_eq!(counts.count_total, 0);
    assert_eq!(counts.bytes_total, 0);
    assert_eq!(counts.count_current, 0);
    assert_eq!(counts.bytes_current, 0);
    control.reclaim();
    Ok(())
}

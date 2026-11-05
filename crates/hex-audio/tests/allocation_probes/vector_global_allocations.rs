//! Actual prepared-callback coverage for Vector global/controller transitions.
use allocation_counter::measure;
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    NativeDeviceClass, PreparedRevision, SineTable,
    live_expression::{LiveExpressionInput, Snapshot},
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, sync::Arc};

#[test]
fn controller_flood_algorithm_handoff_cancel_and_shutdown_do_not_allocate_or_deallocate()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(48_000)?, 16)?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "vector globals".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
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
    let class = NativeDeviceClass::Instrument;
    let mut state = hex_engine::default_native_device_state_for_class(
        class,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )?;
    for (id, value) in [
        (608, ParameterValue::Choice(1)),
        (601, ParameterValue::Choice(1)), // Two sounding voices, three admitted notes.
        (5, ParameterValue::Float(0.7)),
        (8, ParameterValue::Float(0.35)),
        (11, ParameterValue::Float(0.25)),
        (82, ParameterValue::Bool(true)),
        (87, ParameterValue::Float(20.0)),
        (20001, ParameterValue::Bool(true)),
        (20002, ParameterValue::Choice(4)),
        (628, ParameterValue::Choice(7)),
        (629, ParameterValue::Float(100.0)),
        (636, ParameterValue::Choice(9)),
        (637, ParameterValue::Float(50.0)),
        (638, ParameterValue::Choice(10)),
        (639, ParameterValue::Float(-25.0)),
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
    for pitch in [60, 64, 67] {
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(4)?,
            pitch,
            channel: 0,
            velocity: 100,
        })?;
    }
    let table = Arc::new(SineTable::new());
    let mut revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::clone(&table),
        None,
    )?);
    let input = Arc::new(LiveExpressionInput::new(1, track, device));
    let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    playback.prepare_live_expression(editor.project(), Arc::clone(&input))?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    oracle.prepare_live_expression(editor.project(), Arc::clone(&input))?;
    oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = prepared::channel(48_000)?;
    control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 1)
        .map_err(|error| error.reason)?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play refused")?;
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    for step in 0..4 {
        if step == 1 {
            // Native input is represented by its exact atomic producer. Floods
            // coalesce before the callback and never grow its event workload.
            for value in 0..50_000 {
                input.publish(
                    Snapshot::new(
                        (value % 128) as u8,
                        (value % 16384 - 8192) as i16,
                        (value % 128) as u8,
                    )
                    .ok_or("invalid fixture")?,
                );
            }
            input.publish(Snapshot::new(80, 4096, 100).ok_or("invalid fixture")?);
        } else if step == 2 {
            let state = hex_engine::patch_native_device_parameter_for_class(
                class,
                editor
                    .project()
                    .device(device)
                    .ok_or("device missing")?
                    .state(),
                ParameterId::from_static(600),
                ParameterValue::Choice(8),
            )?;
            editor.execute(Command::ReplaceDeviceState { device, state })?;
            let next = Arc::new(PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::clone(&table),
                Some(revision.session()),
            )?);
            let mut candidate = Playback::try_from_prepared_with_session(Arc::clone(&next))?;
            candidate.prepare_live_expression(editor.project(), Arc::clone(&input))?;
            let mut next_oracle = Playback::try_from_prepared_with_session(Arc::clone(&next))?;
            next_oracle.prepare_live_expression(editor.project(), Arc::clone(&input))?;
            next_oracle
                .try_inherit_runtime_from(&mut oracle)
                .map_err(|_| "oracle handoff refused")?;
            control
                .publish_with_policy_and_context_generation(
                    candidate,
                    PublicationPolicy::InheritRuntime,
                    1,
                )
                .map_err(|error| error.reason)?;
            oracle = next_oracle;
            revision = next;
        } else if step == 3 {
            input.cancel();
            assert_eq!(input.snapshot(), Snapshot::default());
        }
        oracle.render(&mut expected)?;
        let mut rendered = None;
        let counts = measure(|| {
            callback.begin_block();
            rendered = Some(callback.render(&mut actual));
        });
        rendered.ok_or("render not called")??;
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0),
            "transition {step}"
        );
        assert_eq!(
            actual, expected,
            "actual callback must preserve the validated engine PCM"
        );
        assert!(actual.iter().flatten().all(|sample| sample.is_finite()));
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
        control.reclaim();
    }
    control.request_shutdown();
    let mut rendered = None;
    let counts = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut actual));
    });
    rendered.ok_or("shutdown not rendered")??;
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    control.reclaim();
    Ok(())
}

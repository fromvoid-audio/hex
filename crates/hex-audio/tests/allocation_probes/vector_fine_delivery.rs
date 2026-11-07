//! Fine-Hz edits must reach the actual active graph, including held notes and history.
use allocation_counter::measure;
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    NativeDeviceClass, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering},
};

#[test]
fn fine_hz_edits_undo_redo_and_new_notes_reach_the_active_graph_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    for global_active in [true, false] {
        check_delivery(global_active)?;
    }
    Ok(())
}

fn check_delivery(global_active: bool) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(48_000)?, 16)?;
    let class = NativeDeviceClass::Instrument;
    let state = hex_engine::default_native_device_state_for_class(
        class,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )?;
    let state = hex_engine::patch_native_device_parameter_for_class(
        class,
        &state,
        ParameterId::from_static(608),
        ParameterValue::Choice(u32::from(global_active)),
    )?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "fine Hz delivery".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state,
    })?
    else {
        return Err("track missing".into());
    };
    let device = editor
        .project()
        .track(track)
        .and_then(|t| t.instrument())
        .ok_or("instrument missing")?
        .id();
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })?
    else {
        return Err("clip missing".into());
    };
    for start in [0, 3] {
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(start)?,
            duration: BeatTime::from_quarters(2)?,
            pitch: 69,
            channel: 0,
            velocity: 127,
        })?;
    }
    let table = Arc::new(SineTable::new());
    let mut graph = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::clone(&table),
        None,
    )?);
    let (mut control, mut callback) = prepared::channel(48_000)?;
    let mut expected_revision = control
        .publish_with_policy_and_context_generation(
            Playback::try_from_prepared_with_session(Arc::clone(&graph))?,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|e| e.reason)?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play refused")?;
    let mut samples = [[0.0; 2]; 4096];
    for (step, offset) in [0.0, 150.0, -125.0, 150.0, -125.0, 0.0, -125.0]
        .into_iter()
        .enumerate()
    {
        if step > 0 {
            match step {
                3 => assert!(editor.undo()),
                4 => assert!(editor.redo()),
                5 => {
                    assert!(editor.undo());
                    assert!(editor.undo());
                }
                6 => {
                    assert!(editor.redo());
                    assert!(editor.redo());
                }
                _ => {
                    let state = hex_engine::patch_native_device_parameter_for_class(
                        class,
                        editor
                            .project()
                            .device(device)
                            .ok_or("device missing")?
                            .state(),
                        ParameterId::from_static(609),
                        ParameterValue::Float(offset),
                    )?;
                    editor.execute(Command::ReplaceDeviceState { device, state })?;
                }
            }
            assert_eq!(
                hex_engine::native_device_parameter_for_class(
                    class,
                    editor
                        .project()
                        .device(device)
                        .ok_or("device missing")?
                        .state(),
                    ParameterId::from_static(609)
                )?,
                ParameterValue::Float(offset)
            );
            let next = Arc::new(PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::clone(&table),
                Some(graph.session()),
            )?);
            expected_revision = control
                .publish_with_policy_and_context_generation(
                    Playback::try_from_prepared_with_session(Arc::clone(&next))?,
                    PublicationPolicy::InheritRuntime,
                    1,
                )
                .map_err(|e| e.reason)?;
            graph = next;
        }
        let mut result = None;
        let allocation = measure(|| {
            callback.begin_block();
            result = Some(callback.render(&mut samples));
        });
        result.ok_or("callback not run")??;
        assert_eq!(
            (
                allocation.count_total,
                allocation.bytes_total,
                allocation.count_current,
                allocation.bytes_current
            ),
            (0, 0, 0, 0),
            "step {step}"
        );
        assert_eq!(
            control.status.applied_revision.load(Ordering::Acquire),
            expected_revision,
            "retained state must reach active graph at step {step}: {:?}",
            control.status.rejected_revision()
        );
        assert!(samples.iter().flatten().all(|x| x.is_finite()));
        let crossings = samples
            .windows(2)
            .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
            .count();
        let frequency = crossings as f64 * 48_000.0 / samples.len() as f64;
        assert!(
            (frequency - (440.0 + f64::from(offset))).abs() < 20.0,
            "held-note frequency {frequency}, offset {offset}, step {step}"
        );
        control.reclaim();
    }
    control
        .command(PlaybackCommand::PlayFrom(72_000))
        .map_err(|_| "new-note seek refused")?;
    callback.begin_block();
    callback.render(&mut samples)?;
    let crossings = samples
        .windows(2)
        .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
        .count();
    assert!(
        (crossings as f64 * 48_000.0 / samples.len() as f64 - 315.0).abs() < 20.0,
        "new notes must use the offset too"
    );
    control.request_shutdown();
    let allocation = measure(|| {
        callback.begin_block();
        callback.render(&mut samples).expect("shutdown render");
    });
    assert_eq!(
        (
            allocation.count_total,
            allocation.bytes_total,
            allocation.count_current,
            allocation.bytes_current
        ),
        (0, 0, 0, 0)
    );
    control.reclaim();
    Ok(())
}

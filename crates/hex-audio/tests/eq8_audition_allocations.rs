//! Actual prepared-callback allocation probe for transient EQ8 audition.
#![cfg(feature = "allocation-probe")]

use allocation_counter::{AllocationInfo, measure};
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, sync::Arc};

const RATE: u32 = 48_000;

fn assert_zero(info: AllocationInfo, stage: &str) {
    assert_eq!(info.count_total, 0, "{stage}: callback allocated");
    assert_eq!(info.bytes_total, 0, "{stage}: callback allocated bytes");
    assert_eq!(
        info.count_current, 0,
        "{stage}: net allocation count changed"
    );
    assert_eq!(
        info.bytes_current, 0,
        "{stage}: net allocation bytes changed"
    );
}

fn prepare(editor: &Editor) -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
    Ok(Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?))
}

#[test]
fn eq8_audition_begin_edit_release_hard_cancel_and_shutdown_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Track(track) = editor.execute(Command::AddMidiTrack {
        name: "eq8 audition allocation".into(),
    })?
    else {
        return Err("track missing".into());
    };
    // Put the default Operator into its mutable feedback path so the later
    // source-removal publication exercises an actual instrument-release tail.
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("operator instrument missing")?
        .id();
    let mut operator_state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )?;
    for (id, value) in [
        (29, ParameterValue::Float(1.0)),
        (33, ParameterValue::Float(1.0)),
        (34, ParameterValue::Float(1.0)),
        (35, ParameterValue::Float(1.0)),
        (36, ParameterValue::Float(0.0)),
        (37, ParameterValue::Float(0.0)),
        (38, ParameterValue::Float(0.005)),
        (78, ParameterValue::Float(0.4)),
    ] {
        operator_state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            &operator_state,
            ParameterId::from_static(id),
            value,
        )?;
    }
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: operator_state,
    })?;
    let mut state = hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.eq8"))?;
    // EQ8 defaults to all bands disabled. Enable the tested band in both
    // stereo banks so the callback admission test exercises valid DSP work.
    for id in [1, 41] {
        state = hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(id),
            ParameterValue::Bool(true),
        )?;
    }
    let Created::Device(device) = editor.execute(Command::InsertDevice {
        track,
        before: None,
        state,
    })?
    else {
        return Err("eq8 device missing".into());
    };
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })?
    else {
        return Err("midi clip missing".into());
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;

    let first_revision = prepare(&editor)?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    let revision = control
        .publish_with_policy_and_context_generation(
            Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|_| "initial publication refused")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play command refused")?;
    let identity = hex_engine::PostNodeIdentity::new(
        hex_engine::ProcessorChainOwner::Track(track.value()),
        device.value(),
        DeviceTypeId::from_static("hex.eq8"),
    );
    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 1, revision, RATE, selection)
        .map_err(|_| "audition admission refused")?;
    let mut output = [[0.0; 2]; 128];
    let mut rendered = None;
    let measured = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut output));
    });
    rendered.ok_or("begin render missing")??;
    assert_zero(measured, "begin audition");
    control.reclaim();
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        revision,
        "initial prepared graph must become active"
    );
    assert!(output.iter().flatten().any(|value| value.abs() > 1.0e-6));
    assert_eq!(
        control
            .processor_audition
            .desired()
            .map(|target| target.selection),
        Some(selection),
        "the transient lease remains held across a compatible parameter handoff"
    );

    let current_state = editor
        .project()
        .track(track)
        .and_then(|track| {
            track
                .devices()
                .iter()
                .find(|candidate| candidate.id() == device)
        })
        .ok_or("eq8 device state missing after first edit")?
        .state();
    let second_state = hex_engine::patch_native_device_parameter(
        current_state,
        ParameterId::from_static(5),
        ParameterValue::Float(2.0),
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: second_state,
    })?;
    let second_edit = prepare(&editor)?;
    let second_edit_revision = control
        .publish_with_policy_and_context_generation(
            Playback::try_from_prepared_with_session(second_edit)?,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|_| "second edited publication refused")?;
    rendered = None;
    let measured = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut output));
    });
    rendered.ok_or("second edit handoff render missing")??;
    assert_zero(measured, "second live parameter handoff");
    control.reclaim();
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        second_edit_revision,
        "the second prepared parameter publication must be adopted"
    );
    assert_eq!(
        control
            .processor_audition
            .desired()
            .map(|target| target.selection),
        Some(selection),
        "repeated compatible edits keep the transient lease held"
    );
    assert!(output.iter().flatten().any(|value| value.abs() > 1.0e-6));

    // Publish a real EQ8 gain edit while the transient target remains held.
    let changed_state = hex_engine::patch_native_device_parameter(
        editor
            .project()
            .track(track)
            .and_then(|track| track.devices().first())
            .ok_or("eq8 device state missing")?
            .state(),
        ParameterId::from_static(4),
        ParameterValue::Float(6.0),
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: changed_state,
    })?;
    let edited = prepare(&editor)?;
    let edited_revision = control
        .publish_with_policy_and_context_generation(
            Playback::try_from_prepared_with_session(edited)?,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|_| "edited publication refused")?;
    rendered = None;
    let measured = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut output));
    });
    rendered.ok_or("edit handoff render missing")??;
    assert_zero(measured, "live parameter handoff");
    control.reclaim();
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        edited_revision,
        "compatible live parameter publication must be adopted"
    );
    assert!(output.iter().flatten().any(|value| value.abs() > 1.0e-6));

    // Removing the active note from the prepared candidate moves Operator and
    // its EQ8 suffix into a release tail. The held transient selector should
    // transfer to the candidate's current EQ8 pair, while the old tail is
    // hard-cleared before this render.
    editor.execute(Command::RemoveMidiClip(clip))?;
    let current_instrument_state = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("operator instrument missing after edit")?
        .state();
    let analytic_operator_state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        current_instrument_state,
        ParameterId::from_static(78),
        ParameterValue::Float(0.0),
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: analytic_operator_state,
    })?;
    let release_candidate = prepare(&editor)?;
    let release_candidate_revision = control
        .publish_with_policy_and_context_generation(
            Playback::try_from_prepared_with_session(release_candidate)?,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|_| "release-tail publication refused")?;
    rendered = None;
    let measured = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut output));
    });
    rendered.ok_or("release-tail handoff render missing")??;
    assert_zero(measured, "release-tail audition transfer");
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        release_candidate_revision,
        "release-tail candidate must be adopted"
    );
    assert_eq!(
        control
            .processor_audition
            .desired()
            .map(|target| target.selection),
        Some(selection),
        "a transferred selector stays accepted on the new current chain"
    );
    assert!(output.iter().flatten().any(|value| value.abs() > 1.0e-6));

    control
        .processor_audition
        .release()
        .map_err(|_| "audition release refused")?;
    rendered = None;
    let measured = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut output));
    });
    rendered.ok_or("release render missing")??;
    assert_zero(measured, "smooth release");
    assert!(output.iter().flatten().any(|value| value.abs() > 1.0e-6));

    control
        .processor_audition
        .cancel_immediately()
        .map_err(|_| "hard audition cancel refused")?;
    rendered = None;
    let measured = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut output));
    });
    rendered.ok_or("hard cancel render missing")??;
    assert_zero(measured, "hard cancel");
    assert!(output.iter().flatten().any(|value| value.abs() > 1.0e-6));

    control.request_shutdown();
    rendered = None;
    let measured = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut output));
    });
    rendered.ok_or("shutdown render missing")??;
    assert_zero(measured, "shutdown");
    control.reclaim();
    Ok(())
}

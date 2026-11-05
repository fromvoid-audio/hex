//! Actual prepared-callback allocation probe for transient EQ8 audition.

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

// Model the documented successful-publication contract outside measured scopes:
// ordinary history inheritance, transient-only suffix transfer when required,
// then hard cancellation of retired auditions before any new output.
fn inherit_audition_oracle(
    previous: &mut Playback,
    revision: &Arc<PreparedRevision>,
    identity: hex_engine::PostNodeIdentity,
    selection: hex_engine::ProcessorAuditionSelection,
) -> Result<Playback, Box<dyn Error>> {
    let mut next = Playback::try_from_prepared_with_session(Arc::clone(revision))?;
    next.try_inherit_runtime_from(previous)
        .map_err(|error| format!("oracle graph handoff refused: {error:?}"))?;
    if !next.processor_audition_is_active(identity, selection) {
        assert!(next.transfer_processor_audition_from_release_tail(identity, selection));
    }
    assert!(next.processor_audition_is_active(identity, selection));
    next.cancel_retired_processor_auditions();
    Ok(next)
}

#[test]
fn eq8_audition_begin_edit_release_hard_cancel_and_shutdown_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    audition_callback_allocations(false)
}

#[test]
fn eq8_return_audition_begin_edit_release_hard_cancel_and_shutdown_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    audition_callback_allocations(true)
}

fn audition_callback_allocations(on_return: bool) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
        name: "eq8 audition allocation".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
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
        (37, ParameterValue::Float(0.001)),
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
    // A second nonneutral band makes normal cascade output distinguishable
    // from auditioning only band0; a single flat band is a weak audible oracle.
    for id in [11, 51] {
        state = hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(id),
            ParameterValue::Bool(true),
        )?;
    }
    for id in [14, 54] {
        state = hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(id),
            ParameterValue::Float(-9.0),
        )?;
    }
    let (owner, insertion) = if on_return {
        let Created::ReturnTrack(return_track) = editor.execute(Command::AddReturnTrack {
            name: "eq8 audition return".into(),
        })?
        else {
            return Err("return missing".into());
        };
        editor.execute(Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        })?;
        (
            hex_engine::ProcessorChainOwner::Return(return_track.value()),
            Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            },
        )
    } else {
        (
            hex_engine::ProcessorChainOwner::Track(track.value()),
            Command::InsertDevice {
                track,
                before: None,
                state,
            },
        )
    };
    let Created::Device(device) = editor.execute(insertion)? else {
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
        owner,
        device.value(),
        DeviceTypeId::from_static("hex.eq8"),
    );
    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    // Oracles and construction stay outside measured callback scopes. Exact PCM
    // proves the typed target was actually selected, not merely requested.
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    oracle.apply(PlaybackCommand::Play);
    assert!(oracle.select_processor_audition(identity, Some(selection)));
    let mut expected = [[0.0; 2]; 128];
    oracle.render(&mut expected)?;
    let mut normal = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    normal.apply(PlaybackCommand::Play);
    let mut normal_output = [[0.0; 2]; 128];
    normal.render(&mut normal_output)?;
    assert_ne!(
        expected, normal_output,
        "audition changes the routed output"
    );
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
    assert_eq!(
        output, expected,
        "the actual callback auditions the selected owner"
    );
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
        .device(device)
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
    oracle = inherit_audition_oracle(&mut oracle, &second_edit, identity, selection)?;
    oracle.render(&mut expected)?;
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
    assert_eq!(
        output, expected,
        "held audition follows the accepted parameter edit"
    );
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
            .device(device)
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
    oracle = inherit_audition_oracle(&mut oracle, &edited, identity, selection)?;
    oracle.render(&mut expected)?;
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
    assert_eq!(
        output, expected,
        "held audition follows the accepted parameter edit"
    );
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

    // Remove the active source and switch instrument kind while independently
    // retained old note tails remain. The current Track/Return target must keep
    // its held audition; any required suffix transfer obeys the same contract
    // as the earlier parameter handoffs, without auditioning old tails.
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
    oracle = inherit_audition_oracle(&mut oracle, &release_candidate, identity, selection)?;
    oracle.render(&mut expected)?;
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
    assert_zero(measured, "source-removal audition handoff");
    assert_eq!(
        output, expected,
        "source removal preserves the current audition target"
    );
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
    assert!(oracle.select_processor_audition(identity, None));
    oracle.render(&mut expected)?;
    rendered = None;
    let measured = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut output));
    });
    rendered.ok_or("release render missing")??;
    assert_zero(measured, "smooth release");
    assert_eq!(
        output, expected,
        "the actual callback follows release/cancel PCM"
    );
    assert!(output.iter().flatten().any(|value| value.abs() > 1.0e-6));

    control
        .processor_audition
        .cancel_immediately()
        .map_err(|_| "hard audition cancel refused")?;
    oracle.cancel_all_processor_auditions();
    oracle.render(&mut expected)?;
    rendered = None;
    let measured = measure(|| {
        callback.begin_block();
        rendered = Some(callback.render(&mut output));
    });
    rendered.ok_or("hard cancel render missing")??;
    assert_zero(measured, "hard cancel");
    assert_eq!(
        output, expected,
        "the actual callback follows release/cancel PCM"
    );
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

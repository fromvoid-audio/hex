//! Borrowed engine capture scopes, with construction/reclamation off callback.
use super::*;
use hex_engine::{
    OutputCapture, OutputCaptureError, OutputCaptureId, SessionPlayback, SessionPlaybackError,
};
use std::cell::Cell;

thread_local! { static REFUSE_GATEOFF: Cell<bool> = const { Cell::new(false) }; }
pub(super) fn refuse_gateoff(events: &[hex_lattice::NativeInstrumentEvent]) -> bool {
    REFUSE_GATEOFF.with(|fault| fault.get())
        && events
            .iter()
            .any(|event| matches!(event, hex_lattice::NativeInstrumentEvent::NoteOff { .. }))
}

fn zero(info: AllocationInfo) {
    assert_eq!(info.count_total, 0, "capture allocated");
    assert_eq!(info.bytes_total, 0, "capture allocated bytes");
    assert_eq!(info.count_current, 0, "capture allocated or freed");
    assert_eq!(info.bytes_current, 0, "capture allocated or freed bytes");
}

fn source() -> Result<(Project, TrackId, hex_project::ReturnId), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::InsertTrackWithDevice {
        name: "capture".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    })? {
        Created::Track(id) => id,
        _ => return Err("track expected".into()),
    };
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(4)?,
    })? {
        Created::Clip(id) => id,
        _ => return Err("clip expected".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_raw(1 << 26),
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("instrument expected")?
        .id();
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: NativeDeviceState::new(MUTABLE_FIXTURE_TYPE, 1, vec![1])?,
    })?;
    let bus = match editor.execute(Command::AddReturnTrack {
        name: "capture return".into(),
    })? {
        Created::ReturnTrack(id) => id,
        _ => return Err("return expected".into()),
    };
    editor.execute(Command::SetReturnSend {
        track,
        return_track: bus,
        gain: 0.5,
    })?;
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: utility_state()?,
        });
    snapshot.next_id += 1;
    snapshot.return_tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: utility_state()?,
        });
    snapshot.next_id += 1;
    Ok((Project::from_snapshot(snapshot)?, track, bus))
}

fn utility_state() -> Result<NativeDeviceState, Box<dyn Error>> {
    Ok(hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::new("hex.utility")?)?,
        ParameterId::new(11)?,
        ParameterValue::Bool(true),
    )?)
}

fn prepare(project: &Project) -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
    let definition = MutableFixtureDefinition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions)?;
    Ok(Arc::new(
        PreparedRevision::prepare_with_test_instrument_registry(
            project,
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
            &registry,
        )?,
    ))
}

#[test]
fn capture_success_preflight_and_late_refusal_allocate_and_free_nothing()
-> Result<(), Box<dyn Error>> {
    let (project, track, bus) = source()?;
    let prepared = prepare(&project)?;
    let mut runtime = SessionPlayback::new_linear_arrangement_export(Arc::clone(&prepared))?;
    runtime.apply(SessionPlaybackCommand::Play)?;
    let ids = [OutputCaptureId::Track(track), OutputCaptureId::Return(bus)];
    let plan = runtime.output_capture_plan(&ids, 512)?;
    let stale = SessionPlayback::new_linear_arrangement_export(Arc::clone(&prepared))?
        .output_capture_plan(&ids, 512)?;
    let mut main = [[9.0; 2]; 512];
    let mut track_buffer = main;
    let mut return_buffer = main;
    let mut buffers: [&mut [Frame]; 2] = [&mut track_buffer, &mut return_buffer];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    REFUSE_GATEOFF.with(|fault| fault.set(true));
    let mut result = Ok(());
    zero(measure(|| {
        result = runtime.render_with_capture(&mut main, &mut capture)
    }));
    REFUSE_GATEOFF.with(|fault| fault.set(false));
    assert!(result.is_err()); // First chunk advanced; the 375-frame note-off refused chunk two.
    assert_eq!(runtime.performance_frame(), 0);
    assert_eq!(capture.valid_frames(), 0);
    assert!(main.iter().flatten().all(|sample| *sample == 0.0));
    zero(measure(|| {
        result = runtime.render_with_capture(&mut main, &mut capture)
    }));
    result?;
    let mut oracle = SessionPlayback::new_linear_arrangement_export(prepared)?;
    oracle.apply(SessionPlaybackCommand::Play)?;
    let mut expected = [[0.0; 2]; 512];
    oracle.render(&mut expected)?;
    assert_eq!(main, expected);
    assert_eq!(capture.valid_frames(), 512);
    let mut capture = OutputCapture::new(&stale, &mut buffers);
    zero(measure(|| {
        result = runtime.render_with_capture(&mut main, &mut capture)
    }));
    assert_eq!(
        result,
        Err(SessionPlaybackError::Capture(OutputCaptureError::StalePlan))
    );
    assert_eq!(capture.valid_frames(), 0);
    let mut short = [[9.0; 2]; 511];
    let mut extra = [[9.0; 2]; 512];
    let mut short_buffers: [&mut [Frame]; 2] = [&mut short, &mut extra];
    let mut capture = OutputCapture::new(&plan, &mut short_buffers);
    zero(measure(|| {
        result = runtime.render_with_capture(&mut main, &mut capture)
    }));
    assert_eq!(
        result,
        Err(SessionPlaybackError::Capture(
            OutputCaptureError::InsufficientCapacity
        ))
    );
    assert_eq!(runtime.performance_frame(), 512);
    Ok(())
}

#[test]
fn actual_deleted_instrument_effect_and_return_tails_capture_without_callback_drops()
-> Result<(), Box<dyn Error>> {
    let (project, track, bus) = source()?;
    let prepared = prepare(&project)?;
    let mut previous = SessionPlayback::new(prepared)?;
    previous.apply(SessionPlaybackCommand::Play)?;
    previous.render(&mut [[0.0; 2]; 128])?;
    let mut editor = Editor::new(project, 4)?;
    editor.execute(Command::RemoveTrack(track))?;
    editor.execute(Command::RemoveReturnTrack(bus))?;
    let mut candidate = SessionPlayback::new(prepare(editor.project())?)?;
    candidate.try_inherit_state_from(&mut previous)?;
    let mut carried = SessionPlayback::new(prepare(editor.project())?)?;
    carried.try_inherit_state_from(&mut candidate)?;
    assert_eq!(
        candidate
            .output_capture_plan(&[OutputCaptureId::Track(track)], 512)
            .unwrap_err(),
        OutputCaptureError::MissingOwner
    );
    assert_eq!(
        candidate
            .output_capture_plan(&[OutputCaptureId::Return(bus)], 512)
            .unwrap_err(),
        OutputCaptureError::MissingOwner
    );
    let mut candidate = carried;
    let plan = candidate.output_capture_plan(
        &[OutputCaptureId::Track(track), OutputCaptureId::Return(bus)],
        512,
    )?;
    let mut main = [[0.0; 2]; 512];
    let mut track_buffer = main;
    let mut return_buffer = main;
    let mut buffers: [&mut [Frame]; 2] = [&mut track_buffer, &mut return_buffer];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    let mut result = Ok(());
    for _ in 0..32 {
        zero(measure(|| {
            result = candidate.render_with_capture(&mut main, &mut capture)
        }));
        result?;
        assert_eq!(capture.valid_frames(), 512);
        for (index, frame) in main.iter().enumerate() {
            let track = capture.output(0).ok_or("track capture unavailable")?[index];
            let bus = capture.output(1).ok_or("return capture unavailable")?[index];
            assert_eq!(*frame, [track[0] + bus[0], track[1] + bus[1]]);
        }
    }
    // Graphs/plans and completed owners are reclaimed outside measured scopes.
    Ok(())
}

// This combined headless fixture retains multiple large host/oracle owners.
// Give its test worker a finite explicit stack; runtime construction and worker
// lifetime remain outside every allocation measurement.
#[test]
fn automated_utility_callback_publication_and_late_failure_retry_are_allocation_free()
-> Result<(), Box<dyn Error>> {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            run_automated_native_allocation_probe(AutomatedFamily::Utility)
                .map_err(|e| e.to_string())
        })?
        .join()
        .map_err(|_| "automation allocation probe panicked")?
        .map_err(Into::into)
}
#[test]
fn automated_saturator_callback_publication_and_late_failure_retry_are_allocation_free()
-> Result<(), Box<dyn Error>> {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            run_automated_native_allocation_probe(AutomatedFamily::Saturator)
                .map_err(|e| e.to_string())
        })?
        .join()
        .map_err(|_| "saturator allocation probe panicked")?
        .map_err(Into::into)
}
/// Pulse #4106: an EQ8 stereo to mid/side switch during playback hands the
/// outgoing routing to a fixed inline crossfade. A second (mid/side to
/// left/right) switch published while that fade runs is deferred as busy by the
/// callback and admitted once the fade drains. The handoffs, the deferral and
/// the crossfading callback blocks allocate and free nothing.
#[test]
fn eq8_channel_mode_switch_crossfade_is_allocation_free() -> Result<(), Box<dyn Error>> {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| run_eq8_mode_switch_probe().map_err(|e| e.to_string()))?
        .join()
        .map_err(|_| "eq8 mode switch allocation probe panicked")?
        .map_err(Into::into)
}
fn run_eq8_mode_switch_probe() -> Result<(), Box<dyn Error>> {
    use hex_devices::eq8::{EQ8_DEFINITION, parameter};
    use hex_lattice_contract::NativeDeviceMetadata;
    let (project, track, _) = source()?;
    let mut editor = Editor::new(project, 16)?;
    let device = editor.project().track(track).ok_or("track")?.devices()[0].id();
    let mut state = EQ8_DEFINITION.default_state()?;
    for (id, value) in [
        (parameter::enabled(0, 0), ParameterValue::Bool(true)),
        (parameter::frequency(0, 0), ParameterValue::Float(1_000.0)),
        (parameter::gain(0, 0), ParameterValue::Float(12.0)),
    ] {
        state = EQ8_DEFINITION.patch_parameter(&state, id, value)?;
    }
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: state.clone(),
    })?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "publish")?;
    control
        .command_with_ticket(PlaybackCommand::Play)
        .map_err(|_| "play")?;
    let mut output = [[0.0; 2]; 512];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: EQ8_DEFINITION.patch_parameter(
            &state,
            parameter::MODE,
            ParameterValue::Choice(2),
        )?,
    })?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "replacement")?;
    // Small blocks keep the mode crossfade active across several callbacks.
    let mut small = [[0.0; 2]; 64];
    measured_block(&mut callback, &mut small);
    assert_eq!(control.reclaim(), 1, "first switch admitted");
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: EQ8_DEFINITION.patch_parameter(
            &state,
            parameter::MODE,
            ParameterValue::Choice(1),
        )?,
    })?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "second replacement")?;
    // 128 of the 256 fade frames have rendered: the second switch is busy.
    measured_block(&mut callback, &mut small);
    assert_eq!(control.reclaim(), 0, "second switch deferred while fading");
    for _ in 0..8 {
        measured_block(&mut callback, &mut small);
    }
    assert_eq!(
        control.reclaim(),
        1,
        "second switch admitted after the fade"
    );
    assert!(control.status.rejected_revision().is_none());
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
/// Pulse #4122: a flexible EQ stereo to mid/side switch during playback
/// captures the outgoing routing (even when the same replacement removes
/// nodes) into fade storage allocated at runtime creation. A second switch
/// published while that fade runs is deferred as busy by the callback and
/// admitted once the fade drains. The handoffs, the deferral and the
/// crossfading callback blocks allocate and free nothing.
#[test]
fn flexible_eq_channel_mode_switch_crossfade_is_allocation_free() -> Result<(), Box<dyn Error>> {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| run_flexible_eq_mode_switch_probe().map_err(|e| e.to_string()))?
        .join()
        .map_err(|_| "flexible eq mode switch allocation probe panicked")?
        .map_err(Into::into)
}
fn run_flexible_eq_mode_switch_probe() -> Result<(), Box<dyn Error>> {
    use hex_devices::eq::{EqState, Node, NodeId};
    let (project, track, _) = source()?;
    let mut editor = Editor::new(project, 16)?;
    let device = editor.project().track(track).ok_or("track")?.devices()[0].id();
    let mut settings = EqState::default();
    settings.add(Node::new(NodeId::new(1)?, 1_000.0, 12.0, 0)?)?;
    settings.add(Node::new(NodeId::new(2)?, 1_000.0, -6.0, 3)?)?;
    settings.add(Node::new(NodeId::new(3)?, 3_000.0, 6.0, 0)?)?;
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: settings.encode()?,
    })?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "publish")?;
    control
        .command_with_ticket(PlaybackCommand::Play)
        .map_err(|_| "play")?;
    let mut output = [[0.0; 2]; 512];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
    // One replacement switches to mid/side and removes both stereo bells, so
    // the outgoing routing has more bands than the incoming one has nodes.
    settings.mode = 2;
    settings.remove(NodeId::new(1)?)?;
    settings.remove(NodeId::new(3)?)?;
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: settings.encode()?,
    })?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "replacement")?;
    // Small blocks keep the mode crossfade active across several callbacks.
    let mut small = [[0.0; 2]; 64];
    measured_block(&mut callback, &mut small);
    assert_eq!(control.reclaim(), 1, "first switch admitted");
    settings.mode = 1;
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: settings.encode()?,
    })?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "second replacement")?;
    // 128 of the 256 fade frames have rendered: the second switch is busy.
    measured_block(&mut callback, &mut small);
    assert_eq!(control.reclaim(), 0, "second switch deferred while fading");
    for _ in 0..8 {
        measured_block(&mut callback, &mut small);
    }
    assert_eq!(
        control.reclaim(),
        1,
        "second switch admitted after the fade"
    );
    assert!(control.status.rejected_revision().is_none());
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
/// Pulse #4085: a direct (unautomated) Saturator drive/output edit is
/// published as a replacement whose history handoff ramps the gains. The
/// handoff and the ramping callback blocks allocate and free nothing.
#[test]
fn direct_saturator_gain_edit_handoff_ramp_is_allocation_free() -> Result<(), Box<dyn Error>> {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            run_direct_saturator_edit_probe(false, false).map_err(|e| e.to_string())?;
            run_direct_saturator_edit_probe(true, false).map_err(|e| e.to_string())?;
            run_direct_saturator_edit_probe(true, true).map_err(|e| e.to_string())
        })?
        .join()
        .map_err(|_| "direct saturator allocation probe panicked")?
        .map_err(Into::into)
}
fn run_direct_saturator_edit_probe(
    wet_automated: bool,
    retained_gains: bool,
) -> Result<(), Box<dyn Error>> {
    use hex_devices::saturator::{SATURATOR_DEFINITION, parameter};
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue};
    let (project, track, _) = source()?;
    let mut editor = Editor::new(project, 16)?;
    let device = editor.project().track(track).ok_or("track")?.devices()[0].id();
    let state = SATURATOR_DEFINITION.default_state()?;
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: state.clone(),
    })?;
    if wet_automated {
        editor.execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::new(3)?,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.75)?,
                outgoing: AutomationInterpolation::Step,
            }],
        })?;
    }
    if retained_gains {
        for (parameter, normalized) in [
            (parameter::DRIVE, 42.0 / 72.0),
            (parameter::OUTPUT, 33.0 / 42.0),
        ] {
            editor.execute(Command::ReplaceDeviceAutomation {
                device,
                parameter,
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(normalized)?,
                    outgoing: AutomationInterpolation::Step,
                }],
            })?;
        }
    }
    let revision = prepare(editor.project())?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(revision)?)
        .map_err(|_| "publish")?;
    control
        .command_with_ticket(PlaybackCommand::Play)
        .map_err(|_| "play")?;
    let mut output = [[0.0; 2]; 512];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
    let edited = SATURATOR_DEFINITION.patch_parameter(
        &SATURATOR_DEFINITION.patch_parameter(
            &state,
            parameter::DRIVE,
            ParameterValue::Float(12.0),
        )?,
        parameter::OUTPUT,
        ParameterValue::Float(-6.0),
    )?;
    if retained_gains {
        editor.execute(Command::ManualDeviceEdit {
            device,
            state: edited,
            targets: vec![
                hex_lattice_contract::DeviceAutomationTarget::Parameter(parameter::DRIVE),
                hex_lattice_contract::DeviceAutomationTarget::Parameter(parameter::OUTPUT),
            ],
        })?;
    } else {
        editor.execute(Command::ReplaceDeviceState {
            device,
            state: edited,
        })?;
    }
    let edited_revision = control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "replacement")?;
    // Small blocks keep the 5 ms ramp active across several callbacks.
    let mut small = [[0.0; 2]; 64];
    for _ in 0..8 {
        measured_block(&mut callback, &mut small);
    }
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        edited_revision
    );
    if retained_gains {
        for parameter in [parameter::DRIVE, parameter::OUTPUT] {
            editor.execute(Command::SetDeviceAutomationReadSelection {
                device,
                target: hex_lattice_contract::DeviceAutomationTarget::Parameter(parameter),
                selection: hex_project::AutomationReadSelection::Authored,
            })?;
        }
        let resumed_revision = control
            .publish(Playback::try_from_prepared_with_session(prepare(
                editor.project(),
            )?)?)
            .map_err(|_| "reenable publication")?;
        measured_block(&mut callback, &mut small);
        assert_eq!(
            control.status.applied_revision.load(Ordering::Acquire),
            resumed_revision
        );
    }
    assert!(control.status.rejected_revision().is_none());
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
/// Pulse #4101: a direct Utility gain edit changes only the host-owned static
/// matrix. The published handoff starts the 5 ms matrix ramp, and the ramping
/// callback blocks allocate and free nothing.
#[test]
fn direct_utility_gain_edit_matrix_ramp_is_allocation_free() -> Result<(), Box<dyn Error>> {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| run_direct_utility_edit_probe().map_err(|e| e.to_string()))?
        .join()
        .map_err(|_| "direct utility allocation probe panicked")?
        .map_err(Into::into)
}
fn run_direct_utility_edit_probe() -> Result<(), Box<dyn Error>> {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::NativeDeviceMetadata;
    let (project, track, _) = source()?;
    let mut editor = Editor::new(project, 16)?;
    let device = editor.project().track(track).ok_or("track")?.devices()[0].id();
    let state = UTILITY_DEFINITION.default_state()?;
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: state.clone(),
    })?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "publish")?;
    control
        .command_with_ticket(PlaybackCommand::Play)
        .map_err(|_| "play")?;
    let mut output = [[0.0; 2]; 512];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: UTILITY_DEFINITION.patch_parameter(
            &state,
            parameter::GAIN,
            ParameterValue::Float(0.25),
        )?,
    })?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "replacement")?;
    // Small blocks keep the 5 ms ramp active across several callbacks.
    let mut small = [[0.0; 2]; 64];
    for _ in 0..8 {
        measured_block(&mut callback, &mut small);
    }
    assert!(control.status.rejected_revision().is_none());
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
/// Pulse #4105: behind a sounding instrument, a stateful DC-remove Utility
/// keeps the effect suffix nonquiescent. A gain edit on the following
/// memoryless Utility is a static-matrix-only change, so the callback handoff
/// retargets the suffix and ramps instead of retaining it as a tail; the
/// published handoff and ramping blocks allocate and free nothing.
#[test]
fn instrument_suffix_static_matrix_handoff_is_allocation_free() -> Result<(), Box<dyn Error>> {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| run_instrument_suffix_matrix_probe().map_err(|e| e.to_string()))?
        .join()
        .map_err(|_| "instrument suffix allocation probe panicked")?
        .map_err(Into::into)
}
fn run_instrument_suffix_matrix_probe() -> Result<(), Box<dyn Error>> {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::NativeDeviceMetadata;
    let (project, track, _) = source()?;
    let state = UTILITY_DEFINITION.default_state()?;
    let mut snapshot = project.snapshot();
    let device = snapshot.next_id;
    snapshot.tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: device,
            bypassed: false,
            state: state.clone(),
        });
    snapshot.next_id += 1;
    let mut editor = Editor::new(Project::from_snapshot(snapshot)?, 16)?;
    let devices = editor.project().track(track).ok_or("track")?.devices();
    assert_eq!(devices.len(), 2);
    let device = devices[1].id();
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "publish")?;
    control
        .command_with_ticket(PlaybackCommand::Play)
        .map_err(|_| "play")?;
    // The fixture note ends at frame 375; hand off while it still sounds.
    let mut output = [[0.0; 2]; 256];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: UTILITY_DEFINITION.patch_parameter(
            &state,
            parameter::GAIN,
            ParameterValue::Float(0.25),
        )?,
    })?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "replacement")?;
    // Small blocks keep the 5 ms ramp active across several callbacks.
    let mut small = [[0.0; 2]; 64];
    for _ in 0..8 {
        measured_block(&mut callback, &mut small);
    }
    assert!(control.status.rejected_revision().is_none());
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
#[test]
fn automated_auto_pan_callback_publication_and_late_failure_retry_are_allocation_free()
-> Result<(), Box<dyn Error>> {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            run_automated_native_allocation_probe(AutomatedFamily::AutoPan)
                .map_err(|e| e.to_string())
        })?
        .join()
        .map_err(|_| "auto pan allocation probe panicked")?
        .map_err(Into::into)
}
#[derive(Clone, Copy)]
enum AutomatedFamily {
    Utility,
    Saturator,
    AutoPan,
}
fn run_automated_native_allocation_probe(family: AutomatedFamily) -> Result<(), Box<dyn Error>> {
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue};
    let (project, track, bus) = source()?;
    let mut editor = Editor::new(project, 16)?;
    let clip = editor.project().track(track).ok_or("track")?.midi_clips()[0].id();
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 67,
        channel: 0,
        velocity: 100,
    })?;
    let devices = [
        editor.project().track(track).ok_or("track")?.devices()[0].id(),
        editor
            .project()
            .return_tracks()
            .iter()
            .find(|t| t.id() == bus)
            .ok_or("return")?
            .devices()[0]
            .id(),
    ];
    for device in devices {
        let (state, controls): (_, &[(u32, f32, f32)]) = match family {
            AutomatedFamily::Utility => (
                None,
                &[
                    (1, 0.0, 0.04),
                    (3, 0.25, 0.75),
                    (12, 1.0, 1.0),
                    (13, 0.25, 0.75),
                ],
            ),
            AutomatedFamily::Saturator => (
                Some(hex_devices::saturator::SATURATOR_DEFINITION.default_state()?),
                &[
                    (1, 0.5, 0.75),
                    (2, 0.75, 1.0),
                    (3, 0.25, 0.75),
                    (4, 0.0, 1.0),
                ],
            ),
            AutomatedFamily::AutoPan => (
                Some(hex_devices::auto_pan::AUTO_PAN_DEFINITION.default_state()?),
                &[
                    (1, 0.25, 0.75),
                    (2, 0.25, 0.75),
                    (3, 0.9, 0.0),
                    (4, 0.0, 1.0),
                ],
            ),
        };
        if let Some(state) = state {
            editor.execute(Command::ReplaceDeviceState { device, state })?;
        }
        for &(parameter, start, end) in controls {
            editor.execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: ParameterId::new(parameter)?,
                points: vec![
                    AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(start)?,
                        outgoing: AutomationInterpolation::Linear,
                    },
                    AutomationPoint {
                        beat: BeatTime::from_raw(1 << 26),
                        value: AutomationValue::new(end)?,
                        outgoing: AutomationInterpolation::Step,
                    },
                ],
            })?;
        }
    }
    for device in devices {
        for target in [
            hex_project::DeviceAutomationTarget::HostInputGain,
            hex_project::DeviceAutomationTarget::HostOutputGain,
        ] {
            editor.execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target,
                points: vec![
                    AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.65)?,
                        outgoing: AutomationInterpolation::Linear,
                    },
                    AutomationPoint {
                        beat: BeatTime::from_raw(1 << 26),
                        value: AutomationValue::new(0.75)?,
                        outgoing: AutomationInterpolation::Step,
                    },
                ],
            })?;
        }
    }
    let revision = prepare(editor.project())?;
    let mut runtime = SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision))?;
    runtime.apply(SessionPlaybackCommand::Play)?;
    let ids = [OutputCaptureId::Track(track), OutputCaptureId::Return(bus)];
    let plan = runtime.output_capture_plan(&ids, 512)?;
    let mut output = [[0.0; 2]; 512];
    let mut track_output = output;
    let mut return_output = output;
    let mut buffers: [&mut [Frame]; 2] = [&mut track_output, &mut return_output];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    let mut result = Ok(());
    REFUSE_GATEOFF.with(|fault| fault.set(true));
    zero(measure(|| {
        result = runtime.render_with_capture(&mut output, &mut capture)
    }));
    REFUSE_GATEOFF.with(|fault| fault.set(false));
    assert!(result.is_err());
    assert_eq!(runtime.performance_frame(), 0);
    assert_eq!(capture.valid_frames(), 0);
    zero(measure(|| {
        result = runtime.render_with_capture(&mut output, &mut capture)
    }));
    result?;
    let mut oracle = SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision))?;
    oracle.apply(SessionPlaybackCommand::Play)?;
    let mut expected = [[0.0; 2]; 512];
    oracle.render(&mut expected)?;
    assert_eq!(output, expected);
    // Actual prepared callback admission, rendering, revision replacement and
    // shutdown borrow the same flat program without allocation or destruction.
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(
            &revision,
        ))?)
        .map_err(|_| "publish")?;
    control
        .command_with_ticket(PlaybackCommand::Play)
        .map_err(|_| "play")?;
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
    editor.execute(Command::ReplaceDeviceAutomation {
        device: devices[0],
        parameter: ParameterId::new(1)?,
        points: vec![AutomationPoint {
            beat: BeatTime::ZERO,
            value: AutomationValue::new(0.02)?,
            outgoing: AutomationInterpolation::Step,
        }],
    })?;
    let replacement = prepare(editor.project())?;
    control
        .publish(Playback::try_from_prepared_with_session(replacement)?)
        .map_err(|_| "replacement")?;
    for _ in 0..4 {
        measured_block(&mut callback, &mut output);
    }
    assert!(control.status.rejected_revision().is_none());
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

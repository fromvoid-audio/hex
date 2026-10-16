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
    let track = match editor.execute(Command::AddMidiTrack {
        name: "capture".into(),
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
            id: snapshot.next_id,
            bypassed: false,
            state: utility_state()?,
        });
    snapshot.next_id += 1;
    snapshot.return_tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
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

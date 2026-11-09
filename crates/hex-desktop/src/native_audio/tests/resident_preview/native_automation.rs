//! Native instrument capture through the real Session and Main output.
use super::*;

#[test]
fn native_fm4_recorded_touch_adopts_and_replays_the_actual_delivered_frames() {
    use hex_audio::native_parameter_input::{Receipt, Target};
    use hex_engine::{NativeDeviceClass, NativeTransientValue};
    use hex_project::{BeatTime, Command, Created, DeviceAutomationTarget, TrackDeviceSlot};
    use hex_session::device_gesture::{ParameterId, ParameterValue};
    use hex_session::service::{
        DeviceAutomationTakeAction as Action, DeviceAutomationTakeProgress as Progress,
        DeviceAutomationTakeScope,
    };
    use hex_storage::automation_draft::DraftProvenance;

    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_recovery(8192, files.0.join("fm4.hex-journal"));
    h.next();
    let mut applied = h.applied(0).0;
    let state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_engine::DeviceTypeId::from_static("hex.instrument.fm4"),
    )
    .unwrap();
    let inserted = h.run(Request::Edit(Command::InsertTrackWithDevice {
        name: "held fm4".into(),
        before_track: None,
        slot: TrackDeviceSlot::Instrument,
        state,
    }));
    let Some(Created::Track(track)) = inserted.created else {
        panic!("source track")
    };
    applied = h.applied(applied).0;
    let duration = BeatTime::from_quarters(4).unwrap();
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: duration,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("midi clip")
    };
    applied = h.applied(applied).0;
    let base = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration,
        pitch: 60,
        channel: 2,
        velocity: 96,
    }));
    applied = h.applied(applied).0;
    let instrument = base
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap();
    assert!(instrument.automation().is_empty());
    let device = instrument.id();
    let id = ParameterId::new(29).unwrap();
    let kind = DeviceAutomationTarget::Parameter(id);
    let context = h.submitted.unwrap().1.context;
    h.run(Request::ArmNativeParameterRecording {
        project_generation: context.project_generation,
        revision: context.editor_revision,
        target: Some((device, kind)),
    });
    applied = h.applied(applied).0;
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let delivered = |h: &ResidentHarness| {
        let mut pcm = [[0.0; 2]; 64];
        let mut host = [0.0; 128];
        let mut slot = h.callback.lock().unwrap();
        slot.as_mut()
            .unwrap()
            .render_direct(&mut pcm, &mut host)
            .unwrap();
        assert!(
            host.chunks_exact(2)
                .zip(&pcm)
                .all(|(out, frame)| out == frame)
        );
        pcm
    };
    let baseline: Vec<_> = (0..4).flat_map(|_| delivered(&h)).collect();
    assert!(baseline.iter().any(|frame| frame[0].abs() > 0.001));
    let (source, transport_epoch) = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .live_native_parameter_source()
        .unwrap();
    let input =
        hex_engine::native_parameter_input_target(&base.state.project, device, kind).unwrap();
    let audio_target = Target {
        source,
        transport_epoch,
        input,
        gesture_serial: 1,
    };
    let scope = DeviceAutomationTakeScope {
        target: input.into(),
        stamp: DraftProvenance {
            sample_rate: 48_000,
            output_identity: source.output_identity,
            project_generation: source.context.project_generation,
            editor_revision: source.context.editor_revision,
            audio_revision: source.audio_revision,
            transport_epoch,
            gesture_serial: audio_target.gesture_serial,
        },
    };
    let mut serial = 1;
    let send = |h: &mut ResidentHarness, serial: &mut u64, action| {
        let polling = matches!(&action, Action::Poll);
        let request = Request::DeviceAutomationTake {
            request_id: *serial,
            scope,
            action,
        };
        if !polling {
            let event = h.run(request);
            let completion = event.device_automation_take_complete.as_ref().unwrap();
            assert_eq!(completion.request_id, *serial);
            *serial += 1;
            return (completion.progress.clone(), event);
        }
        h.service.submit(request).unwrap();
        let limit = Instant::now() + Duration::from_secs(3);
        loop {
            h.pump();
            delivered(h);
            if let Ok(event) = h.service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                if let Some(completion) = &event.device_automation_take_complete
                    && completion.request_id == *serial
                {
                    *serial += 1;
                    return (completion.progress.clone(), event);
                }
            }
            assert!(Instant::now() < limit, "native take request did not settle");
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    assert_eq!(
        send(&mut h, &mut serial, Action::Begin).0,
        Progress::Started
    );
    h.audio
        .as_mut()
        .unwrap()
        .control
        .native_parameter_begin(
            audio_target,
            NativeTransientValue::Parameter {
                id,
                value: ParameterValue::Float(0.0),
            },
        )
        .unwrap();
    let first_muted = delivered(&h);
    let Some(Receipt::Applied {
        sequence,
        application,
        ..
    }) = h.audio.as_mut().unwrap().control.native_parameter_receipt()
    else {
        panic!("delivered FM4 set")
    };
    assert_eq!(
        send(
            &mut h,
            &mut serial,
            Action::Append(Arc::from([(sequence, application)]))
        )
        .0,
        Progress::Appended
    );
    let mut touch_pcm = Vec::from(first_muted);
    for _ in 0..5 {
        touch_pcm.extend(delivered(&h));
    }
    assert!(touch_pcm[256..].iter().all(|frame| *frame == [0.0; 2]));
    h.audio
        .as_mut()
        .unwrap()
        .control
        .native_parameter_release()
        .unwrap();
    let first_restored = delivered(&h);
    let Some(Receipt::Finished {
        application: restoration,
        ..
    }) = h.audio.as_mut().unwrap().control.native_parameter_receipt()
    else {
        panic!("delivered FM4 restoration")
    };
    let mut restored_pcm = Vec::from(first_restored);
    for _ in 0..4 {
        restored_pcm.extend(delivered(&h));
    }
    assert!(
        restored_pcm[256..]
            .iter()
            .any(|frame| frame[0].abs() > 0.001)
    );
    assert_eq!(
        send(&mut h, &mut serial, Action::Finish(restoration)).0,
        Progress::Sealing
    );
    let end = Instant::now() + Duration::from_secs(3);
    let committed = loop {
        let (progress, event) = send(&mut h, &mut serial, Action::Poll);
        if progress == Progress::Committed {
            break event;
        }
        assert_eq!(progress, Progress::Pending);
        assert!(Instant::now() < end, "FM4 take did not commit");
    };
    applied = h.applied(applied).0;
    let recorded = committed.state.project.device(device).unwrap();
    assert_eq!(recorded.state(), instrument.state());
    let lane = recorded.automation_target(kind).unwrap();
    let [touch] = lane.touch.as_slice() else {
        panic!("one recorded FM4 span")
    };
    let map =
        hex_engine::PreparedTempoMap::prepare(committed.state.project.tempo_map().clone(), 48_000)
            .unwrap();
    assert_eq!(
        touch.start,
        map.frame_to_beat(application.arrangement_frame).unwrap()
    );
    assert_eq!(
        touch.end,
        map.frame_to_beat(restoration.arrangement_frame).unwrap()
    );
    assert!(lane.points.is_empty());
    assert_eq!(touch.steps[0].value.get(), 0.5);
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    assert_eq!(h.run(Request::Undo).state.project, base.state.project);
    applied = h.applied(applied).0;
    assert_eq!(h.run(Request::Redo).state.project, committed.state.project);
    h.applied(applied);
    let path = files.0.join("fm4-touch.hex");
    assert!(h.run(Request::SaveNew(path.clone())).save_complete);
    assert_eq!(hex_storage::load(&path).unwrap(), *committed.state.project);
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let reopened = h.run(Request::Open(path));
    super::super::resident_lifecycle::finish_replacement(&mut h, &reopened, old_output);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let start = usize::try_from(application.arrangement_frame).unwrap();
    let end = usize::try_from(restoration.arrangement_frame).unwrap();
    assert!((64..8192).contains(&start) && (start + 64..8192).contains(&end));
    assert_eq!(end - start, touch_pcm.len());
    let pcm: Vec<_> = (0..(end + restored_pcm.len()).div_ceil(64))
        .flat_map(|_| delivered(&h))
        .collect();
    assert!(pcm[..start].iter().any(|frame| frame[0].abs() > 0.001));
    let error = pcm[start..end + restored_pcm.len()]
        .iter()
        .flatten()
        .zip(touch_pcm.iter().chain(&restored_pcm).flatten())
        .map(|(actual, original)| (actual - original).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        error < 1.0e-6,
        "reopened FM4 differs from actual recorded output: {error}"
    );
}

//! Bound source effects share the real Session, callback and take writer.
use super::*;
use hex_audio::native_parameter_input::{Receipt, Target};
use hex_engine::{NativeTransientOperation, NativeTransientValue};
use hex_project::{BeatTime, Command, Created, DeviceAutomationTarget, TrackDeviceSlot};
use hex_session::device_gesture::{ParameterId, ParameterValue};
use hex_session::service::{
    DeviceAutomationTakeAction as Action, DeviceAutomationTakeProgress as Progress,
    DeviceAutomationTakeScope, automation_draft::DraftProvenance,
};

#[derive(Clone, Copy)]
enum Interruption {
    Stop,
    Seek,
    ReceiverChange,
}

#[test]
fn source_effect_touch_reaches_compensated_main_and_reopens_as_one_take() {
    verify_source_effect_take(None);
}

#[test]
fn source_effect_touch_stops_with_only_the_delivered_prefix() {
    verify_source_effect_take(Some(Interruption::Stop));
}

#[test]
fn source_effect_touch_prepared_seek_preserves_original_custody() {
    verify_source_effect_take(Some(Interruption::Seek));
}

#[test]
fn source_effect_touch_receiver_change_preserves_original_custody() {
    verify_source_effect_take(Some(Interruption::ReceiverChange));
}

fn verify_source_effect_take(interruption: Option<Interruption>) {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_recovery(8192, files.0.join("source-effect.hex-journal"));
    assert!(h.next().error.is_none());
    let mut applied = h.applied(0).0;
    let wave = files.constant_wav("source-effect-dry.wav", 48_000, 8192, &[8192, -4096]);
    let imported = h.run(Request::Import(wave));
    applied = h.applied(applied).0;
    let receiver = imported.state.project.tracks()[0].id();
    let dry_clip = imported.state.project.tracks()[0].clips()[0].id();
    let mut edit = |h: &mut ResidentHarness, command| {
        let event = h.run(Request::Edit(command));
        applied = h.applied(applied).0;
        event
    };
    let inserted = edit(
        &mut h,
        Command::InsertTrackWithDevice {
            name: "held source".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        },
    );
    let Some(Created::Track(contributor)) = inserted.created else {
        panic!("contributor");
    };
    let track = inserted.state.project.track(contributor).unwrap();
    let source_id = track.source_id().unwrap();
    let instrument = track.instrument().unwrap().id();
    edit(
        &mut h,
        Command::BindTrackSource {
            track: contributor,
            source: None,
        },
    );
    let effect = edit(
        &mut h,
        Command::InsertNestedDevice {
            parent: instrument,
            state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        },
    );
    let Some(Created::Device(device)) = effect.created else {
        panic!("source effect");
    };
    for track in [receiver, contributor] {
        edit(
            &mut h,
            Command::BindTrackSource {
                track,
                source: Some(source_id),
            },
        );
    }
    let placed = edit(
        &mut h,
        Command::PlaceMidiClip {
            track: contributor,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        },
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("midi clip");
    };
    edit(
        &mut h,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 96,
        },
    );
    let limiter = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.limiter",
    ))
    .unwrap();
    let limiter = hex_engine::patch_native_device_parameter(
        &limiter,
        ParameterId::new(6).unwrap(),
        ParameterValue::Choice(2),
    )
    .unwrap();
    let before = edit(
        &mut h,
        Command::InsertMainDevice {
            before: None,
            state: limiter,
        },
    );
    let project = &before.state.project;
    assert_eq!(
        project.native_source(source_id).unwrap().receiver(),
        Some(receiver)
    );
    assert_eq!(project.track(receiver).unwrap().clips()[0].id(), dry_clip);
    assert_eq!(
        project.track(contributor).unwrap().midi_clips()[0].start(),
        BeatTime::ZERO
    );
    let parameter = ParameterId::new(8).unwrap();
    let kind = DeviceAutomationTarget::Parameter(parameter);
    let input =
        hex_engine::native_parameter_input_target(&before.state.project, device, kind).unwrap();
    let value = NativeTransientValue::Parameter {
        id: parameter,
        value: ParameterValue::Bool(true),
    };

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
    let render = |h: &ResidentHarness| {
        let mut pcm = [[0.0; 2]; 64];
        let mut host = [0.0; 128];
        let mut slot = h.callback.lock().unwrap();
        let callback = || {
            slot.as_mut()
                .unwrap()
                .render_native_direct(&mut pcm, &mut host)
                .unwrap()
        };
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(callback);
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut callback = callback;
            callback();
        }
        pcm
    };
    const DRY: [f32; 2] = [0.25, -0.125];
    let heard_instrument =
        |sample: &[f32; 2]| *sample != [0.0; 2] && (sample[0] - DRY[0]).abs() > 0.005;
    let local_only =
        |sample: &[f32; 2]| (sample[0] - DRY[0]).abs() < 1e-6 && (sample[1] - DRY[1]).abs() < 1e-6;
    let render_playing = |h: &ResidentHarness, minimum_frames: usize| {
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let base = status.performance_frame.load(Acquire);
        let mut pcm = Vec::with_capacity(minimum_frames.next_multiple_of(64));
        let deadline = Instant::now() + Duration::from_secs(3);
        while pcm.len() < minimum_frames {
            let block = render(h);
            if status.stream_buffering.load(Acquire) {
                // A file-backed clip can wait for its first page. Silent
                // buffering callbacks do not advance the playback clock.
                assert_eq!(block, [[0.0; 2]; 64]);
                assert!(Instant::now() < deadline, "the clip remained buffered");
                std::thread::yield_now();
            } else {
                pcm.extend(block);
            }
            assert_eq!(
                status.performance_frame.load(Acquire),
                base + pcm.len() as u64
            );
        }
        pcm
    };
    let warm = render_playing(&h, 512);
    assert!(
        warm[384..].iter().any(heard_instrument),
        "expected shared instrument over the receiver clip: {:?}",
        &warm[384..392]
    );
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let source = status.recording_source().unwrap();
    let transport_epoch = status.transport_epoch.load(Acquire);
    let target = Target {
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
            gesture_serial: 1,
        },
    };
    let mut serial = 0;
    let mut send = |h: &mut ResidentHarness, action| {
        serial += 1;
        let request = Request::DeviceAutomationTake {
            request_id: serial,
            scope,
            action,
        };
        let event = if matches!(
            &request,
            Request::DeviceAutomationTake {
                action: Action::Poll,
                ..
            }
        ) {
            h.service.submit(request).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                h.pump();
                render(h);
                if let Ok(event) = h.service.try_event()
                    && event
                        .device_automation_take_complete
                        .as_ref()
                        .is_some_and(|c| c.request_id == serial)
                {
                    break event;
                }
                assert!(Instant::now() < deadline, "source effect writer stalled");
            }
        } else {
            h.run(request)
        };
        assert!(event.error.is_none(), "{:?}", event.error);
        let complete = event.device_automation_take_complete.as_ref().unwrap();
        assert_eq!(complete.request_id, serial);
        (complete.progress.clone(), event)
    };
    assert_eq!(send(&mut h, Action::Begin).0, Progress::Started);
    let live_start = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .performance_frame
        .load(Acquire);
    let mut live = Vec::new();
    let receipt = |h: &mut ResidentHarness, pcm: &mut Vec<[f32; 2]>| {
        for _ in 0..32 {
            pcm.extend(render(h));
            if let Some(receipt) = h.audio.as_mut().unwrap().control.native_parameter_receipt() {
                return receipt;
            }
        }
        panic!("no delivered source effect receipt");
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .native_parameter_begin(target, value)
        .unwrap();
    let Receipt::Applied {
        target: confirmed,
        sequence,
        application,
    } = receipt(&mut h, &mut live)
    else {
        panic!("the actual compensated source effect did not apply");
    };
    assert_eq!(confirmed, target);
    assert_eq!(application.operation, NativeTransientOperation::Set(value));
    assert!(application.remaining_latency_frames > 0);
    assert_eq!(
        send(&mut h, Action::Append(Arc::from([(sequence, application)]))).0,
        Progress::Appended
    );
    if let Some(interruption) = interruption {
        use hex_audio::native_parameter_input::AbortReason;
        use hex_storage::automation_draft::{
            DraftAbortReason, DraftOutcome, DraftReadLimits, DraftReader,
        };
        let rebind = || Command::SetNativeSourceReceiver {
            source: source_id,
            track: Some(contributor),
        };
        match interruption {
            Interruption::Stop => {}
            Interruption::Seek => {
                let intent = h.seek_intent(1, before.state.revision, None, None);
                h.service.submit_resident_arrangement_seek(intent).unwrap();
                assert!(h.next().error.is_none());
                let result = h.seek_result().result.unwrap_err();
                assert!(
                    result.contains("finish recording before replacing playback"),
                    "{result}"
                );
            }
            Interruption::ReceiverChange => {
                h.service.submit(Request::Edit(rebind())).unwrap();
                let refused = h.next();
                assert_eq!(
                    refused.error.as_deref(),
                    Some("finish recording before replacing playback")
                );
                assert_eq!(refused.state.project, before.state.project);
            }
        }
        // A prepared seek or receiver replacement cannot take the active
        // source from this recorder. Stop settles the original delivered prefix.
        assert_eq!(
            h.audio.as_ref().unwrap().control.native_parameter_target(),
            Some(target)
        );
        h.audio.as_mut().unwrap().control.stop();
        assert!(matches!(
            receipt(&mut h, &mut live),
            Receipt::Aborted { target: confirmed, reason: AbortReason::TransportChanged } if confirmed == target
        ));
        assert_eq!(
            send(&mut h, Action::Abort(DraftAbortReason::TransportChanged)).0,
            Progress::Sealing
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let path = loop {
            let (progress, event) = send(&mut h, Action::Poll);
            match progress {
                Progress::Retained(path) => {
                    assert_eq!(event.state.project, before.state.project);
                    break path;
                }
                Progress::Pending => assert!(Instant::now() < deadline),
                other => {
                    panic!("interrupted source effect take did not retain its prefix: {other:?}")
                }
            }
        };
        let mut reader = DraftReader::open(
            path,
            DraftReadLimits {
                max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
            },
        )
        .unwrap();
        assert_eq!(
            reader.outcome(),
            DraftOutcome::Aborted(DraftAbortReason::TransportChanged)
        );
        let mut rows = [None; 2];
        assert_eq!(reader.read_events(0, &mut rows).unwrap(), 1);
        let row = rows[0].unwrap();
        assert_eq!(row.sequence, sequence);
        assert_eq!(row.performance_frame, application.performance_frame);
        assert_eq!(row.content_frame, application.arrangement_frame);
        assert_eq!(row.loop_pass, application.loop_pass);
        assert_eq!(row.value.get(), 1.0);
        assert!(rows[1].is_none());
        if matches!(interruption, Interruption::ReceiverChange) {
            let edit = h.run(Request::Edit(rebind()));
            h.applied(applied);
            let new_input =
                hex_engine::native_parameter_input_target(&edit.state.project, device, kind)
                    .unwrap();
            assert_eq!(new_input.identity(), input.identity());
            assert_ne!(new_input, input);
        }
        return;
    }
    live.extend(render(&h));
    h.audio
        .as_mut()
        .unwrap()
        .control
        .native_parameter_release()
        .unwrap();
    let Receipt::Finished {
        target: confirmed,
        application: restored,
    } = receipt(&mut h, &mut live)
    else {
        panic!("the actual compensated source effect did not restore");
    };
    assert_eq!(confirmed, target);
    assert_eq!(restored.operation, NativeTransientOperation::Clear(kind));
    assert_eq!(
        restored.remaining_latency_frames,
        application.remaining_latency_frames
    );
    live.extend(render(&h));
    let due = |event: hex_engine::NativeParameterApplication| {
        usize::try_from(event.performance_frame + u64::from(event.remaining_latency_frames))
            .unwrap()
    };
    let shift = usize::try_from(live_start).unwrap();
    let (start, end) = (due(application), due(restored));
    assert!(shift <= start && start < end && end + 64 <= shift + live.len());
    let muted = &live[start - shift..end - shift];
    if let Some((offset, sample)) = muted
        .iter()
        .enumerate()
        .find(|(_, frame)| !local_only(frame))
    {
        panic!(
            "source touch at {start}..{end} (captured from {shift}, delay {}): offset {offset}, got {sample:?}; first {:?}, middle {:?}, last {:?}, next {:?}",
            application.remaining_latency_frames,
            muted[0],
            muted[muted.len() / 2],
            muted[muted.len() - 1],
            live[end - shift]
        );
    }
    assert!(live[end - shift..].iter().any(heard_instrument));

    assert_eq!(send(&mut h, Action::Finish(restored)).0, Progress::Sealing);
    let deadline = Instant::now() + Duration::from_secs(3);
    let committed = loop {
        let (progress, event) = send(&mut h, Action::Poll);
        match progress {
            Progress::Committed => break event,
            Progress::Pending => assert!(Instant::now() < deadline),
            other => panic!("source effect take did not commit: {other:?}"),
        }
    };
    applied = h.applied(applied).0;
    let project = &committed.state.project;
    let lane = project
        .device(device)
        .unwrap()
        .automation_target(kind)
        .unwrap();
    let [touch] = lane.touch.as_slice() else {
        panic!("one source effect touch");
    };
    let beat = |frame| project.tempo_map().frame_to_beat(frame).unwrap();
    assert_eq!(
        (touch.start, touch.end),
        (
            beat(application.arrangement_frame),
            beat(restored.arrangement_frame)
        )
    );
    assert_eq!(touch.steps[0].value.get(), 1.0);
    assert_eq!(h.run(Request::Undo).state.project, before.state.project);
    applied = h.applied(applied).0;
    assert_eq!(&h.run(Request::Redo).state.project, project);
    h.applied(applied);
    let path = files.0.join("source-effect-take.hex");
    assert!(h.run(Request::SaveNew(path.clone())).save_complete);
    assert_eq!(&hex_storage::load(&path).unwrap(), project.as_ref());
    let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let opened = h.run(Request::Open(path));
    assert!(opened.open_complete && opened.project_replaced);
    super::super::resident_lifecycle::finish_replacement(&mut h, &opened, old);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let delay = usize::try_from(application.remaining_latency_frames).unwrap();
    // A newly opened project starts from Arrangement zero. The recording's
    // monotonic output counter only indexes the in-flight callback capture.
    let replay_start = usize::try_from(application.arrangement_frame).unwrap() + delay;
    let replay_end = usize::try_from(restored.arrangement_frame).unwrap() + delay;
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert_eq!(status.performance_frame.load(Acquire), 0);
    let pcm = render_playing(&h, replay_end + 64);
    assert!(
        pcm[replay_start - 64..replay_start]
            .iter()
            .any(heard_instrument)
    );
    if let Some((offset, sample)) = pcm[replay_start..replay_end]
        .iter()
        .enumerate()
        .find(|(_, frame)| !local_only(frame))
    {
        panic!(
            "reopened touch {replay_start}..{replay_end} (live {start}..{end}, delay {delay}) differs at offset {offset}: {sample:?}; first {:?}, middle {:?}, last {:?}",
            pcm[replay_start],
            pcm[(replay_start + replay_end) / 2],
            pcm[replay_end - 1]
        );
    }
    assert!(
        pcm[replay_end..replay_end + 64]
            .iter()
            .any(heard_instrument)
    );
}

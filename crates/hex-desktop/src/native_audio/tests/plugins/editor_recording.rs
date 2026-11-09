//! Actual signed SDK editor → original worker → checked Main → desktop recorder.
use super::*;
use crate::native_audio::tests::resident_preview::EditorRecordingProbe;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, DeviceAutomationTarget,
};
use hex_session::device_gesture::GestureKey;
use hex_session::service::{
    PluginEditorRecordingInput, PluginEditorRecordingTarget,
    PluginParameterObservation as Observation, automation_draft::Vst3ApplicationValue,
};

#[test]
#[ignore = "explicit source-bound signed original macOS SDK receipts required"]
fn original_instrument_editor_joins_actual_main_take_and_late_restoration() {
    const CHILD: &str = "HEX_DESKTOP_ORIGINAL_EDITOR_RECORDING_CHILD";
    if std::env::var_os(CHILD).is_none() {
        for case in ["release", "stop", "session"] {
            let files = Files::new();
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    std::thread::current().name().unwrap(),
                    "--ignored",
                    "--test-threads=1",
                ])
                .env(CHILD, case)
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", "146")
                .env("HEX_ORIGINAL_EDITOR_TOUCH", "1")
                .env("HEX_ORIGINAL_INSTANCE_RELEASE", files.0.join("perform"))
                .env("HEX_ORIGINAL_EDITOR_END_RELEASE", files.0.join("end"))
                .output()
                .unwrap();
            assert!(
                result.status.success()
                    && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        return;
    }
    let _native = native_fixture();
    let session_recording = std::env::var(CHILD).as_deref() == Ok("session");
    let files = Files::new();
    let presentation = Some(EffectPresentation {
        input_frames: 0,
        output_frames: 512,
    });
    let mut h = if session_recording {
        let root = files.0.canonicalize().unwrap();
        let mut selected = selection();
        let module = root.join("original.vst3");
        super::ui::copy_original_module(&selected.module, &module);
        selected.module = module;
        hex_ui::write_plugin_effect_test_catalog(&root, &selected, "Instrument|Synth", 0).unwrap();
        ResidentHarness::with_plugins_at_rate_and_catalog_recovery(
            host(),
            presentation,
            48_000,
            8192,
            root.join("plugin-catalog.json"),
            root.join("recording.hex-journal"),
        )
    } else {
        ResidentHarness::with_plugins(host(), presentation)
    };
    assert!(h.next().error.is_none());
    let mut applied = h.applied(0).0;
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "synth".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("midi track")
    };
    applied = h.applied(applied).0;
    let length = BeatTime::from_quarters(64).unwrap();
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("midi clip")
    };
    applied = h.applied(applied).0;
    let note = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: length,
        pitch: 60,
        channel: 2,
        velocity: 64,
    }));
    applied = h.applied(applied).0;
    h.service
        .submit(Request::InsertPluginEffect(request(
            &note.state,
            PluginEffectDestination::Instrument(track),
        )))
        .unwrap();
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let instrument = inserted
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let class = selection().cid;
    applied = h.applied(applied).0;
    let mut before = Arc::clone(&inserted.state.project);
    for (parameter, value) in [(7, 0.25), (11, 0.5)] {
        let context = h.submitted.unwrap().1.context;
        let edited = automation::edit_with_running_callback(
            &mut h,
            Request::DeviceAutomation {
                project_generation: context.project_generation,
                revision: context.editor_revision,
                device: instrument,
                target: DeviceAutomationTarget::Vst3 { parameter },
                vst3_class: Some(class),
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(value).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                }],
            },
        );
        assert!(edited.error.is_none(), "{:?}", edited.error);
        before = Arc::clone(&edited.state.project);
        applied = h.applied(applied).0;
    }
    let context = h.submitted.unwrap().1.context;
    let key = GestureKey {
        generation: context.project_generation,
        token: 19,
    };
    let input = PluginEditorRecordingInput {
        key,
        context,
        target: PluginEditorRecordingTarget::Single {
            device: instrument,
            class,
            parameter: 7,
            reading: true,
        },
    };
    assert!(probe(&h, |reply| EditorRecordingProbe::Prepare(input, reply)).is_err());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        instruments::render_completed_block(&h);
    }
    let baseline = instruments::render_completed_block(&h)[0];
    assert!(baseline[0] > 0.0);
    if session_recording {
        verify_session_recording(&mut h, input, before, applied, &files.0, baseline[0]);
        return;
    }
    let wrong_class = PluginEditorRecordingInput {
        target: PluginEditorRecordingTarget::Single {
            device: instrument,
            class: [0; 16],
            parameter: 7,
            reading: true,
        },
        ..input
    };
    assert!(
        probe(&h, |reply| EditorRecordingProbe::Prepare(
            wrong_class,
            reply
        ))
        .is_err()
    );
    let ready = probe(&h, |reply| EditorRecordingProbe::Prepare(input, reply)).unwrap();
    let Observation::Ready {
        key: got,
        target,
        source,
        ..
    } = ready
    else {
        panic!("prepare must return exact copied provenance")
    };
    assert_eq!(
        (got, target.device, target.class, target.parameter),
        (key, instrument, class, 7)
    );
    assert!(probe(&h, EditorRecordingProbe::Take).is_none());
    let wrong_key = GestureKey {
        token: key.token + 1,
        ..key
    };
    assert!(probe(&h, |reply| EditorRecordingProbe::Start(wrong_key, reply)).is_err());
    probe(&h, |reply| EditorRecordingProbe::Start(key, reply)).unwrap();
    std::fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"perform",
    )
    .unwrap();
    let Observation::Delivered(point) = render_until_observation(&h, [3.0 * baseline[0]; 2]) else {
        panic!("expected the original editor perform")
    };
    assert_eq!(
        (point.key, point.target, point.source),
        (key, target, source)
    );
    assert_eq!(point.application.value, Vst3ApplicationValue::Value(0.75));
    assert_eq!(
        point.delivery_performance_frame - point.application.origin.performance_frame,
        512
    );
    assert!(probe(&h, EditorRecordingProbe::Take).is_none());
    if std::env::var(CHILD).as_deref() == Ok("stop") {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Stop)
            .unwrap();
        for _ in 0..12 {
            h.render();
            if let Some(observation) = probe(&h, EditorRecordingProbe::Take) {
                assert!(matches!(observation,
                    Observation::Interrupted { key: ended, target: actual, .. }
                    if ended == key && actual == target
                ));
                assert!(probe(&h, EditorRecordingProbe::Take).is_none());
                return;
            }
        }
        panic!("Stop did not retire the exact editor take");
    }
    std::fs::write(
        std::env::var_os("HEX_ORIGINAL_EDITOR_END_RELEASE").unwrap(),
        b"end",
    )
    .unwrap();
    let Observation::Restored(end) = render_until_observation(&h, baseline) else {
        panic!("expected a distinct real restoration")
    };
    assert_eq!((end.key, end.target, end.source), (key, target, source));
    assert_eq!(end.application.value, Vst3ApplicationValue::Restored);
    assert!(end.sequence > point.sequence);
    assert!(end.application.origin.performance_frame > point.application.origin.performance_frame);
    assert_eq!(
        end.delivery_performance_frame - end.application.origin.performance_frame,
        512
    );
    assert!(probe(&h, EditorRecordingProbe::Take).is_none());
}

fn verify_session_recording(
    h: &mut ResidentHarness,
    input: PluginEditorRecordingInput,
    before: Arc<hex_project::Project>,
    mut applied: u64,
    directory: &std::path::Path,
    baseline: f32,
) {
    use crate::native_audio::tests::resident_preview::SESSION_RECORDING_OBSERVATIONS;
    use hex_session::service::DeviceAutomationTakeProgress as Progress;
    const ID: u64 = 23;
    let started = h.run(Request::StartPluginEditorRecording {
        request_id: ID,
        input,
    });
    assert_eq!(
        started.device_automation_take_complete.unwrap().progress,
        Progress::Started
    );
    let safety = h.run(Request::QueryDeviceAutomationCloseSafety { request_id: 27 });
    assert_eq!(
        safety.device_automation_take_complete.unwrap().progress,
        Progress::CloseBlocked
    );
    let perform_gate = PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap());
    let end_gate = PathBuf::from(std::env::var_os("HEX_ORIGINAL_EDITOR_END_RELEASE").unwrap());
    std::fs::write(&perform_gate, b"perform").unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while SESSION_RECORDING_OBSERVATIONS.lock().unwrap().is_empty() {
        instruments::render_completed_block(h);
        assert!(
            Instant::now() < until,
            "actual editor value did not reach the writer"
        );
    }
    std::fs::write(&end_gate, b"end").unwrap();
    let committed = loop {
        h.pump();
        // The Session writer can synchronously await the next publication
        // callback. Pace native PCM without waiting for a Session poll here.
        h.render_worker_step();
        if let Ok(event) = h.service.try_event()
            && let Some(completion) = event.device_automation_take_complete.as_ref()
            && completion.request_id == ID
        {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert_eq!(completion.progress, Progress::Committed);
            break event;
        }
        assert!(Instant::now() < until, "editor take did not publish");
    };
    // Save and Open may create fresh SDK workers. Only the original
    // recording worker should see the synthetic editor trigger.
    std::fs::remove_file(perform_gate).unwrap();
    std::fs::remove_file(end_gate).unwrap();
    let observations = SESSION_RECORDING_OBSERVATIONS.lock().unwrap().clone();
    let [Observation::Delivered(point), Observation::Restored(end)] = observations.as_slice()
    else {
        panic!("exact original point and restoration: {observations:?}");
    };
    let project = &committed.state.project;
    let device = project.device(input.target.device()).unwrap();
    let old = before.device(input.target.device()).unwrap();
    let PluginEditorRecordingTarget::Single { parameter, .. } = input.target else {
        panic!("expected a single target fixture");
    };
    let target = DeviceAutomationTarget::Vst3 { parameter };
    assert_eq!(old.state(), device.state());
    let lane = device.automation_target(target).unwrap();
    assert_eq!(lane.points, old.automation_target(target).unwrap().points);
    assert_eq!(lane.vst3_class, Some(input.target.class()));
    let [span] = lane.touch.as_slice() else {
        panic!("one canonical touch")
    };
    let clock =
        hex_engine::PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate())
            .unwrap();
    assert_eq!(
        span.start,
        clock
            .frame_to_beat(point.application.origin.content_frame)
            .unwrap()
    );
    assert_eq!(
        span.end,
        clock
            .frame_to_beat(end.application.origin.content_frame)
            .unwrap()
    );
    assert_eq!(span.steps.len(), 1);
    assert_eq!(span.steps[0].value.get(), 0.75);
    applied = h.applied(applied).0;
    let undone = automation::edit_with_running_callback(h, Request::Undo);
    assert_eq!(undone.state.project, before);
    applied = h.applied(applied).0;
    let redone = automation::edit_with_running_callback(h, Request::Redo);
    assert_eq!(&redone.state.project, project);
    h.applied(applied);
    let safety = h.run(Request::QueryDeviceAutomationCloseSafety { request_id: 28 });
    assert_eq!(
        safety.device_automation_take_complete.unwrap().progress,
        Progress::CloseSafe
    );
    let path = directory.join("recorded-editor.hex");
    let saved = h.run(Request::SaveNew(path.clone()));
    assert!(saved.save_complete && saved.error.is_none());
    assert_eq!(hex_storage::load(&path).unwrap(), *saved.state.project);

    // Independent authored-Step oracle, calculated from the Main receipts
    // instead of the committed overlay. Compare actual reopened SDK audio.
    let step = |frame, value| AutomationPoint {
        beat: clock.frame_to_beat(frame).unwrap(),
        value: AutomationValue::new(value).unwrap(),
        outgoing: AutomationInterpolation::Step,
    };
    let mut reference = hex_project::Editor::new((*before).clone(), 8).unwrap();
    reference
        .execute(Command::ReplaceDeviceAutomationTarget {
            device: input.target.device(),
            target,
            points: vec![
                step(0, 0.25),
                step(point.application.origin.content_frame, 0.75),
                step(end.application.origin.content_frame, 0.25),
            ],
        })
        .unwrap();
    let reference_path = directory.join("editor-delivered-steps.hex");
    hex_storage::save_new(&reference_path, reference.project()).unwrap();
    let blocks = usize::try_from(end.application.origin.content_frame / 256 + 8).unwrap();
    assert!(blocks < 128);
    let expected =
        instruments::replay_reopened_instrument(h, &reference_path, input.target.device(), blocks);
    let actual = instruments::replay_reopened_instrument(h, &path, input.target.device(), blocks);
    assert_eq!(expected.len(), actual.len());
    assert!(actual.iter().any(|frame| frame[0] > 2.9 * baseline));
    let error = expected
        .iter()
        .flatten()
        .zip(actual.iter().flatten())
        .map(|(left, right)| (left - right).abs())
        .fold(0.0, f32::max);
    assert!(
        error < 1.0e-6,
        "reopened editor take differs from the Main receipt oracle: {error}"
    );
}

fn probe<T>(
    h: &ResidentHarness,
    request: impl FnOnce(mpsc::SyncSender<T>) -> EditorRecordingProbe,
) -> T {
    let (reply, result) = mpsc::sync_channel(1);
    assert!(
        h.editor_recording_probe
            .lock()
            .unwrap()
            .replace(request(reply))
            .is_none()
    );
    result.recv_timeout(Duration::from_secs(2)).unwrap()
}

fn render_until_observation(h: &ResidentHarness, expected: [f32; 2]) -> Observation {
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let mut rendered = Vec::new();
    for _ in 0..40 {
        let start = status.performance_frame.load(Acquire);
        rendered.push((start, instruments::render_completed_block(h)));
        let Some(observation) = probe(h, EditorRecordingProbe::Take) else {
            continue;
        };
        let point = match observation {
            Observation::Delivered(point) | Observation::Restored(point) => point,
            _ => panic!("unexpected editor observation: {observation:?}"),
        };
        let sample = rendered
            .iter()
            .find_map(|(start, pcm)| {
                let index =
                    usize::try_from(point.delivery_performance_frame.checked_sub(*start)?).ok()?;
                pcm.get(index).copied()
            })
            .expect("witness must name one of the rendered Main samples");
        for (got, want) in sample.into_iter().zip(expected) {
            assert!(
                (got - want).abs() < 0.0001,
                "PCM at witness: {sample:?}, expected {expected:?}"
            );
        }
        return observation;
    }
    panic!("signed editor input did not reach the recording reader");
}

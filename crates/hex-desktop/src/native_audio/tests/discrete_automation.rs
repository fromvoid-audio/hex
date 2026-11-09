//! Discrete native values through the real Session, callback and take writer.
use super::{resident_preview::ResidentHarness, *};
use hex_audio::native_parameter_input::{Receipt, Target};
use hex_engine::{NativeTransientOperation, NativeTransientValue};
use hex_project::{Command, DeviceAutomationTarget};
use hex_session::device_gesture::{ParameterId, ParameterValue};
use hex_session::service::{
    DeviceAutomationTakeAction as Action, DeviceAutomationTakeProgress as Progress,
    DeviceAutomationTakeScope, automation_draft::DraftProvenance,
};

#[test]
fn native_toggle_and_choice_take_keeps_only_the_delivered_step() {
    let _native = native_fixture();
    let render = |h: &ResidentHarness| {
        let mut pcm = [[0.0; 2]; 64];
        let mut host = [0.0; 128];
        let mut slot = h.callback.lock().unwrap();
        let callback = slot.as_mut().unwrap();
        let counts = allocation_counter::measure(|| {
            callback.render_native_direct(&mut pcm, &mut host).unwrap();
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        pcm
    };
    for (parameter, value) in [
        (8, ParameterValue::Bool(true)),
        (4, ParameterValue::Choice(3)),
    ] {
        let files = Files::new();
        let directory = files.0.canonicalize().unwrap();
        let wave = files.constant_wav("discrete.wav", 48_000, 8192, &[8192, -16384]);
        let mut h = ResidentHarness::with_recovery(8192, directory.join("discrete.hex-journal"));
        assert!(h.next().error.is_none());
        let applied = h.applied(0).0;
        h.run(Request::Import(wave));
        let applied = h.applied(applied).0;
        let before = h.run(Request::Edit(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        }));
        let (applied, metadata) = h.applied(applied);
        let device = before.state.project.main_devices()[0].id();
        let parameter = ParameterId::new(parameter).unwrap();
        let target = DeviceAutomationTarget::Parameter(parameter);
        let input =
            hex_engine::native_parameter_input_target(&before.state.project, device, target)
                .unwrap();
        let armed = h.run(Request::ArmNativeParameterRecording {
            project_generation: metadata.context.project_generation,
            revision: metadata.context.editor_revision,
            target: Some((device, target)),
        });
        assert_eq!(armed.state.project, before.state.project);
        let (applied, _) = h.applied(applied);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(0))
            .unwrap();
        let base = (0..4).map(|_| render(&h)[63]).last().unwrap();
        assert!(base[0] > 0.01 && base[1] < -0.01);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let source = status.recording_source().unwrap();
        let transport_epoch = status.transport_epoch.load(Acquire);
        let stamp = DraftProvenance {
            sample_rate: 48_000,
            output_identity: source.output_identity,
            project_generation: source.context.project_generation,
            editor_revision: source.context.editor_revision,
            audio_revision: source.audio_revision,
            transport_epoch,
            gesture_serial: 1,
        };
        let scope = DeviceAutomationTakeScope {
            target: input.into(),
            stamp,
        };
        let audio_target = Target {
            source,
            input,
            transport_epoch,
            gesture_serial: 1,
        };
        let mut request_id = 0;
        let mut send = |h: &mut ResidentHarness, action| {
            request_id += 1;
            let mut event = h.run(Request::DeviceAutomationTake {
                request_id,
                scope,
                action,
            });
            let complete = event.device_automation_take_complete.take().unwrap();
            assert_eq!(complete.request_id, request_id);
            (event, complete.progress)
        };
        assert_eq!(send(&mut h, Action::Begin).1, Progress::Started);
        let value = NativeTransientValue::Parameter {
            id: parameter,
            value,
        };
        h.audio
            .as_mut()
            .unwrap()
            .control
            .native_parameter_begin(audio_target, value)
            .unwrap();
        let receipt = |h: &mut ResidentHarness| {
            for _ in 0..8 {
                let pcm = render(h);
                if let Some(receipt) = h.audio.as_mut().unwrap().control.native_parameter_receipt()
                {
                    return (receipt, pcm[63]);
                }
            }
            panic!("the final output did not confirm the native input");
        };
        let (point, sound) = receipt(&mut h);
        let Receipt::Applied {
            target: received,
            sequence,
            application,
        } = point
        else {
            panic!("the only Set must be delivered: {point:?}");
        };
        assert_eq!(received, audio_target);
        assert_eq!(application.operation, NativeTransientOperation::Set(value));
        let touched = if parameter.value() == 8 {
            [0.0; 2]
        } else {
            [base[1], base[0]]
        };
        assert_eq!(sound, touched);
        assert_eq!(
            send(&mut h, Action::Append(Arc::from([(sequence, application)]))).1,
            Progress::Appended
        );
        h.audio
            .as_mut()
            .unwrap()
            .control
            .native_parameter_release()
            .unwrap();
        let (end, sound) = receipt(&mut h);
        let Receipt::Finished {
            target: received,
            application: restored,
        } = end
        else {
            panic!("an actual restored sample must terminate the step: {end:?}");
        };
        assert_eq!(received, audio_target);
        assert_eq!(restored.operation, NativeTransientOperation::Clear(target));
        assert!(restored.arrangement_frame > application.arrangement_frame);
        assert_eq!(sound, base);
        let deadline = Instant::now() + Duration::from_secs(3);
        assert_eq!(send(&mut h, Action::Finish(restored)).1, Progress::Sealing);
        let committed = loop {
            let (event, progress) = send(&mut h, Action::Poll);
            match progress {
                Progress::Committed => break event,
                Progress::Pending => assert!(Instant::now() < deadline),
                other => panic!("unexpected recording terminal: {other:?}"),
            }
        };
        let applied = h.applied(applied).0;
        let lane = committed
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap();
        assert!(lane.points.is_empty() && lane.touch.len() == 1);
        assert_eq!(lane.touch[0].steps.len(), 1);
        assert_eq!(lane.touch[0].steps[0].value.get(), 1.0);
        assert_eq!(
            lane.touch[0].steps[0].beat,
            before
                .state
                .project
                .tempo_map()
                .frame_to_beat(application.arrangement_frame)
                .unwrap()
        );
        assert_eq!(
            lane.touch[0].end,
            before
                .state
                .project
                .tempo_map()
                .frame_to_beat(restored.arrangement_frame)
                .unwrap()
        );
        assert_eq!(
            committed.state.project.device(device).unwrap().state(),
            before.state.project.device(device).unwrap().state()
        );
        let undo = h.run(Request::Undo);
        assert_eq!(undo.state.project, before.state.project);
        let applied = h.applied(applied).0;
        let redo = h.run(Request::Redo);
        assert_eq!(redo.state.project, committed.state.project);
        h.applied(applied);
        let path = directory.join("recorded-discrete.hex");
        assert!(h.run(Request::SaveNew(path.clone())).save_complete);
        assert_eq!(hex_storage::load(&path).unwrap(), *redo.state.project);
        let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        let opened = h.run(Request::Open(path));
        assert!(opened.project_replaced && opened.open_complete);
        super::resident_lifecycle::finish_replacement(&mut h, &opened, output);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(0))
            .unwrap();
        let start = usize::try_from(application.arrangement_frame).unwrap();
        let end = usize::try_from(restored.arrangement_frame).unwrap();
        assert!((1..8192).contains(&start) && (start + 1..8192).contains(&end));
        let pcm = (0..=end / 64).flat_map(|_| render(&h)).collect::<Vec<_>>();
        assert_eq!(pcm[start - 1], base);
        assert!(pcm[start..end].iter().all(|sample| *sample == touched));
        assert_eq!(pcm[end], base);
    }
}

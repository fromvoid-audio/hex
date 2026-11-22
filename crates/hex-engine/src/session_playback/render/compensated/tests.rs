use super::*;
use crate::signal_graph::tests::fixture;

#[test]
fn aligned_main_sum_preserves_sample_order_across_a_loop() {
    let mut f = fixture(&[&[], &[], &[], &[7]], 384);
    let small = f32::EPSILON / 4.0;
    // Summing the small contribution after the cancellation changes the
    // PCM bits. Render the oracle through scalar routing.
    for (audio, frame) in
        f.media
            .values_mut()
            .zip([[1.0, -1.0], [small, -small], [-1.0, 1.0], [0.125, -0.125]])
    {
        *audio = crate::Audio::new(48_000, vec![frame; 384]).unwrap();
    }
    f.editor
        .execute(hex_project::Command::SetLoopRegion {
            start: 32,
            end: 321,
            enabled: true,
        })
        .unwrap();
    let revision = crate::signal_graph::tests::prepare(&f).unwrap();
    assert_eq!(revision.arrangement().main_latency_frames(), 7);
    let mut batched = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut scalar = SessionPlayback::new(revision).unwrap();
    scalar.resident_source_scratch.clear();
    for playback in [&mut batched, &mut scalar] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut actual = [[0.0; 2]; 704];
    let mut expected = actual;
    for block in actual.chunks_mut(257) {
        batched.render(block).unwrap();
    }
    for frame in expected.chunks_mut(1) {
        scalar.render(frame).unwrap();
    }
    assert!(expected.contains(&[0.125, -0.125]));
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
    }
    assert_eq!(batched.arrangement_frame, scalar.arrangement_frame);
    assert_eq!(batched.arrangement_loop_pass, scalar.arrangement_loop_pass);
    assert_eq!(batched.performance_frame, scalar.performance_frame);
}

#[test]
fn unity_batch_preserves_each_meter_tap_and_refuses_active_faders() {
    let f = fixture(&[&[]], 8);
    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut runtime = SessionPlayback::new(revision).unwrap();
    runtime.resident_source_scratch = vec![[[0.0; 2]; 256]];
    runtime.track_fader_scratch[0] =
        crate::session_playback::track_fader::TrackFader::new([1.0; 2]);
    let input = [
        [-0.0, 0.0],
        [f32::from_bits(1), -f32::from_bits(1)],
        [0.75, -0.25],
        [f32::INFINITY, f32::NAN],
    ];
    runtime.resident_source_scratch[0][..input.len()].copy_from_slice(&input);
    let mut routes = [ResidentRoute::Scalar; MAX_TRACKS];
    routes[0] = ResidentRoute::Passthrough;
    let aligned = [false; MAX_TRACKS];
    runtime.prepare_unity_faders(input.len(), &mut routes, &aligned);
    assert!(routes[0] == ResidentRoute::Unity);
    let actual = hex_lattice::DeviceMeter::default();
    let expected = hex_lattice::DeviceMeter::default();
    for (original, batched) in input.into_iter().zip(runtime.resident_source_scratch[0]) {
        let first = original.map(|sample| sample + 0.0);
        let second = first.map(|sample| sample + 0.0);
        expected.observe_input(&[first]);
        actual.observe_input(&[batched]);
        // The display can drain between the input and output publications.
        assert_eq!(actual.take_peaks(), expected.take_peaks());
        expected.observe_output(&[second]);
        actual.observe_output(&[batched]);
        assert_eq!(actual.take_peaks(), expected.take_peaks());
    }
    runtime.track_fader_scratch[0].retarget([0.5; 2], 48_000);
    runtime.resident_source_scratch[0][0] = [-0.25, 0.5];
    routes[0] = ResidentRoute::Passthrough;
    runtime.prepare_unity_faders(1, &mut routes, &aligned);
    assert!(routes[0] == ResidentRoute::Passthrough);
    assert_eq!(runtime.resident_source_scratch[0][0], [-0.25, 0.5]);
}

#[test]
fn unity_return_capture_and_refusal_preserve_pcm_and_transaction() {
    return_capture_and_refusal(1.0, false);
}

#[test]
fn static_gain_return_capture_and_refusal_preserve_pcm_and_transaction() {
    return_capture_and_refusal(0.5, false);
}

#[test]
fn session_return_capture_and_refusal_preserve_pcm_and_transaction() {
    return_capture_and_refusal(0.5, true);
}

fn return_capture_and_refusal(gain: f32, session: bool) {
    use crate::session_playback::capture::{OutputCapture, OutputCaptureId};
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{Command, Created};

    struct RefuseAfter(TrackId, bool);
    impl CaptureSink for RefuseAfter {
        fn selects(&self, id: OutputCaptureId) -> bool {
            id == OutputCaptureId::Track(self.0)
        }
        fn add(
            &mut self,
            id: OutputCaptureId,
            offset: usize,
            frames: &[Frame],
        ) -> Result<(), SessionPlaybackError> {
            if self.selects(id) && (offset..offset + frames.len()).contains(&13) {
                Err(SessionPlaybackError::DeviceRuntime)
            } else {
                Ok(())
            }
        }
        fn invalidate(&mut self) {
            self.1 = true;
        }
    }

    let mut f = if session {
        fixture(&[&[], &[], &[], &[]], 1024)
    } else {
        fixture(&[&[]], 1024)
    };
    f.samples[512..].fill([0.0625, -0.125]);
    for audio in f.media.values_mut() {
        *audio = crate::Audio::new(48_000, f.samples.clone()).unwrap();
    }
    let mut snapshot = f.editor.project().snapshot();
    for track in &mut snapshot.tracks {
        track.gain = gain;
    }
    f.editor = hex_project::Editor::new(hex_project::Project::from_snapshot(snapshot).unwrap(), 32)
        .unwrap();
    let track = f.tracks[0];
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return fixture")
    };
    f.editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
        })
        .unwrap();
    for &id in &f.tracks {
        f.editor
            .execute(Command::SetReturnSend {
                track: id,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
    }
    let scene = if session {
        let Created::Scene(scene) = f
            .editor
            .execute(Command::AddScene {
                name: "session".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        for &id in &f.tracks {
            let asset = f.editor.project().track(id).unwrap().clips()[0].asset();
            f.editor
                .execute(Command::CreateSessionAudioClip {
                    track: id,
                    scene,
                    name: "clip".into(),
                    asset,
                    source_start: 0,
                    source_end: 1024,
                })
                .unwrap();
        }
        Some(scene)
    } else {
        None
    };
    let create_runtime = || {
        let revision = Arc::new(
            PreparedRevision::prepare(
                f.editor.project(),
                &f.media,
                Arc::new(crate::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        if session {
            SessionPlayback::new(revision)
        } else {
            SessionPlayback::new_linear_arrangement_export(revision)
        }
        .unwrap()
    };
    // Each runtime has its own meter endpoints for independent readings.
    let mut actual = create_runtime();
    let mut scalar = create_runtime();
    scalar.resident_source_scratch.clear();
    for runtime in [&mut actual, &mut scalar] {
        runtime.set_device_metering(true);
        if let Some(scene) = scene {
            runtime
                .apply(SessionPlaybackCommand::LaunchScene {
                    scene,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let ids = [OutputCaptureId::Track(track), OutputCaptureId::Return(bus)];
    let actual_plan = (!session).then(|| actual.output_capture_plan(&ids, 256).unwrap());
    let scalar_plan = (!session).then(|| scalar.output_capture_plan(&ids, 256).unwrap());
    let capture =
        |runtime: &mut SessionPlayback,
         plan: Option<&crate::session_playback::capture::OutputCapturePlan>| {
            let mut main = [[0.0; 2]; 256];
            let mut track_frames = [[0.0; 2]; 256];
            let mut return_frames = [[0.0; 2]; 256];
            if let Some(plan) = plan {
                let mut buffers = [&mut track_frames[..], &mut return_frames[..]];
                let mut captured = OutputCapture::new(plan, &mut buffers);
                runtime
                    .render_with_capture(&mut main, &mut captured)
                    .unwrap();
                assert_eq!(captured.valid_frames(), 256);
            } else {
                let mut observer = |signal, offset: usize, frames: &[Frame]| {
                    let destination = match signal {
                        crate::ProjectSignal::Track(_) => &mut track_frames,
                        crate::ProjectSignal::Return(_) => &mut return_frames,
                        _ => return,
                    };
                    for (target, source) in destination[offset..].iter_mut().zip(frames) {
                        target[0] += source[0];
                        target[1] += source[1];
                    }
                };
                let tracks = [track.value()];
                let returns = [bus];
                let mut tap =
                    crate::ProjectOutputTap::with_returns(&tracks, &returns, &mut observer)
                        .unwrap();
                runtime
                    .render_internal::<dyn MediaPages>(&mut main, None, None, Some(&mut tap), None)
                    .unwrap();
            }
            (main, track_frames, return_frames)
        };
    let expected = capture(&mut scalar, scalar_plan.as_ref());
    let captured = capture(&mut actual, actual_plan.as_ref());
    assert!(
        actual.return_scratch[0][1..]
            .iter()
            .any(|frame| *frame != [0.0; 2]),
        "batched send window"
    );
    assert_eq!(captured, expected);
    assert!(captured.1.iter().flatten().any(|sample| *sample != 0.0));
    assert!(captured.2.iter().flatten().any(|sample| *sample != 0.0));

    let position = (actual.performance_frame(), actual.arrangement_frame());
    let mut refused_output = [[9.0; 2]; 256];
    let mut refusal = RefuseAfter(track, false);
    assert_eq!(
        actual.render_internal::<dyn MediaPages>(
            &mut refused_output,
            None,
            None,
            Some(&mut refusal),
            None
        ),
        Err(SessionPlaybackError::DeviceRuntime),
    );
    assert!(refusal.1);
    assert_eq!(refused_output, [[0.0; 2]; 256]);
    assert_eq!(
        (actual.performance_frame(), actual.arrangement_frame()),
        position
    );
    assert_eq!(
        capture(&mut actual, actual_plan.as_ref()),
        capture(&mut scalar, scalar_plan.as_ref())
    );
    for runtime in [&actual, &scalar] {
        for track in runtime.revision.arrangement().tracks() {
            track.instrument_meter.take_peaks();
        }
    }
    for count in [129, 257] {
        let mut output = [[0.0; 2]; 257];
        let mut expected = output;
        actual.render(&mut output[..count]).unwrap();
        scalar.render(&mut expected[..count]).unwrap();
        assert!(expected[..count].iter().any(|frame| *frame != [0.0; 2]));
        for (actual, expected) in output[..count].iter().zip(&expected[..count]) {
            assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
        }
        for (track, reference) in actual
            .revision
            .arrangement()
            .tracks()
            .iter()
            .zip(scalar.revision.arrangement().tracks())
        {
            let peaks = track.instrument_meter.take_peaks();
            assert!(peaks.input > 0.0);
            assert_eq!(peaks, reference.instrument_meter.take_peaks());
        }
    }
    assert_eq!(
        capture(&mut actual, actual_plan.as_ref()),
        capture(&mut scalar, scalar_plan.as_ref())
    );
}

#[test]
fn session_return_quantized_relaunch_matches_scalar_at_boundary() {
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{Command, Created};

    let mut f = fixture(&[&[], &[], &[], &[]], 256);
    for (index, audio) in f.media.values_mut().enumerate() {
        let sample = if index == 1 {
            [0.25, -0.5]
        } else {
            [0.125, -0.25]
        };
        *audio = crate::Audio::new(48_000, vec![sample; 256]).unwrap();
    }
    let replacement_asset = f.editor.project().track(f.tracks[1]).unwrap().clips()[0].asset();
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
        })
        .unwrap();
    let Created::Scene(scene) = f
        .editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::Scene(next) = f
        .editor
        .execute(Command::AddScene {
            name: "next".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let mut replacement = None;
    for (index, &track) in f.tracks.iter().enumerate() {
        let asset = f.editor.project().track(track).unwrap().clips()[0].asset();
        f.editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.25,
            })
            .unwrap();
        f.editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "first".into(),
                asset,
                source_start: 0,
                source_end: 256,
            })
            .unwrap();
        if index == 0 {
            let Created::SessionClip(clip) = f
                .editor
                .execute(Command::CreateSessionAudioClip {
                    track,
                    scene: next,
                    name: "replacement".into(),
                    asset: replacement_asset,
                    source_start: 32,
                    source_end: 256,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            replacement = Some(clip);
        }
    }
    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut actual = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut scalar = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut actual, &mut scalar] {
        runtime.set_device_metering(true);
        runtime.apply(SessionPlaybackCommand::Seek(95_990)).unwrap();
        runtime
            .apply(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut first_actual = [[0.0; 2]; 1];
    let mut first_expected = first_actual;
    actual.render(&mut first_actual).unwrap();
    scalar
        .render_with_post_node_observer(&mut first_expected, Some(&mut |_, _, _| {}))
        .unwrap();
    assert_eq!(first_actual, first_expected);
    for runtime in [&mut actual, &mut scalar] {
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: replacement.unwrap(),
                quantization: LaunchQuantization::FourQuarter,
            })
            .unwrap();
    }
    let mut output = [[0.0; 2]; 150];
    let mut expected = output;
    for (chunk, reference) in output.chunks_mut(17).zip(expected.chunks_mut(17)) {
        actual.render(chunk).unwrap();
        scalar
            .render_with_post_node_observer(reference, Some(&mut |_, _, _| {}))
            .unwrap();
    }
    assert_eq!(output, expected);
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    assert_ne!(
        output[75], output[145],
        "relaunch must change the sounding mix"
    );
    assert_eq!(
        (actual.performance_frame(), actual.arrangement_frame()),
        (scalar.performance_frame(), scalar.arrangement_frame())
    );
}

#[test]
fn compensated_clock_keeps_meter_and_cycle_across_the_actual_loop_wrap() {
    use hex_project::{BeatTime, Command, Meter};

    let start = 23_998;
    let end = 24_002;
    let mut f = fixture(&[&[]], 24_010);
    let limiter = hex_lattice_contract::DeviceTypeId::from_static("hex.limiter");
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: crate::default_native_device_state(limiter).unwrap(),
        })
        .unwrap();
    let seven_eight = Meter::new(7, 8).unwrap();
    f.editor
        .execute(Command::SetMeterPoint {
            beat: BeatTime::from_ratio(1, 1).unwrap(),
            meter: seven_eight,
        })
        .unwrap();
    f.editor
        .execute(Command::SetLoopRegion {
            start,
            end,
            enabled: true,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let timing = revision.session().timing();
    let cycle = Some((
        timing.frame_to_beat(start).unwrap().raw(),
        timing.frame_to_beat(end).unwrap().raw(),
    ));
    let export = SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision)).unwrap();
    let export_stamp = export.compensated_stamp(start, start, 0).unwrap();
    assert_eq!(
        (export_stamp.meter, export_stamp.cycle_beats_raw),
        (Some(Meter::COMMON_TIME), None)
    );
    let mut runtime = SessionPlayback::new(revision).unwrap();
    runtime.apply(SessionPlaybackCommand::Seek(start)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime.render(&mut [[0.0; 2]; 5]).unwrap();
    let history = &runtime.compensation.as_ref().unwrap().delays;
    let wrapped = history.stamp_at_delay(0).unwrap();
    assert_eq!((wrapped.arrangement_frame, wrapped.loop_pass), (start, 1));
    assert_eq!(
        (wrapped.meter, wrapped.cycle_beats_raw),
        (Some(Meter::COMMON_TIME), cycle)
    );
    let before_wrap = history.stamp_at_delay(2).unwrap();
    assert_eq!(
        (before_wrap.arrangement_frame, before_wrap.loop_pass),
        (24_000, 0)
    );
    assert_eq!(
        (before_wrap.meter, before_wrap.cycle_beats_raw),
        (Some(seven_eight), cycle)
    );
    runtime.apply(SessionPlaybackCommand::Seek(end)).unwrap();
    runtime.render(&mut [[0.0; 2]; 1]).unwrap();
    let after_loop = runtime
        .compensation
        .as_ref()
        .unwrap()
        .delays
        .stamp_at_delay(0)
        .unwrap();
    assert_eq!(
        (after_loop.arrangement_frame, after_loop.cycle_beats_raw),
        (end, None)
    );
}

struct RefuseTrack(TrackId);

impl CaptureSink for RefuseTrack {
    fn selects(&self, id: OutputCaptureId) -> bool {
        id == OutputCaptureId::Track(self.0)
    }

    fn add(
        &mut self,
        id: OutputCaptureId,
        offset: usize,
        frames: &[Frame],
    ) -> Result<(), SessionPlaybackError> {
        if self.selects(id) && (offset..offset + frames.len()).contains(&13) {
            Err(SessionPlaybackError::DeviceRuntime)
        } else {
            Ok(())
        }
    }
}

#[test]
fn resident_alignment_matches_scalar_automation_capture_seek_and_rollback() {
    use hex_lattice_contract::{
        DeviceAutomationTarget, DeviceTypeId, HostGain, NativeDeviceMetadata, ParameterValue,
    };
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, GainInterpolation,
        GainPoint,
    };

    let mut f = fixture(&[&[], &[], &[]], 333);
    for audio in f.media.values_mut() {
        *audio = crate::Audio::new(48_000, vec![[0.125, -0.375]; 333]).unwrap();
    }
    f.editor
        .execute(hex_project::Command::SetLoopRegion {
            start: 49,
            end: 313,
            enabled: true,
        })
        .unwrap();
    let wet = f.tracks[1];
    for kind in ["hex.rattle", "hex.utility"] {
        f.editor
            .execute(hex_project::Command::InsertDevice {
                track: wet,
                before: None,
                state: crate::default_native_device_state(DeviceTypeId::from_static(kind)).unwrap(),
            })
            .unwrap();
    }
    // A long independent path must retain its scalar processing without
    // forcing the short-arrival native track onto that path.
    let limiter = &hex_devices::limiter::DEFINITION;
    let limiter_state = limiter
        .patch_parameter(
            &limiter.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(2),
        )
        .unwrap();
    f.editor
        .execute(hex_project::Command::InsertDevice {
            track: f.tracks[2],
            before: None,
            state: limiter_state,
        })
        .unwrap();
    let second_stage = f.editor.project().track(wet).unwrap().devices()[1].id();
    let step = f.editor.project().tempo_map().frame_to_beat(170).unwrap();
    f.editor
        .execute(hex_project::Command::ReplaceDeviceAutomationTarget {
            device: second_stage,
            target: DeviceAutomationTarget::HostOutputGain,
            points: [
                (BeatTime::ZERO, HostGain::ZERO),
                (step, HostGain::from_millidb(-3000).unwrap()),
            ]
            .into_iter()
            .map(|(beat, gain)| AutomationPoint {
                beat,
                value: AutomationValue::new(gain.normalized()).unwrap(),
                outgoing: AutomationInterpolation::Step,
            })
            .collect(),
        })
        .unwrap();
    f.editor
        .execute(hex_project::Command::ReplaceTrackGainEnvelope {
            track: wet,
            points: [(BeatTime::ZERO, 0.3), (step, 1.1)]
                .into_iter()
                .map(|(beat, gain)| GainPoint {
                    incoming_gain: None,
                    beat,
                    gain,
                    outgoing: GainInterpolation::Linear,
                })
                .collect(),
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    assert_eq!(revision.arrangement().main_latency_frames(), 288);
    let mut batched = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut scalar = SessionPlayback::new(revision).unwrap();
    scalar.resident_source_scratch.clear();
    for runtime in [&mut batched, &mut scalar] {
        runtime.track_faders[0].retarget([0.7, 0.2], 48_000);
        runtime.set_device_metering(true);
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut warmup = [[0.0; 2]; 32];
    let mut expected_warmup = warmup;
    batched.render(&mut warmup).unwrap();
    scalar.render(&mut expected_warmup).unwrap();
    assert_eq!(warmup, expected_warmup);
    assert_eq!(
        batched
            .prepare_native_origin_window(32, 32, 0, &mut [None; 4])
            .unwrap(),
        Some(32)
    );
    let mut output = [[9.0; 2]; 512];
    assert_eq!(
        batched.render_internal::<dyn MediaPages>(
            &mut output,
            None,
            None,
            Some(&mut RefuseTrack(wet)),
            None,
        ),
        Err(SessionPlaybackError::DeviceRuntime)
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    let render = |runtime: &mut SessionPlayback, len| {
        let mut out = vec![[0.0; 2]; len];
        let mut track = out.clone();
        let selection = [wet.value()];
        let mut observer = |signal, offset, frames: &[Frame]| {
            if signal == crate::ProjectSignal::Track(wet.value()) {
                track[offset..offset + frames.len()].copy_from_slice(frames);
            }
        };
        let mut capture = crate::ProjectOutputTap::new(&selection, &mut observer).unwrap();
        runtime
            .render_internal::<dyn MediaPages>(&mut out, None, None, Some(&mut capture), None)
            .unwrap();
        (out, track)
    };
    for size in [1, 7, 256, 512] {
        assert_eq!(render(&mut batched, size), render(&mut scalar, size));
    }
    assert_eq!(batched.track_faders[0], scalar.track_faders[0]);
    assert_eq!(batched.arrangement_frame, scalar.arrangement_frame);
    for runtime in [&mut batched, &mut scalar] {
        runtime.apply(SessionPlaybackCommand::Seek(61)).unwrap();
    }
    for size in [9, 128, 256] {
        assert_eq!(render(&mut batched, size), render(&mut scalar, size));
    }
}

#[test]
fn block_compensated_route_matches_scalar_with_sidechain_group_return_and_main_limiter() {
    use hex_lattice_contract::DeviceTypeId;
    use hex_project::{Command, Created, TrackSelectionIds};

    let mut f = fixture(&[&[], &[], &[]], 333);
    for (i, audio) in f.media.values_mut().enumerate() {
        let frames: Vec<Frame> = (0..333)
            .map(|n| {
                let phase = (n as f32 + 1.0) * (0.07 + i as f32 * 0.03);
                [phase.sin() * 0.35, (phase * 1.3).cos() * 0.25]
            })
            .collect();
        *audio = crate::Audio::new(48_000, frames).unwrap();
    }
    f.editor
        .execute(Command::SetLoopRegion {
            start: 49,
            end: 313,
            enabled: true,
        })
        .unwrap();
    // Sidechain compressor on track 1 keyed from track 0, plus latency-bearing
    // Saturator on track 0 and EQ + Delay on track 2.
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: crate::default_native_device_state(DeviceTypeId::from_static("hex.saturator"))
                .unwrap(),
        })
        .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[1],
            before: None,
            state: crate::default_native_device_state(DeviceTypeId::from_static("hex.compressor"))
                .unwrap(),
        })
        .unwrap();
    let comp_id = f.editor.project().track(f.tracks[1]).unwrap().devices()[0].id();
    f.editor
        .execute(Command::SetSidechain {
            device: comp_id,
            source: Some(f.tracks[0]),
        })
        .unwrap();
    for kind in ["hex.eq", "hex.delay"] {
        f.editor
            .execute(Command::InsertDevice {
                track: f.tracks[2],
                before: None,
                state: crate::default_native_device_state(DeviceTypeId::from_static(kind)).unwrap(),
            })
            .unwrap();
    }
    // Group track wrapping track 1 and track 2, with a Saturator on the group bus.
    let Created::Track(group) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[f.tracks[1], f.tracks[2]]).unwrap(),
            name: "bus".into(),
        })
        .unwrap()
    else {
        panic!("group");
    };
    f.editor
        .execute(Command::InsertDevice {
            track: group,
            before: None,
            state: crate::default_native_device_state(DeviceTypeId::from_static("hex.saturator"))
                .unwrap(),
        })
        .unwrap();
    // Return bus fed by track 0 and the group bus, plus Main Limiter.
    let Created::ReturnTrack(ret) = f
        .editor
        .execute(Command::AddReturnTrack { name: "ret".into() })
        .unwrap()
    else {
        panic!("return");
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: ret,
            gain: 0.35,
        })
        .unwrap();
    f.editor
        .execute(Command::SetReturnSend {
            track: group,
            return_track: ret,
            gain: 0.25,
        })
        .unwrap();
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: crate::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))
                .unwrap(),
        })
        .unwrap();

    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut batched = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut scalar = SessionPlayback::new(revision).unwrap();
    scalar.resident_source_scratch.clear();
    for runtime in [&mut batched, &mut scalar] {
        runtime.set_device_metering(true);
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    // Warm up past max_latency_frames so `can_block_compensated_route` activates.
    let latency = batched
        .compensation
        .as_ref()
        .unwrap()
        .graph
        .plan
        .max_latency_frames() as usize;
    let mut warmup_a = vec![[0.0; 2]; latency + 32];
    let mut warmup_b = warmup_a.clone();
    batched.render(&mut warmup_a).unwrap();
    scalar.render(&mut warmup_b).unwrap();
    assert_eq!(warmup_a, warmup_b);
    assert!(batched.can_block_compensated_route(256));

    // Verify rollback leaves batched state exact.
    let mut rolled = [[9.0; 2]; 128];
    assert_eq!(
        batched.render_internal::<dyn MediaPages>(
            &mut rolled,
            None,
            None,
            Some(&mut RefuseTrack(group)),
            None,
        ),
        Err(SessionPlaybackError::DeviceRuntime)
    );
    assert_eq!(rolled, [[0.0; 2]; 128]);

    let render = |runtime: &mut SessionPlayback, len| {
        let mut out = vec![[0.0; 2]; len];
        let mut captured_group = out.clone();
        let mut captured_t1 = out.clone();
        let mut selection = [group.value(), f.tracks[1].value()];
        selection.sort_unstable();
        let mut observer = |signal, offset, frames: &[Frame]| {
            if signal == crate::ProjectSignal::Track(group.value()) {
                captured_group[offset..offset + frames.len()].copy_from_slice(frames);
            } else if signal == crate::ProjectSignal::Track(f.tracks[1].value()) {
                captured_t1[offset..offset + frames.len()].copy_from_slice(frames);
            }
        };
        let mut capture = crate::ProjectOutputTap::new(&selection, &mut observer).unwrap();
        runtime
            .render_internal::<dyn MediaPages>(&mut out, None, None, Some(&mut capture), None)
            .unwrap();
        (out, captured_group, captured_t1)
    };
    // Cross multiple loop boundaries with mixed block sizes.
    for size in [256, 256, 73, 512, 19, 256] {
        assert_eq!(render(&mut batched, size), render(&mut scalar, size));
    }
    assert_eq!(batched.arrangement_frame, scalar.arrangement_frame);
    for runtime in [&mut batched, &mut scalar] {
        runtime.apply(SessionPlaybackCommand::Seek(61)).unwrap();
    }
    for size in [64, 256, 256] {
        assert_eq!(render(&mut batched, size), render(&mut scalar, size));
    }
}

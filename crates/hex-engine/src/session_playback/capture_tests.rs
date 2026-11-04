use super::*;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{ReturnTrackSnapshot, SendLevelSnapshot};

fn audio_fixture_project(sends: bool, return_muted: bool, solo: bool) -> Project {
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    snapshot.session = SessionSnapshot::default();
    snapshot.master_gain = 0.5;
    snapshot.next_id = 30;
    let mut tracks = Vec::new();
    for (id, clip, gain, balance) in [(1, 10, 0.5, 0.5), (6, 11, 0.75, -0.5)] {
        let mut track = snapshot.tracks[0].clone();
        track.id = id;
        track.gain = gain;
        track.balance = balance;
        track.soloed = solo && id == 1;
        track.content = TrackContentSnapshot::Hybrid {
            audio_clips: vec![ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: clip,
                asset: 4,
                start: 3,
                duration: 1024,
                source_offset: 0,
                source_length: 32,
                looping: true,
                source_phase: 0,
                fade: ClipFade::neutral(1024).unwrap(),
            }],
            midi_clips: Vec::new(),
        };
        if sends {
            track.sends = vec![SendLevelSnapshot {
                return_track: 20,
                gain: if id == 1 { 0.5 } else { 1.0 },
            }];
        }
        tracks.push(track);
    }
    snapshot.tracks = tracks;
    if sends {
        let utility = &hex_devices::utility::UTILITY_DEFINITION;
        let state = utility
            .patch_parameter(
                &utility.default_state().unwrap(),
                hex_devices::utility::parameter::WIDTH,
                ParameterValue::Float(0.0),
            )
            .unwrap();
        snapshot.return_tracks = vec![ReturnTrackSnapshot {
            id: 20,
            name: "shared".into(),
            gain: 0.25,
            muted: return_muted,
            devices: vec![DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: 21,
                bypassed: false,
                state,
            }],
        }];
    }
    Project::from_snapshot(snapshot).unwrap()
}

fn audio_fixture(sends: bool, return_muted: bool, solo: bool) -> Arc<PreparedRevision> {
    prepare_audio(&audio_fixture_project(sends, return_muted, solo))
}

fn prepare_audio(project: &Project) -> Arc<PreparedRevision> {
    let audio = crate::Audio::new(48_000, vec![[0.4, -0.2]; 32]).unwrap();
    Arc::new(
        PreparedRevision::prepare(
            project,
            &audio_media(project, &audio, &audio),
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

fn linear(revision: Arc<PreparedRevision>) -> SessionPlayback {
    let mut runtime = SessionPlayback::new_linear_arrangement_export(revision).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime
}

#[test]
fn metered_analytic_capture_applies_track_gain_once_and_preserves_main_pcm() {
    let mut snapshot = project(0.375, 0, 1 << 26).snapshot();
    snapshot.tracks[0].balance = 0.4;
    snapshot.master_gain = 0.5;
    let project = Project::from_snapshot(snapshot).unwrap();
    let prepared = revision(&project, Arc::new(hex_devices::SineTable::new()));
    let mut reference = linear(Arc::clone(&prepared));
    let mut metered = linear(prepared);
    metered.set_device_metering(true);
    let id = OutputCaptureId::Track(project.tracks()[0].id());
    let plan = metered.output_capture_plan(&[id], 512).unwrap();
    let mut main = [[0.0; 2]; 512];
    let mut expected = main;
    let mut captured = main;
    let mut buffers: [&mut [Frame]; 1] = [&mut captured];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    metered
        .render_with_capture(&mut main, &mut capture)
        .unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(main, expected, "metering and capture preserve main PCM");
    assert!(main[..375].iter().flatten().any(|sample| *sample != 0.0));
    for (frame, output) in capture.output(0).unwrap().iter().zip(main) {
        assert_eq!(frame.map(|sample| sample * 0.5), output);
    }
}

#[test]
fn track_meter_reports_post_fader_audio_including_zero_gain_and_mute() {
    for (gain, muted) in [(1.0, false), (0.25, false), (0.0, false), (1.0, true)] {
        let mut snapshot = audio_fixture_project(false, false, false).snapshot();
        snapshot.tracks[0].gain = gain;
        snapshot.tracks[0].muted = muted;
        let project = Project::from_snapshot(snapshot).unwrap();
        let revision = prepare_audio(&project);
        let observation =
            crate::PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
        let mut runtime = linear(revision);
        runtime.set_device_metering(true);
        runtime.render(&mut [[0.0; 2]; 64]).unwrap();
        let actual = observation
            .track_output_meter(project.tracks()[0].id())
            .unwrap()
            .take_track_output();
        let expected = if muted { 0.0 } else { 0.4 * 0.5 * gain };
        assert!(
            (actual - expected).abs() < 1e-7,
            "gain={gain} muted={muted} actual={actual} expected={expected}"
        );
    }
}

#[test]
fn nonlinear_track_and_shared_return_capture_follow_actual_processing_order() {
    for metered in [false, true] {
        use hex_devices::saturator::{SATURATOR_DEFINITION, parameter};
        let digital = |drive| {
            let state = SATURATOR_DEFINITION
                .patch_parameter(
                    &SATURATOR_DEFINITION.default_state().unwrap(),
                    parameter::DRIVE,
                    ParameterValue::Float(drive),
                )
                .unwrap();
            SATURATOR_DEFINITION
                .patch_parameter(&state, parameter::CURVE, ParameterValue::Choice(1))
                .unwrap()
        };
        let mut snapshot = audio_fixture_project(true, false, false).snapshot();
        snapshot.tracks[0].devices.push(DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: 22,
            bypassed: false,
            state: digital(24.0),
        });
        snapshot.return_tracks[0].devices.insert(
            0,
            DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: 23,
                bypassed: false,
                state: digital(9.0),
            },
        );
        let prepared = prepare_audio(&Project::from_snapshot(snapshot).unwrap());
        let mut runtime = linear(Arc::clone(&prepared));
        runtime.set_device_metering(metered);
        let plan = runtime
            .output_capture_plan(
                &[
                    OutputCaptureId::Track(runtime.routes[0].id),
                    OutputCaptureId::Return(prepared.arrangement().return_buses()[0].id),
                ],
                512,
            )
            .unwrap();
        let mut main = [[0.0; 2]; 512];
        let mut track = main;
        let mut returned = main;
        let mut buffers: [&mut [Frame]; 2] = [&mut track, &mut returned];
        let mut capture = OutputCapture::new(&plan, &mut buffers);
        runtime
            .render_with_capture(&mut main, &mut capture)
            .unwrap();
        let mut reference = linear(prepared);
        let mut expected = main;
        reference.render(&mut expected).unwrap();
        assert_eq!(main, expected);
        // Oracle: standalone oversampled runtimes in processing order. Each
        // device output is silent before its origin reaches the playback
        // epoch; the track tap is delayed to Main's arrival (22 frames) and
        // the dry track waits on its send edge for the track Saturator.
        let saturate = |drive, input: &[Frame], input_arrival: usize| {
            let prepared = hex_devices::saturator::SaturatorDefinition::prepare_owned(
                &digital(drive),
                48_000,
                &[],
            )
            .unwrap();
            let arrival = input_arrival
                + hex_lattice::PreparedNativeProcessor::algorithmic_latency_frames(&prepared)
                    .unwrap() as usize;
            let mut frames = input.to_vec();
            hex_lattice::PreparedNativeProcessor::create_runtime(&prepared)
                .process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            frames[..arrival].fill([0.0; 2]);
            (frames, arrival)
        };
        let delayed = |frames: &[Frame], index: usize, delay: usize| {
            index
                .checked_sub(delay)
                .map_or([0.0; 2], |index| frames[index])
        };
        let source: Vec<Frame> = (0..512)
            .map(|index| if index < 3 { [0.0; 2] } else { [0.4, -0.2] })
            .collect();
        let (track_wet, track_arrival) = saturate(24.0, &source, 0);
        let a: Vec<Frame> = track_wet
            .iter()
            .map(|f| [f[0] * 0.25, f[1] * 0.5])
            .collect();
        let b: Vec<Frame> = source.iter().map(|f| [f[0] * 0.75, f[1] * 0.375]).collect();
        let sent: Vec<Frame> = (0..512)
            .map(|index| {
                let b = delayed(&b, index, track_arrival);
                [a[index][0] * 0.5 + b[0], a[index][1] * 0.5 + b[1]]
            })
            .collect();
        let (return_wet, return_arrival) = saturate(9.0, &sent, track_arrival);
        assert_eq!((track_arrival, return_arrival), (11, 22));
        assert!(a.iter().any(|frame| frame[0] > 0.24));
        for (index, wet) in return_wet.iter().enumerate() {
            let mono = (wet[0] + wet[1]) * 0.5 * 0.25;
            assert_eq!(
                capture.output(0).unwrap()[index],
                delayed(&a, index, return_arrival - track_arrival)
            );
            assert!((capture.output(1).unwrap()[index][0] - mono).abs() < 1e-7);
            assert_eq!(
                capture.output(1).unwrap()[index][0],
                capture.output(1).unwrap()[index][1]
            );
        }
    }
}

#[test]
fn selected_outputs_keep_shared_unselected_sends_and_independent_post_mix_pcm() {
    for (muted, solo) in [(false, false), (true, false), (false, true)] {
        let revision = audio_fixture(true, muted, solo);
        let mut runtime = linear(Arc::clone(&revision));
        runtime.set_device_metering(true);
        let mut main_only = linear(Arc::clone(&revision));
        let track = runtime.routes[0].id;
        let bus = revision.arrangement().return_buses()[0].id;
        let plan = runtime
            .output_capture_plan(
                &[OutputCaptureId::Return(bus), OutputCaptureId::Track(track)],
                512,
            )
            .unwrap();
        let mut main = [[9.0; 2]; 512];
        let mut track_out = main;
        let mut return_out = main;
        let mut buffers: [&mut [Frame]; 2] = [&mut return_out, &mut track_out];
        let mut capture = OutputCapture::new(&plan, &mut buffers);
        runtime
            .render_with_capture(&mut main, &mut capture)
            .unwrap();
        let observation =
            crate::PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
        for (index, meter) in [
            observation.return_output_meter(bus).unwrap(),
            observation.track_output_meter(track).unwrap(),
        ]
        .into_iter()
        .enumerate()
        {
            let expected = capture
                .output(index)
                .unwrap()
                .iter()
                .flatten()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            assert_eq!(meter.take_track_output(), expected);
        }
        assert_eq!(capture.valid_frames(), 512);
        let mut expected_main = [[0.0; 2]; 512];
        main_only.render(&mut expected_main).unwrap();
        assert_eq!(main, expected_main); // Selection preserves exact Main arithmetic.
        for (i, main_frame) in main.iter().enumerate() {
            // Original constant PCM, independent fader/balance/send/matrix oracle.
            let a = if i < 3 {
                [0.0; 2]
            } else {
                [0.4 * 0.25, -0.2 * 0.5]
            };
            let b = if i < 3 || solo {
                [0.0; 2]
            } else {
                [0.4 * 0.75, -0.2 * 0.375]
            };
            let sent = [a[0] * 0.5 + b[0], a[1] * 0.5 + b[1]];
            let mono = (sent[0] + sent[1]) * 0.5 * 0.25;
            let returned = if muted { [0.0; 2] } else { [mono; 2] };
            assert_eq!(capture.output(1).unwrap()[i], a);
            assert_eq!(capture.output(0).unwrap()[i], returned);
            for channel in 0..2 {
                assert!(
                    (main_frame[channel] - (a[channel] + b[channel] + returned[channel]) * 0.5)
                        .abs()
                        < 1e-7
                );
            }
        }
    }
}

#[test]
fn empty_chains_keep_direct_main_rounding_and_capture_partition_offsets() {
    let revision = audio_fixture(false, false, false);
    let mut runtime = linear(Arc::clone(&revision));
    let id = OutputCaptureId::Track(runtime.routes[1].id);
    let plan = runtime.output_capture_plan(&[id], 512).unwrap();
    let mut expected_main = [[0.0; 2]; 512];
    linear(Arc::clone(&revision))
        .render(&mut expected_main)
        .unwrap();
    let mut actual = Vec::new();
    let mut captured = Vec::new();
    for count in [1, 2, 17, 127, 256, 109] {
        let mut main = vec![[9.0; 2]; count];
        let mut output = vec![[9.0; 2]; count + 3];
        let mut buffers = [&mut output[..]];
        let mut view = OutputCapture::new(&plan, &mut buffers);
        runtime.render_with_capture(&mut main, &mut view).unwrap();
        captured.extend_from_slice(view.output(0).unwrap());
        assert_eq!(view.valid_frames(), count);
        actual.extend_from_slice(&main);
    }
    assert_eq!(actual, expected_main);
    for (index, frame) in captured.iter().enumerate() {
        assert_eq!(
            *frame,
            if index < 3 {
                [0.0; 2]
            } else {
                [0.4 * 0.75, -0.2 * 0.375]
            }
        );
    }
}

#[test]
fn capture_preflight_refuses_duplicate_missing_stale_capacity_without_runtime_mutation() {
    let revision = audio_fixture(false, false, false);
    let mut runtime = linear(Arc::clone(&revision));
    let id = OutputCaptureId::Track(runtime.routes[0].id);
    assert_eq!(
        runtime.output_capture_plan(&[id, id], 1).unwrap_err(),
        OutputCaptureError::DuplicateOwner
    );
    // Return ID is valid canonically but absent from this runtime's registry.
    let with_bus = audio_fixture(true, false, false);
    let absent = OutputCaptureId::Return(with_bus.arrangement().return_buses()[0].id);
    assert_eq!(
        runtime.output_capture_plan(&[absent], 1).unwrap_err(),
        OutputCaptureError::MissingOwner
    );
    assert_eq!(
        runtime.output_capture_plan(&[id], 0).unwrap_err(),
        OutputCaptureError::InsufficientCapacity
    );
    assert_eq!(
        runtime
            .output_capture_plan(&[id], MAX_OUTPUT_CAPTURE_FRAMES + 1)
            .unwrap_err(),
        OutputCaptureError::InsufficientCapacity
    );
    assert_eq!(
        runtime
            .output_capture_plan(&vec![id; MAX_OUTPUT_CAPTURES + 1], 1)
            .unwrap_err(),
        OutputCaptureError::TooManyOutputs
    );
    let stale = linear(Arc::clone(&revision))
        .output_capture_plan(&[id], 512)
        .unwrap();
    let plan = runtime.output_capture_plan(&[id], 512).unwrap();
    for (plan, capacity, error) in [
        (&stale, 512, OutputCaptureError::StalePlan),
        (&plan, 511, OutputCaptureError::InsufficientCapacity),
    ] {
        let before = (
            runtime.performance_frame,
            runtime.arrangement_frame,
            runtime.candidate_admission_open,
            runtime.routes.clone(),
        );
        let mut main = [[9.0; 2]; 512];
        let mut output = vec![[9.0; 2]; capacity];
        let mut buffers = [&mut output[..]];
        let mut capture = OutputCapture::new(plan, &mut buffers);
        assert_eq!(
            runtime.render_with_capture(&mut main, &mut capture),
            Err(SessionPlaybackError::Capture(error))
        );
        assert_eq!(capture.valid_frames(), 0);
        assert_eq!(main, [[0.0; 2]; 512]);
        assert!(output.iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(
            (
                runtime.performance_frame,
                runtime.arrangement_frame,
                runtime.candidate_admission_open,
                runtime.routes.clone()
            ),
            before
        );
    }
}

#[test]
fn mutable_held_and_released_history_capture_matches_independent_feedback_recurrence() {
    let (revision, track) = mutable_checkpoint_fixture(1 << 26); // 375-frame gate at 48k/120.
    let mut runtime = linear(Arc::clone(&revision));
    let plan = runtime
        .output_capture_plan(&[OutputCaptureId::Track(track)], 512)
        .unwrap();
    let mut main = [[0.0; 2]; 512];
    let mut output = main;
    let mut buffers = [&mut output[..]];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    runtime
        .render_with_capture(&mut main, &mut capture)
        .unwrap();
    let mut reference = linear(revision);
    let mut expected = main;
    reference.render(&mut expected).unwrap();
    assert_eq!(main, expected);
    let mut feedback = 0.0_f32;
    for index in 0..512 {
        let direct = if index < 383 { 0.2 } else { 0.0 };
        feedback = feedback * 0.5 + direct * 0.25;
        let oracle = direct + feedback;
        assert!(
            (capture.output(0).unwrap()[index][0] - oracle).abs() < 1e-6,
            "frame {index}"
        );
        if index == 389 {
            feedback = 0.0;
        }
    }
}

#[test]
fn late_chunk_refusal_invalidates_capture_and_retry_preserves_clock_and_history() {
    for metered in [false, true] {
        let (revision, track) = mutable_checkpoint_fixture(1 << 26);
        let mut runtime = linear(Arc::clone(&revision));
        runtime.set_device_metering(metered);
        runtime.render(&mut [[0.0; 2]; 1]).unwrap();
        // Inject a finite range-overflow fault only on chunk two; the held
        // instrument has committed history and advances speculatively in chunk one.
        runtime.arrangement_frame = u64::MAX - 300;
        let routes = runtime.routes.clone();
        let plan = runtime
            .output_capture_plan(&[OutputCaptureId::Track(track)], 512)
            .unwrap();
        let mut main = [[9.0; 2]; 512];
        let mut output = main;
        let mut buffers = [&mut output[..]];
        let mut capture = OutputCapture::new(&plan, &mut buffers);
        assert_eq!(
            runtime.render_with_capture(&mut main, &mut capture),
            Err(SessionPlaybackError::Arrangement("render range overflow"))
        );
        assert_eq!(main, [[0.0; 2]; 512]);
        assert_eq!(capture.valid_frames(), 0);
        assert_eq!(runtime.performance_frame, 1);
        assert_eq!(runtime.arrangement_frame, u64::MAX - 300);
        assert_eq!(runtime.routes, routes);
        assert!(runtime.releases.is_empty());
        runtime.arrangement_frame = 1;
        runtime
            .render_with_capture(&mut main, &mut capture)
            .unwrap();
        let mut reference = linear(revision);
        reference.render(&mut [[0.0; 2]; 1]).unwrap();
        let mut expected = main;
        reference.render(&mut expected).unwrap();
        assert_eq!(main, expected);
        assert_eq!(capture.output(0).unwrap(), expected);
    }
}

#[test]
fn interactive_capture_rechecks_arrangement_cut_after_launch_and_revision_loop_change() {
    let original = project(0.5, 0, 8 << 32);
    let prepared = revision(&original, Arc::new(hex_devices::SineTable::new()));
    let mut runtime = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let id = OutputCaptureId::Track(runtime.routes[0].id);
    let plan = runtime.output_capture_plan(&[id], 8).unwrap();
    let clip = prepared.session().midi_clips()[0].id;
    runtime
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let clock = (runtime.performance_frame, runtime.arrangement_frame);
    let mut main = [[9.0; 2]; 8];
    let mut captured = main;
    let mut buffers = [&mut captured[..]];
    let mut view = OutputCapture::new(&plan, &mut buffers);
    assert_eq!(
        runtime.render_with_capture(&mut main, &mut view),
        Err(SessionPlaybackError::Capture(
            OutputCaptureError::UnsupportedRuntime
        ))
    );
    assert_eq!(
        (runtime.performance_frame, runtime.arrangement_frame),
        clock
    );
    assert_eq!(view.valid_frames(), 0);
    assert_eq!(main, [[0.0; 2]; 8]);
    runtime
        .apply(SessionPlaybackCommand::CancelPending { track: None })
        .unwrap();
    runtime
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        runtime.output_capture_plan(&[id], 8).unwrap_err(),
        OutputCaptureError::UnsupportedRuntime
    );
    runtime
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    runtime.render_with_capture(&mut main, &mut view).unwrap();
    let mut changed = original.snapshot();
    changed.loop_region = Some(LoopRegion::new(0, 1024, true).unwrap());
    let prepared_loop = revision(
        &Project::from_snapshot(changed).unwrap(),
        Arc::new(hex_devices::SineTable::new()),
    );
    let mut looped = SessionPlayback::new(prepared_loop).unwrap();
    assert_eq!(
        looped.output_capture_plan(&[id], 8).unwrap_err(),
        OutputCaptureError::UnsupportedRuntime
    );
    assert_eq!(
        looped.render_with_capture(&mut main, &mut view),
        Err(SessionPlaybackError::Capture(
            OutputCaptureError::UnsupportedRuntime
        ))
    );
}

fn analytic_hybrid_fixture() -> (Project, BTreeMap<hex_project::AssetId, crate::Audio>) {
    let device_type = hex_lattice_contract::DeviceTypeId::from_static("test.capture.analytic");
    let base = with_test_instrument(&project(1.0, 0, 1 << 26), device_type, 0);
    let mut snapshot = base.snapshot();
    snapshot.session = SessionSnapshot::default();
    snapshot.next_id = 90;
    snapshot.master_gain = 0.25;
    snapshot.tracks[0].gain = 0.5;
    snapshot.tracks[0].balance = -0.5;
    snapshot.assets = vec![AssetSnapshot {
        original_source: None,
        id: 80,
        path: "hybrid.wav".into(),
        source_sample_rate: 48_000,
        source_frames: 32,
        playback_frames: 32,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    }];
    let midi_clips = match &snapshot.tracks[0].content {
        TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips.clone(),
        _ => panic!("canonical hybrid instrument"),
    };
    snapshot.tracks[0].content = TrackContentSnapshot::Hybrid {
        midi_clips,
        audio_clips: vec![ClipSnapshot {
            active: true,
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            edit: None,
            gain: hex_project::ClipGain::UNITY,
            id: 81,
            asset: 80,
            start: 3,
            duration: 512,
            source_offset: 0,
            source_length: 32,
            looping: true,
            source_phase: 0,
            fade: ClipFade::neutral(512).unwrap(),
        }],
    };
    routed_midi_fixture::separate_midi(&mut snapshot, 0);
    let project = Project::from_snapshot(snapshot).unwrap();
    let media = BTreeMap::from([(
        project.assets().next().unwrap().id(),
        crate::Audio::new(48_000, vec![[0.4, -0.2]; 32]).unwrap(),
    )]);
    (project, media)
}

#[test]
fn loop_end_fades_audio_without_fading_the_hybrid_instrument() {
    let device_type = hex_lattice_contract::DeviceTypeId::from_static("test.capture.analytic");
    let definition = ContinuityInstrumentDefinition {
        id: device_type,
        levels: [0.25, 0.5],
        release_frames: 4,
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let (project, media) = analytic_hybrid_fixture();
    let mut snapshot = project.snapshot();
    snapshot.loop_region = Some(LoopRegion::new(0, 64, true).unwrap());
    let project = Project::from_snapshot(snapshot).unwrap();
    let asset = project.assets().next().unwrap().id();
    let silent_media = BTreeMap::from([(
        asset,
        crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap(),
    )]);
    let prepare = |media| {
        Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                &project,
                media,
                Arc::new(hex_devices::SineTable::new()),
                None,
                &registry,
            )
            .unwrap(),
        )
    };
    let mut hybrid = SessionPlayback::new(prepare(&media)).unwrap();
    let mut midi_only = SessionPlayback::new(prepare(&silent_media)).unwrap();
    hybrid.apply(SessionPlaybackCommand::Play).unwrap();
    midi_only.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 128];
    let mut instrument = [[0.0; 2]; 128];
    hybrid.render(&mut actual).unwrap();
    midi_only.render(&mut instrument).unwrap();
    for seam in [63, 127] {
        assert_ne!(instrument[seam], [0.0; 2], "instrument remains audible");
        assert_eq!(actual[seam], instrument[seam], "audio fades independently");
        assert_ne!(
            actual[seam - 1],
            instrument[seam - 1],
            "audio is still fading before the seam"
        );
    }
}

#[test]
fn analytic_hybrid_metered_capture_is_partition_independent() {
    for metered in [false, true] {
        let device_type = hex_lattice_contract::DeviceTypeId::from_static("test.capture.analytic");
        let definition = ContinuityInstrumentDefinition {
            id: device_type,
            levels: [0.25, 0.5],
            release_frames: 4,
        };
        let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
        let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
        let (project, media) = analytic_hybrid_fixture();
        let revision = Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                &project,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
                &registry,
            )
            .unwrap(),
        );
        let mut expected = [[0.0; 2]; 512];
        linear(Arc::clone(&revision)).render(&mut expected).unwrap();
        // Same physical oracle across callback-sized partitions; the uncaptured
        // reference above intentionally leaves metering off.
        let mut partitioned = linear(Arc::clone(&revision));
        partitioned.set_device_metering(metered);
        let partition_plan = partitioned
            .output_capture_plan(&[OutputCaptureId::Track(project.tracks()[0].id())], 512)
            .unwrap();
        let expected_capture: Vec<_> = (0..512)
            .map(|index| {
                let level = if index < 379 { 0.25 } else { 0.0 };
                let audio = if index < 3 { [0.0; 2] } else { [0.4, -0.2] };
                [(audio[0] + level) * 0.5, (audio[1] + level) * 0.25]
            })
            .collect();
        let mut offset = 0;
        for count in [1, 2, 17, 127, 256, 109] {
            let mut block = vec![[9.0; 2]; count];
            let mut selected = block.clone();
            let mut views = [&mut selected[..]];
            let mut partition_capture = OutputCapture::new(&partition_plan, &mut views);
            partitioned
                .render_with_capture(&mut block, &mut partition_capture)
                .unwrap();
            assert_eq!(block, expected[offset..offset + count]);
            assert_eq!(
                partition_capture.output(0).unwrap(),
                &expected_capture[offset..offset + count]
            );
            offset += count;
        }
    }
}

#[test]
fn hybrid_metered_capture_preserves_prefix_suffix_send_return_processing_order() {
    for metered in [false, true] {
        let device_type = hex_lattice_contract::DeviceTypeId::from_static("test.capture.analytic");
        let definition = ContinuityInstrumentDefinition {
            id: device_type,
            levels: [0.25, 0.5],
            release_frames: 4,
        };
        let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
        let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
        let (project, media) = analytic_hybrid_fixture();
        // Real hybrid prefix -> instrument -> nonlinear suffix -> track mix ->
        // send -> return -> master. Neither metering nor capture may move a stage.
        let utility = &hex_devices::utility::UTILITY_DEFINITION;
        let prefix = utility
            .patch_parameter(
                &utility.default_state().unwrap(),
                hex_devices::utility::parameter::GAIN,
                ParameterValue::Float(2.0),
            )
            .unwrap();
        let saturator = &hex_devices::saturator::SATURATOR_DEFINITION;
        let suffix = saturator
            .patch_parameter(
                &saturator.default_state().unwrap(),
                hex_devices::saturator::parameter::DRIVE,
                ParameterValue::Float(6.0),
            )
            .unwrap();
        let suffix = saturator
            .patch_parameter(
                &suffix,
                hex_devices::saturator::parameter::CURVE,
                ParameterValue::Choice(1),
            )
            .unwrap();
        let mut routed = project.snapshot();
        routed.next_id = 110;
        routed.tracks[0].devices = vec![
            DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: 100,
                bypassed: false,
                state: prefix,
            },
            DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: 101,
                bypassed: false,
                state: suffix.clone(),
            },
        ];
        routed.tracks[0].instrument_position = Some(1);
        routed.tracks[0].sends = vec![SendLevelSnapshot {
            return_track: 102,
            gain: 0.5,
        }];
        routed.return_tracks = vec![ReturnTrackSnapshot {
            id: 102,
            name: "oracle".into(),
            gain: 0.25,
            muted: false,
            devices: vec![DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: 103,
                bypassed: false,
                state: utility.default_state().unwrap(),
            }],
        }];
        let routed = Project::from_snapshot(routed).unwrap();
        let prepared = Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                &routed,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
                &registry,
            )
            .unwrap(),
        );
        let mut routed_runtime = linear(Arc::clone(&prepared));
        routed_runtime.set_device_metering(metered);
        let routed_plan = routed_runtime
            .output_capture_plan(
                &[
                    OutputCaptureId::Track(routed.tracks()[0].id()),
                    OutputCaptureId::Return(routed.return_tracks()[0].id()),
                ],
                512,
            )
            .unwrap();
        let mut routed_main = [[0.0; 2]; 512];
        let mut track_output = routed_main;
        let mut return_output = routed_main;
        let mut outputs: [&mut [Frame]; 2] = [&mut track_output, &mut return_output];
        let mut routed_capture = OutputCapture::new(&routed_plan, &mut outputs);
        routed_runtime
            .render_with_capture(&mut routed_main, &mut routed_capture)
            .unwrap();
        let mut unobserved = linear(prepared);
        let mut unobserved_pcm = routed_main;
        unobserved.render(&mut unobserved_pcm).unwrap();
        assert_eq!(routed_main, unobserved_pcm);
        // Oracle: the suffix is a standalone oversampled Saturator runtime
        // over prefix + instrument. Its declared latency delays every later
        // stage equally, and its output is silent before the playback epoch.
        let gain = 2.0_f32;
        let suffix_input: Vec<Frame> = (0..routed_main.len())
            .map(|index| {
                let audio = if index < 3 { [0.0; 2] } else { [0.4_f32, -0.2] };
                let level = if index < 379 { 0.25 } else { 0.0 };
                audio.map(|sample| sample * gain + level)
            })
            .collect();
        let prepared_suffix =
            hex_devices::saturator::SaturatorDefinition::prepare_owned(&suffix, 48_000, &[])
                .unwrap();
        let latency =
            hex_lattice::PreparedNativeProcessor::algorithmic_latency_frames(&prepared_suffix)
                .unwrap() as usize;
        assert_eq!(latency, 11);
        let mut suffix_output = suffix_input;
        hex_lattice::PreparedNativeProcessor::create_runtime(&prepared_suffix)
            .process_block(&mut suffix_output, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        suffix_output[..latency].fill([0.0; 2]);
        assert!(suffix_output.iter().any(|frame| frame[0] > 0.9));
        for (index, main_frame) in routed_main.iter().enumerate() {
            let processed = suffix_output[index];
            let track = [processed[0] * 0.5, processed[1] * 0.25];
            let returned = track.map(|sample| sample * 0.5 * 0.25);
            for channel in 0..2 {
                assert!(
                    (routed_capture.output(0).unwrap()[index][channel] - track[channel]).abs()
                        < 1e-7
                );
                assert!(
                    (routed_capture.output(1).unwrap()[index][channel] - returned[channel]).abs()
                        < 1e-7
                );
                assert!(
                    (main_frame[channel] - (track[channel] + returned[channel]) * 0.25).abs()
                        < 1e-7
                );
            }
        }
    }
}

#[test]
fn analytic_hybrid_empty_chain_captures_audio_and_release_under_one_owner() {
    for metered in [false, true] {
        let device_type = hex_lattice_contract::DeviceTypeId::from_static("test.capture.analytic");
        let definition = ContinuityInstrumentDefinition {
            id: device_type,
            levels: [0.25, 0.5],
            release_frames: 4,
        };
        let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
        let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
        let (project, media) = analytic_hybrid_fixture();
        let revision = Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                &project,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
                &registry,
            )
            .unwrap(),
        );
        let mut runtime = linear(Arc::clone(&revision));
        runtime.set_device_metering(metered);
        let plan = runtime
            .output_capture_plan(&[OutputCaptureId::Track(project.tracks()[0].id())], 512)
            .unwrap();
        let mut main = [[0.0; 2]; 512];
        let mut captured = main;
        let mut buffers: [&mut [Frame]; 1] = [&mut captured];
        let mut capture = OutputCapture::new(&plan, &mut buffers);
        runtime
            .render_with_capture(&mut main, &mut capture)
            .unwrap();
        let mut reference = linear(Arc::clone(&revision));
        let mut expected = main;
        reference.render(&mut expected).unwrap();
        assert_eq!(main, expected);
        for (index, frame) in capture.output(0).unwrap().iter().enumerate() {
            let level = if index < 379 { 0.25 } else { 0.0 };
            let audio = if index < 3 { [0.0; 2] } else { [0.4, -0.2] };
            let expected = [(audio[0] + level) * 0.5, (audio[1] + level) * 0.25];
            assert_eq!(*frame, expected);
            assert_eq!(main[index], expected.map(|sample| sample * 0.25));
        }
        let mut gain_source = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        gain_source.set_device_metering(metered);
        gain_source.apply(SessionPlaybackCommand::Play).unwrap();
        gain_source.render(&mut [[0.0; 2]; 128]).unwrap();
        let mut retarget_snapshot = project.snapshot();
        retarget_snapshot.tracks[0].gain = 0.25;
        retarget_snapshot.tracks[0].balance = 0.5;
        let retarget_project = Project::from_snapshot(retarget_snapshot).unwrap();
        let retarget_revision = Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                &retarget_project,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                Some(revision.session()),
                &registry,
            )
            .unwrap(),
        );
        let observation =
            crate::PreparedPlaybackObservation::prepare_on_control(&retarget_revision).unwrap();
        observation
            .track_output_meter(project.tracks()[0].id())
            .unwrap()
            .take_track_output();
        let mut retarget = SessionPlayback::new(retarget_revision).unwrap();
        retarget.set_device_metering(metered);
        retarget.try_inherit_state_from(&mut gain_source).unwrap();
        assert!(retarget.releases.is_empty());
        assert!(retarget.tail_groups.iter().all(Option::is_none));
        let retarget_plan = retarget
            .output_capture_plan(&[OutputCaptureId::Track(project.tracks()[0].id())], 16)
            .unwrap();
        let mut retarget_main = [[9.0; 2]; 16];
        let mut retarget_track = retarget_main;
        let mut retarget_buffers: [&mut [Frame]; 1] = [&mut retarget_track];
        let mut retarget_capture = OutputCapture::new(&retarget_plan, &mut retarget_buffers);
        retarget
            .render_with_capture(&mut retarget_main, &mut retarget_capture)
            .unwrap();
        for (index, frame) in retarget_capture.output(0).unwrap().iter().enumerate() {
            let expected = [(0.4 + 0.25) * 0.125, (-0.2 + 0.25) * 0.25];
            assert_eq!(*frame, expected);
            assert_eq!(retarget_main[index], expected.map(|sample| sample * 0.25));
        }
        if metered {
            let observed = observation
                .track_output_meter(project.tracks()[0].id())
                .unwrap()
                .take_track_output();
            assert!(
                (observed - (0.4 + 0.25) * 0.125).abs() < 1e-7,
                "track meter must follow the new post-fader gain: {observed}"
            );
        }
        // Owner removal uses a local MIDI source, preserving the current
        // handoff contract that does not replace a live routed source layout.
        // The routed hybrid path and exact audio sum are checked above.
        let mut local = project.snapshot();
        let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut local.tracks[1].content else {
            panic!("routed source")
        };
        let midi_clips = std::mem::take(midi_clips);
        local.tracks[0].content = TrackContentSnapshot::Hybrid {
            audio_clips: Vec::new(),
            midi_clips,
        };
        local.tracks.truncate(1);
        local.midi_routes.clear();
        let local = Project::from_snapshot(local).unwrap();
        let local_revision = Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                &local,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
                &registry,
            )
            .unwrap(),
        );
        let mut old = SessionPlayback::new(local_revision).unwrap();
        old.set_device_metering(metered);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 128]).unwrap();
        let id = project.tracks()[0].id();
        let mut removed = local.snapshot();
        removed.tracks.clear();
        for source in &mut removed.native_sources {
            source.receiver = None;
        }
        removed.midi_routes.clear();
        let removed = Project::from_snapshot(removed).unwrap();
        let removed_revision = Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                &removed,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
                &registry,
            )
            .unwrap(),
        );
        let mut inherited = SessionPlayback::new(removed_revision).unwrap();
        inherited.set_device_metering(metered);
        inherited.try_inherit_state_from(&mut old).unwrap();
        let retired = inherited
            .output_capture_plan(&[OutputCaptureId::Track(id)], 8)
            .unwrap();
        let mut main = [[9.0; 2]; 8];
        let mut tail = main;
        let mut buffers: [&mut [Frame]; 1] = [&mut tail];
        let mut capture = OutputCapture::new(&retired, &mut buffers);
        inherited
            .render_with_capture(&mut main, &mut capture)
            .unwrap();
        for (index, frame) in capture.output(0).unwrap().iter().enumerate() {
            assert_eq!(*frame, if index < 4 { [0.125, 0.0625] } else { [0.0; 2] });
            assert_eq!(main[index], frame.map(|sample| sample * 0.25));
        }
        assert_eq!(
            inherited.render_with_capture(&mut main, &mut capture),
            Err(SessionPlaybackError::Capture(
                OutputCaptureError::MissingOwner
            ))
        );
        assert_eq!(capture.valid_frames(), 0);
        assert_eq!(inherited.performance_frame(), 136);
    }
}

#[test]
fn capture_bounds_and_handoff_generation_exhaustion_refuse_before_runtime_mutation() {
    let revision = audio_fixture(false, false, false);
    let mut runtime = linear(Arc::clone(&revision));
    let id = OutputCaptureId::Track(runtime.routes[0].id);
    let plan = runtime.output_capture_plan(&[id], 1).unwrap();
    let mut oversized = vec![[9.0; 2]; MAX_OUTPUT_CAPTURE_FRAMES + 1];
    let mut buffers: [&mut [Frame]; 1] = [&mut oversized];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    let mut main = [[9.0; 2]; 1];
    assert_eq!(
        runtime.render_with_capture(&mut main, &mut capture),
        Err(SessionPlaybackError::Capture(
            OutputCaptureError::InsufficientCapacity
        ))
    );
    assert_eq!(capture.valid_frames(), 0);
    assert!(capture.output(0).unwrap().is_empty());
    assert_eq!(runtime.performance_frame(), 0);
    assert_eq!(main, [[0.0; 2]; 1]);
    assert!(
        oversized[..MAX_OUTPUT_CAPTURE_FRAMES]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    // Out-of-contract raw storage remains inaccessible through the invalid view.
    assert_eq!(oversized[MAX_OUTPUT_CAPTURE_FRAMES], [9.0; 2]);
    for donor_exhausted in [false, true] {
        let mut previous = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        previous.render(&mut [[0.0; 2]; 4]).unwrap();
        let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        if donor_exhausted {
            previous.capture_epoch = u64::MAX;
        } else {
            candidate.capture_epoch = u64::MAX;
        }
        let before = (
            previous.capture_epoch,
            candidate.capture_epoch,
            previous.performance_frame(),
            candidate.performance_frame(),
            previous.routes.clone(),
            candidate.routes.clone(),
        );
        assert_eq!(
            candidate.try_inherit_state_from(&mut previous),
            Err(SessionPlaybackError::ArithmeticOverflow)
        );
        assert_eq!(
            (
                previous.capture_epoch,
                candidate.capture_epoch,
                previous.performance_frame(),
                candidate.performance_frame(),
                previous.routes.clone(),
                candidate.routes.clone()
            ),
            before
        );
    }
}

#[test]
fn reused_capture_buffers_only_clear_rendered_prefix_on_success() {
    let mut runtime = linear(audio_fixture(true, false, false));
    let ids = [
        OutputCaptureId::Track(runtime.routes[0].id),
        OutputCaptureId::Return(runtime.revision.arrangement().return_buses()[0].id),
    ];
    let plan = runtime
        .output_capture_plan(&ids, MAX_OUTPUT_CAPTURE_FRAMES)
        .unwrap();
    let sentinel = [9.0, -9.0];
    let mut track = vec![sentinel; MAX_OUTPUT_CAPTURE_FRAMES];
    let mut bus = track.clone();
    for frames in [256, 0, 17] {
        let previous_track = track.clone();
        let previous_bus = bus.clone();
        let mut main = vec![[0.0; 2]; frames];
        let mut buffers: [&mut [Frame]; 2] = [&mut track, &mut bus];
        let mut capture = OutputCapture::new(&plan, &mut buffers);
        runtime
            .render_with_capture(&mut main, &mut capture)
            .unwrap();
        assert_eq!(capture.valid_frames(), frames);
        assert!(
            capture
                .output(0)
                .unwrap()
                .iter()
                .flatten()
                .all(|sample| sample.abs() < 1.0)
        );
        assert!(
            capture
                .output(1)
                .unwrap()
                .iter()
                .flatten()
                .all(|sample| sample.abs() < 1.0)
        );
        assert_eq!(track[frames..], previous_track[frames..]);
        assert_eq!(bus[frames..], previous_bus[frames..]);
    }
    // Refusal still performs the documented bounded full-slice cleanup.
    let mut buffers: [&mut [Frame]; 2] = [&mut track, &mut bus];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    let mut oversized = vec![[9.0; 2]; MAX_OUTPUT_CAPTURE_FRAMES + 1];
    assert!(
        runtime
            .render_with_capture(&mut oversized, &mut capture)
            .is_err()
    );
    assert_eq!(capture.valid_frames(), 0);
    assert!(track.iter().chain(&bus).all(|frame| *frame == [0.0; 2]));
}

#[path = "capture_routing_tests.rs"]
mod routing;

fn owned_effect_revision(compensated: bool) -> Arc<PreparedRevision> {
    use hex_lattice::conformance::owned_delay;
    let mut snapshot = audio_fixture_project(true, false, false).snapshot();
    snapshot.tracks[0].devices = vec![DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: 31,
        bypassed: false,
        state: owned_delay::state(17).unwrap(),
    }];
    if compensated {
        snapshot.tracks[1].devices = vec![DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: 32,
            bypassed: false,
            state: crate::signal_graph::tests::state(7).unwrap(),
        }];
    }
    snapshot.return_tracks[0].devices = vec![DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: 21,
        bypassed: false,
        state: owned_delay::state(19).unwrap(),
    }];
    snapshot.next_id = 40;
    let project = Project::from_snapshot(snapshot).unwrap();
    let samples = crate::Audio::new(48_000, vec![[0.4, -0.2]; 32]).unwrap();
    let effects: [&dyn hex_lattice::NativeDeviceDefinition; 3] = [
        &owned_delay::DEFINITION,
        &hex_devices::utility::UTILITY_DEFINITION,
        &crate::signal_graph::tests::Definition,
    ];
    let registry = hex_lattice::NativeDeviceRegistry::new(&effects).unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    Arc::new(
        PreparedRevision::prepare_with_registries(
            &project,
            &audio_media(&project, &samples, &samples),
            Arc::new(hex_devices::SineTable::new()),
            None,
            &registry,
            &instruments,
        )
        .unwrap(),
    )
}

#[test]
fn owned_track_and_return_late_abort_close_transactions_before_retry_or_handoff() {
    for compensated in [false, true] {
        let revision = owned_effect_revision(compensated);
        let mut tested = linear(Arc::clone(&revision));
        let mut reference = linear(Arc::clone(&revision));
        assert_eq!(tested.compensation.is_some(), compensated);
        for value in [&mut tested, &mut reference] {
            value.render(&mut [[0.0; 2]; 4]).unwrap();
        }
        for value in [&mut tested, &mut reference] {
            let bus = &revision.arrangement().return_buses()[0];
            let mut chain =
                hex_lattice::NativeRuntimeChain::new_prepared(bus.device_chain.to_vec()).unwrap();
            chain.begin_block();
            chain.process_block(&mut [[0.6, -0.3]; 3]).unwrap();
            chain.commit_block();
            value.return_effect_tails.slots_mut()[0] = Some(
                super::return_effect_tails::ReturnEffectTail::new(chain, bus),
            );
        }
        tested.set_device_metering(true);
        let observation =
            crate::PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
        let track_id = tested.routes[0].id;
        let return_id = revision.arrangement().return_buses()[0].id;
        tested.arrangement_frame = u64::MAX - 300;
        let mut observed = 0;
        let mut actual = [[9.0; 2]; 512];
        assert!(
            tested
                .render_with_post_node_observer(
                    &mut actual,
                    Some(&mut |_, _, _: &[Frame]| observed += 1)
                )
                .is_err()
        );
        assert!(
            observed > 0,
            "failure must follow speculative effect processing"
        );
        assert_eq!(actual, [[0.0; 2]; 512]);
        assert_eq!(
            observation
                .track_output_meter(track_id)
                .unwrap()
                .take_track_output(),
            0.0
        );
        assert_eq!(
            observation
                .return_output_meter(return_id)
                .unwrap()
                .take_track_output(),
            0.0
        );
        assert_eq!(tested.performance_frame, 4);
        let candidate = linear(revision);
        // This provider forbids inheriting an open transaction. A next-begin
        // implicit restore is insufficient at the immediate handoff frontier.
        assert!(candidate.device_chains[0].can_inherit_from(&tested.device_chains[0]));
        assert!(candidate.return_chains[0].can_inherit_from(&tested.return_chains[0]));
        assert!(
            candidate.return_chains[0].can_inherit_from(
                &tested.return_effect_tails.slots()[0]
                    .as_ref()
                    .unwrap()
                    .chain
            )
        );
        tested.arrangement_frame = 4;
        let mut expected = actual;
        tested.render(&mut actual).unwrap();
        reference.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(tested.performance_frame, reference.performance_frame);
    }
}

#[test]
fn owned_stopped_monitor_drains_private_history_and_closes_transactions() {
    for compensated in [false, true] {
        let revision = owned_effect_revision(compensated);
        let mut tested = linear(Arc::clone(&revision));
        tested.render(&mut [[0.0; 2]; 16]).unwrap();
        tested.playing = false;
        // Install an idle canonical keyboard fixture so the public renderer
        // enters stopped monitoring rather than its no-monitor silence path.
        let keyboard_project = project(0.375, 0, 1 << 26);
        let keyboard = crate::computer_keyboard::ComputerKeyboardRuntime::prepare(
            &keyboard_project,
            keyboard_project.tracks()[0].id(),
            Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default()),
        )
        .unwrap();
        tested.computer_keyboard = Some(Box::new(keyboard));
        let before = (tested.arrangement_frame, tested.performance_frame);
        let mut output = [[0.0; 2]; 128];
        let mut master = [[0.0; 2]; 128];
        let mut stem = [[0.0; 2]; 128];
        let mut observer = |signal, offset: usize, frames: &[Frame]| match signal {
            ProjectSignal::Master => master[offset..offset + frames.len()].copy_from_slice(frames),
            ProjectSignal::Track(1) => {
                for (to, from) in stem[offset..offset + frames.len()].iter_mut().zip(frames) {
                    to[0] += from[0];
                    to[1] += from[1];
                }
            }
            ProjectSignal::Track(_) => panic!("unexpected track"),
            ProjectSignal::Unavailable => panic!("stopped canonical monitor is available"),
        };
        let mut tap = ProjectOutputTap::new(&[1], &mut observer).unwrap();
        tested
            .render_with_signal_observers(&mut output, None, Some(&mut tap))
            .unwrap();
        assert_eq!(
            master, output,
            "stopped monitor master is actual rendered PCM"
        );
        assert!(
            stem.iter().flatten().any(|s| *s != 0.0),
            "post-device history is retained in its direct track layer"
        );
        assert!(
            output.iter().any(|frame| *frame != [0.0; 2]),
            "compensated={compensated}"
        );
        assert_eq!((tested.arrangement_frame, tested.performance_frame), before);
        let candidate = linear(revision);
        assert!(candidate.device_chains[0].can_inherit_from(&tested.device_chains[0]));
        assert!(candidate.return_chains[0].can_inherit_from(&tested.return_chains[0]));
        tested.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 128]);
    }
}

#[test]
fn live_project_taps_preserve_pcm_and_independent_tracks_across_loop_wraps() {
    for looping in [false, true] {
        let mut snapshot = audio_fixture_project(true, false, false).snapshot();
        snapshot.loop_region = looping.then(|| LoopRegion::new(0, 1024, true).unwrap());
        let project = Project::from_snapshot(snapshot).unwrap();
        let prepared = prepare_audio(&project);
        let playback = crate::playback::Playback::from_prepared(Arc::clone(&prepared));
        assert!(playback.project_output_supported(&[1, 6]));
        assert!(playback.project_output_supported(&[]));
        assert!(!playback.project_output_supported(&[6, 1]));
        assert!(!playback.project_output_supported(&[1, 999]));

        let mut observed = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        let mut reference = SessionPlayback::new(prepared).unwrap();
        observed.apply(SessionPlaybackCommand::Play).unwrap();
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        // Live selection does not weaken the offline plan's explicit contract.
        if looping {
            assert!(matches!(
                observed.output_capture_plan(&[], 1536),
                Err(OutputCaptureError::UnsupportedRuntime)
            ));
        }
        let mut tracks = [[[0.0; 2]; 1536]; 2];
        let mut master = [[0.0; 2]; 1536];
        let mut seen = [false; 1536];
        let mut observe = |signal, offset: usize, frames: &[Frame]| {
            let end = offset + frames.len();
            match signal {
                ProjectSignal::Track(id) => {
                    let layer = match id {
                        1 => 0,
                        6 => 1,
                        _ => panic!("unselected owner"),
                    };
                    for (to, from) in tracks[layer][offset..end].iter_mut().zip(frames) {
                        to[0] += from[0];
                        to[1] += from[1];
                    }
                }
                ProjectSignal::Master => {
                    assert!(seen[offset..end].iter().all(|seen| !seen));
                    master[offset..end].copy_from_slice(frames);
                    seen[offset..end].fill(true);
                }
                ProjectSignal::Unavailable => panic!("valid live selection unavailable"),
            }
        };
        let mut tap = ProjectOutputTap::new(&[1, 6], &mut observe).unwrap();
        let mut output = [[0.0; 2]; 1536];
        observed
            .render_with_signal_observers(&mut output, None, Some(&mut tap))
            .unwrap();
        let mut expected = output;
        reference.render(&mut expected).unwrap();
        assert_eq!(output, expected, "observation must not alter actual audio");
        assert_eq!(master, output);
        assert!(seen.iter().all(|seen| *seen));
        for index in 0..1536 {
            let frame = if looping { index % 1024 } else { index };
            let sounding = (3..1027).contains(&frame);
            let fade = if looping && frame >= 896 {
                (1023 - frame) as f32 / 127.0
            } else {
                1.0
            };
            let level = |at: usize| f32::from(u8::from(sounding)) * [0.1, 0.3][at] * fade;
            let a = [level(0), -level(0)];
            let b = [level(1), -level(1) * 0.25];
            for channel in 0..2 {
                assert!((tracks[0][index][channel] - a[channel]).abs() < 1e-6);
                assert!((tracks[1][index][channel] - b[channel]).abs() < 1e-6);
            }
            // Shared Return is independent of the direct track stems.
            if sounding && fade > 0.0 {
                assert_ne!(master[index], [a[0] + b[0], a[1] + b[1]]);
            }
        }
    }
}

#[test]
fn live_project_selection_refuses_unsorted_duplicate_and_unbounded_id_lists() {
    let mut observer = |_, _, _: &[Frame]| {};
    for ids in [&[0][..], &[2, 1], &[1, 1]] {
        assert!(ProjectOutputTap::new(ids, &mut observer).is_none());
    }
    assert!(ProjectOutputTap::new(&[1; hex_project::MAX_TRACKS + 1], &mut observer).is_none());
    assert!(ProjectOutputTap::new(&[], &mut observer).is_some());
}

#[test]
fn solo_overrides_mute_for_launched_session_audio_without_changing_authored_flags() {
    for solo in [false, true] {
        let mut snapshot = audio_project(0, 32, 4).snapshot();
        snapshot.tracks[0].muted = true;
        snapshot.tracks[0].soloed = solo;
        let project = Project::from_snapshot(snapshot).unwrap();
        let prepared = prepare_audio(&project);
        let clip = prepared.session().audio_clips()[0].id;
        let mut runtime = SessionPlayback::new(prepared).unwrap();
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut pcm = [[0.; 2]; 8];
        for chunk in pcm.chunks_mut(3) {
            runtime.render(chunk).unwrap();
        }
        assert_eq!(pcm, [if solo { [0.4, -0.2] } else { [0., 0.] }; 8]);
        assert!(project.tracks()[0].muted());
        assert_eq!(project.tracks()[0].soloed(), solo);
    }
}

#[test]
fn removed_midi_contributor_cannot_capture_a_separate_shared_source_stem() {
    use hex_project::{Command, Created, Editor};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(receiver) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("contributor")
    };
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: contributor,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            velocity: 100,
            channel: 0,
        })
        .unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut old = SessionPlayback::new(revision(editor.project(), Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 128]).unwrap();
    editor.execute(Command::RemoveTrack(contributor)).unwrap();
    let mut candidate = SessionPlayback::new(revision(editor.project(), table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(
        matches!(
            candidate.output_capture_plan(&[OutputCaptureId::Track(contributor)], 8),
            Err(OutputCaptureError::MissingOwner)
        ),
        "MIDI identity does not establish an independently separable audio stem"
    );
    let plan = candidate
        .output_capture_plan(&[OutputCaptureId::Track(receiver)], 8)
        .unwrap();
    let mut main = [[0.0; 2]; 8];
    let mut track = main;
    let mut buffers: [&mut [Frame]; 1] = [&mut track];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    candidate
        .render_with_capture(&mut main, &mut capture)
        .unwrap();
    assert_eq!(main, capture.output(0).unwrap());
    assert!(main.iter().flatten().any(|sample| sample.abs() > 0.000_001));
}

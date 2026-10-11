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
fn nonlinear_track_and_shared_return_capture_follow_actual_processing_order() {
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
        id: 22,
        bypassed: false,
        state: digital(24.0),
    });
    snapshot.return_tracks[0].devices.insert(
        0,
        DeviceSnapshot {
            id: 23,
            bypassed: false,
            state: digital(9.0),
        },
    );
    let prepared = prepare_audio(&Project::from_snapshot(snapshot).unwrap());
    let mut runtime = linear(Arc::clone(&prepared));
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
    for index in 0..512 {
        let a = if index < 3 { [0.0; 2] } else { [0.25, -0.5] };
        let b = if index < 3 {
            [0.0; 2]
        } else {
            [0.4 * 0.75, -0.2 * 0.375]
        };
        let input = [a[0] * 0.5 + b[0], a[1] * 0.5 + b[1]];
        let drive = 10.0_f64.powf(9.0 / 20.0);
        let clipped = input.map(|sample| (f64::from(sample) * drive).clamp(-1.0, 1.0) as f32);
        let mono = (clipped[0] + clipped[1]) * 0.5 * 0.25;
        assert_eq!(capture.output(0).unwrap()[index], a);
        assert!((capture.output(1).unwrap()[index][0] - mono).abs() < 1e-7);
        assert_eq!(
            capture.output(1).unwrap()[index][0],
            capture.output(1).unwrap()[index][1]
        );
    }
}

#[test]
fn selected_outputs_keep_shared_unselected_sends_and_independent_post_mix_pcm() {
    for (muted, solo) in [(false, false), (true, false), (false, true)] {
        let revision = audio_fixture(true, muted, solo);
        let mut runtime = linear(Arc::clone(&revision));
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
    let (revision, track) = mutable_checkpoint_fixture(1 << 26);
    let mut runtime = linear(Arc::clone(&revision));
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

#[test]
fn analytic_hybrid_empty_chain_captures_audio_and_release_under_one_owner() {
    let device_type = hex_lattice_contract::DeviceTypeId::from_static("test.capture.analytic");
    let definition = ContinuityInstrumentDefinition {
        id: device_type,
        levels: [0.25, 0.5],
        release_frames: 4,
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let base = with_test_instrument(&project(1.0, 0, 1 << 26), device_type, 0);
    let mut snapshot = base.snapshot();
    snapshot.session = SessionSnapshot::default();
    snapshot.next_id = 100;
    snapshot.master_gain = 0.25;
    snapshot.tracks[0].gain = 0.5;
    snapshot.tracks[0].balance = -0.5;
    snapshot.assets = vec![AssetSnapshot {
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
    let project = Project::from_snapshot(snapshot).unwrap();
    let media = BTreeMap::from([(
        project.assets().next().unwrap().id(),
        crate::Audio::new(48_000, vec![[0.4, -0.2]; 32]).unwrap(),
    )]);
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
    let mut retarget = SessionPlayback::new(retarget_revision).unwrap();
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
    // Actual public inheritance of a removed analytic owner, not a synthetic
    // capture-only release. The old held gate is released at the cut.
    let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 128]).unwrap();
    let id = project.tracks()[0].id();
    let mut removed = project.snapshot();
    removed.tracks.clear();
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

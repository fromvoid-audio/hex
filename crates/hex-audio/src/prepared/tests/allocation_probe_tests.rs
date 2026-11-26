use super::*;

#[test]
fn prepared_channel_accepts_only_supported_sample_rates() {
    for rate in [0, 7_999, 192_001, u32::MAX] {
        assert!(channel(rate).is_err(), "accepted {rate} hz");
    }
    for rate in [8_000, 192_000] {
        assert!(channel(rate).is_ok(), "rejected {rate} hz");
    }
}

#[test]
fn metronome_admission_is_reported_and_priority_stop_discards_older_toggle() {
    let (mut control, mut callback) = channel(8_000).unwrap();
    control.publish(metronome_graph()).unwrap();
    callback.begin_block();
    let ticket = control
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    assert!(!control.status.metronome_enabled.load(Relaxed));
    callback.begin_block();
    assert!(control.status.metronome_enabled.load(Relaxed));
    assert!(control.status.consumed_command.load(Acquire) >= ticket);
    control
        .action_with_ticket(PlaybackAction::SetMetronome(false))
        .unwrap();
    control.stop();
    callback.begin_block();
    assert!(control.status.metronome_enabled.load(Relaxed));
    assert!(!control.status.playing.load(Relaxed));
    control
        .action_with_ticket(PlaybackAction::SetMetronome(false))
        .unwrap();
    callback.begin_block();
    assert!(!control.status.metronome_enabled.load(Relaxed));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn metronome_fresh_seek_preserves_only_monitoring_after_accepted_exact_source_fence() {
    for (same_source, same_context, conditional, enabled) in [
        (true, true, true, true),
        (false, true, true, false),
        (true, false, true, false),
        (true, true, false, false),
    ] {
        let (old, _) = fresh_spectrum_eq8_graph();
        let old_source = Arc::clone(old.prepared_revision().unwrap());
        assert!(old_source.arrangement_seek_requires_preroll());
        let source = if same_source {
            Arc::clone(&old_source)
        } else {
            Arc::clone(fresh_spectrum_eq8_graph().0.prepared_revision().unwrap())
        };
        let mut oracle = metronome_prerolled_seek(Arc::clone(&source), 200);
        let mut expected = [[0.0; 2]; 128];
        oracle.render(&mut expected).unwrap();
        let candidate = metronome_prerolled_seek(source, 200);
        assert_eq!(
            Arc::ptr_eq(candidate.prepared_revision().unwrap(), &old_source),
            same_source
        );
        assert!(!candidate.metronome_enabled());
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish_with_policy_and_context_generation(
                old,
                PublicationPolicy::InstallFreshRuntime,
                12,
            )
            .unwrap();
        control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 37]).unwrap();
        assert!(control.status.metronome_enabled.load(Relaxed));
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let policy = if conditional {
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence)
        } else {
            PublicationPolicy::InstallFreshRuntime
        };
        let revision = control
            .publish_with_policy_and_context_generation(
                candidate,
                policy,
                if same_context { 12 } else { 13 },
            )
            .unwrap();
        let mut output = [[0.0; 2]; 128];
        let render = || {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let allocations = allocation_counter::measure(render);
            assert_eq!(
                (
                    allocations.count_total,
                    allocations.bytes_total,
                    allocations.count_current,
                    allocations.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut render = render;
            render();
        }
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert_eq!(control.status.metronome_enabled.load(Relaxed), enabled);
        assert_eq!(control.status.position.load(Relaxed), 328);
        assert_eq!(control.status.performance_frame.load(Relaxed), 328);
        assert_eq!(
            output, expected,
            "fresh DSP/transport history and a reset click tail must match the preroll oracle"
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn metronome_rejected_fresh_seek_keeps_active_toggle_and_position() {
    for cancelled in [false, true] {
        let (old, _) = fresh_spectrum_eq8_graph();
        let source = Arc::clone(old.prepared_revision().unwrap());
        let candidate = metronome_prerolled_seek(source, 200);
        let (mut control, mut callback) = channel(48_000).unwrap();
        let active_revision = control
            .publish_with_policy_and_context_generation(
                old,
                PublicationPolicy::InstallFreshRuntime,
                12,
            )
            .unwrap();
        control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 37]).unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let rejected = control
            .publish_with_policy_and_context_generation(
                candidate,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                12,
            )
            .unwrap();
        if cancelled {
            assert_eq!(
                control.cancel_fresh_runtime_publication(rejected),
                FreshRuntimeCancellationResult::Cancelled
            );
        } else {
            control.command(PlaybackCommand::Pause).unwrap();
        }
        callback.begin_block();
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            active_revision
        );
        assert_eq!(
            control.status.rejected_revision(),
            Some((
                rejected,
                if cancelled {
                    PlaybackHandoffRejection::FreshRuntimeCancelled
                } else {
                    PlaybackHandoffRejection::RuntimeFenceChanged
                }
            ))
        );
        assert!(control.status.metronome_enabled.load(Relaxed));
        assert_eq!(control.status.position.load(Relaxed), 37);
        // A queued Pause is Applied when consumed but keeps playing
        // through its fade until a render reaches its audible end (#4151).
        assert!(control.status.playing.load(Relaxed));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn clip_gain_with_shaped_fades_and_replacement_allocates_and_frees_nothing_in_callback() {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Imported { asset, clip, .. } = editor
        .execute(hex_project::Command::ImportAudioTrack {
            name: "clip gain".into(),
            path: "gain.wav".into(),
            sample_rate: 48_000,
            frames: 1_024,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let fade = hex_project::ClipFade::new(1_024, 0, 64, 64)
        .unwrap()
        .with_shape(4, 4, 700, -700)
        .unwrap()
        .with_curve_positions(250, 750)
        .unwrap()
        .with_curve_laws([
            hex_project::ClipFade::DEFAULT_S_CURVE,
            hex_project::FadeCurveLaw::Linear,
        ]);
    editor
        .execute(hex_project::Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    let audio = hex_engine::Audio::new(48_000, vec![[0.5, -0.25]; 1_024]).unwrap();
    let media = std::collections::BTreeMap::from([(asset, audio)]);
    let prepare = |editor: &hex_project::Editor| {
        let revision = hex_engine::PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap();
        Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap()
    };
    editor
        .execute(hex_project::Command::SetAudioClipGain {
            clip,
            millidb: -6_000,
        })
        .unwrap();
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(prepare(&editor)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut prefix).unwrap();
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let gain = hex_project::ClipGain::new(-6_000).unwrap().linear();
    assert_eq!(prefix[0], [0.0; 2]);
    assert_eq!(
        prefix[32],
        [
            0.5 * fade.gain_at(32) * gain,
            -0.25 * fade.gain_at(32) * gain
        ]
    );
    assert_eq!(prefix[68], [0.5 * gain, -0.25 * gain]);
    editor
        .execute(hex_project::Command::SetAudioClipGain {
            clip,
            millidb: 6_000,
        })
        .unwrap();
    control.publish(prepare(&editor)).unwrap();
    let mut output = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let gain = hex_project::ClipGain::new(6_000).unwrap().linear();
    assert_eq!(output, [[0.5 * gain, -0.25 * gain]; 128]);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn metronome_commands_loops_and_handoff_allocate_and_free_nothing_in_callback() {
    let (mut control, mut callback) = channel(8_000).unwrap();
    control.publish(metronome_graph()).unwrap();
    callback.begin_block();
    control
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        for _ in 0..65 {
            callback.render(&mut output).unwrap();
        }
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(control.status.metronome_enabled.load(Relaxed));
    control.publish(metronome_graph()).unwrap();
    control.command(PlaybackCommand::Seek(0)).unwrap();
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    // The Seek applies after its 21-frame fade at 8 kHz, where the
    // undamped metronome click at frame zero follows (#4140).
    assert_eq!(output[21], [0.16; 2]);
    control.command(PlaybackCommand::Pause).unwrap();
    let allocations = allocation_counter::measure(|| {
        // The first block is the queued Pause's 128-frame fade (#4151).
        for _ in 0..2 {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        }
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(output, [[0.0; 2]; 128]);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn held_mutable_arrangement_loop_handoff_preserves_pcm_without_callback_allocation() {
    for (old_enabled, new_enabled, block_frames) in
        [(true, true, 256), (false, true, 17), (true, false, 1)]
    {
        check_held_arrangement_loop_handoff(old_enabled, new_enabled, block_frames);
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn saturator_track_and_return_revision_admission_and_render_allocate_nothing() {
    let (mut control, mut callback) = channel(48_000).expect("channel");
    let playback = saturator_track_and_return_graph(36.0);
    control.publish(playback).expect("publish");
    control.command(PlaybackCommand::Play).expect("play");
    callback.begin_block();
    let mut original_output = [[0.0; 2]; 128];
    callback
        .render(&mut original_output)
        .expect("initial callback render");
    // Each oversampled Saturator answers a step with exact DC once its
    // declared latency and finite tail have elapsed. Main waits for the
    // return path, two Saturators deep; pre-epoch output is silent.
    let saturator = hex_devices::saturator::SaturatorDefinition::prepare_owned(
        &hex_lattice_contract::NativeDeviceMetadata::default_state(
            &hex_devices::saturator::SATURATOR_DEFINITION,
        )
        .expect("default saturator"),
        48_000,
        &[],
    )
    .expect("standalone saturator");
    let latency = hex_lattice::PreparedNativeProcessor::algorithmic_latency_frames(&saturator)
        .expect("declared latency") as usize;
    let hex_lattice::OutputTail::Finite { max_tail_frames } =
        hex_lattice::PreparedNativeProcessor::output_tail(&saturator)
    else {
        panic!("finite saturator tail");
    };
    let settled = 2 * (latency + max_tail_frames as usize);
    assert_eq!((latency, settled), (11, 72));
    assert!(
        original_output[..2 * latency]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert!(
        original_output[settled..]
            .iter()
            .all(|frame| { (frame[0] - 1.75).abs() < 1.0e-6 && (frame[1] + 1.75).abs() < 1.0e-6 })
    );

    control
        .publish(saturator_track_and_return_graph(0.0))
        .expect("publish edited prepared revision");
    let mut output = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).expect("callback render");
        callback.begin_block();
        callback
            .render(&mut output)
            .expect("callback render during ramp");
        callback.begin_block();
        callback
            .render(&mut output)
            .expect("callback render after ramp");
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert_ne!(output, original_output);
    // At 48 kHz the 5 ms ramp from the edit at frame 128 ends at input
    // frame 368; Main settles on the exact target `settled` frames later,
    // inside the third 128-frame block (frames 384..512).
    let target = 128 + 240 + settled - 384;
    assert_eq!(target, 56);
    assert!(
        output[target..].iter().all(|frame| {
            (frame[0] - 0.34375).abs() < 1.0e-6 && (frame[1] + 0.55).abs() < 1.0e-6
        })
    );
    assert!(output[..target].iter().any(|frame| {
        (frame[0] - 0.34375).abs() >= 1.0e-6 || (frame[1] + 0.55).abs() >= 1.0e-6
    }));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn overdrive_track_return_handoff_and_render_allocate_nothing() {
    use hex_devices::overdrive::{OVERDRIVE_DEFINITION as d, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    let (mut control, mut callback) = channel(48_000).unwrap();
    let original = d.default_state().unwrap();
    control
        .publish(native_effect_track_and_return_graph(original.clone()))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 128];
    callback.begin_block();
    callback.render(&mut output).unwrap();
    let changed = d
        .patch_parameter(&original, parameter::DRIVE, ParameterValue::Float(1.0))
        .unwrap();
    control
        .publish(native_effect_track_and_return_graph(changed))
        .unwrap();
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn rattle_track_return_handoff_and_render_allocate_nothing() {
    use hex_devices::rattle::{RATTLE_DEFINITION as d, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    let (mut control, mut callback) = channel(48_000).unwrap();
    let original = d.default_state().unwrap();
    control
        .publish(native_effect_track_and_return_graph(original.clone()))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 128];
    callback.begin_block();
    callback.render(&mut output).unwrap();
    // The edited revision inherits oversampler/noise history and glides drive.
    let changed = d
        .patch_parameter(&original, parameter::DRIVE, ParameterValue::Float(1.0))
        .unwrap();
    control
        .publish(native_effect_track_and_return_graph(changed))
        .unwrap();
    let allocations = allocation_counter::measure(|| {
        for _ in 0..4 {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        }
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn compressor_track_and_return_revision_admission_and_render_allocate_nothing() {
    let (mut control, mut callback) = channel(48_000).expect("channel");
    control
        .publish(compressor_track_and_return_graph(-18.0))
        .expect("publish compressor graph");
    control.command(PlaybackCommand::Play).expect("play");
    callback.begin_block();

    let input = [[0.25, -0.4]; 128];
    let mut reference_track = CompressorReferenceHistory::default();
    let mut reference_return = CompressorReferenceHistory::default();
    let expected_before = compressor_routed_reference(
        -18.0,
        -18.0,
        &mut reference_track,
        &mut reference_return,
        &input,
    );
    let mut original_output = [[0.0; 2]; 128];
    callback
        .render(&mut original_output)
        .expect("initial callback render");
    assert_eq!(original_output.as_slice(), expected_before.as_slice());

    let mut changed_track = CompressorReferenceHistory {
        envelope: reference_track.envelope,
        gain_db: reference_track.gain_db,
    };
    let mut changed_return = CompressorReferenceHistory {
        envelope: reference_return.envelope,
        gain_db: reference_return.gain_db,
    };
    let expected_after = compressor_routed_reference(
        -42.0,
        -42.0,
        &mut changed_track,
        &mut changed_return,
        &input,
    );
    control
        .publish(compressor_track_and_return_graph(-42.0))
        .expect("publish edited compressor revision");
    let mut output = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback
            .render(&mut output)
            .expect("edited callback render");
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert_eq!(output.as_slice(), expected_after.as_slice());
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn reverb_track_return_render_edit_removal_stop_seek_and_shutdown_allocate_nothing() {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    let state =
        hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.reverb")).unwrap();
    let changed = hex_engine::patch_native_device_parameter(
        &state,
        ParameterId::from_static(1),
        ParameterValue::Float(2.5),
    )
    .unwrap();
    let frozen = hex_engine::patch_native_device_parameter(
        &changed,
        ParameterId::from_static(26),
        ParameterValue::Bool(true),
    )
    .unwrap();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut block = [[0.0; 2]; 127];
    for state in [state, changed, frozen] {
        control
            .publish(native_track_and_return_graph(Some(state)))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let counts = allocation_counter::measure(|| {
            for _ in 0..40 {
                callback.begin_block();
                callback.render(&mut block).unwrap();
            }
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
        assert!(block.iter().flatten().any(|v| v.abs() > 0.0));
        control.reclaim();
    }
    control
        .publish(native_track_and_return_graph(None))
        .unwrap();
    let counts = allocation_counter::measure(|| {
        for _ in 0..8 {
            callback.begin_block();
            callback.render(&mut block).unwrap();
        }
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
    assert!(
        block.iter().flatten().any(|v| v.abs() > 0.0),
        "removed effects drain their real boxed histories"
    );
    control.reclaim();
    for command in [PlaybackCommand::Seek(0), PlaybackCommand::Stop] {
        control.command(command).unwrap();
        let counts = allocation_counter::measure(|| {
            // A queued Stop fades over 128 frames, past one block (#4151).
            for _ in 0..1 + usize::from(command == PlaybackCommand::Stop) {
                callback.begin_block();
                callback.render(&mut block).unwrap();
            }
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
    }
    assert_eq!(block, [[0.0; 2]; 127]);
    control.request_shutdown();
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
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
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn gate_track_and_return_initial_and_edited_callback_render_allocate_nothing() {
    let (mut control, mut callback) = channel(48_000).expect("channel");
    control
        .publish(gate_track_and_return_graph(-60.0))
        .expect("publish gate graph");
    control.command(PlaybackCommand::Play).expect("play");
    let mut initial = [[0.0; 2]; 128];
    let initial_allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback
            .render(&mut initial)
            .expect("initial callback render");
    });
    assert_eq!(
        (
            initial_allocations.count_total,
            initial_allocations.bytes_total,
            initial_allocations.count_current,
            initial_allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert!(initial.iter().all(|frame| {
        (frame[0] - 0.00025009375).abs() < 1.0e-8 && (frame[1] + 0.00040015).abs() < 1.0e-8
    }));

    control
        .publish(gate_track_and_return_graph(-20.0))
        .expect("publish floor-edited Gate graph");
    let mut edited = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback
            .render(&mut edited)
            .expect("edited callback render");
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
    let start = 10.0_f64.powf(-60.0 / 20.0);
    let target = 10.0_f64.powf(-20.0 / 20.0);
    let step = (target - start) / 4_800.0;
    for (index, frame) in edited.iter().enumerate() {
        let gain = start + step * (index as f64 + 1.0);
        let direct_left = 0.25 * gain;
        let direct_right = -0.4 * gain;
        let expected = [
            direct_left + direct_left * 0.5 * gain * 0.75,
            direct_right + direct_right * 0.5 * gain * 0.75,
        ];
        assert!((f64::from(frame[0]) - expected[0]).abs() < 2.0e-8);
        assert!((f64::from(frame[1]) - expected[1]).abs() < 2.0e-8);
    }
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

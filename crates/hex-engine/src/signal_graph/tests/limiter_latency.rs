use super::*;
use hex_lattice_contract::{
    DeviceAutomationTarget, DeviceTypeId, HostGain, ParameterId, ParameterValue,
};

fn prepare_native(f: &Fixture) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

#[test]
fn first_track_limiter_after_play_or_pause_becomes_audible() {
    for paused in [false, true] {
        let mut f = fixture(&[&[], &[]], 8192);
        for (index, audio) in f.media.values_mut().enumerate() {
            let frame = if index == 0 {
                [0.5, -0.25]
            } else {
                [0.0625; 2]
            };
            *audio = Audio::new(48_000, vec![frame; 8192]).unwrap();
        }
        let mut donor = SessionPlayback::new(prepare_native(&f)).unwrap();
        donor.apply(SessionPlaybackCommand::Play).unwrap();
        donor.render(&mut [[0.0; 2]; 257]).unwrap();
        if paused {
            donor.apply(SessionPlaybackCommand::Pause).unwrap();
        }
        let mut state = hex_devices::limiter::DEFINITION.default_state().unwrap();
        state = hex_devices::limiter::DEFINITION
            .patch_parameter(
                &state,
                hex_devices::limiter::parameter::CEILING,
                ParameterValue::Float(-12.0),
            )
            .unwrap();
        f.editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state,
            })
            .unwrap();
        let prepared = prepare_native(&f);
        let delay = prepared.arrangement().main_latency_frames() as usize;
        let mut candidate = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        candidate.try_inherit_state_from(&mut donor).unwrap();
        assert_eq!(candidate.playing(), !paused);
        assert_eq!(candidate.performance_frame(), donor.performance_frame());
        assert_eq!(candidate.arrangement_frame(), donor.arrangement_frame());
        candidate.apply(SessionPlaybackCommand::Play).unwrap();
        let mut oracle = SessionPlayback::new(prepared).unwrap();
        oracle.apply(SessionPlaybackCommand::Play).unwrap();
        let mut actual = vec![[0.0; 2]; delay + 513];
        let mut expected = actual.clone();
        for block in actual.chunks_mut(63) {
            candidate.render(block).unwrap();
        }
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected, "paused={paused}");
        assert!(actual[..delay].iter().all(|f| *f == [0.0; 2]));
        assert!(actual[delay..].iter().all(|f| f[0] > 0.3 && f[0] < 0.315));
    }
}

#[test]
fn first_track_limiter_delays_existing_upstream_and_other_track_reverb_without_loss() {
    for reverb_track in [0, 1] {
        let mut f = fixture(&[&[], &[]], 8192);
        f.editor
            .execute(Command::InsertDevice {
                track: f.tracks[reverb_track],
                before: None,
                state: crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Effect,
                    DeviceTypeId::from_static("hex.reverb"),
                )
                .unwrap(),
            })
            .unwrap();
        let prepared = prepare_native(&f);
        let mut donor = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        let mut oracle = SessionPlayback::new(prepared).unwrap();
        for runtime in [&mut donor, &mut oracle] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 257]).unwrap();
        }
        f.editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
            })
            .unwrap();
        let prepared = prepare_native(&f);
        let delay = prepared.arrangement().main_latency_frames() as usize;
        let mut candidate = SessionPlayback::new(prepared).unwrap();
        candidate.try_inherit_state_from(&mut donor).unwrap();
        let mut actual = vec![[0.0; 2]; delay + 2048];
        let mut expected = actual.clone();
        oracle.render(&mut expected[delay..]).unwrap();
        assert!(expected.iter().flatten().any(|v| *v != 0.0));
        for block in actual.chunks_mut(63) {
            candidate.render(block).unwrap();
        }
        assert_eq!(actual, expected, "reverb track {reverb_track}");
    }
}

#[test]
fn first_track_limiter_while_playing_restarts_audio_at_the_cursor() {
    for active_reverb in [false, true] {
        let mut f = fixture(&[&[], &[]], 8192);
        if active_reverb {
            f.editor
                .execute(Command::InsertDevice {
                    track: f.tracks[0],
                    before: None,
                    state: crate::default_native_device_state_for_class(
                        crate::NativeDeviceClass::Effect,
                        DeviceTypeId::from_static("hex.reverb"),
                    )
                    .unwrap(),
                })
                .unwrap();
        }
        let prepared = prepare_native(&f);
        let mut donor = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        let mut oracle = SessionPlayback::new(prepared).unwrap();
        for runtime in [&mut donor, &mut oracle] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 1]).unwrap();
        }
        f.editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: active_reverb
                    .then(|| f.editor.project().track(f.tracks[0]).unwrap().devices()[0].id()),
                state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
            })
            .unwrap();
        if !active_reverb {
            f.editor
                .execute(Command::SetAudioFeed {
                    source: hex_project::RoutingOwner::Track(f.tracks[1]),
                    destination: f.tracks[0],
                    gain: 0.5,
                })
                .unwrap();
        }
        // A changed latency layout while playing restarts audio at the
        // running cursor (Pulse #3769): tails restart, routing is the new
        // revision's, and the transport keeps running.
        let current = prepare_native(&f);
        let mut candidate = SessionPlayback::new(Arc::clone(&current)).unwrap();
        candidate.try_inherit_state_from(&mut donor).unwrap();
        assert!(candidate.playing());
        assert_eq!(candidate.performance_frame(), oracle.performance_frame());
        let mut restarted = SessionPlayback::new(current).unwrap();
        restarted.play_from(oracle.performance_frame()).unwrap();
        let mut actual = [[0.0; 2]; 513];
        let mut expected = actual;
        candidate.render(&mut actual).unwrap();
        restarted.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn live_flat_eq_insertion_under_main_limiter_preserves_pending_pcm_and_clock() {
    for (warm, remove_main) in [(1, false), (63, false), (257, false), (1, true)] {
        let mut f = fixture(&[&[]], 1024);
        let Created::Device(main) = f
            .editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Effect,
                    DeviceTypeId::from_static("hex.limiter"),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("main limiter");
        };
        let original = prepare_native(&f);
        assert!(original.arrangement().main_latency_frames() > 0);
        let mut donor = SessionPlayback::new(Arc::clone(&original)).unwrap();
        let mut oracle = SessionPlayback::new(original).unwrap();
        for runtime in [&mut donor, &mut oracle] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut vec![[0.0; 2]; warm]).unwrap();
        }
        let Created::Device(eq) = f
            .editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state: hex_devices::eq::EqState::default().encode().unwrap(),
            })
            .unwrap()
        else {
            panic!("eq");
        };
        if remove_main {
            // Exercise the retained larger stamp owner too: removing Main
            // cannot discard its pending impulse when EQ changes the indices.
            f.editor.execute(Command::RemoveDevice(main)).unwrap();
        }
        // Insert/remove repeatedly while the Main processor still owns queued
        // output. A flat zero-latency EQ must not change any delivered sample.
        for remove in [false, true] {
            if remove {
                f.editor.execute(Command::RemoveDevice(eq)).unwrap();
            }
            let mut candidate = SessionPlayback::new(prepare_native(&f)).unwrap();
            candidate.try_inherit_state_from(&mut donor).unwrap();
            assert!(candidate.playing());
            let mut actual = [[0.0; 2]; 129];
            let mut expected = actual;
            candidate.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(actual, expected, "warm={warm}, remove={remove}");
            assert_eq!(candidate.arrangement_frame(), oracle.arrangement_frame());
            assert_eq!(candidate.clock_anchor(), oracle.clock_anchor());
            donor = candidate;
        }
    }
}

#[test]
fn clock_only_history_restarts_on_changed_upstream_delay_or_feed() {
    for feed in [false, true] {
        let mut f = fixture(&[&[], &[]], 1024);
        let limiter = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        f.editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: limiter.clone(),
            })
            .unwrap();
        let original = prepare_native(&f);
        let mut donor = SessionPlayback::new(Arc::clone(&original)).unwrap();
        let mut oracle = SessionPlayback::new(original).unwrap();
        for runtime in [&mut donor, &mut oracle] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 3]).unwrap();
        }
        f.editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state: hex_devices::eq::EqState::default().encode().unwrap(),
            })
            .unwrap();
        let mut current = SessionPlayback::new(prepare_native(&f)).unwrap();
        current.try_inherit_state_from(&mut donor).unwrap();
        let command = if feed {
            Command::SetAudioFeed {
                source: hex_project::RoutingOwner::Track(f.tracks[1]),
                destination: f.tracks[0],
                gain: 0.5,
            }
        } else {
            Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state: limiter,
            }
        };
        f.editor.execute(command).unwrap();
        // The clock-only history cannot carry the changed upstream delay or
        // feed, so the running graph restarts at its cursor instead of
        // refusing (Pulse #3769).
        let revised = prepare_native(&f);
        let mut changed = SessionPlayback::new(Arc::clone(&revised)).unwrap();
        changed.try_inherit_state_from(&mut current).unwrap();
        let frame = current.performance_frame();
        assert!(changed.playing());
        assert_eq!(changed.performance_frame(), frame);
        let mut restarted = SessionPlayback::new(revised).unwrap();
        restarted.play_from(frame).unwrap();
        let mut actual = [[0.0; 2]; 129];
        let mut expected = actual;
        changed.render(&mut actual).unwrap();
        restarted.render(&mut expected).unwrap();
        assert_eq!(actual, expected, "running restart; feed={feed}");
    }
}

#[test]
fn limiter_lookahead_is_reported_and_compensated_in_actual_rendering() {
    let mut fixture = fixture(&[&[]], 32);
    let kind = DeviceTypeId::from_static("hex.limiter");
    let mut state =
        crate::default_native_device_state_for_class(crate::NativeDeviceClass::Effect, kind)
            .expect("registered limiter defaults");
    state = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Effect,
        &state,
        ParameterId::from_static(6),
        ParameterValue::Choice(2),
    )
    .expect("select six millisecond lookahead");
    fixture
        .editor
        .execute(Command::InsertDevice {
            track: fixture.tracks[0],
            before: None,
            state,
        })
        .expect("insert limiter on the audio track");

    // Use the production registry; the surrounding delay fixture's prepare()
    // helper deliberately registers only its synthetic delay provider.
    let revision = Arc::new(
        PreparedRevision::prepare(
            fixture.editor.project(),
            &fixture.media,
            Arc::new(SineTable::new()),
            None,
        )
        .expect("prepare actual registered native limiter graph"),
    );
    let latency = revision.arrangement().main_latency_frames();
    assert_eq!(latency, 288);

    let rendered = session(
        Arc::clone(&revision),
        fixture.samples.len() + latency as usize,
        63,
    );
    assert!(
        rendered[..latency as usize]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert_eq!(&rendered[latency as usize..], fixture.samples.as_slice());
}

#[test]
fn limiter_host_input_automation_follows_entering_content() {
    for location in [
        HostLocation::Track,
        HostLocation::Main,
        HostLocation::Return,
    ] {
        assert_limiter_host_automation_content_origin(
            DeviceAutomationTarget::HostInputGain,
            location,
            None,
        );
    }
}

macro_rules! output_automation_case {
    ($name:ident, $location:expr) => {
        #[test]
        fn $name() {
            assert_limiter_host_automation_content_origin(
                DeviceAutomationTarget::HostOutputGain,
                $location,
                None,
            );
        }
    };
}

output_automation_case!(limiter_output_automation_track, HostLocation::Track);
output_automation_case!(limiter_output_automation_main, HostLocation::Main);
output_automation_case!(limiter_output_automation_return, HostLocation::Return);

#[test]
fn utility_host_automation_before_and_after_limiter_follows_content() {
    for before in [true, false] {
        assert_limiter_host_automation_content_origin(
            DeviceAutomationTarget::HostOutputGain,
            HostLocation::Track,
            Some(before),
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum HostLocation {
    Track,
    Main,
    Return,
}

// Host automation follows the content at its physical stage: input gain and
// provider controls use the entering content origin; output gain uses the emitted
// origin after the processor's own algorithmic latency. A transparent real
// Limiter must therefore produce the same delayed gain step at either boundary.
// This oracle derives that PCM directly from source indices and authored steps,
// independently of renderer clocks, compensation stamps and envelope evaluation.
fn assert_limiter_host_automation_content_origin(
    target: DeviceAutomationTarget,
    location: HostLocation,
    utility_before: Option<bool>,
) {
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime};

    const FRAMES: usize = 4096;
    const LATENCY: usize = 288;
    let mut f = fixture(&[&[]], FRAMES);
    f.samples.fill([0.01, -0.02]);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, f.samples.clone()).unwrap();
    }
    let state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        hex_devices::limiter::TYPE_ID,
    )
    .unwrap();
    let state = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Effect,
        &state,
        hex_devices::limiter::parameter::LOOKAHEAD,
        ParameterValue::Choice(2),
    )
    .unwrap();
    let command = match location {
        HostLocation::Track => Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state,
        },
        HostLocation::Main => Command::InsertMainDevice {
            before: None,
            state,
        },
        HostLocation::Return => {
            let Created::ReturnTrack(bus) = f
                .editor
                .execute(Command::AddReturnTrack {
                    name: "host origin".into(),
                })
                .unwrap()
            else {
                panic!("return fixture");
            };
            f.editor
                .execute(Command::SetReturnSend {
                    track: f.tracks[0],
                    return_track: bus,
                    gain: 1.0,
                })
                .unwrap();
            Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state,
            }
        }
    };
    let Created::Device(mut device) = f.editor.execute(command).unwrap() else {
        panic!("limiter fixture device");
    };
    if let Some(before) = utility_before {
        let Created::Device(utility) = f
            .editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: before.then_some(device),
                state: hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("utility fixture device");
        };
        device = utility;
    }
    // Return send and direct paths are both audible and compensated to 288 frames.
    let dry_copies = if matches!(location, HostLocation::Return) {
        2.0
    } else {
        1.0
    };
    let dry = prepare_native(&f);
    assert_eq!(dry.arrangement().main_latency_frames(), LATENCY as u32);
    let dry_output = session(dry, FRAMES + LATENCY, 63);
    assert_eq!(&dry_output[..LATENCY], &[[0.0; 2]; LATENCY]);
    for (actual, source) in dry_output[LATENCY..].iter().zip(&f.samples) {
        assert_eq!(
            *actual,
            [source[0] * dry_copies, source[1] * dry_copies],
            "the real limiter must be transparent below its ceiling at {location:?}"
        );
    }

    let step_beat = BeatTime::from_raw(1 << 28);
    let step_frame = f
        .editor
        .project()
        .tempo_map()
        .beat_to_frame(step_beat)
        .unwrap();
    assert_eq!(step_frame, 1500);
    let attenuated = HostGain::from_millidb(-6000).unwrap();
    f.editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points: [(BeatTime::ZERO, HostGain::ZERO), (step_beat, attenuated)]
                .into_iter()
                .map(|(beat, gain)| AutomationPoint {
                    beat,
                    value: AutomationValue::new(gain.normalized()).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                })
                .collect(),
        })
        .unwrap();
    let revision = prepare_native(&f);
    assert_eq!(revision.arrangement().main_latency_frames(), LATENCY as u32);
    let mut expected = vec![[0.0; 2]; FRAMES + LATENCY];
    for (content_frame, (output, source)) in
        expected[LATENCY..].iter_mut().zip(&f.samples).enumerate()
    {
        let gain = if content_frame as u64 >= step_frame {
            attenuated.linear()
        } else {
            1.0
        };
        let gain = gain + (dry_copies - 1.0);
        *output = [source[0] * gain, source[1] * gain];
    }
    for partition in [1, 63, 256] {
        let actual = session(Arc::clone(&revision), expected.len(), partition);
        assert_host_automation_pcm(&actual, &expected, target, location, partition, "session");
        let mut renderer = Renderer::new(revision.arrangement());
        renderer.play();
        let mut offline = vec![[0.0; 2]; expected.len()];
        for block in offline.chunks_mut(partition) {
            renderer.render(block).unwrap();
        }
        assert_host_automation_pcm(&offline, &expected, target, location, partition, "renderer");
    }
}

fn assert_host_automation_pcm(
    actual: &[Frame],
    expected: &[Frame],
    target: DeviceAutomationTarget,
    location: HostLocation,
    partition: usize,
    renderer: &str,
) {
    assert_eq!(actual.len(), expected.len());
    for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..2 {
            assert!(
                (actual[channel] - expected[channel]).abs() < 1e-7,
                "{renderer} {location:?} {target:?} content-origin mismatch at frame {frame}, channel {channel}, partition {partition}: {actual:?} != {expected:?}"
            );
        }
    }
}

/// Measures the real Session host, including source dispatch, PDC, meters and
/// transaction copies; this is not physical device deadline qualification.
#[test]
#[cfg(not(debug_assertions))]
#[ignore = "run explicitly with --release --ignored --nocapture"]
fn optimized_limiter_host_throughput() {
    for (rate, mode, location, tracks) in [
        (48000, 0, 0, 16),
        (48000, 1, 0, 16),
        (48000, 2, 0, 16),
        (48000, 0, 1, 16),
        (48000, 0, 2, 16),
        (192000, 2, 0, 16),
        (48000, 0, 1, 100),
        (48000, 0, 2, 100),
    ] {
        let mut f = fixture_at_rate(&[&[] as &[u32]; 100][..tracks], rate as usize * 2, rate);
        for audio in f.media.values_mut() {
            *audio = Audio::new(
                rate,
                (0..rate * 2)
                    .map(|i| {
                        let phase = f64::from(i) * 0.071;
                        [
                            (phase.sin() * 0.1) as f32,
                            ((phase * 1.37).cos() * 0.1) as f32,
                        ]
                    })
                    .collect(),
            )
            .unwrap();
        }
        let state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        let state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Effect,
            &state,
            ParameterId::from_static(8),
            ParameterValue::Choice(mode),
        )
        .unwrap();
        if location == 0 {
            f.editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state,
                })
                .unwrap();
        } else if location == 1 {
            f.editor
                .execute(Command::InsertDevice {
                    track: f.tracks[0],
                    before: None,
                    state,
                })
                .unwrap();
        }
        let revision = Arc::new(
            PreparedRevision::prepare(
                f.editor.project(),
                &f.media,
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut runtime = SessionPlayback::new(revision).unwrap();
        runtime.set_device_metering(true);
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut work = [[0.0; 2]; 64];
        for _ in 0..100 {
            runtime.render(&mut work).unwrap();
        }
        let blocks = rate as usize / work.len();
        let start = std::time::Instant::now();
        for _ in 0..blocks {
            runtime.render(&mut work).unwrap();
            std::hint::black_box(&work);
        }
        let elapsed = start.elapsed().as_secs_f64();
        eprintln!(
            "limiter_host tracks={tracks} rate={rate} mode={mode} location={location} block=64 warmup=100 frames={} elapsed_s={elapsed:.6}",
            blocks * 64
        );
    }
}

#[test]
fn main_limiter_modes_keep_gain_pan_pcm_and_partition_contracts() {
    for mode in 0..3 {
        for routing in 0..2 {
            let mut f = fixture(&[&[], &[]], 1024);
            for (index, track) in f.tracks.iter().copied().enumerate() {
                f.editor
                    .execute(Command::Gain {
                        track,
                        gain: 0.37 + index as f32 * 0.1,
                    })
                    .unwrap();
                f.editor
                    .execute(Command::Balance {
                        track,
                        balance: -0.3,
                    })
                    .unwrap();
            }
            let dry = Arc::new(
                PreparedRevision::prepare(
                    f.editor.project(),
                    &f.media,
                    Arc::new(SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let mut state = crate::default_native_device_state_for_class(
                crate::NativeDeviceClass::Effect,
                DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap();
            for (id, value) in [(8, mode), (7, routing)] {
                state = crate::patch_native_device_parameter_for_class(
                    crate::NativeDeviceClass::Effect,
                    &state,
                    ParameterId::from_static(id),
                    ParameterValue::Choice(value),
                )
                .unwrap();
            }
            f.editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: state.clone(),
                })
                .unwrap();
            let revision = Arc::new(
                PreparedRevision::prepare(
                    f.editor.project(),
                    &f.media,
                    Arc::new(SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let graph = revision.arrangement().signal_graph.as_ref().unwrap();
            assert!(graph.needs_runtime());
            assert!(!graph.needs_upstream_frame_routing());
            let length = 1024 + revision.arrangement().main_latency_frames() as usize;
            let mut expected = session(dry, length, 63);
            let Some(hex_lattice::PreparedProcessorFactory::Owned(factory)) =
                hex_devices::limiter::DEFINITION
                    .prepare_runtime_factory(
                        &state,
                        &hex_lattice::DevicePreparationContext {
                            sample_rate: 48000,
                            tuning: &hex_lattice::TuningTable::default(),
                            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                        },
                    )
                    .unwrap()
            else {
                panic!("owned limiter")
            };
            let mut runtime = factory.create_runtime().unwrap();
            runtime.begin_block();
            runtime.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            runtime.commit_block();
            for block in [1, 63, 256, 513] {
                assert_eq!(
                    session(Arc::clone(&revision), length, block),
                    expected,
                    "mode={mode} routing={routing} block={block}"
                );
            }
            let mut legacy = Renderer::new(revision.arrangement());
            legacy.play();
            let mut actual = vec![[0.0; 2]; length];
            legacy.render(&mut actual).unwrap();
            assert_eq!(actual, expected, "legacy mode={mode} routing={routing}");
        }
    }
}

#[test]
fn main_lookahead_keeps_upstream_node_observations_in_bounded_blocks() {
    let mut f = fixture(&[&[]], 1024);
    let utility = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let Created::Device(device) = f
        .editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: utility,
        })
        .unwrap()
    else {
        panic!("utility")
    };
    let state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut runtime = SessionPlayback::new(revision).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut observed = Vec::new();
    let mut output = [[0.0; 2]; 1024];
    runtime
        .render_with_post_node_observer(
            &mut output,
            Some(&mut |identity, offset, frames| {
                if identity.device_id == device.value() {
                    assert!(frames.len() <= 256);
                    assert_eq!(offset, observed.len());
                    observed.extend_from_slice(frames);
                    assert!(
                        frames.len() > 1,
                        "Main lookahead must not turn upstream work into per-sample calls"
                    );
                }
            }),
        )
        .unwrap();
    assert_eq!(observed, f.samples);
}

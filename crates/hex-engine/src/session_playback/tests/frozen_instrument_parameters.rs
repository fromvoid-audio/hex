use super::*;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId};
use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue};

fn fixture() -> (Editor, hex_project::TrackId, hex_project::DeviceId) {
    fixture_at_rate(8000)
}
fn fixture_at_rate(rate: u32) -> (Editor, hex_project::TrackId, hex_project::DeviceId) {
    let mut editor = Editor::new(Project::new(rate).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "frozen fm4".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_devices::fm4::device::FM4_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(4).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 120,
        })
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::from_static(29),
            points: vec![
                point(BeatTime::ZERO, 0.75),
                point(BeatTime::from_raw(BeatTime::ONE.raw() / 128), 0.9),
            ],
        })
        .unwrap();
    (editor, track, device)
}
fn point(beat: BeatTime, value: f32) -> AutomationPoint {
    AutomationPoint {
        beat,
        value: AutomationValue::new(value).unwrap(),
        outgoing: AutomationInterpolation::Step,
    }
}
fn prepared(editor: &Editor) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}
fn playback(editor: &Editor) -> SessionPlayback {
    SessionPlayback::new(prepared(editor)).unwrap()
}
fn render(runtime: &mut SessionPlayback, count: usize, partition: usize) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; count];
    for block in output.chunks_mut(partition) {
        runtime.render(block).unwrap();
    }
    output
}
fn assert_signal_eq(actual: Vec<Frame>, expected: Vec<Frame>) {
    assert!(
        actual.iter().flatten().any(|sample| sample.abs() > 1e-6),
        "PCM comparison must include audible nonzero samples"
    );
    assert_eq!(actual, expected);
}
#[test]
fn detached_fm4_freezes_latest_target_across_pause_bypass_removal_and_revision() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            for paused in [false, true] {
                for removed in [false, true] {
                    let (mut editor, track, device) = fixture();
                    let mut old = playback(&editor);
                    let mut reference = playback(&editor);
                    for runtime in [&mut old, &mut reference] {
                        runtime.apply(SessionPlaybackCommand::Play).unwrap();
                        render(runtime, 20, 20);
                        // A planning observation must not freeze the eventual value.
                        runtime.mutable_instruments[0]
                            .as_ref()
                            .unwrap()
                            .preflight_tail_parameters()
                            .unwrap();
                        render(runtime, 33, 7);
                    }
                    let mut oracle_editor = Editor::new(editor.project().clone(), 16).unwrap();
                    oracle_editor
                        .execute(Command::ReplaceDeviceAutomation {
                            device,
                            parameter: ParameterId::from_static(29),
                            points: vec![point(BeatTime::ZERO, 0.9)],
                        })
                        .unwrap();
                    let mut oracle = playback(&oracle_editor);
                    oracle.try_inherit_state_from(&mut reference).unwrap();
                    oracle.apply(SessionPlaybackCommand::Pause).unwrap();
                    if paused {
                        old.apply(SessionPlaybackCommand::Pause).unwrap();
                    }
                    if removed {
                        editor.execute(Command::RemoveTrack(track)).unwrap();
                    } else {
                        editor
                            .execute(Command::SetDeviceBypass {
                                device,
                                bypassed: true,
                            })
                            .unwrap();
                    }
                    let mut actual = playback(&editor);
                    actual.try_inherit_state_from(&mut old).unwrap();
                    assert_eq!(actual.tail_mutable_instruments.iter().flatten().count(), 1);
                    // A refused tail render must restore the borrowed frame before
                    // rollback; the retry below must match uninterrupted audio.
                    let tail = actual
                        .tail_mutable_instruments
                        .iter_mut()
                        .flatten()
                        .next()
                        .unwrap();
                    tail.runtime.begin_block();
                    let failed = tail
                        .runtime
                        .with_frozen_parameters(|runtime, values| {
                            runtime.process_frozen_tail_segment(
                                Some(tail.track_id),
                                u64::MAX,
                                &mut [[0.0; 2]; 2],
                                &mut Default::default(),
                                &mut Vec::new(),
                                None,
                                values,
                            )
                        })
                        .unwrap();
                    assert!(failed.is_err());
                    tail.runtime.rollback_block();
                    let expected = render(&mut oracle, 113, 113);
                    let output = render(&mut actual, 113, 3);
                    assert_eq!(output, expected, "paused={paused} removed={removed}");
                    assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
                    // A further canonical revision carries the existing tail frame.
                    let mut successor = playback(&editor);
                    successor.try_inherit_state_from(&mut actual).unwrap();
                    let expected = render(&mut oracle, 9000, 4097);
                    assert_eq!(render(&mut successor, 9000, 31), expected);
                    assert!(expected[8000..].iter().all(|frame| *frame == [0.0; 2]));
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn detached_fm4_keeps_the_last_modulated_sample_for_its_release() {
    exercise_modulated_fm4_release(false);
}

#[test]
fn fm4_output_trim_is_sample_accurate_through_live_handoff_and_detached_release() {
    exercise_modulated_fm4_release(true);
}

fn exercise_modulated_fm4_release(output_trim: bool) {
    use hex_devices::{
        fm4::device::{FM4_DEFINITION, FM4_TYPE_ID, parameter},
        modulation::lfo::{Rate, Settings, Waveform},
    };
    use hex_lattice::{
        DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeInstrumentDefinition,
    };
    use hex_lattice_contract::{HostGain, ParameterValue};
    use hex_project::{
        DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, ModulationAmount,
        ModulationDestination, ModulationOutputId, ModulationRange, ModulationRouteId,
        ModulationSourceId, Modulator,
    };

    let (mut editor, track, device) = fixture();
    let carrier = parameter::carrier(1);
    let target = if output_trim {
        DeviceAutomationTarget::HostOutputGain
    } else {
        DeviceAutomationTarget::Parameter(carrier)
    };
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: carrier,
            points: vec![point(BeatTime::ZERO, 0.75)],
        })
        .unwrap();
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: DeviceModulation::new(
                vec![Modulator {
                    id: source,
                    color: Default::default(),
                    enabled: true,
                    state: Settings {
                        rate: Rate::Hertz(1000.0),
                        phase: 0.0,
                        waveform: Waveform::Square,
                    }
                    .encode()
                    .unwrap(),
                }],
                vec![DeviceModulationRoute {
                    id: ModulationRouteId::new(next + 1).unwrap(),
                    source,
                    output: ModulationOutputId::MAIN,
                    destination: ModulationDestination::Device {
                        device_type: FM4_TYPE_ID,
                        target,
                        vst3_class: None,
                    },
                    amount: ModulationAmount::new(if output_trim { 6.0 / 84.0 } else { 0.125 })
                        .unwrap(),
                    range: ModulationRange::default(),
                    enabled: true,
                }],
            )
            .unwrap(),
        })
        .unwrap();
    let mut old = playback(&editor);
    if output_trim {
        old.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    }
    old.apply(SessionPlaybackCommand::Play).unwrap();
    const DETACH: u64 = 53;
    let attached = if output_trim {
        let mut first = render(&mut old, 13, 5);
        let mut successor = playback(&editor);
        successor.try_inherit_state_live_from(&mut old).unwrap();
        first.extend(render(&mut successor, DETACH as usize - 13, 11));
        old = successor;
        first
    } else {
        render(&mut old, DETACH as usize, 13)
    };
    editor.execute(Command::RemoveTrack(track)).unwrap();
    let mut detached = playback(&editor);
    detached.try_inherit_state_from(&mut old).unwrap();

    // At 8 kHz the square is negative on frame 52, yielding a normalized
    // target of 0.625 (canonical carrier weight 0.25) at the release boundary.
    let state = FM4_DEFINITION
        .patch_parameter(
            &FM4_DEFINITION.default_state().unwrap(),
            carrier,
            ParameterValue::Float(if output_trim { 0.5 } else { 0.25 }),
        )
        .unwrap();
    let note = FM4_DEFINITION
        .prepare_instrument(
            &state,
            &DevicePreparationContext {
                sample_rate: 8000,
                tuning: &Default::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .prepare_note(60, 120)
        .unwrap();
    let gain = |millidb| HostGain::from_millidb(millidb).unwrap().linear();
    if output_trim {
        let expected = (0..DETACH)
            .map(|age| {
                let level = gain(if age % 8 < 4 { 6_000 } else { -6_000 });
                note.sample(age, u64::MAX, &EMPTY_DEVICE_RESOURCES)
                    .map(|sample| sample * level)
            })
            .collect();
        assert_signal_eq(attached, expected);
    }
    let release_gain = if output_trim { gain(-6_000) } else { 1.0 };
    let expected = (DETACH..DETACH + 113)
        .map(|age| {
            note.sample(age, DETACH, &EMPTY_DEVICE_RESOURCES)
                .map(|sample| sample * release_gain)
        })
        .collect();
    assert_signal_eq(render(&mut detached, 113, 11), expected);
}

#[test]
fn second_capture_refusal_preserves_both_donor_voices_and_retry() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let (mut editor, track, _) = fixture();
            editor
                .execute(Command::DuplicateTracks {
                    tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                    without_clips: false,
                })
                .unwrap();
            let mut old = playback(&editor);
            let mut reference = playback(&editor);
            for runtime in [&mut old, &mut reference] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                render(runtime, 73, 7);
            }
            let tracks: Vec<_> = editor
                .project()
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect();
            for track in tracks {
                editor.execute(Command::RemoveTrack(track)).unwrap();
            }
            let mut candidate = playback(&editor);
            crate::mutable_instrument::frozen_parameters::with_capture_failure(1, || {
                assert!(candidate.try_inherit_state_from(&mut old).is_err());
            });
            assert_eq!(old.mutable_instruments.iter().flatten().count(), 2);
            assert!(
                candidate
                    .tail_mutable_instruments
                    .iter()
                    .all(Option::is_none)
            );
            assert_eq!(render(&mut old, 97, 7), render(&mut reference, 97, 97));
            // Existing publication candidates are single-use after refusal.
            let mut candidate = playback(&editor);
            candidate.try_inherit_state_from(&mut old).unwrap();
            assert_eq!(
                candidate.tail_mutable_instruments.iter().flatten().count(),
                2
            );
            assert!(
                render(&mut candidate, 64, 3)
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 1e-6)
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
#[cfg(not(debug_assertions))]
#[ignore = "release CPU measurement; run with --release --ignored --nocapture"]
fn profile_four_carrier_playing_and_detached_release_cost() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            for voices in [1, 16] {
                for block in [128, 512] {
                    for repeat in 0..3 {
                        let cases = if repeat % 2 == 0 { [false, true] } else { [true, false] };
                        for curves in cases {
                            let (mut editor, track, device) = fixture_at_rate(48000);
                            let mut state = crate::patch_native_device_parameter_for_class(
                                crate::NativeDeviceClass::Instrument,
                                editor.project().device(device).unwrap().state(),
                                ParameterId::from_static(36),
                                hex_lattice_contract::ParameterValue::Float(2.0),
                            ).unwrap();
                            // Both cases render the same four active operators;
                            // authored curves start from these exact Base values.
                            for (index,id) in (29..=32).enumerate() {
                                let normalized=0.55+index as f32*0.05;
                                state=crate::patch_native_device_parameter_for_class(
                                    crate::NativeDeviceClass::Instrument,&state,
                                    ParameterId::from_static(id),
                                    hex_lattice_contract::ParameterValue::Float(2.0*normalized-1.0),
                                ).unwrap();
                            }
                            editor.execute(Command::ReplaceDeviceState { device, state }).unwrap();
                            let clip = editor.project().track(track).unwrap().midi_clips()[0].id();
                            for index in 1..voices {
                                editor.execute(Command::AddMidiNote {
                                    clip, start: BeatTime::ZERO,
                                    duration: BeatTime::from_quarters(4).unwrap(),
                                    pitch: 60 + index as u8, channel: 0, velocity: 120,
                                }).unwrap();
                            }
                            for (index, id) in (29..=32).enumerate() {
                                let points = if curves {
                                    vec![
                                        AutomationPoint {
                                            beat: BeatTime::ZERO,
                                            value: AutomationValue::new(0.55 + index as f32 * 0.05).unwrap(),
                                            outgoing: AutomationInterpolation::Linear,
                                        },
                                        AutomationPoint {
                                            beat: BeatTime::from_quarters(16).unwrap(),
                                            value: AutomationValue::new(0.8 + index as f32 * 0.03).unwrap(),
                                            outgoing: AutomationInterpolation::Linear,
                                        },
                                    ]
                                } else { vec![] };
                                editor.execute(Command::ReplaceDeviceAutomation {
                                    device, parameter: ParameterId::from_static(id), points,
                                }).unwrap();
                            }
                            let mut runtime = playback(&editor);
                            runtime.apply(SessionPlaybackCommand::Play).unwrap();
                            let mut output = vec![[0.0; 2]; block];
                            for _ in 0..8 { runtime.render(&mut output).unwrap(); }
                            for detached in [false, true] {
                                if detached {
                                    editor.execute(Command::SetDeviceBypass { device, bypassed: true }).unwrap();
                                    let mut tail = playback(&editor);
                                    tail.try_inherit_state_from(&mut runtime).unwrap();
                                    runtime = tail;
                                }
                                let start = std::time::Instant::now();
                                for _ in 0..64 {
                                    runtime.render(std::hint::black_box(&mut output)).unwrap();
                                    std::hint::black_box(&output);
                                }
                                let elapsed = start.elapsed();
                                if detached {
                                    let tail = runtime.tail_mutable_instruments.iter().flatten().next().unwrap();
                                    assert!(!tail.runtime.committed_is_quiescent());
                                }
                                eprintln!(
                                    "fm4-carriers rate=48000 tracks=1 voices={} block={} repeat={} authored-streams={} phase={} frozen-targets={} release-seconds=2 \
                                     warmup-playing-blocks=8 measured-blocks=64 total-ns={} ns-per-frame={:.3}; \
                                     excludes construction/handoff; no physical deadline claim",
                                    voices, block, repeat, if curves { 4 } else { 0 },
                                    if detached { "detached" } else { "playing" }, if detached {4}else{0},
                                    elapsed.as_nanos(), elapsed.as_nanos() as f64 / (block * 64) as f64,
                                );
                            }
                        }
                    }
                }
            }
        })
        .unwrap().join().unwrap();
}

#[test]
fn four_carrier_curves_match_base_oracle_with_shared_clip_gain_pause_and_seek() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let (mut editor, track, device) = fixture();
            let source = editor.project().track(track).unwrap().source_id().unwrap();
            let Created::Track(contributor) = editor
                .execute(Command::AddMidiTrack {
                    name: "contributor".into(),
                })
                .unwrap()
            else {
                panic!("track")
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
                    length: BeatTime::from_quarters(4).unwrap(),
                })
                .unwrap()
            else {
                panic!("clip")
            };
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::from_quarters(4).unwrap(),
                    pitch: 67,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
            for (track, db) in [(track, -6000), (contributor, -12000)] {
                let clip = editor.project().track(track).unwrap().midi_clips()[0].id();
                editor
                    .execute(Command::SetMidiClipGain {
                        clip,
                        gain: hex_project::ClipGain::new(db).unwrap(),
                    })
                    .unwrap();
            }
            for (id, value) in (29..=32).zip([0.75, 0.625, 0.375, 0.25]) {
                editor
                    .execute(Command::ReplaceDeviceAutomation {
                        device,
                        parameter: ParameterId::from_static(id),
                        points: vec![point(BeatTime::ZERO, value)],
                    })
                    .unwrap();
            }
            let original_base = editor.project().device(device).unwrap().state().clone();
            let mut oracle_editor = Editor::new(editor.project().clone(), 16).unwrap();
            let mut state = oracle_editor
                .project()
                .device(device)
                .unwrap()
                .state()
                .clone();
            for (id, value) in (29..=32).zip([0.5, 0.25, -0.25, -0.5]) {
                state = crate::patch_native_device_parameter_for_class(
                    crate::NativeDeviceClass::Instrument,
                    &state,
                    ParameterId::from_static(id),
                    hex_lattice_contract::ParameterValue::Float(value),
                )
                .unwrap();
                oracle_editor
                    .execute(Command::ReplaceDeviceAutomation {
                        device,
                        parameter: ParameterId::from_static(id),
                        points: vec![],
                    })
                    .unwrap();
            }
            oracle_editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: state.clone(),
                })
                .unwrap();
            let mut actual = playback(&editor);
            let mut expected = playback(&oracle_editor);
            assert_eq!(actual.mutable_instruments.iter().flatten().count(), 1);
            assert!(
                actual
                    .revision
                    .session()
                    .instrument_automation(track, device)
                    .is_some()
            );
            assert!(
                actual
                    .revision
                    .session()
                    .instrument_automation(contributor, device)
                    .is_none()
            );
            for runtime in [&mut actual, &mut expected] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                render(runtime, 256, 17);
            }
            assert_signal_eq(
                render(&mut actual, 4097, 31),
                render(&mut expected, 4097, 4097),
            );
            for id in 29..=32 {
                editor
                    .execute(Command::ReplaceDeviceAutomation {
                        device,
                        parameter: ParameterId::from_static(id),
                        points: vec![],
                    })
                    .unwrap();
            }
            oracle_editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: original_base,
                })
                .unwrap();
            let adopt = |editor: &Editor, old: &mut SessionPlayback| {
                let mut next = playback(editor);
                next.try_inherit_state_from(old).unwrap();
                *old = next;
            };
            adopt(&editor, &mut actual);
            adopt(&oracle_editor, &mut expected);
            assert_signal_eq(render(&mut actual, 257, 7), render(&mut expected, 257, 257));
            for _ in 0..4 {
                assert!(editor.undo());
            }
            oracle_editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
            adopt(&editor, &mut actual);
            adopt(&oracle_editor, &mut expected);
            assert_signal_eq(render(&mut actual, 257, 7), render(&mut expected, 257, 257));
            for runtime in [&mut actual, &mut expected] {
                runtime.apply(SessionPlaybackCommand::Pause).unwrap();
            }
            assert_signal_eq(
                render(&mut actual, 2500, 13),
                render(&mut expected, 2500, 2500),
            );
            // Supported interactive starts reset source history and use the current
            // content position; wait only for the deliberate five-ms carrier ramp.
            for runtime in [&mut actual, &mut expected] {
                runtime.play_from(4000).unwrap();
                render(runtime, 256, 256);
            }
            assert_signal_eq(render(&mut actual, 257, 7), render(&mut expected, 257, 257));
            for editor in [&mut editor, &mut oracle_editor] {
                editor
                    .execute(Command::SetLoopRegion {
                        start: 0,
                        end: 8000,
                        enabled: true,
                    })
                    .unwrap();
            }
            let mut actual = playback(&editor);
            let mut expected = playback(&oracle_editor);
            for runtime in [&mut actual, &mut expected] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                render(runtime, 256, 256);
            }
            assert_signal_eq(
                render(&mut actual, 9000, 31),
                render(&mut expected, 9000, 512),
            );

            let mut session_ids = Vec::new();
            for editor in [&mut editor, &mut oracle_editor] {
                let Created::Scene(scene) = editor
                    .execute(Command::AddScene {
                        name: "carrier session".into(),
                    })
                    .unwrap()
                else {
                    panic!("scene")
                };
                let source = editor.project().track(track).unwrap().midi_clips()[0].id();
                let Created::SessionClip(clip) = editor
                    .execute(Command::CopyArrangementMidiToSession { source, scene })
                    .unwrap()
                else {
                    panic!("session clip")
                };
                session_ids.push(clip);
            }
            let mut actual = playback(&editor);
            let mut expected = playback(&oracle_editor);
            for (runtime, clip) in [&mut actual, &mut expected].into_iter().zip(session_ids) {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                runtime
                    .apply(SessionPlaybackCommand::LaunchClip {
                        clip,
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
                render(runtime, 256, 256);
            }
            assert_signal_eq(
                render(&mut actual, 9000, 31),
                render(&mut expected, 9000, 512),
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

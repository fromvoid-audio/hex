use hex_engine::live_input::{PreparedTypingArm, TypingTarget};
use hex_lattice::NoteGate;
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};

fn fixture(kind: DeviceTypeId, rate: u32) -> (Editor, TypingTarget) {
    let mut editor = Editor::new(Project::new(rate).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "typing".into(),
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
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        kind,
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    (
        editor,
        TypingTarget {
            project_generation: 1,
            project_revision: 1,
            track,
            device,
        },
    )
}

#[test]
fn empty_native_tracks_prepare_all_twelve_real_pitches_without_project_mutation() {
    for kind in [
        hex_devices::SINE_INSTRUMENT_TYPE_ID,
        hex_devices::fm4::device::FM4_TYPE_ID,
        DeviceTypeId::from_static("hex.instrument.operator"),
    ] {
        let (editor, target) = fixture(kind, 48_000);
        let before = editor.project().clone();
        let arm = PreparedTypingArm::prepare(editor.project(), target).unwrap();
        assert_eq!(arm.allowed_keys(), 0xfff);
        assert!(!arm.is_mutable());
        assert_eq!(arm.voice_limit(), 64);
        assert!(arm.matches_project(editor.project()));
        assert_eq!(editor.project(), &before);
        assert!(arm.declared_storage_bytes().unwrap() >= std::mem::size_of::<PreparedTypingArm>());
        for key in 0..12 {
            for age in [0, 1, 240, 4096, u64::MAX] {
                assert!(
                    arm.sample(key, age, NoteGate::Held)
                        .unwrap()
                        .iter()
                        .all(|value| value.is_finite())
                );
            }
            assert_eq!(
                arm.sample(
                    key,
                    arm.release_frames() + 300,
                    NoteGate::Released { at_age_frames: 300 }
                )
                .unwrap(),
                [0.0; 2]
            );
        }
        assert!(arm.sample(12, 0, NoteGate::Held).is_err());
    }
}

#[test]
fn mutable_operator_palette_is_validation_not_an_analytic_fallback() {
    for parameter in [ParameterId::from_static(82), ParameterId::from_static(78)] {
        let (mut editor, target) =
            fixture(DeviceTypeId::from_static("hex.instrument.operator"), 48_000);
        let original = editor
            .project()
            .track(target.track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .clone();
        let value = if parameter == ParameterId::from_static(82) {
            ParameterValue::Bool(true)
        } else {
            ParameterValue::Float(0.25)
        };
        let state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            &original,
            parameter,
            value,
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: target.device,
                state,
            })
            .unwrap();
        let arm = PreparedTypingArm::prepare(editor.project(), target).unwrap();
        assert!(arm.is_mutable());
        assert_eq!(arm.voice_limit(), 16);
        assert_eq!(arm.allowed_keys(), 0xfff);
        assert!(arm.sample(0, 100, NoteGate::Held).is_err());
    }
}

#[test]
fn arm_refuses_invalid_identity_and_bypass_and_detects_stale_state() {
    let (mut editor, target) = fixture(hex_devices::SINE_INSTRUMENT_TYPE_ID, 48_000);
    assert!(
        PreparedTypingArm::prepare(
            editor.project(),
            TypingTarget {
                project_generation: u64::MAX,
                ..target
            }
        )
        .is_err()
    );
    let arm = PreparedTypingArm::prepare(editor.project(), target).unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device: target.device,
            bypassed: true,
        })
        .unwrap();
    assert!(!arm.matches_project(editor.project()));
    assert!(PreparedTypingArm::prepare(editor.project(), target).is_err());
    assert!(editor.undo());
    assert!(arm.matches_project(editor.project()));
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: target.device,
            state,
        })
        .unwrap();
    assert!(!arm.matches_project(editor.project()));
}

fn playback(editor: &Editor, target: TypingTarget) -> hex_engine::SessionPlayback {
    let mut playback = candidate(editor, target);
    playback
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    playback
}
fn candidate(editor: &Editor, target: TypingTarget) -> hex_engine::SessionPlayback {
    let revision = std::sync::Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &std::collections::BTreeMap::new(),
            std::sync::Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = hex_engine::SessionPlayback::new(revision).unwrap();
    let arm = PreparedTypingArm::prepare(editor.project(), target).unwrap();
    let input = playback.prepare_typing_input(arm, 1).unwrap();
    assert!(playback.install_typing_input(input).is_ok());
    playback
}
fn action(target: TypingTarget, serial: u64, key: u8) -> hex_engine::typing_protocol::Action {
    use hex_engine::typing_protocol::{Action, Token};
    Action::On {
        owner: target.owner().unwrap(),
        token: Token { epoch: 1, serial },
        key,
        pitch: 60 + key,
        velocity: 100,
    }
}
fn off(on: hex_engine::typing_protocol::Action) -> hex_engine::typing_protocol::Action {
    let hex_engine::typing_protocol::Action::On { owner, token, .. } = on else {
        panic!("on")
    };
    hex_engine::typing_protocol::Action::Off { owner, token }
}

#[test]
fn real_empty_track_pcm_and_committed_receipt_match_all_twelve_prepared_providers() {
    for kind in [
        hex_devices::SINE_INSTRUMENT_TYPE_ID,
        hex_devices::fm4::device::FM4_TYPE_ID,
        DeviceTypeId::from_static("hex.instrument.operator"),
    ] {
        for key in 0..12 {
            let (editor, target) = fixture(kind, 48_000);
            let oracle = PreparedTypingArm::prepare(editor.project(), target).unwrap();
            let mut playback = playback(&editor, target);
            let on = action(target, 1, key);
            playback.queue_typing_input(1, on).unwrap();
            assert!(
                playback.take_typing_application().is_none(),
                "queue admission is not an applied receipt"
            );
            let mut pcm = [[0.0; 2]; 513];
            playback.render(&mut pcm).unwrap();
            let receipt = playback.take_typing_application().unwrap();
            assert_eq!(receipt.ticket, 1);
            assert_eq!(receipt.action, on);
            assert_eq!(receipt.performance_frame, 0);
            assert!(receipt.result.is_ok());
            for (age, sample) in pcm.iter().enumerate() {
                assert_eq!(
                    *sample,
                    oracle
                        .sample(key as usize, age as u64, NoteGate::Held)
                        .unwrap()
                );
            }
            playback
                .apply(hex_engine::SessionPlaybackCommand::Pause)
                .unwrap();
            playback.queue_typing_input(2, off(on)).unwrap();
            playback.render(&mut pcm[..128]).unwrap();
            assert!(pcm[..128].iter().all(|sample| *sample == [0.0; 2]));
            assert!(playback.take_typing_application().is_none());
            assert_eq!(playback.performance_frame(), 513);
            playback
                .apply(hex_engine::SessionPlaybackCommand::Play)
                .unwrap();
            playback.render(&mut pcm[..128]).unwrap();
            for (index, sample) in pcm[..128].iter().enumerate() {
                assert_eq!(
                    *sample,
                    oracle
                        .sample(
                            key as usize,
                            513 + index as u64,
                            NoteGate::Released { at_age_frames: 513 }
                        )
                        .unwrap()
                );
            }
            assert!(playback.take_typing_application().unwrap().result.is_ok());
        }
    }
}

#[test]
fn real_mutable_lfo_and_feedback_share_the_existing_native_owner_and_release_once() {
    for parameter in [82, 78] {
        let (mut editor, target) =
            fixture(DeviceTypeId::from_static("hex.instrument.operator"), 48_000);
        let original = editor
            .project()
            .track(target.track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .clone();
        let value = if parameter == 82 {
            ParameterValue::Bool(true)
        } else {
            ParameterValue::Float(0.25)
        };
        let state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            &original,
            ParameterId::from_static(parameter),
            value,
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: target.device,
                state,
            })
            .unwrap();
        let tail = PreparedTypingArm::prepare(editor.project(), target)
            .unwrap()
            .release_frames();
        let mut playback = playback(&editor, target);
        let on = action(target, 1, 9);
        playback.queue_typing_input(1, on).unwrap();
        let mut pcm = [[0.0; 2]; 513];
        playback.render(&mut pcm).unwrap();
        assert!(pcm.iter().any(|sample| sample[0].abs() > 0.01));
        assert!(playback.take_typing_application().unwrap().result.is_ok());
        playback.queue_typing_input(2, off(on)).unwrap();
        playback.render(&mut pcm).unwrap();
        assert!(playback.take_typing_application().unwrap().result.is_ok());
        let mut remaining_tail = vec![[0.0; 2]; tail as usize + 1];
        playback.render(&mut remaining_tail).unwrap();
        assert_eq!(remaining_tail.last(), Some(&[0.0; 2]));
        playback.render(&mut pcm).unwrap();
        assert!(pcm.iter().all(|sample| *sample == [0.0; 2]));
    }
}

#[test]
fn cut_before_pending_prefix_cannot_resurrect_an_onset() {
    let (editor, target) = fixture(hex_devices::SINE_INSTRUMENT_TYPE_ID, 48_000);
    let mut playback = playback(&editor, target);
    playback
        .queue_typing_input(1, action(target, 1, 9))
        .unwrap();
    playback.cut_typing_input(1);
    let mut pcm = [[0.0; 2]; 513];
    playback.render(&mut pcm).unwrap();
    assert!(pcm.iter().all(|sample| *sample == [0.0; 2]));
    assert_eq!(
        playback.take_typing_application().unwrap().result,
        Err(hex_engine::typing_protocol::Refusal::Stale)
    );
}

fn mutable_authored_fixture() -> (
    Editor,
    TypingTarget,
    hex_project::ClipId,
    hex_project::NoteId,
) {
    authored_fixture(DeviceTypeId::from_static("hex.instrument.operator"), true)
}
fn authored_fixture(
    kind: DeviceTypeId,
    mutable: bool,
) -> (
    Editor,
    TypingTarget,
    hex_project::ClipId,
    hex_project::NoteId,
) {
    use hex_project::BeatTime;
    let (mut editor, target) = fixture(kind, 48_000);
    if mutable {
        let state = editor
            .project()
            .track(target.track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .clone();
        let state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(82),
            ParameterValue::Bool(true),
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: target.device,
                state,
            })
            .unwrap();
    }
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: target.track,
            start: BeatTime::ZERO,
            length: BeatTime::from_ratio(1, 1).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let Created::Note(note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 4).unwrap(),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .unwrap()
    else {
        panic!("note")
    };
    (editor, target, clip, note)
}

#[test]
fn compatible_publication_preserves_real_typed_and_authored_gates_and_phase() {
    use hex_project::BeatTime;
    for (kind, mutable) in [
        (hex_devices::SINE_INSTRUMENT_TYPE_ID, false),
        (hex_devices::fm4::device::FM4_TYPE_ID, false),
        (DeviceTypeId::from_static("hex.instrument.operator"), false),
        (DeviceTypeId::from_static("hex.instrument.operator"), true),
    ] {
        for change in [false, true] {
            let (mut editor, target, _, note) = authored_fixture(kind, mutable);
            let mut old = playback(&editor, target);
            let mut oracle = playback(&editor, target);
            let on = action(target, 1, 0);
            for runtime in [&mut old, &mut oracle] {
                runtime.queue_typing_input(1, on).unwrap();
                runtime.render(&mut [[0.0; 2]; 173]).unwrap();
                assert!(runtime.take_typing_application().unwrap().result.is_ok());
            }
            if change {
                editor
                    .execute(Command::UpdateMidiNote {
                        note,
                        start: BeatTime::ZERO,
                        duration: BeatTime::from_ratio(1, 2).unwrap(),
                        pitch: 72,
                        channel: 0,
                        velocity: 50,
                    })
                    .unwrap();
            }
            let mut next = candidate(
                &editor,
                TypingTarget {
                    project_revision: 2,
                    ..target
                },
            );
            next.try_inherit_state_from(&mut old).unwrap();
            let mut expected = [[0.0; 2]; 256];
            let mut actual = expected;
            for block in 0..28 {
                next.render(&mut actual).unwrap();
                oracle.render(&mut expected).unwrap();
                for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
                    assert_eq!(
                        actual, expected,
                        "kind={kind:?} mutable={mutable} change={change} block={block} offset={index}"
                    );
                }
            }
            for runtime in [&mut next, &mut oracle] {
                runtime.queue_typing_input(2, off(on)).unwrap();
            }
            for _ in 0..20 {
                next.render(&mut actual).unwrap();
                oracle.render(&mut expected).unwrap();
                assert_eq!(
                    actual, expected,
                    "original physical key-up must release carried voice"
                );
            }
            assert!(next.take_typing_application().unwrap().result.is_ok());
            assert!(actual.iter().all(|sample| *sample == [0.0; 2]));
        }
    }
}

#[test]
fn edited_future_occurrence_is_distinct_from_the_inherited_original_note_id() {
    use hex_project::BeatTime;
    for (kind, mutable) in [
        (hex_devices::SINE_INSTRUMENT_TYPE_ID, false),
        (hex_devices::fm4::device::FM4_TYPE_ID, false),
        (DeviceTypeId::from_static("hex.instrument.operator"), false),
        (DeviceTypeId::from_static("hex.instrument.operator"), true),
    ] {
        let (mut editor, target, clip, note) = authored_fixture(kind, mutable);
        let mut old = playback(&editor, target);
        let on = action(target, 1, 0);
        old.queue_typing_input(1, on).unwrap();
        old.render(&mut [[0.0; 2]; 173]).unwrap();
        old.take_typing_application().unwrap();
        let mut expected_editor = Editor::new(editor.project().clone(), 16).unwrap();
        expected_editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::from_ratio(1, 40).unwrap(),
                duration: BeatTime::from_ratio(1, 8).unwrap(),
                pitch: 72,
                channel: 0,
                velocity: 90,
            })
            .unwrap();
        let mut oracle = playback(&expected_editor, target);
        oracle.queue_typing_input(1, on).unwrap();
        oracle.render(&mut [[0.0; 2]; 173]).unwrap();
        oracle.take_typing_application().unwrap();
        editor
            .execute(Command::UpdateMidiNote {
                note,
                start: BeatTime::from_ratio(1, 40).unwrap(),
                duration: BeatTime::from_ratio(1, 8).unwrap(),
                pitch: 72,
                channel: 0,
                velocity: 90,
            })
            .unwrap();
        let mut next = candidate(
            &editor,
            TypingTarget {
                project_revision: 2,
                ..target
            },
        );
        next.try_inherit_state_from(&mut old).unwrap();
        let mut expected = [[0.0; 2]; 128];
        let mut actual = expected;
        for _ in 0..60 {
            next.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(
                actual, expected,
                "future changed signature must join original runtime only at actual onset"
            );
        }
    }
}

#[test]
fn combined_original_and_candidate_capacity_refusal_preserves_old_pcm_and_key_up() {
    use hex_project::BeatTime;
    let (mut editor, target, clip, note) = mutable_authored_fixture();
    let mut old = playback(&editor, target);
    let mut oracle = playback(&editor, target);
    let on = action(target, 1, 0);
    for runtime in [&mut old, &mut oracle] {
        runtime.queue_typing_input(1, on).unwrap();
        runtime.render(&mut [[0.0; 2]; 173]).unwrap();
        runtime.take_typing_application().unwrap();
    }
    editor
        .execute(Command::UpdateMidiNote {
            note,
            start: BeatTime::from_ratio(1, 40).unwrap(),
            duration: BeatTime::from_ratio(1, 8).unwrap(),
            pitch: 72,
            channel: 0,
            velocity: 90,
        })
        .unwrap();
    for pitch in 40..54 {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::from_ratio(1, 40).unwrap(),
                duration: BeatTime::from_ratio(1, 8).unwrap(),
                pitch,
                channel: 0,
                velocity: 90,
            })
            .unwrap();
    }
    let mut next = candidate(
        &editor,
        TypingTarget {
            project_revision: 2,
            ..target
        },
    );
    assert!(
        next.try_inherit_state_from(&mut old).is_err(),
        "15 prospective + original authored + held typed cannot fit 16"
    );
    assert_eq!(old.performance_frame(), 173);
    assert!(old.typing_admission().unwrap().armed);
    let mut expected = [[0.0; 2]; 256];
    let mut actual = expected;
    for _ in 0..30 {
        old.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            actual, expected,
            "refusal must not partially move native/typed ownership"
        );
    }
    for runtime in [&mut old, &mut oracle] {
        runtime.queue_typing_input(2, off(on)).unwrap();
    }
    for _ in 0..20 {
        old.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
    }
    assert!(old.take_typing_application().unwrap().result.is_ok());
    assert!(actual.iter().all(|sample| *sample == [0.0; 2]));
}

#[test]
fn structural_removal_releases_through_the_original_instrument_owner_without_redirect() {
    for mutable in [false, true] {
        let (mut editor, target) =
            fixture(DeviceTypeId::from_static("hex.instrument.operator"), 48_000);
        if mutable {
            let state = editor
                .project()
                .track(target.track)
                .unwrap()
                .instrument()
                .unwrap()
                .state()
                .clone();
            let state = hex_engine::patch_native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                &state,
                ParameterId::from_static(82),
                ParameterValue::Bool(true),
            )
            .unwrap();
            editor
                .execute(Command::ReplaceDeviceState {
                    device: target.device,
                    state,
                })
                .unwrap();
        }
        let mut old = playback(&editor, target);
        let mut oracle = playback(&editor, target);
        let on = action(target, 1, 0);
        for runtime in [&mut old, &mut oracle] {
            runtime.queue_typing_input(1, on).unwrap();
            runtime.render(&mut [[0.0; 2]; 173]).unwrap();
            runtime.take_typing_application().unwrap();
        }
        let release = PreparedTypingArm::prepare(editor.project(), target)
            .unwrap()
            .release_frames();
        editor
            .execute(Command::SetDeviceBypass {
                device: target.device,
                bypassed: true,
            })
            .unwrap();
        let revision = std::sync::Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::new(),
                std::sync::Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut next = hex_engine::SessionPlayback::new(revision).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        oracle.queue_typing_input(2, off(on)).unwrap();
        let mut actual = [[0.0; 2]; 128];
        let mut expected = actual;
        for _ in 0..(release / 128 + 3) {
            next.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(
                actual, expected,
                "structural cut must preserve old release law and physical runtime"
            );
        }
        assert!(next.typing_admission().is_none());
        assert!(actual.iter().all(|frame| *frame == [0.0; 2]));
    }
}

#[test]
fn held_typing_uses_performance_age_across_real_transport_loop_wraps() {
    for kind in [
        hex_devices::SINE_INSTRUMENT_TYPE_ID,
        hex_devices::fm4::device::FM4_TYPE_ID,
        DeviceTypeId::from_static("hex.instrument.operator"),
    ] {
        let (mut editor, target) = fixture(kind, 48_000);
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 512,
                enabled: true,
            })
            .unwrap();
        let arm = PreparedTypingArm::prepare(editor.project(), target).unwrap();
        let mut runtime = playback(&editor, target);
        let on = action(target, 1, 9);
        runtime.queue_typing_input(1, on).unwrap();
        let mut pcm = [[0.0; 2]; 173];
        for block in 0..30 {
            runtime.render(&mut pcm).unwrap();
            for (offset, frame) in pcm.iter().enumerate() {
                assert_eq!(
                    *frame,
                    arm.sample(9, (block * 173 + offset) as u64, NoteGate::Held)
                        .unwrap()
                );
            }
        }
        assert_eq!(runtime.performance_frame(), 5190);
        assert_eq!(runtime.arrangement_frame(), 5190 % 512);
        runtime.queue_typing_input(2, off(on)).unwrap();
        runtime.render(&mut pcm).unwrap();
        for (offset, frame) in pcm.iter().enumerate() {
            assert_eq!(
                *frame,
                arm.sample(
                    9,
                    5190 + offset as u64,
                    NoteGate::Released {
                        at_age_frames: 5190
                    }
                )
                .unwrap()
            );
        }
    }
}

#[test]
fn session_source_changes_preserve_held_typing_without_implicit_transport_changes() {
    use hex_project::BeatTime;
    for mutable in [false, true] {
        let (mut editor, target) =
            fixture(DeviceTypeId::from_static("hex.instrument.operator"), 48_000);
        if mutable {
            let state = editor
                .project()
                .track(target.track)
                .unwrap()
                .instrument()
                .unwrap()
                .state()
                .clone();
            let state = hex_engine::patch_native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                &state,
                ParameterId::from_static(82),
                ParameterValue::Bool(true),
            )
            .unwrap();
            editor
                .execute(Command::ReplaceDeviceState {
                    device: target.device,
                    state,
                })
                .unwrap();
        }
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "silent source".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionMidiClip {
                track: target.track,
                scene,
                name: "no authored notes".into(),
                source_length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let mut runtime = playback(&editor, target);
        let mut oracle = playback(&editor, target);
        let on = action(target, 1, 9);
        for owner in [&mut runtime, &mut oracle] {
            owner.queue_typing_input(1, on).unwrap();
            owner.render(&mut [[0.0; 2]; 173]).unwrap();
            owner.take_typing_application().unwrap();
        }
        for command in [
            hex_engine::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
            hex_engine::SessionPlaybackCommand::StopTrack {
                track: target.track,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
            hex_engine::SessionPlaybackCommand::BackToArrangement {
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
        ] {
            runtime.apply(command).unwrap();
            let mut actual = [[0.0; 2]; 256];
            let mut expected = actual;
            runtime.render(&mut actual).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(
                actual, expected,
                "source routing cannot reset live phase/LFO or invent a release"
            );
            assert!(runtime.playing());
            assert!(runtime.typing_admission().unwrap().armed);
        }
        for owner in [&mut runtime, &mut oracle] {
            owner.queue_typing_input(2, off(on)).unwrap();
        }
        let mut actual = [[0.0; 2]; 128];
        let mut expected = actual;
        runtime.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn real_hybrid_gap_prefix_suffix_automation_sends_return_feeds_and_main_run_once() {
    let (mut editor, target) = fixture(hex_devices::SINE_INSTRUMENT_TYPE_ID, 48_000);
    let effect = |gain| {
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Effect,
            &state,
            hex_devices::utility::parameter::GAIN,
            ParameterValue::Float(gain),
        )
        .unwrap()
    };
    let Created::Device(prefix) = editor
        .execute(Command::InsertDevice {
            track: target.track,
            before: None,
            state: effect(0.5),
        })
        .unwrap()
    else {
        panic!("prefix")
    };
    editor
        .execute(Command::MoveTrackDevice {
            device: prefix,
            track: target.track,
            before: Some(target.device),
        })
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track: target.track,
            before: None,
            state: effect(2.0),
        })
        .unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "disposable-live-hybrid.wav".into(),
            sample_rate: 48_000,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor
        .execute(Command::PlaceClip {
            track: target.track,
            asset,
            start: 0,
            duration: 1024,
            source_offset: 0,
            source_length: 1024,
            looping: false,
        })
        .unwrap();
    editor
        .execute(Command::Gain {
            track: target.track,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track: target.track,
            points: vec![hex_project::GainPoint {
                beat: hex_project::BeatTime::ZERO,
                gain: 0.25,
                outgoing: hex_project::GainInterpolation::Step,
            }],
        })
        .unwrap();
    let Created::ReturnTrack(a) = editor
        .execute(Command::AddReturnTrack { name: "a".into() })
        .unwrap()
    else {
        panic!("a")
    };
    let Created::ReturnTrack(b) = editor
        .execute(Command::AddReturnTrack { name: "b".into() })
        .unwrap()
    else {
        panic!("b")
    };
    editor
        .execute(Command::SetReturnSend {
            track: target.track,
            return_track: a,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::ReturnGain {
            return_track: a,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnFeed {
            source: a,
            destination: b,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::ReturnGain {
            return_track: b,
            gain: 0.5,
        })
        .unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let before = editor.project().clone();
    let media = std::collections::BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[1.0, 0.25]; 1024]).unwrap(),
    )]);
    let revision = std::sync::Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &media,
            std::sync::Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let arm = PreparedTypingArm::prepare(editor.project(), target).unwrap();
    let mut runtime = hex_engine::SessionPlayback::new(std::sync::Arc::clone(&revision)).unwrap();
    let input = runtime
        .prepare_typing_input(
            PreparedTypingArm::prepare(editor.project(), target).unwrap(),
            1,
        )
        .unwrap();
    assert!(runtime.install_typing_input(input).is_ok());
    runtime
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    runtime.queue_typing_input(1, action(target, 1, 9)).unwrap();
    let mut pcm = [[0.0; 2]; 513];
    runtime.render(&mut pcm).unwrap();
    for (age, frame) in pcm.iter().enumerate() {
        let sine = arm.sample(9, age as u64, NoteGate::Held).unwrap()[0];
        let expected = [
            (1.0 + 2.0 * sine) * 0.25 * 1.3125 * 0.5,
            (0.25 + 2.0 * sine) * 0.25 * 1.3125 * 0.5,
        ];
        for channel in 0..2 {
            assert!(
                (frame[channel] - expected[channel]).abs() < 1e-7,
                "single gap/mix/feed oracle age={age} channel={channel}: {} vs {}",
                frame[channel],
                expected[channel]
            );
        }
    }
    assert_eq!(editor.project(), &before);
    assert_eq!(
        editor
            .project()
            .track(target.track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        target.device
    );
    let mut oracle = hex_engine::SessionPlayback::new(std::sync::Arc::clone(&revision)).unwrap();
    let input = oracle
        .prepare_typing_input(
            PreparedTypingArm::prepare(editor.project(), target).unwrap(),
            1,
        )
        .unwrap();
    assert!(oracle.install_typing_input(input).is_ok());
    oracle
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let on = action(target, 1, 9);
    oracle.queue_typing_input(1, on).unwrap();
    oracle.render(&mut pcm).unwrap();
    oracle.queue_typing_input(2, off(on)).unwrap();
    let mut changed_editor = Editor::new(editor.project().clone(), 16).unwrap();
    changed_editor
        .execute(Command::SetDeviceBypass {
            device: target.device,
            bypassed: true,
        })
        .unwrap();
    let changed = std::sync::Arc::new(
        hex_engine::PreparedRevision::prepare(
            changed_editor.project(),
            &media,
            std::sync::Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut next = hex_engine::SessionPlayback::new(changed).unwrap();
    next.try_inherit_state_from(&mut runtime).unwrap();
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    for _ in 0..24 {
        next.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        for (actual, expected) in actual.iter().zip(expected.iter()) {
            for channel in 0..2 {
                assert!(
                    (actual[channel] - expected[channel]).abs() < 1e-7,
                    "retained suffix/send/Return owner must render original release exactly once"
                );
            }
        }
    }
    let mut export = hex_engine::SessionPlayback::new_linear_arrangement_export(revision).unwrap();
    assert!(
        export
            .prepare_typing_input(
                PreparedTypingArm::prepare(editor.project(), target).unwrap(),
                1
            )
            .is_err()
    );
    export
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    export.render(&mut pcm).unwrap();
    for frame in pcm {
        assert_eq!(frame, [0.25 * 1.3125 * 0.5, 0.25 * 0.25 * 1.3125 * 0.5]);
    }
}

#[test]
fn missing_retained_return_owner_refuses_without_redirecting_the_held_voice() {
    let (mut editor, target) = fixture(hex_devices::SINE_INSTRUMENT_TYPE_ID, 48_000);
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "original bus".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track: target.track,
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    let mut old = playback(&editor, target);
    let mut oracle = playback(&editor, target);
    let on = action(target, 1, 9);
    for runtime in [&mut old, &mut oracle] {
        runtime.queue_typing_input(1, on).unwrap();
        runtime.render(&mut [[0.0; 2]; 173]).unwrap();
        runtime.take_typing_application().unwrap();
    }
    editor
        .execute(Command::SetDeviceBypass {
            device: target.device,
            bypassed: true,
        })
        .unwrap();
    editor.execute(Command::RemoveReturnTrack(bus)).unwrap();
    editor
        .execute(Command::AddReturnTrack {
            name: "different bus at old index".into(),
        })
        .unwrap();
    let revision = std::sync::Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &std::collections::BTreeMap::new(),
            std::sync::Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut next = hex_engine::SessionPlayback::new(revision).unwrap();
    assert!(next.try_inherit_state_from(&mut old).is_err());
    assert_eq!(old.performance_frame(), 173);
    assert!(old.typing_admission().unwrap().armed);
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    old.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    for runtime in [&mut old, &mut oracle] {
        runtime.queue_typing_input(2, off(on)).unwrap();
    }
    old.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(old.take_typing_application().unwrap().result.is_ok());
}

#[test]
fn new_epoch_counts_original_release_pools_and_recovers_capacity_after_retirement() {
    let (mut editor, target) =
        fixture(DeviceTypeId::from_static("hex.instrument.operator"), 48_000);
    let state = editor
        .project()
        .track(target.track)
        .unwrap()
        .instrument()
        .unwrap()
        .state()
        .clone();
    let state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &state,
        ParameterId::from_static(82),
        ParameterValue::Bool(true),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: target.device,
            state,
        })
        .unwrap();
    let release = PreparedTypingArm::prepare(editor.project(), target)
        .unwrap()
        .release_frames();
    let mut old = playback(&editor, target);
    for serial in 1..=12 {
        old.queue_typing_input(serial, action(target, serial, (serial - 1) as u8))
            .unwrap();
    }
    old.render(&mut [[0.0; 2]; 128]).unwrap();
    while old.take_typing_application().is_some() {}
    old.cut_typing_input(1);
    old.render(&mut [[0.0; 2]; 1]).unwrap();
    let revision = std::sync::Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &std::collections::BTreeMap::new(),
            std::sync::Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut next = hex_engine::SessionPlayback::new(revision).unwrap();
    let input = next
        .prepare_typing_input(
            PreparedTypingArm::prepare(editor.project(), target).unwrap(),
            2,
        )
        .unwrap();
    assert!(next.install_typing_input(input).is_ok());
    next.try_inherit_state_from(&mut old).unwrap();
    let on = |serial| hex_engine::typing_protocol::Action::On {
        owner: target.owner().unwrap(),
        token: hex_engine::typing_protocol::Token { epoch: 2, serial },
        key: 9,
        pitch: 69,
        velocity: 100,
    };
    for serial in 1..=5 {
        next.queue_typing_input(serial + 12, on(serial)).unwrap();
    }
    next.render(&mut [[0.0; 2]; 1]).unwrap();
    let results: Vec<_> = std::iter::from_fn(|| next.take_typing_application()).collect();
    assert_eq!(
        results
            .iter()
            .filter(|receipt| receipt.result.is_ok())
            .count(),
        4
    );
    assert_eq!(
        results[4].result,
        Err(hex_engine::typing_protocol::Refusal::Capacity)
    );
    let mut pcm = [[0.0; 2]; 128];
    for _ in 0..(release / 128 + 3) {
        next.render(&mut pcm).unwrap();
    }
    // No project edit or fresh publication should be needed to reclaim space.
    for serial in 6..=17 {
        next.queue_typing_input(serial + 12, on(serial)).unwrap();
    }
    next.render(&mut pcm).unwrap();
    let results: Vec<_> = std::iter::from_fn(|| next.take_typing_application()).collect();
    assert_eq!(results.len(), 12);
    assert!(results.iter().all(|receipt| receipt.result.is_ok()));
    assert!(next.typing_admission().unwrap().armed);
}

#[test]
fn unchanged_dense_authored_source_does_not_double_reserve_its_original_voices() {
    use hex_project::BeatTime;
    let (mut editor, target, clip, _) =
        authored_fixture(hex_devices::SINE_INSTRUMENT_TYPE_ID, false);
    for pitch in [70, 71, 72] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_ratio(1, 4).unwrap(),
                pitch,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    let mut old = playback(&editor, target);
    let mut oracle = playback(&editor, target);
    for batch in [1..=30, 31..=60] {
        for serial in batch {
            for owner in [&mut old, &mut oracle] {
                owner
                    .queue_typing_input(serial, action(target, serial, 9))
                    .unwrap();
            }
        }
        for owner in [&mut old, &mut oracle] {
            owner.render(&mut [[0.0; 2]; 128]).unwrap();
            while owner.take_typing_application().is_some() {}
        }
    }
    let mut next = candidate(
        &editor,
        TypingTarget {
            project_revision: 2,
            ..target
        },
    );
    next.try_inherit_state_from(&mut old)
        .expect("60 original live + 4 unchanged authored = 64, not 68");
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    next.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

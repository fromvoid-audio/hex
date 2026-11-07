use super::*;
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::sync::{Arc, atomic::AtomicBool};
fn fixture() -> (Project, hex_project::AssetId) {
    let mut editor = Editor::new(Project::new(16000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "original.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
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
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    state.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
        gate: hex_devices::sampler::GateMode::Trigger,
    };
    state.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    };
    let native = state
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: native,
        })
        .unwrap();
    (editor.project().clone(), asset)
}
#[test]
fn current_sampler_actual_constructor_has_literal_pcm_stop_replay_and_closed_custody() {
    sampler_constructor_case(false);
}
#[test]
fn grouped_sampler_constructor_routes_live_monitor_and_authored_playback() {
    sampler_constructor_case(true);
}
fn sampler_constructor_case(grouped: bool) {
    assert!(crate::audio_storage_constructor_layout_supported());
    let (project, asset) = fixture();
    let project = if grouped {
        let mut editor = Editor::new(project, 8).unwrap();
        let track = editor.project().tracks()[0].id();
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        editor
            .execute(Command::Gain {
                track: group,
                gain: 0.5,
            })
            .unwrap();
        editor.project().clone()
    } else {
        project
    };
    let gain = if grouped { 0.5 } else { 1.0 };
    assert!(crate::native_device_resources::current_sampler::validate_project(&project).unwrap());
    let quote = AudioCandidateReservationQuote::for_project(&project, 1).unwrap();
    let limits = quote.storage_reservation(65536).unwrap();
    let mut cell = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
    let source = crate::OwnedAudioSource::try_resident(
        8000,
        vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
    )
    .ok()
    .unwrap();
    let original = crate::OwnedAudioInputs::try_new(vec![(asset, source)], limits)
        .ok()
        .unwrap();
    let token = original.source_token(asset).unwrap();
    cell.copy_original_source_on_control(asset, original.source_view(&token).unwrap())
        .unwrap();
    cell.prepare_current_sampler_on_control(
        &project,
        11,
        crate::AudioSourceContext {
            project_generation: 4,
            editor_revision: 7,
        },
        4,
        &AtomicBool::new(false),
    )
    .unwrap_or_else(|e| {
        panic!(
            "{e:?}; custody={:?}/{:?}, token={}, runtime={}",
            cell.custody.as_ref().map(|c| c.phase()),
            cell.custody.as_ref().and_then(|c| c.reason()),
            cell.token.is_some(),
            cell.draft.as_ref().is_some_and(|d| d.runtime.is_some())
        )
    });
    let owner = cell.owner_on_control().unwrap();
    owner
        .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
            data_rows: quote.data_rows,
            handle_rows: quote.handle_rows,
            root_rows: 8,
            max_retained_bytes: limits.max_prepared_data_bytes,
            max_construction_peak_bytes: limits.max_construction_data_bytes,
        })
        .unwrap();
    let runtime = owner.inner.runtime.as_ref().unwrap();
    assert_ne!(
        runtime
            .sampler_sources
            .as_ref()
            .unwrap()
            .0
            .bank
            .source_view(
                &runtime
                    .sampler_sources
                    .as_ref()
                    .unwrap()
                    .0
                    .bank
                    .source_token(asset)
                    .unwrap()
            )
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr(),
        original
            .source_view(&token)
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr()
    );
    // The constructor-born live pair borrows that same source scope while the
    // transport is stopped, without consuming the authored frame-zero note.
    let sampler = project
        .tracks()
        .iter()
        .find(|track| track.instrument().is_some())
        .unwrap();
    let track = sampler.id();
    let device = sampler.instrument().unwrap().id();
    let words = std::array::from_fn(|i| {
        if i == 0 {
            (1 << 16) | (127 << 8) | 61
        } else {
            0
        }
    });
    let frame = crate::computer_keyboard::CurrentSamplerKeyboardFrame {
        epoch: 1,
        track: track.value(),
        device: device.value(),
        words,
    };
    let before_monitor = owner.test_transport();
    owner
        .inner
        .runtime
        .as_mut()
        .unwrap()
        .playback
        .apply_current_sampler_keyboard(Some(frame))
        .unwrap();
    let mut monitor = [[0.0; 2]; 12];
    owner.test_render(&mut monitor).unwrap();
    assert_eq!(
        &monitor[..8],
        &[
            [0.125, -0.25],
            [0.3125, -0.125],
            [0.5, 0.0],
            [0.1875, 0.125],
            [-0.125, 0.25],
            [-0.0625, 0.125],
            [0.0, 0.0],
            [0.0, 0.0],
        ]
        .map(|frame| frame.map(|sample| sample * gain))
    );
    assert_eq!(before_monitor, owner.test_transport());
    owner
        .inner
        .runtime
        .as_mut()
        .unwrap()
        .playback
        .apply_current_sampler_keyboard(None)
        .unwrap();
    assert!(
        owner
            .inner
            .runtime
            .as_mut()
            .unwrap()
            .playback
            .apply_current_sampler_keyboard(Some(frame))
            .is_err()
    );
    owner
        .test_apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Play,
        ))
        .unwrap();
    let mut output = [[0.0; 2]; 12];
    owner.test_render(&mut output).unwrap();
    assert_eq!(
        output,
        [
            [0.125, -0.25],
            [0.3125, -0.125],
            [0.5, 0.0],
            [0.1875, 0.125],
            [-0.125, 0.25],
            [-0.0625, 0.125],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0]
        ]
        .map(|frame| frame.map(|sample| sample * gain))
    );
    let before = owner.test_transport();
    assert!(
        owner
            .test_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Seek(1)
            ))
            .is_err()
    );
    assert_eq!(before, owner.test_transport());
    owner
        .test_apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Stop,
        ))
        .unwrap();
    owner
        .test_apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Play,
        ))
        .unwrap();
    let mut replay = [[0.0; 2]; 12];
    owner.test_render(&mut replay).unwrap();
    assert_eq!(replay, output);
    cell.cleanup_on_control().unwrap();
    assert!(cell.owner_on_control().is_none());
    assert!(original.source_view(&token).is_ok());
}

#[test]
fn sampler_compiler_reserves_empty_clip_and_note_workspace_before_entering() {
    for (session, note_count) in [
        (false, 0),
        (false, 3),
        (false, 21),
        (false, 256),
        (true, 0),
        (true, 21),
    ] {
        let (project, asset) = fixture();
        let mut editor = Editor::new(project, 8).unwrap();
        let track = editor.project().tracks()[0].id();
        let clip = editor.project().tracks()[0].midi_clips()[0].id();
        editor.execute(Command::RemoveMidiClip(clip)).unwrap();
        if note_count != 0 || session {
            let created = if session {
                let Created::Scene(scene) = editor
                    .execute(Command::AddScene {
                        name: "console".into(),
                    })
                    .unwrap()
                else {
                    panic!("scene")
                };
                editor
                    .execute(Command::CreateSessionMidiClip {
                        track,
                        scene,
                        name: "pattern".into(),
                        source_length: BeatTime::from_raw(BeatTime::ONE.raw() * note_count.max(1)),
                    })
                    .unwrap()
            } else {
                editor
                    .execute(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::ZERO,
                        length: BeatTime::from_raw(BeatTime::ONE.raw() * note_count),
                    })
                    .unwrap()
            };
            let clip = match created {
                Created::Clip(clip) | Created::SessionClip(clip) => clip,
                _ => {
                    panic!("clip")
                }
            };
            for i in 0..note_count {
                editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_raw(BeatTime::ONE.raw() * i),
                        duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                        pitch: 60,
                        channel: 0,
                        velocity: 127,
                    })
                    .unwrap();
            }
        } else {
            for i in 0..128 {
                editor
                    .execute(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::from_raw(BeatTime::ONE.raw() * i),
                        length: BeatTime::ONE,
                    })
                    .unwrap();
            }
        }
        let project = editor.project();
        let quote = AudioCandidateReservationQuote::for_project(project, 1).unwrap();
        let original_limits = quote.storage_reservation(65536).unwrap();
        let mut peak = 0;
        let mut retained = 0;
        for pass in 0..4 {
            let mut limits = original_limits;
            if pass > 0 {
                limits.max_construction_data_bytes = peak - usize::from(pass == 1);
                limits.max_prepared_data_bytes = retained - usize::from(pass == 3);
            }
            let source = crate::OwnedAudioSource::try_resident(8000, vec![[0.0; 2]; 4])
                .ok()
                .unwrap();
            let mut bank = Some(
                crate::OwnedAudioInputs::try_new(vec![(asset, source)], limits)
                    .ok()
                    .unwrap(),
            );
            let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
            crate::midi_compile::CURRENT_SAMPLER_COMPILER_ENTRIES.with(|n| n.set(0));
            let result = custody.begin_current_sampler_in_place_on_control(
                project,
                11,
                crate::AudioSourceContext {
                    project_generation: 4,
                    editor_revision: 7,
                },
                &mut bank,
                4,
                &AtomicBool::new(false),
            );
            let entered = crate::midi_compile::CURRENT_SAMPLER_COMPILER_ENTRIES.with(|n| n.get());
            if pass == 1 || pass == 3 {
                assert!(
                    matches!(result, Err(ref e) if e.reason == AudioStorageError::ReservationExceeded)
                );
                assert_eq!(
                    entered, 0,
                    "under-budget constructor must refuse BEFORE compiler"
                );
                assert!(bank.is_some(), "refusal preserves actual original bank");
            } else {
                assert!(
                    result.is_ok(),
                    "exact compiler peak failed: {:?}",
                    result.err().map(|e| e.reason)
                );
                assert_eq!(entered, 1);
                let report = custody.capacity_report();
                assert!(report.native_compiler_workspace_bytes > 0);
                assert!(
                    report.construction_peak_bytes
                        >= report.retained_stage_bytes + report.native_compiler_workspace_bytes
                );
                if pass == 0 {
                    peak = report.construction_peak_bytes;
                    retained = report.retained_stage_bytes;
                }
            }
            custody.cleanup_on_control();
        }
    }
}

#[test]
fn current_sampler_without_arrangement_notes_has_a_constructor_owned_live_keyboard_pair() {
    let (project, asset) = fixture();
    let clip = project.tracks()[0].midi_clips()[0].id();
    let mut editor = Editor::new(project, 8).unwrap();
    editor.execute(Command::RemoveMidiClip(clip)).unwrap();
    let project = editor.project().clone();
    let track = project.tracks()[0].id().value();
    let device = project.tracks()[0].instrument().unwrap().id().value();
    let quote = AudioCandidateReservationQuote::for_project(&project, 1).unwrap();
    let limits = quote.storage_reservation(65536).unwrap();
    let original = crate::OwnedAudioInputs::try_new(
        vec![(
            asset,
            crate::OwnedAudioSource::try_resident(
                8000,
                vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
            )
            .ok()
            .unwrap(),
        )],
        limits,
    )
    .ok()
    .unwrap();
    let token = original.source_token(asset).unwrap();
    let mut cell = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
    cell.copy_original_source_on_control(asset, original.source_view(&token).unwrap())
        .unwrap();
    cell.prepare_current_sampler_on_control(
        &project,
        11,
        crate::AudioSourceContext {
            project_generation: 4,
            editor_revision: 7,
        },
        4,
        &AtomicBool::new(false),
    )
    .unwrap();
    let owner = cell.owner_on_control().unwrap();
    owner
        .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
            data_rows: quote.data_rows,
            handle_rows: quote.handle_rows,
            root_rows: 8,
            max_retained_bytes: limits.max_prepared_data_bytes,
            max_construction_peak_bytes: limits.max_construction_data_bytes,
        })
        .unwrap();
    owner
        .inner
        .runtime
        .as_mut()
        .unwrap()
        .playback
        .apply_current_sampler_keyboard(Some(
            crate::computer_keyboard::CurrentSamplerKeyboardFrame {
                epoch: 1,
                track,
                device,
                words: std::array::from_fn(|i| {
                    if i == 0 {
                        (1 << 16) | (127 << 8) | 61
                    } else {
                        0
                    }
                }),
            },
        ))
        .unwrap();
    let mut output = [[0.0; 2]; 2];
    owner.test_render(&mut output).unwrap();
    assert_eq!(output, [[0.125, -0.25], [0.3125, -0.125]]);
    let invalid = crate::computer_keyboard::CurrentSamplerKeyboardFrame {
        epoch: 2,
        track,
        device,
        words: std::array::from_fn(|i| if i == 0 { (1 << 16) | 61 } else { 0 }),
    };
    assert!(
        owner
            .inner
            .runtime
            .as_mut()
            .unwrap()
            .playback
            .apply_current_sampler_keyboard(Some(invalid))
            .is_err()
    );
    owner.test_render(&mut output).unwrap();
    assert_eq!(
        output,
        [[0.5, 0.0], [0.1875, 0.125]],
        "refused malformed activation must preserve the prior live voice"
    );
    cell.cleanup_on_control().unwrap();
}

#[test]
fn current_sampler_keyboard_pool_preflight_and_host_route_gains_are_bounded_once() {
    use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry};
    use hex_lattice_contract::ParameterValue;
    let (project, asset) = fixture();
    let track = project.tracks()[0].id();
    let device = project.tracks()[0].instrument().unwrap();
    let sampler = device
        .state()
        .clone()
        .with_output_gain_millidb(-6000)
        .unwrap();
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: device.id(),
            state: sampler,
        })
        .unwrap();
    editor.execute(Command::Gain { track, gain: 0.5 }).unwrap();
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&hex_devices::utility::UTILITY_DEFINITION];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let utility = registry
        .patch_parameter(
            &registry
                .default_state(hex_devices::utility::UTILITY_TYPE_ID)
                .unwrap(),
            hex_devices::utility::parameter::GAIN,
            ParameterValue::Float(0.5),
        )
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: utility,
        })
        .unwrap();
    let project = editor.project();
    let quote = AudioCandidateReservationQuote::for_project(project, 1).unwrap();
    let limits = quote.storage_reservation(65536).unwrap();
    let original = crate::OwnedAudioInputs::try_new(
        vec![(
            asset,
            crate::OwnedAudioSource::try_resident(
                8000,
                vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
            )
            .ok()
            .unwrap(),
        )],
        limits,
    )
    .ok()
    .unwrap();
    let token = original.source_token(asset).unwrap();
    let mut cell = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
    cell.copy_original_source_on_control(asset, original.source_view(&token).unwrap())
        .unwrap();
    cell.prepare_current_sampler_on_control(
        project,
        11,
        crate::AudioSourceContext {
            project_generation: 4,
            editor_revision: 7,
        },
        4,
        &AtomicBool::new(false),
    )
    .unwrap();
    let owner = cell.owner_on_control().unwrap();
    let storage = crate::AudioStorageLedgerReservation {
        data_rows: quote.data_rows,
        handle_rows: quote.handle_rows,
        root_rows: 8,
        max_retained_bytes: limits.max_prepared_data_bytes,
        max_construction_peak_bytes: limits.max_construction_data_bytes,
    };
    let summary = owner.seal_constructor_on_control(storage).unwrap();
    // Actual closed ledger includes the new native keyboard pair, input state,
    // Box and Vec storage. Exact retained/peak caps pass; one under refuses.
    let exact = crate::AudioStorageLedgerReservation {
        max_retained_bytes: summary.data.retained_bytes + summary.reserved_split_bytes,
        max_construction_peak_bytes: summary.data.construction_peak_bytes
            + summary.reserved_split_bytes,
        ..storage
    };
    assert!(
        owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                max_retained_bytes: exact.max_retained_bytes - 1,
                ..exact
            })
            .is_err()
    );
    assert!(
        owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                max_construction_peak_bytes: exact.max_construction_peak_bytes - 1,
                ..exact
            })
            .is_err()
    );
    owner.seal_constructor_on_control(exact).unwrap();
    {
        let runtime = owner.inner.runtime.as_ref().unwrap();
        let sources = &runtime.sampler_sources.as_ref().unwrap().0;
        let revision = runtime.playback.prepared_revision().unwrap();
        let bytes =
            crate::SessionPlayback::current_sampler_runtime_preflight(revision, sources).unwrap();
        let interactive = crate::SessionPlayback::interactive_runtime_allowance(revision).unwrap();
        let export = crate::SessionPlayback::linear_export_runtime_allowance(revision).unwrap();
        assert!(
            interactive > export,
            "interactive retains release/handoff banks"
        );
        assert!(bytes > interactive, "CURRENT also funds its keyboard pool");
        struct Restore(usize);
        impl Drop for Restore {
            fn drop(&mut self) {
                crate::session_playback::CURRENT_SAMPLER_RUNTIME_TEST_LIMIT.with(|n| n.set(self.0));
            }
        }
        let previous = crate::session_playback::CURRENT_SAMPLER_RUNTIME_TEST_LIMIT
            .with(|n| n.replace(bytes - 1));
        let _restore = Restore(previous);
        let before =
            crate::computer_keyboard::CURRENT_SAMPLER_CONSTRUCTION_ENTRIES.with(|n| n.get());
        assert!(
            crate::SessionPlayback::new_current_sampler_candidate(Arc::clone(revision), sources)
                .is_err()
        );
        assert_eq!(
            crate::computer_keyboard::CURRENT_SAMPLER_CONSTRUCTION_ENTRIES.with(|n| n.get()),
            before
        );
        crate::session_playback::CURRENT_SAMPLER_RUNTIME_TEST_LIMIT.with(|n| n.set(bytes));
        let candidate =
            crate::SessionPlayback::new_current_sampler_candidate(Arc::clone(revision), sources)
                .unwrap();
        assert!(candidate.release_storage_capacity() > 0);
        assert!(candidate.accounted_runtime_storage_bytes().unwrap() <= bytes);
        assert_eq!(
            crate::computer_keyboard::CURRENT_SAMPLER_CONSTRUCTION_ENTRIES.with(|n| n.get()),
            before + 1
        );
        drop(candidate);
    }
    let device = project.track(track).unwrap().instrument().unwrap().id();
    let mut frame = crate::computer_keyboard::CurrentSamplerKeyboardFrame {
        epoch: 1,
        track: track.value(),
        device: device.value(),
        words: std::array::from_fn(|i| {
            if i == 0 {
                (1 << 16) | (127 << 8) | 61
            } else {
                0
            }
        }),
    };
    owner
        .inner
        .runtime
        .as_mut()
        .unwrap()
        .playback
        .apply_current_sampler_keyboard(Some(frame))
        .unwrap();
    let mut output = [[0.0; 2]; 2];
    owner.test_render(&mut output).unwrap();
    let scale = 10.0_f32.powf(-6.0 / 20.0) * 0.5 * 0.5;
    let expected = [
        [0.125 * scale, -0.25 * scale],
        [0.3125 * scale, -0.125 * scale],
    ];
    for (actual, expected) in output.iter().flatten().zip(expected.iter().flatten()) {
        assert!((*actual - *expected).abs() < 1e-6);
    }
    owner
        .test_apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Stop,
        ))
        .unwrap();
    owner
        .test_apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Play,
        ))
        .unwrap();
    frame.epoch = 2;
    owner
        .inner
        .runtime
        .as_mut()
        .unwrap()
        .playback
        .apply_current_sampler_keyboard(Some(frame))
        .unwrap();
    owner.test_render(&mut output).unwrap();
    for (actual, expected) in output.iter().flatten().zip(expected.iter().flatten()) {
        assert!(
            (*actual - 2.0 * expected).abs() < 1e-6,
            "{actual} != twice{expected}"
        );
    }
    owner
        .test_apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Stop,
        ))
        .unwrap();
    frame.epoch = 3;
    frame.words[0] = (2 << 16) | (100 << 8) | 61;
    owner
        .inner
        .runtime
        .as_mut()
        .unwrap()
        .playback
        .apply_current_sampler_keyboard(Some(frame))
        .unwrap();
    owner.test_render(&mut output).unwrap();
    for (actual, expected) in output.iter().flatten().zip(expected.iter().flatten()) {
        assert!(
            (*actual - expected * (100.0 / 127.0)).abs() < 1e-6,
            "velocity100: {actual} != scaled{expected}"
        );
    }
    cell.cleanup_on_control().unwrap();
}

#[path = "console_tests.rs"]
mod console;

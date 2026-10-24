//! Actual Root source -> original queued endpoint -> closed claimed rendering
//! -> device host-format completion. The claimed graph stays local: this does
//! not forge or claim the still-gated native graph-publication witness.
use super::*;
use hex_lattice::{
    NativeDeviceDefinition, NativeDeviceRegistry, ProcessorChainOwner, ProcessorInstanceIdentity,
};
use hex_project::{Command, Created, Editor, Project};

struct Fixture {
    control: Control,
    data: Guard<NativeCallbackOwner>,
    errors: Guard<ErrorCallbackOwner>,
    retirement: NativeRetirement,
    binding: Binding,
    runtime: hex_engine::ClaimedAudioCandidateRuntime,
    anchors: [Option<hex_engine::AudioCandidateAnchors>; 1],
    samples: Vec<[f32; 2]>,
}
fn fixture(location: usize, bypassed: bool) -> Fixture {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let bus = if location == 1 {
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 1.0,
            })
            .unwrap();
        Some(bus)
    } else {
        None
    };
    let owner = match location {
        0 => ProcessorChainOwner::Track(track.value()),
        1 => ProcessorChainOwner::Return(bus.unwrap().value()),
        _ => ProcessorChainOwner::Main,
    };
    let identity = ProcessorInstanceIdentity::new(owner, editor.project().next_identity()).unwrap();
    let (mut control, data, errors, retirement, binding) = setup_for_identity(identity);
    let state = control.native_source_prepared_state(binding).unwrap();
    let command = match location {
        0 => Command::InsertDevice {
            track,
            before: None,
            state,
        },
        1 => Command::InsertReturnDevice {
            return_track: bus.unwrap(),
            before: None,
            state,
        },
        _ => Command::InsertMainDevice {
            before: None,
            state,
        },
    };
    let Created::Device(device) = editor.execute(command).unwrap() else {
        panic!("device")
    };
    assert_eq!(device.value(), identity.device_id());
    editor
        .execute(Command::SetDeviceBypass { device, bypassed })
        .unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "original-memory.wav".into(),
            sample_rate: 48_000,
            frames: 32,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 32,
            source_offset: 0,
            source_length: 32,
            looping: false,
        })
        .unwrap();
    let mut samples = vec![[0.25, -0.5]; 32];
    samples[31] = [-0.125, 0.375];
    let bank = hex_engine::OwnedAudioInputs::try_new(
        vec![(
            asset,
            hex_engine::OwnedAudioSource::try_resident(48_000, samples.clone())
                .ok()
                .unwrap(),
        )],
        hex_engine::AudioStorageReservation {
            allocation_items: 1024,
            handle_obligations: 256,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap();
    let (mut runtime, anchor) = control
        .with_native_effect_definition(&[binding], |definition| {
            let definitions: [&dyn NativeDeviceDefinition; 1] = [definition];
            let registry = NativeDeviceRegistry::new(&definitions).unwrap();
            let mut custody = hex_engine::AudioPreparationCustody::try_new_on_control().unwrap();
            let token = custody
                .begin_input_on_control(
                    hex_engine::AudioArrangementInput::new(editor.project(), 11, bank)
                        .with_audio_prepared_revision()
                        .with_effect_registry(&registry),
                )
                .ok()
                .unwrap();
            let mut owner = hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
                .ok()
                .unwrap();
            assert!(owner.requires_effect_graph_custody_on_control().unwrap());
            let summary = owner
                .seal_constructor_on_control(hex_engine::AudioStorageLedgerReservation {
                    data_rows: 4096,
                    handle_rows: 2048,
                    root_rows: 8,
                    max_retained_bytes: usize::MAX,
                    max_construction_peak_bytes: usize::MAX,
                })
                .unwrap();
            assert_eq!(summary.data.unknown_storage, 0);
            Ok(owner.split_constructor_on_control().ok().unwrap())
        })
        .unwrap();
    runtime
        .apply_action(hex_engine::PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    control.publish_native_source_endpoint(binding).unwrap();
    Fixture {
        control,
        data,
        errors,
        retirement,
        binding,
        runtime,
        anchors: [Some(anchor)],
        samples,
    }
}
fn dispose(mut f: Fixture, expected_blocks: u64) {
    f.runtime
        .dispose_on_control()
        .unwrap()
        .record_anchor_cells_on_control(&mut f.anchors)
        .unwrap();
    let anchor = f.anchors[0].as_mut().unwrap();
    anchor.finish_disposal_on_control().unwrap();
    assert!(anchor.recover_inputs_on_control().is_some());
    assert!(f.control.retire_native_source_preparation(f.binding));
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    let returned = recover(&mut f.control, f.binding);
    let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
        panic!("native reap")
    };
    assert!(disposal.status.success());
    let terminal = disposal.terminal.unwrap();
    assert_eq!(
        (terminal.blocks, terminal.frames),
        (expected_blocks, expected_blocks * 8)
    );
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    drop(f.control);
    drop(f.data);
    drop(f.errors);
    finish(&mut f.retirement);
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_endpoint_bank_renders_closed_track_return_main_at_final_host_acceptance() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    for location in 0..3 {
        for bypassed in [false, true] {
            let mut f = fixture(location, bypassed);
            let root = f.control.native_candidate_root();
            let mut output = [[0.0; 2]; 64];
            let mut committed = 0;
            for chunk in output.chunks_mut(5) {
                let mut host = [0.0_f32; 10];
                allocation_free(|| {
                    f.data.owner_mut().unwrap().run(|callback, _| {
                        callback.begin_block();
                        assert_eq!(callback.native_effect_quantum(), Some(8));
                        assert!(crate::device::valid_transport_buffer(
                            callback,
                            chunk.len() * 2,
                            2
                        ));
                        assert!(!crate::device::valid_transport_buffer(callback, 18, 2));
                        assert!(!crate::device::valid_transport_buffer(callback, 3, 2));
                        callback.begin_native_effect_callback(chunk.len()).unwrap();
                        let split = chunk.len().min(2);
                        let rendered = {
                            let mut window = crate::prepared::NativeEffectWindow::new(
                                &mut callback.native_sources,
                                0,
                            );
                            f.runtime.render_with_foreign_processors(
                                &mut chunk[..split],
                                None,
                                None,
                                None,
                                &mut window,
                            )
                        }
                        .and_then(|()| {
                            let mut window = crate::prepared::NativeEffectWindow::new(
                                &mut callback.native_sources,
                                split,
                            );
                            f.runtime.render_with_foreign_processors(
                                &mut chunk[split..],
                                None,
                                None,
                                None,
                                &mut window,
                            )
                        });
                        assert!(rendered.is_ok());
                        assert_eq!(
                            callback.native_sources.test_committed_frames(f.binding),
                            Some(committed)
                        );
                        assert!(crate::device::finish_host_chunk(
                            callback,
                            &mut host[..chunk.len() * 2],
                            chunk,
                            2,
                            &rendered,
                            committed,
                            Instant::now()
                        ));
                        assert_eq!(
                            callback.native_sources.test_committed_frames(f.binding),
                            Some(committed + chunk.len() as u64)
                        );
                    })
                });
                for (actual, expected) in host[..chunk.len() * 2].chunks_exact(2).zip(chunk.iter())
                {
                    assert_eq!(actual, expected);
                }
                committed += chunk.len() as u64;
                let deadline = Instant::now() + Duration::from_secs(3);
                while crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(&root, f.binding).unwrap() < committed / 8 {
                    assert!(Instant::now() < deadline, "actual original Root worker completion");
                    std::thread::yield_now();
                }
            }
            let mut expected = [[0.0; 2]; 64];
            for (index, frame) in f.samples.iter().enumerate() {
                let gain = (if bypassed {
                    1.0
                } else {
                    0.5 + (index / 8) as f32 * 0.125
                }) + if location == 1 { 1.0 } else { 0.0 };
                expected[index + 16] = [frame[0] * gain, frame[1] * gain];
            }
            assert_eq!(
                output, expected,
                "Root endpoint location {location}, bypass {bypassed}"
            );
            dispose(f, 8);
        }
    }
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_endpoint_bank_refuses_final_conversion_and_late_stop_without_dispatch_or_replay() {
    let _serial = serial();
    for stop in [false, true] {
        let mut f = fixture(2, false);
        let callback = &mut f.data.owner_mut().unwrap().callback;
        let mut source = [[0.0; 2]; 8];
        callback.begin_block();
        callback.begin_native_effect_callback(8).unwrap();
        let rendered = f.runtime.render_with_foreign_processors(
            &mut source,
            None,
            None,
            None,
            &mut callback.native_sources,
        );
        assert!(rendered.is_ok());
        assert_eq!(
            callback.native_sources.test_committed_frames(f.binding),
            Some(0)
        );
        if stop {
            f.control.stop();
        } else {
            source[7][0] = f32::NAN;
        }
        let mut host = [9.0_f32; 16];
        allocation_free(|| {
            let valid = crate::device::finish_host_chunk(
                callback,
                &mut host,
                &mut source,
                2,
                &rendered,
                0,
                Instant::now(),
            );
            assert_eq!(valid, stop);
            assert_eq!(host, [0.0; 16]);
            assert_eq!(
                callback.native_sources.test_committed_frames(f.binding),
                Some(0)
            );
            assert!(callback.native_effects_revoked());
        });
        callback.begin_block();
        callback.begin_native_effect_callback(8).unwrap();
        let mut replay = [[9.0; 2]; 8];
        assert!(
            f.runtime
                .render_with_foreign_processors(
                    &mut replay,
                    None,
                    None,
                    None,
                    &mut callback.native_sources
                )
                .is_err()
        );
        assert_eq!(
            callback.native_sources.test_committed_frames(f.binding),
            Some(0)
        );
        dispose(f, 0);
    }
}

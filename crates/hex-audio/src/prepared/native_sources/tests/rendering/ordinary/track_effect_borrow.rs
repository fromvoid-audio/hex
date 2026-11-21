//! Exact publication proofs for a fresh direct Track effect move and the retired compiler experiment.
use super::*;
use hex_lattice::{DevicePreparationContext, PreparedProcessorFactory, TuningTable};
use hex_project::DeviceSelectionIds;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn private_compiler_borrows_original_root_binding_for_the_exact_receiver_chain() {
    let _serial = serial();
    let mut f = fixture_at("processing", 0);
    let donor = f.editor.project().tracks()[0].id();
    let device = f.editor.project().tracks()[0].devices()[0].id();
    let Created::Track(receiver) = f
        .editor
        .execute(Command::AddTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("receiver");
    };
    let asset = *f.media.keys().next().unwrap();
    f.editor
        .execute(Command::PlaceClip {
            track: receiver,
            asset,
            start: 0,
            duration: 32,
            source_offset: 0,
            source_length: 32,
            looping: false,
        })
        .unwrap();
    let playback = compile(&f, true);
    let original_revision = Arc::clone(playback.prepared_revision().unwrap());
    let original = playback.foreign_nodes_on_control().next().unwrap().clone();
    let binding = f.binding;
    let physical =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(donor.value()), device.value())
            .unwrap();
    let logical = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Track(receiver.value()),
        device.value(),
    )
    .unwrap();
    assert!(playback.has_foreign_binding_on_control(physical.owner(), device.value(), binding));
    assert_eq!(playback.foreign_nodes_on_control().count(), 1);
    let revision = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    let root = f.control.native_candidate_root();
    for index in 0..3 {
        let output = block(&mut f);
        replacement::wait_for_blocks(&root, binding, index + 1);
        assert_eq!(
            output,
            if index < 2 {
                [[0.0; 2]; 8]
            } else {
                [[0.375, -0.75]; 8]
            }
        );
    }
    let applied = f.control.status.recording_source().unwrap();
    assert_eq!(applied.audio_revision, revision);
    assert_eq!(applied.output_identity, binding.root);

    let staged = f
        .editor
        .stage_edit(Command::MoveTrackDevices {
            source: donor,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
            destination: receiver,
        })
        .unwrap();
    assert!(staged.project().track(donor).unwrap().devices().is_empty());
    let [moved] = staged.project().track(receiver).unwrap().devices() else {
        panic!("the staged receiver has exactly one effect");
    };
    assert_eq!(moved.id(), device);
    let state = moved.state();
    let saved = hex_plugin_state::decode(state).unwrap();
    let wrong_state = hex_plugin_state::encode(hex_plugin_state::State {
        identity: saved.identity,
        component: Some(b"different processing state"),
        controller: saved.controller,
    })
    .unwrap();
    let tuning = TuningTable::default();
    let preparation_context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let binding_error = "original vst3 effect binding differs";
    let wrong_root = original
        .foreign_processor()
        .unwrap()
        .with_binding(Binding {
            root: binding.root + 1,
            ..binding
        })
        .unwrap();
    let wrong_root = hex_lattice::PreparedRuntimeInstance {
        definition: Some(PreparedProcessorFactory::Foreign(wrong_root)),
        ..original.clone()
    };
    assert_eq!(
        f.control
            .with_retained_track_effect_definition(applied, &wrong_root, logical, |_| panic!(
                "a different Root must not reach the compiler"
            )),
        Err::<(), String>(binding_error.into())
    );
    let stale = AppliedAudioSource {
        audio_revision: revision + 1,
        ..applied
    };
    assert_eq!(
        f.control
            .with_retained_track_effect_definition(stale, &original, logical, |_| panic!(
                "a different installed revision must not reach the compiler"
            )),
        Err::<(), String>("original vst3 effect move source is no longer installed".into())
    );
    let deadline = Instant::now() + Duration::from_millis(500);
    let candidate = loop {
        let selected = f.control.with_retained_track_effect_definition(
            applied,
            &original,
            logical,
            |definition| {
                assert!(
                    definition
                        .prepare_instance_runtime_factory(logical, state, &preparation_context)
                        .is_err(),
                    "the factory still accepts only the original physical identity"
                );
                assert_eq!(
                    definition
                        .retained_foreign_effect_identity(logical, state, &preparation_context)
                        .unwrap(),
                    physical
                );
                let other = ProcessorInstanceIdentity::new(
                    ProcessorChainOwner::Track(receiver.value() + 1),
                    device.value(),
                )
                .unwrap();
                assert_eq!(
                    definition
                        .retained_foreign_effect_identity(other, state, &preparation_context)
                        .unwrap(),
                    other
                );
                assert!(
                    definition
                        .retained_foreign_effect_identity(
                            logical,
                            &wrong_state,
                            &preparation_context,
                        )
                        .is_err()
                );
                let wrong_rate = DevicePreparationContext {
                    sample_rate: 96_000,
                    ..preparation_context
                };
                assert!(
                    definition
                        .retained_foreign_effect_identity(logical, state, &wrong_rate)
                        .is_err()
                );
                let prepared = original.foreign_processor().unwrap();
                let wrong_epoch = prepared
                    .with_binding(Binding {
                        epoch: binding.epoch + 1,
                        ..binding
                    })
                    .unwrap();
                for wrong in [wrong_epoch, prepared.with_instrument_profile()] {
                    let node = hex_lattice::PreparedRuntimeInstance {
                        definition: Some(PreparedProcessorFactory::Foreign(wrong)),
                        ..original.clone()
                    };
                    assert!(
                        definition
                            .including_retained_track_effect(applied, logical, &node)
                            .is_err()
                    );
                }
                prepare_registered_candidate(&f, staged.project(), definition)
            },
        );
        match selected {
            Ok(candidate) => break candidate,
            Err(reason)
                if matches!(
                    reason.as_str(),
                    "native effect preparation is busy" | "native effect selection is busy"
                ) =>
            {
                assert!(Instant::now() < deadline, "private effect compiler borrow");
                std::thread::yield_now();
            }
            Err(reason) => panic!("original Root did not lend the track effect: {reason}"),
        }
    };
    assert_eq!(
        hex_engine::compile::test_retained_track_effect_alignment(
            original_revision.arrangement().as_ref(),
            candidate.arrangement().as_ref(),
        )
        .unwrap(),
        [[0, 16], [16, 0]]
    );
    let compiled =
        Playback::try_from_prepared_with_device_resources(Arc::new(candidate), &f.resources)
            .unwrap();
    let nodes = compiled
        .foreign_nodes_on_control()
        .cloned()
        .collect::<Vec<_>>();
    let [borrowed] = nodes.as_slice() else {
        panic!("one exact logical receiver chain");
    };
    assert_eq!(borrowed.owner, physical.owner());
    assert_eq!(borrowed.device_id, device.value());
    assert_eq!(borrowed.foreign_processor(), original.foreign_processor());
    assert_eq!(borrowed.matrix, original.matrix);
    assert_eq!(borrowed.sample_rate, original.sample_rate);
    assert!(
        borrowed
            .configuration
            .as_ref()
            .unwrap()
            .processing_eq(state)
    );
    root.check_native_borrow_nodes_for_test(&nodes).unwrap();
    let wrong_owner = hex_lattice::PreparedRuntimeInstance {
        owner: logical.owner(),
        ..borrowed.clone()
    };
    assert_eq!(
        root.check_native_borrow_nodes_for_test(&[wrong_owner]),
        Err("native graph configuration or original source differs")
    );
    assert_eq!(
        root.check_native_borrow_nodes_for_test(&[borrowed.clone(), borrowed.clone()]),
        Err("native source appears more than once in graph")
    );
    assert_eq!(f.control.submitted_revision(), revision);
    assert_eq!(f.control.status.recording_source(), Some(applied));
    assert_eq!(block(&mut f), [[0.40625, -0.8125]; 8]);
    replacement::wait_for_blocks(&root, binding, 4);
    assert_eq!(f.control.status.recording_source(), Some(applied));
    dispose(f);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn signed_track_effect_cut_drains_the_old_route_and_starts_an_empty_receiver() {
    verify_track_effect_cut(false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn cancelling_a_signed_track_effect_cut_restores_the_old_route_and_worker() {
    verify_track_effect_cut(true);
}

fn verify_track_effect_cut(cancel_after_fade: bool) {
    use hex_engine::playback::HostedTrackEffectMove;
    const QUANTUM: usize = 256;
    const FRAMES: u64 = 8192;
    const DONOR: Frame = [0.125, -0.0625];
    const RECEIVER: Frame = [0.25, -0.125];
    const PRESET_GAIN: f32 = 0.75;
    let _serial = serial();
    let (_, source, _) = crate::candidate_custody::native_source_test_fixture::input_for(
        "processing",
        [
            0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
        ],
    );
    // The signed processing fixture restores HXPS component state. Bytes
    // 4..8 leave SDK latency at zero, and 8..12 carry its authored gain.
    let mut component = [0; 32];
    component[..4].copy_from_slice(b"HXPS");
    component[8..12].copy_from_slice(&PRESET_GAIN.to_le_bytes());
    let preset = hex_plugin_state::encode(hex_plugin_state::State {
        identity: source.identity(),
        component: Some(&component),
        controller: None,
    })
    .unwrap();
    let mut f = fixture_with_rate(
        "processing",
        0,
        QUANTUM as u16,
        &[],
        false,
        2,
        48_000,
        Some(preset),
    );
    let donor = f.editor.project().tracks()[0].id();
    let device = f.editor.project().tracks()[0].devices()[0].id();
    let clip = f.editor.project().tracks()[0].clips().first().unwrap().id();
    f.editor.execute(Command::RemoveClip(clip)).unwrap();
    let Created::Track(receiver) = f
        .editor
        .execute(Command::AddTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("receiver track");
    };
    for (track, signal) in [(donor, DONOR), (receiver, RECEIVER)] {
        let Created::Asset(asset) = f
            .editor
            .execute(Command::AddAsset {
                path: format!("track-{}.wav", track.value()).into(),
                sample_rate: 48_000,
                frames: FRAMES,
            })
            .unwrap()
        else {
            panic!("audio asset");
        };
        f.media.insert(
            asset,
            hex_engine::Audio::new(48_000, vec![signal; FRAMES as usize]).unwrap(),
        );
        f.editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: FRAMES,
                source_offset: 0,
                source_length: FRAMES,
                looping: false,
            })
            .unwrap();
    }
    let playback = compile(&f, true);
    let previous = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    let root = f.control.native_candidate_root();
    let binding = f.binding;
    let mut completed = 0;
    let mut render = |f: &mut OrdinaryFixture| {
        let pcm = physical_block::<QUANTUM>(f);
        completed += 1;
        replacement::wait_for_blocks(&root, binding, completed);
        (completed, pcm)
    };
    let (_, first) = render(&mut f);
    assert_eq!(first, [[0.0; 2]; QUANTUM]);
    render(&mut f);
    let (_, incumbent) = render(&mut f);
    assert_eq!(incumbent[QUANTUM - 1], [0.296875, -0.1484375]);
    let predecessor = f.control.status.recording_source().unwrap();
    assert_eq!(predecessor.audio_revision, previous);
    let staged = f
        .editor
        .stage_edit(Command::MoveTrackDevices {
            source: donor,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
            destination: receiver,
        })
        .unwrap();
    let logical = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Track(receiver.value()),
        device.value(),
    )
    .unwrap();
    let saved = staged.project().track(receiver).unwrap().devices()[0]
        .state()
        .clone();
    let fresh = source_for_profile(
        &mut f.control,
        logical,
        "processing",
        QUANTUM as u16,
        Some(saved.clone()),
    );
    assert_ne!(fresh, binding);
    assert!(
        f.control
            .native_source_prepared_state(fresh)
            .unwrap()
            .processing_eq(&saved)
    );
    f.control.publish_native_source_endpoint(fresh).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let candidate = loop {
        match f
            .control
            .with_native_effect_definition(&[fresh], |definition| {
                prepare_registered_candidate(&f, staged.project(), definition)
            }) {
            Ok(candidate) => break candidate,
            Err(reason)
                if matches!(
                    reason.as_str(),
                    "native effect preparation is busy" | "native effect selection is busy"
                ) =>
            {
                assert!(Instant::now() < deadline, "fresh receiver effect compiler");
                std::thread::yield_now();
            }
            Err(reason) => panic!("candidate compiler: {reason}"),
        }
    };
    let mut successor =
        Playback::try_from_prepared_with_device_resources(Arc::new(candidate), &f.resources)
            .unwrap();
    let movement = HostedTrackEffectMove {
        donor,
        receiver,
        device,
        binding: fresh,
    };
    assert!(
        successor
            .require_hosted_track_effect_move(HostedTrackEffectMove {
                binding,
                ..movement
            })
            .is_err()
    );
    successor
        .require_hosted_track_effect_move(movement)
        .unwrap();
    let fence = f.control.capture_fresh_runtime_fence().unwrap();
    let context = hex_engine::AudioSourceContext {
        editor_revision: context().editor_revision + 1,
        ..context()
    };
    let revision = publish_in_context(
        &mut f.control,
        successor,
        PublicationPolicy::DeviceInsert { predecessor, fence },
        context,
    );
    let receipt = f.control.fresh_runtime_receipt(revision).unwrap();
    let (_, faded) = render(&mut f);
    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    assert_eq!(f.control.status.recording_source(), Some(predecessor));
    assert!(faded[0][0] > RECEIVER[0]);
    assert_eq!(faded[QUANTUM - 1], RECEIVER);
    if cancel_after_fade {
        assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled);
        for _ in 0..4 {
            render(&mut f);
        }
        assert_eq!(receipt.state(), PublicationReceiptState::Cancelled);
        assert_eq!(f.control.status.recording_source(), Some(predecessor));
        let (block, restored) = render(&mut f);
        let wet_gain = PRESET_GAIN * (0.5 + (block - 3) as f32 * 0.125);
        assert_eq!(
            restored[QUANTUM - 1],
            [
                RECEIVER[0] + DONOR[0] * wet_gain,
                RECEIVER[1] + DONOR[1] * wet_gain,
            ]
        );
        dispose(f);
        return;
    }
    for _ in 0..2 {
        render(&mut f);
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        assert_eq!(f.control.status.recording_source(), Some(predecessor));
    }
    let first_fresh = physical_block::<QUANTUM>(&mut f);
    replacement::wait_for_blocks(&root, fresh, 1);
    assert_eq!(first_fresh, [[0.0; 2]; QUANTUM]);
    assert_eq!(receipt.state(), PublicationReceiptState::Installed);
    assert_eq!(
        f.control.status.recording_source(),
        Some(AppliedAudioSource {
            context,
            audio_revision: revision,
            output_identity: binding.root,
        })
    );
    assert_eq!(physical_block::<QUANTUM>(&mut f), [[0.0; 2]; QUANTUM]);
    replacement::wait_for_blocks(&root, fresh, 2);
    let moved = physical_block::<QUANTUM>(&mut f);
    replacement::wait_for_blocks(&root, fresh, 3);
    let wet_gain = PRESET_GAIN * 0.5;
    assert_eq!(
        moved[QUANTUM - 1],
        [
            DONOR[0] + RECEIVER[0] * wet_gain,
            DONOR[1] + RECEIVER[1] * wet_gain
        ]
    );
    dispose(f);
}

fn prepare_registered_candidate(
    f: &OrdinaryFixture,
    project: &hex_project::Project,
    definition: &crate::plugin_effects::NativeEffectDefinition<'_>,
) -> Result<hex_engine::PreparedRevision, String> {
    hex_engine::AudioCandidateEffects::with_host_definitions(&[definition], |effects| {
        effects.prepare_registered_revision(
            project,
            &f.media,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &f.resources,
            1_000_000,
            &AtomicBool::new(false),
            None,
        )
    })
    .map_err(|error| error.to_string())
}

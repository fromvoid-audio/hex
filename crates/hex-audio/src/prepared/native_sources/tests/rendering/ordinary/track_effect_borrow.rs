//! Prototype an original effect borrow without publishing the moved graph.
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
    let deadline = Instant::now() + Duration::from_millis(500);
    let compiled = loop {
        let selected = f
            .control
            .with_native_effect_definition(&[binding], |definition| {
                assert!(
                    definition
                        .prepare_instance_runtime_factory(logical, state, &preparation_context)
                        .is_err(),
                    "the ordinary compiler cannot select a copied logical identity"
                );
                let probe = |active,
                             requested,
                             node: &hex_lattice::PreparedRuntimeInstance,
                             state: &hex_lattice_contract::NativeDeviceState,
                             context: &DevicePreparationContext<'_>| {
                    definition.probe_retained_track_effect_identity(
                        active,
                        (applied, logical),
                        requested,
                        node,
                        state,
                        context,
                    )
                };
                let resolved =
                    probe(applied, logical, &original, state, &preparation_context).unwrap();
                assert_eq!(resolved, physical);
                let wrong_receiver = ProcessorInstanceIdentity::new(
                    ProcessorChainOwner::Track(receiver.value() + 1),
                    device.value(),
                )
                .unwrap();
                assert!(
                    probe(
                        applied,
                        wrong_receiver,
                        &original,
                        state,
                        &preparation_context
                    )
                    .is_err()
                );
                let stale = AppliedAudioSource {
                    audio_revision: revision + 1,
                    ..applied
                };
                assert!(probe(stale, logical, &original, state, &preparation_context).is_err());
                assert!(
                    probe(
                        applied,
                        logical,
                        &original,
                        &wrong_state,
                        &preparation_context
                    )
                    .is_err()
                );
                let wrong_rate = DevicePreparationContext {
                    sample_rate: 96_000,
                    ..preparation_context
                };
                assert!(probe(applied, logical, &original, state, &wrong_rate).is_err());
                let prepared = original.foreign_processor().unwrap();
                let wrong_epoch = prepared
                    .with_binding(Binding {
                        epoch: binding.epoch + 1,
                        ..binding
                    })
                    .unwrap();
                let wrong_root = prepared
                    .with_binding(Binding {
                        root: binding.root + 1,
                        ..binding
                    })
                    .unwrap();
                for invalid in [wrong_epoch, wrong_root, prepared.with_instrument_profile()] {
                    let impostor = hex_lattice::PreparedRuntimeInstance {
                        definition: Some(PreparedProcessorFactory::Foreign(invalid)),
                        ..original.clone()
                    };
                    assert!(
                        probe(applied, logical, &impostor, state, &preparation_context).is_err()
                    );
                }
                let definitions: [&dyn NativeDeviceDefinition; 1] = [definition];
                let registry = NativeDeviceRegistry::new(&definitions).unwrap();
                hex_engine::compile::test_compile_retained_track_effect_receiver(
                    staged.project(),
                    receiver,
                    resolved,
                    &registry,
                )
                .map_err(|error| error.to_string())
            });
        match selected {
            Ok(chain) => break chain,
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
    let [borrowed] = compiled.as_slice() else {
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
    assert_eq!(
        hex_engine::compile::test_retained_track_effect_alignment(
            original_revision.arrangement().as_ref(),
            staged.project(),
            borrowed,
        )
        .unwrap(),
        [[0, 16], [16, 0]]
    );
    root.check_native_borrow_nodes_for_test(&compiled).unwrap();
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

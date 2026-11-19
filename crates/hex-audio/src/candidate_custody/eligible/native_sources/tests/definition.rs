//! Exact actual Root source selection through the public compiler factory.
use super::*;
use hex_lattice::{DevicePreparationContext, NativeDeviceDefinition, PreparedProcessorFactory};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn root_compiler_borrow_selects_exact_source_epoch_with_equal_device_state() {
    let outer = CandidateRoot::new(2).unwrap();
    outer
        .configure_domains(AudioCandidateDomainReservation {
            max_retained_data_bytes: 64 * 1024 * 1024,
            max_construction_peak_data_bytes: 64 * 1024 * 1024,
        })
        .unwrap();
    outer
        .configure_native_source_preparation(2, source_reservation())
        .unwrap();
    let root = &outer.0.eligible;
    let mut bindings = Vec::new();
    for _ in 0..2 {
        let binding = if let Some(previous) = bindings.last().copied() {
            // Construct another actual child from the exact retained source's
            // copied metadata, with an independent native epoch. Catalog state
            // is neither consulted nor substituted for the source owner.
            let metadata = outer.native_source_metadata(previous).unwrap();
            let selected = metadata.selection();
            let (build, _, preparation) = input();
            let source = native::Source::inspect_on_control(
                selected.module,
                selected.executable,
                ClassId::new(selected.cid).unwrap(),
            )
            .unwrap();
            let state = root.native_source_prepared_state(previous).unwrap();
            let hex_digest = |bytes: [u8; 32]| {
                bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            };
            assert_eq!(
                selected.executable_sha256,
                hex_digest(source.identity().binary.executable_sha256)
            );
            assert_eq!(
                selected.info_sha256,
                hex_digest(source.identity().binary.info_sha256)
            );
            assert_eq!(
                source.identity(),
                hex_plugin_state::decode(&state).unwrap().identity
            );
            outer
                .begin_native_source_preparation(
                    identity(),
                    12,
                    &build,
                    source,
                    preparation,
                    Some(state),
                    native::TrustUserCode::Acknowledged,
                    None,
                )
                .unwrap()
        } else {
            start(root)
        };
        assert_eq!(binding.root, outer.root_id());
        root.complete_native_source(
            binding,
            Some(hex_plugin_host::protocol::PresentationLatency {
                // Declared SDK-fixture presentation inputs, not measured hardware.
                input_frames: 77,
                output_frames: 91,
            }),
        )
        .unwrap();
        root.start_native_effect(binding).unwrap();
        bindings.push(binding);
    }
    let copied = outer.native_source_metadata(bindings[0]).unwrap();
    assert_eq!(copied, outer.native_source_metadata(bindings[1]).unwrap());
    assert!(
        copied.data_bytes().unwrap() <= crate::plugin_effects::EffectSourceMetadata::MAX_DATA_BYTES
    );
    assert_ne!(bindings[0], bindings[1]);
    let state = root.native_source_prepared_state(bindings[0]).unwrap();
    assert!(state.processing_eq(&root.native_source_prepared_state(bindings[1]).unwrap()));
    let tuning = hex_lattice::TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    for binding in &bindings {
        let selected = outer
            .with_native_effect_definition(&[*binding], |definition| {
                assert!(
                    root.work.try_lock().is_err(),
                    "actual owners remain borrowed through compilation"
                );
                assert!(outer.native_source_metadata(*binding).is_err());
                assert!(
                    root.bank.try_lock().is_ok(),
                    "ordinary bank lock must not span compilation"
                );
                let factory = definition
                    .prepare_instance_runtime_factory(identity(), &state, &context)
                    .map_err(|error| error.to_string())?;
                let Some(PreparedProcessorFactory::Foreign(prepared)) = factory else {
                    return Err("actual foreign factory required".into());
                };
                assert!(
                    definition
                        .prepare_instance_runtime_factory(
                            ProcessorInstanceIdentity::new(
                                ProcessorChainOwner::Track(7),
                                identity().device_id()
                            )
                            .unwrap(),
                            &state,
                            &context,
                        )
                        .is_err()
                );
                prepared
                    .binding()
                    .ok_or_else(|| "actual binding required".into())
            })
            .unwrap();
        assert_eq!(
            selected, *binding,
            "equal state must not select another actual source"
        );
    }
    assert!(
        outer
            .with_native_effect_definition(&bindings, |_| Ok(()))
            .is_err(),
        "canonical factory selection cannot contain two epochs of the same device"
    );
    assert!(
        outer
            .with_native_effect_definition(&[bindings[0], bindings[0]], |_| Ok(()))
            .is_err()
    );
    let stale = Binding {
        epoch: bindings[1].epoch + 1,
        ..bindings[1]
    };
    assert!(outer.native_source_metadata(stale).is_err());
    assert!(
        outer
            .with_native_effect_definition::<()>(&[stale], |_| panic!(
                "stale selection must not compile"
            ))
            .is_err()
    );
    assert!(root.retire_native_source(bindings[0]));
    assert!(outer.native_source_metadata(bindings[0]).is_err());
    assert_eq!(copied, outer.native_source_metadata(bindings[1]).unwrap());
    assert!(
        outer
            .with_native_effect_definition(&[bindings[0]], |_| Ok(()))
            .is_err()
    );
    assert!(
        outer
            .with_native_effect_definition(&[bindings[1]], |_| Ok(()))
            .is_ok(),
        "unrelated current source remains selectable"
    );
    outer.announce_retirement();
    assert!(outer.native_source_metadata(bindings[1]).is_err());
    assert_eq!(
        outer.complete_native_source_preparation(bindings[1], None),
        Err("native source control is closing")
    );
    assert_eq!(
        outer.start_native_effect(bindings[1]),
        Err("native source control is closing")
    );
    let (build, source, preparation) = input();
    assert_eq!(
        outer.begin_native_source_preparation(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged
        ),
        Err("native source control is closing")
    );
    assert!(
        outer
            .with_native_effect_definition::<()>(&[bindings[1]], |_| panic!(
                "retiring Root must not compile an existing source"
            ))
            .is_err()
    );
    dispose(root, bindings[0]);
    outer
        .recover_native_source_preparation(bindings[0])
        .unwrap();
    outer.close();
    assert!(
        outer
            .with_native_effect_definition(&[bindings[1]], |_| Ok(()))
            .is_err()
    );
    dispose(root, bindings[1]);
    outer
        .recover_native_source_preparation(bindings[1])
        .unwrap();
    assert!(root.complete());
}

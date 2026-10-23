//! Exact actual Root source selection through the public compiler factory.
use super::*;
use hex_lattice::{DevicePreparationContext, NativeDeviceDefinition, PreparedProcessorFactory};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn root_compiler_borrow_selects_exact_source_epoch_with_equal_device_state() {
    let outer = CandidateRoot::new(2).unwrap();
    outer
        .configure_domains(AudioCandidateDomainReservation {
            max_retained_data_bytes: 8 * 1024 * 1024,
            max_construction_peak_data_bytes: 8 * 1024 * 1024,
        })
        .unwrap();
    outer.configure_native_source_preparation(2).unwrap();
    let root = &outer.0.eligible;
    let mut bindings = Vec::new();
    for _ in 0..2 {
        let binding = start(root);
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
    assert!(
        outer
            .with_native_effect_definition::<()>(&[stale], |_| panic!(
                "stale selection must not compile"
            ))
            .is_err()
    );
    assert!(root.retire_native_source(bindings[0]));
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

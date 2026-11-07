//! Actual Root-held source/worker/endpoint construction, without publication.
use super::*;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_effect_cannot_spend_beyond_its_native_pool_cell() {
    let root = root();
    let (build, source, _) = input();
    let limit = source
        .preparation_storage_bound_on_control(&build, None)
        .unwrap()
        .max_retained_data_bytes();
    root.configure_native_sources(
        1,
        NativeSourceDataReservation {
            max_retained_data_bytes: limit,
            // Admit the checked startup bound, then refuse the larger actual
            // worker/queue construction at this same retained allowance.
            max_construction_peak_data_bytes: source_reservation().max_construction_peak_data_bytes,
        },
    )
    .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    {
        let work = root.work.try_lock().unwrap();
        let Some(Owner::Prepared(instance)) = work.sources.cells[0].owner.as_ref() else {
            panic!("original prepared source");
        };
        assert!(instance.retained_storage_on_control().unwrap().data_bytes < limit);
        assert!(
            crate::plugin_worker::NativeWorker::preconstruction_data_bytes_on_control(instance)
                .unwrap()
                > limit
        );
    }
    let held = root.usage();
    root.start_native_effect(binding).unwrap();
    assert_eq!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Retiring)
    );
    assert!(
        root.native_source_error(binding)
            .unwrap()
            .contains("data reservation exceeded")
    );
    {
        let work = root.work.try_lock().unwrap();
        let Some(Owner::WorkerRefused(failure)) = work.sources.cells[0].owner.as_ref() else {
            panic!("original source must remain in refusal custody");
        };
        assert_eq!(failure.binding(), binding);
    }
    assert_eq!(root.usage(), held);
    assert!(root.native_source_prepared_state(binding).is_none());
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    assert!(
        returned
            .error
            .unwrap()
            .to_string()
            .contains("data reservation exceeded")
    );
    assert!(returned.cleanup_error.is_none());
    assert!(root.complete());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_constructs_and_retires_the_original_unsplit_native_effect() {
    for close_during_construction in [false, true] {
        let root = root();
        root.configure_native_sources(1, source_reservation())
            .unwrap();
        let metadata = root.actual_control_backing_bytes().unwrap();
        let binding = start(&root);
        assert!(
            root.start_native_effect(binding).is_err(),
            "Pending is not Prepared"
        );
        // The original SDK fixture's declared presentation inputs exercise the
        // handshake. These are not measured physical output-device IAP facts
        // and cannot grant native graph/publication admission.
        root.complete_native_source(
            binding,
            Some(hex_plugin_host::protocol::PresentationLatency {
                input_frames: 77,
                output_frames: 91,
            }),
        )
        .unwrap();
        let state = root.native_source_prepared_state(binding).unwrap();
        root.start_native_effect_between(binding, None, || {
            if close_during_construction {
                root.close();
            }
        })
        .unwrap();
        if !close_during_construction {
            let Some(NativeSourcePreparationProgress::Effect { prepared }) =
                root.native_source_progress(binding)
            else {
                panic!("actual unsplit effect");
            };
            assert_eq!(prepared.binding(), Some(binding));
            assert_eq!(prepared.quantum_frames(), 8);
            assert_eq!(prepared.total_latency_frames(), 16);
            assert_eq!(root.native_source_prepared_state(binding), Some(state));
            let data = {
                let work = root.work.try_lock().unwrap();
                let Some(Owner::Effect(effect)) = work.sources.cells[0].owner.as_ref() else {
                    panic!("actual Root-owned effect");
                };
                assert_eq!(effect.binding(), binding);
                assert_eq!(effect.identity(), identity());
                let transport = effect.transport_storage_on_control().unwrap();
                assert_eq!(transport.shared_groups, 3);
                assert_eq!(transport.shared_handle_bound, 7);
                assert!(transport.data_bytes > 0);
                let source = effect.retained_source_storage_on_control().unwrap();
                assert_eq!(source.shared_groups, 3);
                assert_eq!(source.ipc_handle_bound, 2);
                assert_eq!(source.native_child_lifetime_bound, 1);
                let constructor = effect.constructor_storage_on_control().unwrap();
                assert_eq!(
                    constructor.dry_data_bytes,
                    16 * std::mem::size_of::<[f32; 2]>()
                );
                assert_eq!(constructor.state_alias_handles, 1);
                assert_eq!(
                    constructor.data_bytes(),
                    Some(
                        constructor.worker.data_bytes
                            + constructor.dry_data_bytes
                            + constructor.source_metadata_data_bytes
                    )
                );
                assert!(constructor.source_metadata_data_bytes > 128);
                assert_eq!(
                    constructor.shared_handle_bound(),
                    Some(constructor.worker.shared_handle_bound + 1)
                );
                // The existing factory borrows this exact actual owner; copied
                // latency/state facts alone cannot construct the definition.
                let _definition = crate::plugin_effects::NativeEffectDefinition::new(
                    std::slice::from_ref(effect),
                )
                .unwrap();
                constructor.data_bytes().unwrap()
            };
            assert!(root.start_native_effect(binding).is_err());
            assert_eq!(root.actual_control_backing_bytes(), Some(metadata + data));
            {
                let mut bank = lock(&root.bank);
                bank.sources.slots[0].binding.as_mut().unwrap().epoch += 1;
            }
            assert_eq!(
                root.actual_control_backing_bytes(),
                None,
                "copied/stale facts cannot replace the original owner"
            );
            lock(&root.bank).sources.slots[0].binding = Some(binding);
            assert_eq!(root.actual_control_backing_bytes(), Some(metadata + data));
            assert!(root.retire_native_source(binding));
        } else {
            assert_eq!(
                root.native_source_progress(binding),
                Some(NativeSourcePreparationProgress::Retiring)
            );
        }
        dispose(&root, binding);
        let returned = root.recover_native_source(binding).unwrap();
        let PreparationDisposal::Reaped(native) = returned.disposal else {
            panic!("actual native process retirement");
        };
        if close_during_construction {
            assert!(native.terminal.is_none());
            assert!(returned.error.unwrap().to_string().contains("cancelled"));
        } else {
            assert!(native.status.success());
            assert_eq!(native.terminal.unwrap().blocks, 0);
            assert!(returned.error.is_none());
        }
        assert!(root.complete());
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_effect_constructor_refusal_retains_the_source_until_observed_disposal() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let (build, source, preparation) = input_for(
        "effect",
        [
            0x48, 0x45, 0x58, 0x33, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x10, 0x11, 0,
        ],
    );
    let binding = root
        .begin_native_source(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    root.complete_native_source(binding, None).unwrap();
    root.start_native_effect(binding).unwrap(); // Retained refusal, never effect acceptance.
    assert_eq!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Retiring)
    );
    assert!(
        root.native_source_error(binding)
            .unwrap()
            .contains("component")
    );
    {
        let work = root.work.try_lock().unwrap();
        let Some(Owner::WorkerRefused(failure)) = work.sources.cells[0].owner.as_ref() else {
            panic!("actual retained constructor failure");
        };
        assert_eq!(failure.binding(), binding);
    }
    assert!(root.native_source_prepared_state(binding).is_none());
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    assert!(returned.error.unwrap().to_string().contains("component"));
    assert!(root.complete());
}

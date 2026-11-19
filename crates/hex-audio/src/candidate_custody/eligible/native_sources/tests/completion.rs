//! Actual SDK completion and unchanged-source cancellation inside Root.
use super::*;
use hex_plugin_host::protocol::{Facts, Presentation, PresentationPlan};

fn absent_input() -> (InstanceBuild, native::Source, native::Preparation) {
    input_for(
        "effect",
        [
            0x48, 0x45, 0x58, 0x33, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x10, 0x11, 0,
        ],
    )
}
fn start_absent(root: &Custody, restore: Option<NativeDeviceState>) -> Binding {
    let (build, source, preparation) = absent_input();
    root.begin_native_source(
        identity(),
        12,
        &build,
        source,
        preparation,
        restore,
        native::TrustUserCode::Acknowledged,
    )
    .unwrap()
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_final_ack_retains_prepared_source_and_original_state_until_disposal() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start_absent(&root, None);
    assert!(root.native_source_prepared_state(binding).is_none());
    assert!(
        root.complete_native_source(
            Binding {
                epoch: binding.epoch + 1,
                ..binding
            },
            None
        )
        .is_err()
    );
    assert!(matches!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Facts(_))
    ));
    root.complete_native_source(binding, None).unwrap();
    assert!(matches!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Prepared {
            facts: Facts {
                presentation_available: false,
                restored: false,
                ..
            },
            presentation: Presentation::Absent,
        })
    ));
    let state = root.native_source_prepared_state(binding).unwrap();
    {
        let work = root.work.try_lock().unwrap();
        let Some(Owner::Prepared(actual)) = work.sources.cells[0].owner.as_ref() else {
            panic!("actual prepared lease");
        };
        assert_eq!(actual.configuration().binding, binding);
        assert!(actual.child_id_on_control().is_some());
        assert_eq!(actual.completed_blocks_on_control(), 0);
        assert_eq!(&state, &actual.ready().initial_state);
    }
    assert!(
        root.complete_native_source(binding, None).is_err(),
        "final ACK is single use"
    );
    assert_eq!(
        root.actual_control_backing_bytes(),
        None,
        "ACK cannot seal publication resources"
    );
    assert!(root.retire_native_source(binding));
    assert!(root.native_source_prepared_state(binding).is_none());
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));

    // This fixture cannot restore a missing component chunk. Its capture stays
    // intact; validation refuses before spawn rather than using a fresh/default load.
    let original = state.clone();
    let restored = start_absent(&root, Some(state.clone()));
    assert_eq!(
        root.native_source_progress(restored),
        Some(NativeSourcePreparationProgress::Retiring)
    );
    assert!(root.complete_native_source(restored, None).is_err());
    assert!(root.native_source_prepared_state(restored).is_none());
    dispose(&root, restored);
    let returned = root.recover_native_source(restored).unwrap();
    assert!(returned.error.unwrap().to_string().contains("component"));
    assert!(matches!(returned.disposal, PreparationDisposal::NoInstance));
    assert_eq!(state, original);
    assert!(root.complete());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_missing_required_presentation_retains_refusal_instead_of_prepared_source() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(binding, None).unwrap();
    assert_eq!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Retiring)
    );
    assert!(root.native_source_prepared_state(binding).is_none());
    assert!(
        root.native_source_error(binding)
            .unwrap()
            .contains("presentation")
    );
    assert!(root.complete_native_source(binding, None).is_err());
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    assert!(returned.error.unwrap().to_string().contains("presentation"));
    assert!(root.complete());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_retains_explicitly_unknown_presentation_receipt() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(binding, PresentationPlan::AllowUnknown)
        .unwrap();
    assert!(matches!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Prepared {
            facts: Facts {
                presentation_available: true,
                ..
            },
            presentation: Presentation::Unknown,
        })
    ));
    assert!(root.retire_native_source(binding));
    dispose(&root, binding);
    assert!(matches!(
        root.recover_native_source(binding).unwrap().disposal,
        PreparationDisposal::Reaped(_)
    ));
    assert!(root.complete());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_close_during_completion_cancels_exact_source_without_prepared_acceptance() {
    for close_after_ack in [false, true] {
        let root = root();
        root.configure_native_sources(1, source_reservation())
            .unwrap();
        let binding = start_absent(&root, None);
        root.complete_native_source_between(
            binding,
            None,
            || {
                assert!(root.work.try_lock().is_err());
                assert_eq!(
                    root.native_source_progress(binding),
                    Some(NativeSourcePreparationProgress::Preparing)
                );
                if !close_after_ack {
                    root.close();
                }
            },
            || {
                if close_after_ack {
                    assert_eq!(
                        root.native_source_progress(binding),
                        Some(NativeSourcePreparationProgress::Preparing)
                    );
                    root.close();
                }
            },
        )
        .unwrap();
        assert_eq!(
            root.native_source_progress(binding),
            Some(NativeSourcePreparationProgress::Retiring)
        );
        assert!(root.native_source_prepared_state(binding).is_none());
        dispose(&root, binding);
        let returned = root.recover_native_source(binding).unwrap();
        assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
        if close_after_ack {
            assert!(
                returned.error.is_none(),
                "late final ACK is retained for cleanup, never accepted"
            );
        } else {
            assert!(returned.error.unwrap().to_string().contains("cancelled"));
        }
        assert!(root.complete());
    }
}

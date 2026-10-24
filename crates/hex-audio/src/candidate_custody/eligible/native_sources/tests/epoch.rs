//! Original Root cell/endpoint/lease renewal; full output controls live at prepared.
use super::*;

fn presentation() -> Option<hex_plugin_host::protocol::PresentationLatency> {
    Some(hex_plugin_host::protocol::PresentationLatency {
        input_frames: 77,
        output_frames: 91,
    })
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_epoch_requires_original_endpoint_return_and_matching_resume_ack() {
    let root = root();
    root.configure_native_sources(2, source_reservation())
        .unwrap();
    let (build, source, preparation) = input();
    let other = root
        .begin_native_source(
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 3).unwrap(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    root.complete_native_source(other, presentation()).unwrap();
    root.start_native_effect(other).unwrap();
    let other_state = root.native_source_prepared_state(other).unwrap();
    let mut binding = start(&root);
    root.complete_native_source(binding, presentation())
        .unwrap();
    let child = {
        let work = lock(&root.work);
        let Some(Owner::Prepared(owner)) = work.sources.cells[1].owner.as_ref() else {
            panic!("actual prepared child")
        };
        owner.child_id_on_control()
    };
    let state = root.native_source_prepared_state(binding).unwrap();
    root.start_native_effect(binding).unwrap();
    for _ in 0..3 {
        let endpoint = root.loan_native_endpoint(binding).unwrap();
        assert!(
            root.quiesce_native_source(binding, binding.graph + 1)
                .is_err()
        );
        assert_eq!(lock(&root.bank).sources.slots[1].binding, Some(binding));
        assert!(root.ingest_native_endpoint(endpoint).is_ok());
        assert!(root.quiesce_native_source(binding, 0).is_err());
        assert!(
            root.quiesce_native_source(binding, binding.graph - 1)
                .is_err()
        );
        let next = root
            .quiesce_native_source(binding, binding.graph + 1)
            .unwrap();
        assert_eq!((next.root, next.instance), (binding.root, binding.instance));
        assert!(next.epoch > binding.epoch);
        assert!(root.native_source_progress(binding).is_none());
        assert!(!root.retire_native_source(binding));
        assert!(matches!(
            root.native_source_progress(next),
            Some(NativeSourcePreparationProgress::Facts(_))
        ));
        assert!(root.native_source_prepared_state(next).is_none());
        assert!(root.loan_native_endpoint(next).is_err());
        assert!(root.start_native_effect(next).is_err());
        assert!(
            root.with_native_effect_definition(&[next], |_| Ok(()))
                .is_err()
        );
        {
            let work = lock(&root.work);
            let Some(Owner::Quiescent(owner)) = work.sources.cells[1].owner.as_ref() else {
                panic!("actual stopped source")
            };
            assert_eq!(
                owner.child_id_on_control(),
                child,
                "diagnostic alongside retained original owner"
            );
            assert_eq!(owner.completed_blocks(), 0);
        }
        assert_eq!(
            root.native_source_prepared_state(other).as_ref(),
            Some(&other_state)
        );
        assert!(
            root.with_native_effect_definition(&[other], |_| Ok(()))
                .is_ok()
        );
        let unrelated_endpoint = root.loan_native_endpoint(other).unwrap();
        assert!(root.ingest_native_endpoint(unrelated_endpoint).is_ok());
        root.complete_native_source(next, presentation()).unwrap();
        assert_eq!(root.native_source_prepared_state(next), Some(state.clone()));
        root.start_native_effect(next).unwrap();
        assert!(matches!(
            root.native_source_progress(next),
            Some(NativeSourcePreparationProgress::Effect { .. })
        ));
        binding = next;
    }
    assert!(root.retire_native_source(binding));
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.success() && retired.terminal.is_some())
    );
    assert!(root.retire_native_source(other));
    dispose(&root, other);
    let returned = root.recover_native_source(other).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some_and(|t| t.blocks == 0 && t.frames == 0))
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_epoch_completion_refusal_and_late_close_retain_disposal() {
    for mode in 0..3 {
        let root = root();
        root.configure_native_sources(1, source_reservation())
            .unwrap();
        let old = start(&root);
        root.complete_native_source(old, presentation()).unwrap();
        root.start_native_effect(old).unwrap();
        let next = root
            .quiesce_native_source_between(old, old.graph + 1, || {
                if mode == 1 {
                    root.close();
                }
            })
            .unwrap();
        if mode == 0 {
            // The actual native facts require presentation; refusal is retained,
            // never an accepted resumed effect or an implicit retry.
            root.complete_native_source(next, None).unwrap();
        }
        if mode == 2 {
            assert!(root.retire_native_source(next));
            assert!(root.complete_native_source(next, presentation()).is_err());
        }
        assert!(root.start_native_effect(next).is_err());
        dispose(&root, next);
        let returned = root.recover_native_source(next).unwrap();
        if mode != 2 {
            assert!(returned.error.is_some());
        }
        assert!(
            matches!(returned.disposal, PreparationDisposal::Reaped(retired) if !retired.status.success() && retired.terminal.is_none())
        );
    }
}

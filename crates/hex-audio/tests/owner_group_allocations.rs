//! Actual sealed owners, no backend/device or successful admission fixture.
#![cfg(feature = "allocation-probe")]
use allocation_counter::measure;
use hex_engine::{
    PreparedRevision, SineTable,
    storage_report::{ClaimReservation, incomplete_owned_pair_for_test},
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn tracked_owner_group_handoff_moves_persistent_owners_without_allocation_free_or_finaldrop() {
    let make = || {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(hex_project::Command::AddTrack {
                name: "owned".into(),
            })
            .unwrap();
        let root = PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap()
        .into_owned_shared();
        let (mut owner, anchors) = incomplete_owned_pair_for_test(root).ok().unwrap();
        owner.prepare_owner_groups_for_test().unwrap();
        owner
            .try_acquire_owner_groups_on_control(
                &anchors,
                ClaimReservation {
                    items: 32_768,
                    anchors: 16,
                    max_prepared_data_bytes: usize::MAX,
                    max_construction_data_bytes: usize::MAX,
                    other_retained_bytes: 0,
                    other_construction_bytes: 0,
                },
            )
            .unwrap()
    };
    let (previous, archive, _) = make();
    let (mut candidate, _, _) = make();
    let previous_identity = previous.identity();
    let mut previous = Some(previous);
    let mut retired = None;
    let mut manifest = None;
    let mut result = None;
    let measured = measure(|| {
        result = Some(candidate.try_inherit_and_retire_in_place(
            &mut previous,
            &mut retired,
            &mut manifest,
        ));
    });
    result.take().unwrap().unwrap();
    assert_eq!(measured.count_total, 0);
    assert_eq!(measured.count_current, 0);
    assert_eq!(measured.bytes_total, 0);
    assert_eq!(measured.bytes_current, 0);
    assert_eq!(retired.as_ref().unwrap().identity(), previous_identity);
    assert!(previous.is_none());
    assert!(!archive.original_runtime_was_disposed());
    assert!(archive.remaining_handle_count() > 0);
    assert!(manifest.as_ref().unwrap().is_incomplete_fixture());
    let positive = measure(|| {
        let value = std::hint::black_box(Box::new(std::hint::black_box(7_u64)));
        std::hint::black_box(&value);
        drop(value);
    });
    assert_eq!(positive.count_total, 1);
    assert_eq!(positive.count_current, 0);
    assert_eq!(positive.bytes_total, 8);
}

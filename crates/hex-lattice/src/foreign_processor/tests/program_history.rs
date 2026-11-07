//! Control-side pruning preserves the worker's unfinished version and newer graphs.
use super::*;

const BINDING: ForeignProcessorBinding = ForeignProcessorBinding {
    root: 1,
    instance: 2,
    graph: 3,
    epoch: 4,
};

struct Value(f64);
impl ForeignAutomationProgram for Value {
    fn render_quantum(
        &self,
        _: &[Option<u64>],
        _: &[u16],
        output: &mut [ForeignParameterPoint],
    ) -> Result<usize, DeviceContractError> {
        output[0] = ForeignParameterPoint {
            id: 0,
            frame: 0,
            normalized: self.0,
        };
        Ok(1)
    }
}

fn key(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}
fn program(value: u64) -> PreparedForeignAutomation {
    PreparedForeignAutomation::new(BINDING, key(value), Value(value as f64 / 1000.0), 0).unwrap()
}
fn assert_value(program: &PreparedForeignAutomation, revision: u64) {
    let mut points = [ForeignParameterPoint::default(); 1];
    assert_eq!(
        program
            .borrow()
            .render_revision(key(revision), &[Some(0)], &[], &mut points),
        Ok(1)
    );
    assert_eq!(points[0].normalized, revision as f64 / 1000.0);
}

#[test]
fn received_frontier_advances_without_losing_unfinished_or_newer_versions() {
    let mut previous = program(1);
    let bytes = previous.storage_bytes();
    for revision in 2u64..=64 {
        // Model receipt lagging behind two prepared revisions. Numeric gaps
        // from unused source reservations do not alter the frontier rule.
        let received = revision.saturating_sub(3).max(1);
        let original = previous.clone();
        let mut next = program(revision);
        next.retain_received_on_control(&previous, key(received))
            .unwrap();
        assert!(previous.shares_storage_with(&original));
        let count = usize::try_from(revision - received + 1).unwrap();
        assert_eq!(next.storage_owners().count(), count);
        assert_eq!(next.storage_bytes(), count * bytes);
        for retained in received..=revision {
            assert_value(&next, retained);
        }
        if received > 1 {
            assert!(!next.borrow().contains_revision(key(received - 1)));
            assert_value(&original, received - 1);
        }
        previous = next;
    }
    let mut skipped = program(100);
    skipped
        .retain_received_on_control(&previous, key(64))
        .unwrap();
    assert_eq!(skipped.storage_owners().count(), 2);
    assert_value(&skipped, 64);
    assert_value(&skipped, 100);
}

#[test]
fn no_progress_or_invalid_frontier_refuses_without_changing_candidate() {
    let mut previous = program(1);
    for revision in 2..=9 {
        let mut next = program(revision);
        next.retain_previous_on_control(&previous).unwrap();
        previous = next;
    }
    for (revision, received, other_source) in [
        (10, 1, false),  // All eight historical slots remain required.
        (10, 10, false), // Queued/current compiler keys are not receipt evidence.
        (9, 9, false),   // A reused key cannot identify different contents.
        (8, 8, false),   // Source revisions cannot move backwards.
        (10, 9, true),
    ] {
        let mut candidate = program(revision);
        if other_source {
            candidate.binding.epoch += 1;
        }
        let original = candidate.clone();
        assert!(
            candidate
                .retain_received_on_control(&previous, key(received))
                .is_err()
        );
        assert!(candidate.shares_storage_with(&original));
        assert_eq!(candidate.storage_bytes(), original.storage_bytes());
        assert_value(&candidate, revision);
    }
    let mut next = program(10);
    next.retain_received_on_control(&previous, key(2)).unwrap();
    assert_eq!(next.storage_owners().count(), 9);
    assert_value(&next, 2);
    assert_value(&previous, 1);
}

#[test]
fn pruning_existing_candidate_is_atomic_and_keeps_allocation_keys_exact() {
    let mut old = program(1);
    for revision in 2..=8 {
        let mut next = program(revision);
        next.retain_previous_on_control(&old).unwrap();
        old = next;
    }
    let mut candidate = program(20);
    candidate.retain_previous_on_control(&old).unwrap();
    let mut previous = program(17);
    for revision in 18..=19 {
        let mut next = program(revision);
        next.retain_previous_on_control(&previous).unwrap();
        previous = next;
    }
    candidate
        .retain_received_on_control(&previous, key(17))
        .unwrap();
    assert_eq!(candidate.storage_owners().count(), 4);
    assert_eq!(
        candidate.storage_bytes(),
        candidate.storage_owners().map(|(_, bytes)| bytes).sum()
    );
    assert_eq!(old.storage_owners().count(), 8);
    assert_value(&old, 1);
    assert_value(&candidate, 17);
    let mut conflicting = program(21);
    conflicting
        .retain_previous_on_control(&program(17))
        .unwrap();
    let original = conflicting.clone();
    assert!(
        conflicting
            .retain_received_on_control(&previous, key(17))
            .is_err()
    );
    assert!(conflicting.shares_storage_with(&original));
    // A nonmonotonic history assembled through the generic API is not a
    // source-issued chain and cannot justify dropping older programs.
    let mut malformed = program(5);
    malformed.retain_previous_on_control(&program(6)).unwrap();
    let mut next = program(7);
    assert!(next.retain_received_on_control(&malformed, key(5)).is_err());
    assert_eq!(next.storage_owners().count(), 1);
}

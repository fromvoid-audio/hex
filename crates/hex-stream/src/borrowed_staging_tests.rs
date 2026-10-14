use super::*;
use hex_project::{Command, Created, Editor, Project};
fn fixture() -> (Vec<BorrowedSourcePath>, Vec<StreamedSourceMetadata>) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut paths = Vec::with_capacity(32);
    let mut facts = Vec::new();
    for name in ["first.wav", "second.wav"] {
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: name.into(),
                sample_rate: 48_000,
                frames: 8192,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let key = StreamSourceKey::new(91, asset);
        let mut path = OsString::with_capacity(2048);
        path.push(name);
        paths.push((
            key,
            path.into(),
            VerifiedWavReaderLimits::new(
                hex_media::WavReaderLimits::new(65536, 8192, 64, 8192),
                4096,
            ),
        ));
        facts.push(
            StreamedSourceMetadata::new(
                key,
                48000,
                48000,
                8192,
                8192,
                hex_project::IDENTITY_CONVERSION,
                [1; 32],
            )
            .unwrap(),
        );
    }
    (paths, facts)
}
fn budget(paths: &Vec<BorrowedSourcePath>) -> CompactCopyReservations {
    CompactCopyReservations {
        max_copy_bytes: 65536,
        max_overlap_bytes: 65536,
        other_retained_bytes: paths.capacity() * std::mem::size_of::<BorrowedSourcePath>(),
        other_construction_bytes: 0,
    }
}
fn identities(paths: &Vec<BorrowedSourcePath>) -> (usize, usize, Vec<(usize, usize, PathBuf)>) {
    (
        paths.as_ptr() as usize,
        paths.capacity(),
        paths
            .iter()
            .map(|p| {
                (
                    p.1.as_os_str().as_encoded_bytes().as_ptr() as usize,
                    p.1.capacity(),
                    p.1.clone(),
                )
            })
            .collect(),
    )
}
#[test]
fn every_reservation_failure_preserves_exact_original_owners() {
    for failure in 0..4 {
        let (mut paths, facts) = fixture();
        let old = identities(&paths);
        let limits = budget(&paths);
        let mut attempt = 0;
        let result = prepare(
            &mut paths,
            |key| facts.iter().find(|m| m.key() == key),
            StreamConfig::with_pool_pages(2),
            limits,
            || {
                let current = attempt;
                attempt += 1;
                if current == failure {
                    Err(CompactCopyError::AllocationFailed)
                } else {
                    Ok(())
                }
            },
        );
        let Err(error) = result else {
            panic!("injected allocation must refuse")
        };
        assert_eq!(error.reason, CompactCopyError::AllocationFailed);
        assert_eq!(identities(&paths), old);
        assert_eq!(error.observed.originals.path_capacity_bytes, 4096);
        assert_eq!(
            error.observed.originals.source_slots.capacity,
            if failure == 0 { 0 } else { 2 }
        );
        assert_eq!(
            error.observed.compact.source_slots.capacity,
            if failure < 2 { 0 } else { 2 }
        );
    }
}
#[test]
fn commit_moves_only_after_copy_and_preserves_recovery_capacities() {
    let (mut paths, facts) = fixture();
    let old = identities(&paths);
    let limits = budget(&paths);
    let staged = CompactSourceInput::prepare_borrowed(
        &mut paths,
        |key| facts.iter().find(|m| m.key() == key),
        StreamConfig::with_pool_pages(2),
        limits,
    )
    .unwrap();
    assert!(
        staged.report().compact.path_capacity_bytes < staged.report().originals.path_capacity_bytes
    );
    let mut committed = None;
    let allocation = allocation_counter::measure(|| committed = Some(staged.commit()));
    assert_eq!(allocation.count_total, 0);
    assert_eq!(allocation.count_current, 0);
    assert_eq!(allocation.bytes_total, 0);
    assert_eq!(allocation.bytes_current, 0);
    let input = committed.unwrap();
    let positive = allocation_counter::measure(|| {
        let value = Box::new([1u8; 1024]);
        std::hint::black_box(&value);
        drop(value);
    });
    assert!(positive.count_total > 0 && positive.bytes_total >= 1024);
    assert_eq!(positive.count_current, 0);
    assert_eq!(input.compact_sources().unwrap()[0].path, old.2[0].2);
    assert!(paths.iter().all(|p| p.1.as_os_str().is_empty()));
    for (target, source) in paths.iter_mut().zip(input.into_originals()) {
        target.1 = source.path;
    }
    assert_eq!(identities(&paths), old);
}
#[test]
fn dropped_plan_capacity_overflow_duplicate_and_metadata_refusals_leave_originals() {
    let (mut paths, facts) = fixture();
    let old = identities(&paths);
    let limits = budget(&paths);
    drop(
        CompactSourceInput::prepare_borrowed(
            &mut paths,
            |key| facts.iter().find(|m| m.key() == key),
            StreamConfig::with_pool_pages(2),
            limits,
        )
        .unwrap(),
    );
    assert_eq!(identities(&paths), old);
    for reservation in [
        CompactCopyReservations {
            max_copy_bytes: 1,
            ..limits
        },
        CompactCopyReservations {
            other_retained_bytes: usize::MAX,
            ..limits
        },
    ] {
        assert!(
            CompactSourceInput::prepare_borrowed(
                &mut paths,
                |key| facts.iter().find(|m| m.key() == key),
                StreamConfig::with_pool_pages(2),
                reservation
            )
            .is_err()
        );
        assert_eq!(identities(&paths), old);
    }
    assert!(
        CompactSourceInput::prepare_borrowed(
            &mut paths,
            |_| None,
            StreamConfig::with_pool_pages(2),
            limits
        )
        .is_err()
    );
    assert_eq!(identities(&paths), old);
    paths[1].0 = paths[0].0;
    let changed = identities(&paths);
    let Err(error) = CompactSourceInput::prepare_borrowed(
        &mut paths,
        |key| facts.iter().find(|m| m.key() == key),
        StreamConfig::with_pool_pages(2),
        limits,
    ) else {
        panic!("duplicate")
    };
    assert_eq!(error.reason, CompactCopyError::DuplicateKey);
    assert_eq!(identities(&paths), changed);
}
#[test]
fn late_metadata_refusal_and_empty_plan_do_not_move_or_invent_allocations() {
    let (mut paths, facts) = fixture();
    let old = identities(&paths);
    let limits = budget(&paths);
    let mut calls = 0;
    let Err(error) = CompactSourceInput::prepare_borrowed(
        &mut paths,
        |key| {
            calls += 1;
            if calls == 4 {
                None
            } else {
                facts.iter().find(|m| m.key() == key)
            }
        },
        StreamConfig::with_pool_pages(2),
        limits,
    ) else {
        panic!("late validation must refuse")
    };
    assert_eq!(error.reason, CompactCopyError::KeyMetadataMismatch);
    assert!(error.observed.compact.path_capacity_bytes > 0);
    assert_eq!(identities(&paths), old);
    let mut paths = Vec::with_capacity(32);
    let pointer = paths.as_ptr();
    let limits = budget(&paths);
    let plan = prepare(
        &mut paths,
        |_| None,
        StreamConfig::with_pool_pages(2),
        limits,
        || panic!("empty plan has no allocations"),
    )
    .unwrap();
    assert_eq!(plan.report().originals.source_slots.capacity, 0);
    assert_eq!(plan.report().compact.source_slots.capacity, 0);
    assert!(plan.commit().compact_sources().unwrap().is_empty());
    assert_eq!(paths.as_ptr(), pointer);
    assert_eq!(paths.capacity(), 32);
}
#[cfg(unix)]
#[test]
fn non_utf8_path_order_and_allocation_survive_commit_and_recovery() {
    use std::os::unix::ffi::OsStringExt;
    let (mut paths, facts) = fixture();
    paths[0].1 = OsString::from_vec(vec![b'a', 255, b'b']).into();
    let old = identities(&paths);
    let limits = budget(&paths);
    let input = CompactSourceInput::prepare_borrowed(
        &mut paths,
        |key| facts.iter().find(|m| m.key() == key),
        StreamConfig::with_pool_pages(2),
        limits,
    )
    .unwrap()
    .commit();
    assert_eq!(
        input.compact_sources().unwrap()[0]
            .path
            .as_os_str()
            .as_encoded_bytes(),
        [b'a', 255, b'b']
    );
    for (target, source) in paths.iter_mut().zip(input.into_originals()) {
        target.1 = source.path;
    }
    assert_eq!(identities(&paths), old);
}

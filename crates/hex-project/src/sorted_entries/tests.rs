use super::*;

#[test]
fn sorted_lookup_replacement_removal_and_spare_backing_are_observed() {
    let mut entries = SortedEntries::new();
    entries.try_reserve_on_control(8, 16).unwrap();
    let reserved = entries.backing_bytes().unwrap();
    assert!(reserved >= 8 * mem::size_of::<(u32, String)>());
    assert!(entries.is_empty());
    for key in [7, 2, 5] {
        assert!(
            entries
                .try_insert_on_control(key, key.to_string(), 3)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(entries.keys().copied().collect::<Vec<_>>(), [2, 5, 7]);
    assert_eq!(entries.get(&5).unwrap(), "5");
    assert_eq!(
        entries
            .try_insert_on_control(5, "replace".into(), 3)
            .unwrap()
            .unwrap(),
        "5"
    );
    assert!(
        entries
            .try_insert_on_control(9, "refuse".into(), 3)
            .is_err()
    );
    assert_eq!(entries.len(), 3);
    assert_eq!(entries.backing_bytes().unwrap(), reserved);
    assert_eq!(entries.remove(&2).unwrap(), "2");
    entries.retain(|key, value| {
        value.push('!');
        *key != 7
    });
    assert_eq!(entries.get(&5).unwrap(), "replace!");
    assert_eq!(entries.backing_bytes().unwrap(), reserved);
    entries.clear();
    assert_eq!(entries.backing_bytes().unwrap(), reserved); // empty != unallocated
}

#[test]
fn bulk_sort_is_unique_checked_and_preserves_actual_input_backing() {
    let mut input = Vec::with_capacity(16);
    input.extend([(3, "third"), (1, "first"), (2, "second")]);
    let capacity = input.capacity();
    let entries = SortedEntries::try_from_unsorted_on_control(input, 3).unwrap();
    assert_eq!(entries.capacity(), capacity);
    assert_eq!(
        entries.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
        [(1, "first"), (2, "second"), (3, "third")]
    );
    assert!(SortedEntries::try_from_unsorted_on_control(vec![(1, 1), (1, 2)], 2).is_err());
    assert!(SortedEntries::try_from_unsorted_on_control(vec![(1, 1), (2, 2)], 1).is_err());
    let mut empty = SortedEntries::<u32, u64>::new();
    assert!(
        empty
            .try_reserve_on_control(usize::MAX, usize::MAX)
            .is_err()
    );
    assert!(empty.is_empty());
    assert_eq!(empty.capacity(), 0);
}

#[test]
fn explicit_control_clone_has_independent_backing_and_refuses_changed_or_failed_keys() {
    let original = SortedEntries::try_from_unsorted_on_control(
        vec![(2, "two".to_string()), (1, "one".into())],
        2,
    )
    .unwrap();
    let mut clone = original
        .try_clone_with_on_control(2, |key, value| Ok((*key, value.clone())))
        .unwrap();
    assert!(!std::ptr::eq(
        original.as_slice().as_ptr(),
        clone.as_slice().as_ptr()
    ));
    clone.get_mut(&1).unwrap().push('!');
    assert_eq!(original.get(&1).unwrap(), "one");
    assert!(
        original
            .try_clone_with_on_control(1, |key, value| Ok((*key, value.clone())))
            .is_err()
    );
    assert!(
        original
            .try_clone_with_on_control(2, |key, value| Ok((*key + 1, value.clone())))
            .is_err()
    );
    let mut visits = 0;
    assert!(
        original
            .try_clone_with_on_control(2, |key, value| {
                visits += 1;
                if visits == 2 {
                    Err(Invalid("canceled control clone"))
                } else {
                    Ok((*key, value.clone()))
                }
            })
            .is_err()
    );
    assert_eq!(original.len(), 2);
    assert_eq!(original.get(&1).unwrap(), "one");
    assert_eq!(
        clone.backing_bytes().unwrap(),
        clone.capacity() * mem::size_of::<(i32, String)>()
    );
}

#[test]
fn maximum_asset_bulk_and_retained_history_backings_remain_independent_without_max_reserve() {
    let count = crate::MAX_ASSETS;
    let entries = SortedEntries::try_from_unsorted_on_control(
        (0..count).rev().map(|key| (key, key as u64)).collect(),
        count,
    )
    .unwrap();
    assert_eq!(
        entries.keys().copied().collect::<Vec<_>>(),
        (0..count).collect::<Vec<_>>()
    );
    let mut snapshots = Vec::new();
    for _ in 0..128 {
        snapshots.push(
            entries
                .try_clone_with_on_control(count, |key, value| Ok((*key, *value)))
                .unwrap(),
        );
    }
    let actual: usize = snapshots
        .iter()
        .map(|entry| entry.backing_bytes().unwrap())
        .sum();
    assert_eq!(
        actual,
        snapshots
            .iter()
            .map(|entry| entry.capacity() * mem::size_of::<(usize, u64)>())
            .sum()
    );
    for snapshot in &snapshots {
        assert!(!std::ptr::eq(
            entries.as_slice().as_ptr(),
            snapshot.as_slice().as_ptr()
        ));
    }
    let small = SortedEntries::try_from_unsorted_on_control(vec![(1_u32, 1_u64)], count).unwrap();
    let copy = small
        .try_clone_with_on_control(count, |key, value| Ok((*key, *value)))
        .unwrap();
    assert!(copy.capacity() < count);
}

#[test]
fn actual_project_candidate_undo_redo_keep_distinct_asset_and_path_backings() {
    use crate::{Command, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let add = |name: &str| Command::AddAsset {
        path: name.into(),
        sample_rate: 48_000,
        frames: 4,
    };
    editor.execute(add("first.wav")).unwrap();
    editor.execute(add("second.wav")).unwrap();
    for expected_current in [2, 1, 2] {
        let projects: Vec<_> = editor.retained_projects_on_control().collect();
        assert_eq!(projects[0].assets().count(), expected_current);
        assert_eq!(
            projects
                .iter()
                .map(|project| project.assets().count())
                .sum::<usize>(),
            3
        );
        let nonempty: Vec<_> = projects
            .iter()
            .map(|project| project.asset_entries_on_control())
            .filter(|entries| !entries.is_empty())
            .collect();
        assert_eq!(nonempty.len(), 2);
        assert!(!std::ptr::eq(
            nonempty[0].as_slice().as_ptr(),
            nonempty[1].as_slice().as_ptr()
        ));
        let first_paths: Vec<_> = projects
            .iter()
            .flat_map(|project| project.assets())
            .filter(|asset| asset.path().ends_with("first.wav"))
            .map(|asset| asset.path_buffer_on_control())
            .collect();
        assert_eq!(first_paths.len(), 2);
        assert!(!std::ptr::eq(
            first_paths[0].as_os_str(),
            first_paths[1].as_os_str()
        ));
        let candidate = editor.project().clone();
        assert!(!std::ptr::eq(
            candidate.asset_entries_on_control().as_slice().as_ptr(),
            editor
                .project()
                .asset_entries_on_control()
                .as_slice()
                .as_ptr()
        ));
        assert_eq!(candidate, *editor.project());
        drop(candidate);
        drop(projects);
        if expected_current == 2 && editor.can_undo() && !editor.can_redo() {
            assert!(editor.undo());
        } else if expected_current == 1 {
            assert!(editor.redo());
        }
    }
}

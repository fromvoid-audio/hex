use super::*;
fn editor(capacity: usize) -> Editor {
    Editor::new(Project::new(48_000).unwrap(), capacity).unwrap()
}
fn add(name: &str) -> OriginalSamplerTransitionRequest {
    OriginalSamplerTransitionRequest::command(Command::AddTrack { name: name.into() })
}
fn run(
    editor: &mut Editor,
    request: OriginalSamplerTransitionRequest,
) -> OriginalSamplerTransitionResult {
    let mut source = Some(request);
    {
        let mut loan = editor
            .original_sampler_transition_on_control(&mut source)
            .unwrap();
        loan.stage_original_sampler_after_prefix_on_control()
            .unwrap();
    }
    assert!(source.is_none());
    let result = editor
        .commit_original_sampler_transition_on_control()
        .unwrap();
    editor
        .clear_original_sampler_transition_on_control()
        .unwrap();
    result
}
#[test]
fn staged_command_undo_redo_preserve_ids_history_and_atomic_refusal() {
    let mut actual = editor(2);
    let before = actual.project.clone();
    let mut request = Some(add("actual pending track"));
    {
        let mut loan = actual
            .original_sampler_transition_on_control(&mut request)
            .unwrap();
        assert_eq!(loan.editor().project(), &before);
        assert!(loan.storage().candidate().is_none());
        loan.stage_original_sampler_after_prefix_on_control()
            .unwrap();
        assert_eq!(loan.editor().project(), &before);
        assert_eq!(loan.storage().candidate().unwrap().tracks().len(), 1);
        assert_eq!(loan.retained_projects_on_control().unwrap().count(), 2);
        assert!(
            actual
                .require_no_original_sampler_pending_on_control()
                .is_err()
        );
    }
    assert!(
        actual
            .execute(Command::AddTrack {
                name: "must refuse".into()
            })
            .is_err()
    );
    assert!(!actual.undo());
    assert!(!actual.redo());
    assert!(
        actual
            .commit_original_sampler_transition_on_control()
            .unwrap()
            .changed
    );
    assert!(
        actual
            .commit_original_sampler_transition_on_control()
            .is_err()
    );
    actual
        .clear_original_sampler_transition_on_control()
        .unwrap();
    let added = actual.project.clone();
    run(&mut actual, OriginalSamplerTransitionRequest::undo());
    assert!(actual.project.tracks().is_empty());
    assert_eq!(actual.project.next_id, added.next_id);
    assert_eq!((actual.undo.len(), actual.redo.len()), (0, 1));
    run(&mut actual, OriginalSamplerTransitionRequest::redo());
    assert_eq!(actual.project, added);
    assert_eq!((actual.undo.len(), actual.redo.len()), (1, 0));
    assert!(
        actual
            .require_no_original_sampler_pending_on_control()
            .is_ok()
    );
}
#[test]
fn staged_noop_preserves_redo_and_history_and_cancel_preserves_canonical() {
    let mut actual = editor(3);
    run(&mut actual, add("one"));
    run(&mut actual, OriginalSamplerTransitionRequest::undo());
    let before = (
        actual.project.clone(),
        actual.undo.clone(),
        actual.redo.clone(),
    );
    let noop = run(
        &mut actual,
        OriginalSamplerTransitionRequest::command(Command::ClearLoopRegion),
    );
    assert!(!noop.changed);
    assert_eq!(
        (&actual.project, &actual.undo, &actual.redo),
        (&before.0, &before.1, &before.2)
    );
    let mut request = Some(add("cancel"));
    {
        let mut loan = actual
            .original_sampler_transition_on_control(&mut request)
            .unwrap();
        loan.stage_original_sampler_after_prefix_on_control()
            .unwrap();
    }
    actual
        .clear_original_sampler_transition_on_control()
        .unwrap();
    assert_eq!(
        (&actual.project, &actual.undo, &actual.redo),
        (&before.0, &before.1, &before.2)
    );
    actual
        .clear_original_sampler_transition_on_control()
        .unwrap();
}
#[test]
fn staged_invalid_command_and_exhaustion_never_become_committable() {
    for exhaustion in [false, true] {
        let mut actual = editor(2);
        if exhaustion {
            actual.project.next_id = u64::MAX;
        }
        let before = actual.project.clone();
        let request = if exhaustion {
            add("exhausted")
        } else {
            OriginalSamplerTransitionRequest::command(Command::SetLoopRegion {
                start: 2,
                end: 1,
                enabled: true,
            })
        };
        let mut request = Some(request);
        {
            let mut loan = actual
                .original_sampler_transition_on_control(&mut request)
                .unwrap();
            assert!(
                loan.stage_original_sampler_after_prefix_on_control()
                    .is_err()
            );
            assert!(
                loan.stage_original_sampler_after_prefix_on_control()
                    .is_err()
            );
        }
        assert!(
            actual
                .commit_original_sampler_transition_on_control()
                .is_err()
        );
        actual
            .clear_original_sampler_transition_on_control()
            .unwrap();
        assert_eq!(actual.project, before);
        assert!(actual.undo.is_empty());
        assert!(actual.redo.is_empty());
    }
}
#[test]
fn staged_reentrancy_keeps_request_and_pending_owner() {
    let mut actual = editor(2);
    let mut first = Some(add("first"));
    let mut second = Some(OriginalSamplerTransitionRequest::redo());
    {
        let mut loan = actual
            .original_sampler_transition_on_control(&mut first)
            .unwrap();
        assert!(
            actual
                .original_sampler_transition_on_control(&mut second)
                .is_err()
        );
        assert!(second.is_some());
        loan.stage_original_sampler_after_prefix_on_control()
            .unwrap();
        assert!(
            actual
                .original_sampler_transition_on_control(&mut second)
                .is_err()
        );
    }
    assert!(
        actual
            .original_sampler_transition_on_control(&mut second)
            .is_err()
    );
    assert!(second.is_some());
    assert!(
        actual
            .execute_many_prepared::<_, Invalid>(Vec::new(), |_, _| Ok(()))
            .is_err()
    );
    actual
        .clear_original_sampler_transition_on_control()
        .unwrap();
    run(&mut actual, second.take().unwrap());
    assert!(actual.project.tracks().is_empty());
}
#[test]
fn staged_history_order_capacity_and_gain_epochs_match_ordinary_transitions() {
    let mut staged = editor(3);
    let mut ordinary = editor(3);
    for index in 0..7 {
        let command = Command::AddTrack {
            name: format!("track{index}"),
        };
        let a = run(
            &mut staged,
            OriginalSamplerTransitionRequest::command(command.clone()),
        );
        let b = ordinary.execute(command).unwrap();
        assert_eq!(a.created, Some(b));
        assert_eq!(
            (&staged.project, &staged.undo),
            (&ordinary.project, &ordinary.undo)
        );
    }
    for _ in 0..3 {
        run(&mut staged, OriginalSamplerTransitionRequest::undo());
        assert!(ordinary.undo());
        assert_eq!(
            (&staged.project, &staged.undo, &staged.redo),
            (&ordinary.project, &ordinary.undo, &ordinary.redo)
        );
    }
    for _ in 0..3 {
        run(&mut staged, OriginalSamplerTransitionRequest::redo());
        assert!(ordinary.redo());
        assert_eq!(
            (&staged.project, &staged.undo, &staged.redo),
            (&ordinary.project, &ordinary.undo, &ordinary.redo)
        );
    }
    let track = staged.project.tracks[0].id;
    staged.project.tracks[0].gain_lane.recording_epoch = 19;
    run(&mut staged, OriginalSamplerTransitionRequest::undo());
    assert_eq!(
        staged
            .project
            .track(track)
            .unwrap()
            .gain_lane
            .recording_epoch,
        19
    );
    run(&mut staged, OriginalSamplerTransitionRequest::redo());
    assert_eq!(
        staged
            .project
            .track(track)
            .unwrap()
            .gain_lane
            .recording_epoch,
        19
    );
}
#[test]
fn actual_pending_cleanup_panic_preserves_first_payload_and_remaining_candidate() {
    let mut actual = editor(2);
    let before = actual.project.clone();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut request = Some(add("pending"));
    {
        let mut loan = actual
            .original_sampler_transition_on_control(&mut request)
            .unwrap();
        loan.stage_original_sampler_after_prefix_on_control()
            .unwrap();
        loan.pending.cleanup_probe = Some(CleanupProbe(calls.clone()));
    }
    assert!(
        actual
            .clear_original_sampler_transition_on_control()
            .is_err()
    );
    let payload = actual
        .original_sampler_pending
        .get_mut()
        .first_panic
        .as_ref()
        .unwrap()
        .as_ref() as *const dyn Any as *const ();
    assert!(
        actual
            .original_sampler_pending
            .get_mut()
            .candidate
            .is_some()
    );
    assert_eq!(actual.project, before);
    for _ in 0..3 {
        assert!(
            actual
                .clear_original_sampler_transition_on_control()
                .is_err()
        );
        assert!(
            actual
                .commit_original_sampler_transition_on_control()
                .is_err()
        );
        assert!(
            actual
                .original_sampler_transition_on_control(&mut Some(
                    OriginalSamplerTransitionRequest::undo()
                ))
                .is_err()
        );
        assert_eq!(
            actual
                .original_sampler_pending
                .get_mut()
                .first_panic
                .as_ref()
                .unwrap()
                .as_ref() as *const dyn Any as *const (),
            payload
        );
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

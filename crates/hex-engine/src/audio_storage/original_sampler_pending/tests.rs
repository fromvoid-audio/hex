use super::*;
use hex_project::{Editor, OriginalSamplerTransitionRequest, Project};

fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 256,
        handle_rows: 128,
        root_rows: 8,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}

#[test]
fn held_pending_guard_observes_actual_command_capacity_and_same_model_fields() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::AddTrack {
            name: "history".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let baseline = {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        ledger
            .borrow_editor_history_buffers_on_control(&editor)
            .unwrap();
        ledger
            .borrow_editor_exclusive_buffers_on_control(&editor)
            .unwrap();
        ledger
            .borrow_editor_midi_sources_on_control(&editor)
            .unwrap();
        ledger
            .borrow_editor_native_states_on_control(&editor)
            .unwrap();
        ledger.summary().unwrap().data_bytes
    };
    let mut name = String::with_capacity(128);
    name.push_str("actual pending name");
    let expected = baseline + name.capacity();
    let mut request = Some(OriginalSamplerTransitionRequest::command(
        Command::AddTrack { name },
    ));
    let transition = editor
        .original_sampler_transition_on_control(&mut request)
        .unwrap();
    assert!(request.is_none());
    assert!(transition.storage().candidate().is_none());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .borrow_original_sampler_pending_model_fields_on_control(&transition)
        .unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, expected);
    ledger
        .borrow_original_sampler_pending_model_fields_on_control(&transition)
        .unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, expected);
    assert!(ledger.summary().unwrap().require_complete().is_err());
    let mut subset = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert_eq!(
        subset.borrow_editor_history_buffers_on_control(&editor),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(subset.summary().is_err());
    let cost = AudioRootStorageLedger::arithmetic_cost_for_limits(
        reservation(),
        expected,
        expected,
        |ledger| ledger.borrow_original_sampler_pending_model_fields_on_control(&transition),
    )
    .unwrap();
    assert_eq!(cost.data_bytes, expected);
    for (retained, peak) in [(expected - 1, expected), (expected, expected - 1)] {
        assert!(
            AudioRootStorageLedger::arithmetic_cost_for_limits(
                reservation(),
                retained,
                peak,
                |ledger| ledger
                    .borrow_original_sampler_pending_model_fields_on_control(&transition),
            )
            .is_err()
        );
    }
    // This is selected model DATA evidence, not whole-host admission. No clone,
    // stage or constructor is invoked and current/history remain untouched.
    assert!(editor.project().tracks().is_empty());
    assert!(editor.can_redo());
}

#[test]
fn pending_owning_command_without_child_schema_invalidates_normal_and_bootstrap() {
    let editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut request = Some(OriginalSamplerTransitionRequest::command(
        Command::SetRouting(Vec::with_capacity(16)),
    ));
    let transition = editor
        .original_sampler_transition_on_control(&mut request)
        .unwrap();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert_eq!(
        ledger.borrow_original_sampler_pending_model_fields_on_control(&transition),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(ledger.summary().is_err());
    assert_eq!(
        AudioRootStorageLedger::arithmetic_cost_for_limits(
            reservation(),
            usize::MAX,
            usize::MAX,
            |ledger| ledger.borrow_original_sampler_pending_model_fields_on_control(&transition),
        ),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(
        matches!(transition.storage().request().unwrap().command_value(), Some(Command::SetRouting(values)) if values.capacity() >= 16)
    );
}

use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole,
    owned_shared::OwnedShared,
};
fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 64,
        root_rows: 32,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
#[test]
fn actual_bank_body_and_entry_buffer_require_every_alias_with_unchanged_policy_caps() {
    if !crate::pcm_arc::native_layout_known() {
        return;
    }
    let mut keys = PreparedMidiNoteBank::empty_keys();
    PreparedMidiNoteBank::add_key(&mut keys, 60, 100).unwrap();
    let bank = PreparedMidiNoteBank::prepare(&mut keys, &SilentInstrument, None, None).unwrap();
    let alias = bank.clone_on_control();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .mutate(|ledger| storage::visit(&bank, ledger, AudioStorageRole::Current))
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    let before = ledger.summary().unwrap();
    ledger
        .mutate(|ledger| storage::visit(&alias, ledger, AudioStorageRole::Archive))
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let after = ledger.summary().unwrap();
    assert_eq!(after.data_allocations, 2); // Actual owner and entry backing.
    assert_eq!(after.data_allocations, before.data_allocations);
    assert_eq!(
        after.data_bytes - before.data_bytes,
        std::mem::size_of_val(&alias)
    );
    let parts = bank.parts().unwrap();
    assert_eq!(
        after.data_bytes,
        parts.0
            + parts.1
            + 2 * std::mem::size_of_val(&bank)
            + bank.entries.capacity() * std::mem::size_of::<BankEntry>()
    );
    drop(ledger);
    let inventory = |reservation| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
        ledger.mutate(|ledger| {
            storage::visit(&bank, ledger, AudioStorageRole::Current)?;
            storage::visit(&alias, ledger, AudioStorageRole::Archive)
        })?;
        ledger.validate_closed_shared_handles()?;
        ledger.summary()
    };
    assert_eq!(
        inventory(AudioStorageLedgerReservation {
            max_retained_bytes: after.retained_bytes,
            max_construction_peak_bytes: after.construction_peak_bytes,
            ..reservation()
        })
        .unwrap(),
        after
    );
    assert!(
        inventory(AudioStorageLedgerReservation {
            max_retained_bytes: after.retained_bytes - 1,
            ..reservation()
        })
        .is_err()
    );
    assert!(
        inventory(AudioStorageLedgerReservation {
            max_construction_peak_bytes: after.construction_peak_bytes - 1,
            ..reservation()
        })
        .is_err()
    );
    assert!(
        inventory(AudioStorageLedgerReservation {
            handle_rows: 1,
            ..reservation()
        })
        .is_err()
    );
    assert_eq!(std::mem::size_of::<BankEntry>(), 360);
    assert_eq!(std::mem::size_of::<PreparedMidiNoteBank>(), 48);
    assert_eq!(complete_revision_limits().unwrap(), (47_747_072, 196_608));
}
#[test]
fn raw_bank_arc_cannot_gain_a_declaration_from_empty_contents_or_unique_counts() {
    let raw = OwnedShared::untracked(Arc::new(PreparedMidiNoteBank {
        entries: Vec::new(),
        context_id: None,
        context_index: None,
    }));
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert_eq!(
        ledger.mutate(|ledger| storage::visit(&raw, ledger, AudioStorageRole::Current)),
        Err(crate::AudioStorageError::UnknownStorage)
    );
    assert!(ledger.summary().is_err());
    assert_eq!(Arc::strong_count(raw.arc()), 1);
}
fn midi_project() -> hex_project::Project {
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "bank source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "bank scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let length = BeatTime::from_quarters(4).unwrap();
    let Created::Clip(arrangement) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let Created::SessionClip(session) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "bank cell".into(),
            source_length: length,
        })
        .unwrap()
    else {
        panic!("cell")
    };
    for clip in [arrangement, session] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    editor.project().clone()
}
#[test]
fn actual_cached_bank_union_survives_older_revision_retirement_without_rebuilding_entries() {
    if !crate::pcm_arc::native_layout_known() {
        return;
    }
    let project = midi_project();
    let table = Arc::new(hex_devices::SineTable::new());
    let media = std::collections::BTreeMap::new();
    let first =
        crate::PreparedRevision::prepare(&project, &media, Arc::clone(&table), None).unwrap();
    let second =
        crate::PreparedRevision::prepare(&project, &media, table, Some(first.session())).unwrap();
    let first_bank = first.session().midi_clips()[0]
        .instrument_template()
        .note_bank();
    let second_bank = second.session().midi_clips()[0]
        .instrument_template()
        .note_bank();
    assert!(Arc::ptr_eq(first_bank.arc(), second_bank.arc()));
    assert_eq!(first_bank.parts(), second_bank.parts());
    let entry_pointer = second_bank.entries.as_ptr() as usize;
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .borrow_revision_note_banks_on_control(&second, AudioStorageRole::Current)
        .unwrap();
    ledger
        .borrow_revision_note_banks_on_control(&first, AudioStorageRole::Predecessor)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let union = ledger.summary().unwrap();
    // Separate Arrangement banks plus one physically reused Session bank.
    // Whole omission of that Session allocation must fail this independent
    // constructor oracle, even if the remaining selected aliases close.
    assert_eq!(union.data_allocations, 6);
    assert_eq!(union.unresolved_handles, 3);
    drop(ledger);
    drop(first);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .borrow_revision_note_banks_on_control(&second, AudioStorageRole::Current)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert!(ledger.summary().unwrap().data_bytes < union.data_bytes);
    assert_eq!(ledger.summary().unwrap().data_allocations, 4);
    assert_eq!(ledger.summary().unwrap().unresolved_handles, 2);
    assert_eq!(
        second.session().midi_clips()[0]
            .instrument_template()
            .note_bank()
            .entries
            .as_ptr() as usize,
        entry_pointer
    );
}

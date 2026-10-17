use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
fn project() -> Project {
    use hex_project::{BeatTime, Command, Created, Editor};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi metadata".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "midi scene".into(),
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
            name: "midi cell".into(),
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
fn actual_cached_template_container_union_survives_old_revision_control_retirement() {
    if !crate::pcm_arc::native_layout_known() {
        return;
    }
    let project = project();
    let table = Arc::new(SineTable::new());
    let media = BTreeMap::new();
    let first =
        crate::PreparedRevision::prepare(&project, &media, Arc::clone(&table), None).unwrap();
    let second =
        crate::PreparedRevision::prepare(&project, &media, table, Some(first.session())).unwrap();
    let first_clip = &first.session().midi_clips[0];
    let second_clip = &second.session().midi_clips[0];
    assert!(Arc::ptr_eq(
        first_clip.source.arc(),
        second_clip.source.arc()
    ));
    assert!(Arc::ptr_eq(
        first_clip.instrument.arc(),
        second_clip.instrument.arc()
    ));
    let source_pointer = second_clip.source.arc().as_ref() as *const _ as usize;
    let instrument_pointer = second_clip.instrument.arc().as_ref() as *const _ as usize;
    let reservation = AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 64,
        root_rows: 32,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    };
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    ledger
        .borrow_revision_midi_templates_on_control(&second, AudioStorageRole::Current)
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    ledger
        .borrow_revision_midi_templates_on_control(&first, AudioStorageRole::Predecessor)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let union = ledger.summary().unwrap();
    // Two table/body-buffer groups (6), three bank/body-buffer groups (6),
    // two Arrangement span/index backings (4), one cached source body plus all
    // FOUR reserved edge/span arrays including empty repeats (5), and one
    // cached instrument body plus its populated voice/index backings (3).
    assert_eq!(union.data_allocations, 24);
    drop(ledger);
    drop(first);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    ledger
        .borrow_revision_midi_templates_on_control(&second, AudioStorageRole::Current)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let retained = ledger.summary().unwrap();
    assert_eq!(retained.data_allocations, 20);
    assert!(retained.data_bytes < union.data_bytes);
    let clip = &second.session().midi_clips[0];
    assert_eq!(
        clip.source.arc().as_ref() as *const _ as usize,
        source_pointer
    );
    assert_eq!(
        clip.instrument.arc().as_ref() as *const _ as usize,
        instrument_pointer
    );
}

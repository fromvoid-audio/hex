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
    let table = crate::SharedSineTable::new_on_control();
    let media = BTreeMap::new();
    let first = crate::PreparedRevision::prepare(&project, &media, table.clone(), None).unwrap();
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

#[test]
fn actual_session_midi_row_fields_close_model_state_and_cached_template_aliases() {
    if !crate::audio_storage_constructor_layout_supported()
        || !crate::canonical_midi_storage_layout_supported()
        || !crate::canonical_native_state_storage_layout_supported()
    {
        eprintln!("unsupported actual Session MIDI row field constructor gates");
        return;
    }
    eprintln!(
        "SUPPORTED actual Session MIDI row fields: canonical source/state aliases, cached templates and names"
    );
    for routed in [false, true] {
        let project = if routed {
            let mut editor = hex_project::Editor::new(project(), 8).unwrap();
            let source = editor.project().tracks()[0].id();
            let hex_project::Created::Track(destination) = editor
                .execute(hex_project::Command::AddMidiTrack {
                    name: "destination".into(),
                })
                .unwrap()
            else {
                panic!("destination")
            };
            editor
                .execute(hex_project::Command::SetMidiSource {
                    destination,
                    source: Some(source),
                })
                .unwrap();
            editor.project().clone()
        } else {
            project()
        };
        eprintln!("SUPPORTED actual Session MIDI row field routed={routed}");
        let sine = crate::SharedSineTable::new_on_control();
        let first = PreparedSessionInventory::prepare(&project, sine.clone(), None).unwrap();
        let second = PreparedSessionInventory::prepare(&project, sine, Some(&first)).unwrap();
        assert!(Arc::ptr_eq(
            first.midi_clips[0].source.arc(),
            second.midi_clips[0].source.arc()
        ));
        assert!(Arc::ptr_eq(
            first.midi_clips[0].instrument.arc(),
            second.midi_clips[0].instrument.arc()
        ));
        assert_eq!(first.routed_midi_clips.len(), usize::from(routed));
        let first_rows_bytes = (first.midi_clips.capacity() + first.routed_midi_clips.capacity())
            * size_of::<PreparedSessionMidiClip>()
            + first
                .midi_clips
                .iter()
                .chain(&first.routed_midi_clips)
                .map(|row| row.name.capacity())
                .sum::<usize>();
        let second_rows_bytes = (second.midi_clips.capacity()
            + second.routed_midi_clips.capacity())
            * size_of::<PreparedSessionMidiClip>()
            + second
                .midi_clips
                .iter()
                .chain(&second.routed_midi_clips)
                .map(|row| row.name.capacity())
                .sum::<usize>();
        let reservation = AudioStorageLedgerReservation {
            data_rows: 256,
            handle_rows: 256,
            root_rows: 64,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        };
        let collect = |old: Option<&PreparedSessionInventory>,
                       budget|
         -> Result<_, crate::AudioStorageError> {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
            ledger.borrow_project_midi_sources_on_control(&project)?;
            ledger.borrow_project_native_states_on_control(&project)?;
            for (inventory, role) in old
                .into_iter()
                .map(|i| (i, AudioStorageRole::Predecessor))
                .chain(std::iter::once((&second, AudioStorageRole::Current)))
            {
                crate::shared_analytic::resource_storage::visit_table(
                    &inventory.shared_contexts,
                    &mut ledger,
                    role,
                )?;
                inventory.visit_midi_templates_on_control(&mut ledger, role)?;
            }
            // Names/row backings and ACTUAL prepared state/model-identity fields
            // remain unvisited. Template IDs/owners cannot substitute for them.
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            let before = ledger.summary()?;
            if let Some(old) = old {
                visit_midi_rows(&old.midi_clips, &mut ledger, AudioStorageRole::Predecessor)?;
                visit_midi_rows(
                    &old.routed_midi_clips,
                    &mut ledger,
                    AudioStorageRole::Predecessor,
                )?;
            }
            visit_midi_rows(&second.midi_clips, &mut ledger, AudioStorageRole::Current)?;
            visit_midi_rows(
                &second.routed_midi_clips,
                &mut ledger,
                AudioStorageRole::Current,
            )?;
            visit_midi_rows(&second.midi_clips, &mut ledger, AudioStorageRole::Current)?;
            ledger.validate_selected_shared_handles_on_control()?;
            let summary = ledger.summary()?;
            assert_eq!(
                summary.data_bytes - before.data_bytes,
                second_rows_bytes + if old.is_some() { first_rows_bytes } else { 0 }
            );
            assert!(summary.require_complete().is_err());
            Ok(summary)
        };
        let union = collect(Some(&first), reservation).unwrap();
        let mut exact = reservation;
        exact.max_retained_bytes = union.retained_bytes;
        exact.max_construction_peak_bytes = union.construction_peak_bytes;
        assert_eq!(collect(Some(&first), exact).unwrap(), union);
        exact.max_retained_bytes -= 1;
        assert_eq!(
            collect(Some(&first), exact),
            Err(crate::AudioStorageError::ReservationExceeded)
        );
        drop(first);
        let retained = collect(None, reservation).unwrap();
        assert!(retained.data_bytes < union.data_bytes);
        assert_eq!(
            second.midi_clips[0]
                .source_identity
                .source_storage_on_control()
                .owning_counts()
                .0,
            2 + usize::from(routed)
        );
    }
}

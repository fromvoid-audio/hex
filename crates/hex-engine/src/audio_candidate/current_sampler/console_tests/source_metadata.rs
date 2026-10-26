use super::*;

fn projects() -> (Project, Project, hex_project::ClipId) {
    let (project, _) = fixture();
    let track = project.tracks()[0].id();
    let mut editor = Editor::new(project, 16).unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "console".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "source".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    for pitch in 60..65 {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    // Removing authors retains spare canonical Vec capacity. Reopening the same
    // snapshot compacts it without changing any musical data or compiled events.
    let notes: Vec<_> = editor
        .project()
        .session_midi_clip(clip)
        .unwrap()
        .source()
        .notes()
        .iter()
        .skip(1)
        .map(|n| n.id())
        .collect();
    for note in notes {
        editor.execute(Command::RemoveMidiNote(note)).unwrap();
    }
    let original = editor.project().clone();
    let compact = Project::from_snapshot(original.snapshot()).unwrap();
    assert_eq!(original.snapshot(), compact.snapshot());
    (original, compact, clip)
}

#[test]
fn canonical_midi_source_capacity_is_retained_in_current_quotes_and_ledger() {
    let (original, compact, clip) = projects();
    let charge = |p: &Project| {
        p.session_midi_clip(clip)
            .unwrap()
            .source_identity()
            .retained_control_metadata_bytes()
            .unwrap()
    };
    let delta = (charge(&original) - charge(&compact)) * 2;
    assert!(delta > 0, "fixture must retain spare capacity");
    let quote = |p| AudioCandidateReservationQuote::for_project(p, 1).unwrap();
    assert_eq!(
        quote(&original).metadata_data_bytes - quote(&compact).metadata_data_bytes,
        delta
    );
    let compiler =
        |p| crate::session_inventory::audio_assembly::current_midi::compiler_quote(p).unwrap();
    assert_eq!(compiler(&original).0 - compiler(&compact).0, delta);
    assert_eq!(compiler(&original).1, compiler(&compact).1);
    let mut totals = Vec::new();
    let mut restored = Vec::new();
    let mut stages = Vec::new();
    for project in [&compact, &original] {
        let asset = project.assets().next().unwrap().id();
        let source = crate::OwnedAudioSource::try_resident(8000, vec![[0.125; 2]; 4])
            .ok()
            .unwrap();
        let mut cell = prepare(project, vec![(asset, source)]);
        stages.push(
            cell.owner_on_control()
                .unwrap()
                .preparation()
                .capacity_report()
                .assembly
                .sampler_midi_bytes,
        );
        let q = quote(project);
        let limits = q.storage_reservation(65536).unwrap();
        let owner = cell.owner_on_control().unwrap();
        let storage = crate::AudioStorageLedgerReservation {
            data_rows: q.data_rows,
            handle_rows: q.handle_rows,
            root_rows: 64,
            max_retained_bytes: limits.max_prepared_data_bytes,
            max_construction_peak_bytes: limits.max_construction_data_bytes,
        };
        let total = owner.seal_constructor_on_control(storage).unwrap();
        totals.push(total);
        // Refusal cannot consume a candidate or its canonical backing.
        let exact = crate::AudioStorageLedgerReservation {
            max_retained_bytes: total.data.retained_bytes + total.reserved_split_bytes,
            max_construction_peak_bytes: total.data.construction_peak_bytes
                + total.reserved_split_bytes,
            ..storage
        };
        assert_eq!(
            owner.seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                max_retained_bytes: exact.max_retained_bytes - 1,
                ..exact
            }),
            Err(AudioStorageError::ReservationExceeded)
        );
        assert_eq!(
            owner.seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                max_construction_peak_bytes: exact.max_construction_peak_bytes - 1,
                ..exact
            }),
            Err(AudioStorageError::ReservationExceeded)
        );
        assert_eq!(owner.seal_constructor_on_control(exact).unwrap(), total);
        let inspect = |owner: &AudioCandidateOwner| {
            let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(storage).unwrap();
            ledger
                .mutate(|l| owner.preparation().visit_audio_storage(l))
                .unwrap();
            ledger.summary().unwrap().data_bytes
        };
        let receipt = owner.dispose_runtime_on_control().unwrap();
        restored.push(inspect(owner));
        // Resume the ordinary local-book fanout after inspecting the retained
        // roots between actual runtime and control-side root destruction.
        owner.inner.runtime_receipt = Some(receipt);
        owner.dispose_on_control().unwrap();
        cell.owner_runtime_done = true;
        cell.cleanup_on_control().unwrap();
        assert!(cell.complete);
        assert!(cell.owner.is_none());
    }
    assert_eq!(
        totals[1].data.retained_bytes - totals[0].data.retained_bytes,
        delta
    );
    assert_eq!(
        totals[1].data.construction_peak_bytes - totals[0].data.construction_peak_bytes,
        delta
    );
    assert_eq!(stages[1] - stages[0], delta);
    assert_eq!(restored[1] - restored[0], delta);
    assert_eq!(original.snapshot(), compact.snapshot());
}

#[test]
fn canonical_source_reservation_refuses_before_compiler_and_preserves_original_bank() {
    let (project, _, _) = projects();
    let asset = project.assets().next().unwrap().id();
    let quote = AudioCandidateReservationQuote::for_project(&project, 1).unwrap();
    let original_limits = quote.storage_reservation(65536).unwrap();
    let mut observed = None;
    for pass in 0..3 {
        let mut limits = original_limits;
        if let Some((retained, peak)) = observed {
            limits.max_prepared_data_bytes = retained - usize::from(pass == 1);
            limits.max_construction_data_bytes = peak - usize::from(pass == 2);
        }
        let source = crate::OwnedAudioSource::try_resident(8000, vec![[0.125; 2]; 4])
            .ok()
            .unwrap();
        let mut bank = Some(
            crate::OwnedAudioInputs::try_new(vec![(asset, source)], limits)
                .ok()
                .unwrap(),
        );
        let token = bank.as_ref().unwrap().source_token(asset).unwrap();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        crate::midi_compile::CURRENT_SAMPLER_COMPILER_ENTRIES.with(|n| n.set(0));
        let result = custody.begin_current_sampler_in_place_on_control(
            &project,
            11,
            crate::AudioSourceContext {
                project_generation: 4,
                editor_revision: 7,
            },
            &mut bank,
            4,
            &AtomicBool::new(false),
        );
        let entered = crate::midi_compile::CURRENT_SAMPLER_COMPILER_ENTRIES.with(|n| n.get());
        if pass == 0 {
            result.unwrap();
            assert!(entered > 0);
            let report = custody.capacity_report();
            observed = Some((report.retained_stage_bytes, report.construction_peak_bytes));
        } else {
            assert!(matches!(result, Err(e) if e.reason == AudioStorageError::ReservationExceeded));
            assert_eq!(
                entered, 0,
                "refuse before compiling either MIDI representation"
            );
            assert!(bank.as_ref().unwrap().source_view(&token).is_ok());
        }
        custody.cleanup_on_control();
    }
}

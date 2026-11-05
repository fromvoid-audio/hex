use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
pub(super) fn fixture(
    streamed: bool,
    nonneutral: bool,
    clips: usize,
) -> (hex_project::Project, OwnedAudioInputs, AudioSourceToken) {
    let (p, b) = super::tests::fixture(clips);
    let mut snapshot = p.snapshot();
    if nonneutral {
        for c in snapshot.tracks[0].audio_clips_mut().unwrap() {
            c.source_offset = 1;
            c.source_length = 4;
            c.looping = true;
            c.source_phase = 2;
            c.fade = hex_project::ClipFade::new(c.duration, 0, 2, 2).unwrap();
        }
    }
    let p = hex_project::Project::from_snapshot(snapshot).unwrap();
    let asset = p.assets().next().unwrap().id();
    let b = if streamed {
        let reservation = b.reservation;
        let m = crate::StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(11, asset),
            48000,
            48000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            [9; 32],
        )
        .unwrap();
        OwnedAudioInputs::try_new(
            vec![(asset, OwnedAudioSource::try_streamed(m).ok().unwrap())],
            reservation,
        )
        .ok()
        .unwrap()
    } else {
        b
    };
    let token = b.source_token(asset).unwrap();
    (p, b, token)
}
fn begin(
    c: &mut AudioPreparationCustody,
    p: &hex_project::Project,
    b: OwnedAudioInputs,
) -> Result<AudioPreparationToken, Box<AudioArrangementRefusal>> {
    c.begin_input_on_control(AudioArrangementInput::new(p, 11, b).with_audio_clip_inventory())
}

#[test]
fn retained_receiver_refuses_closed_preparation_without_losing_inputs() {
    let (project, bank, token) = fixture(false, false, 1);
    let track = project.tracks()[0].id();
    let mut editor = hex_project::Editor::new(project, 8).unwrap();
    editor
        .execute(hex_project::Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.retained_source_paths.push(track.value());
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let pointer = bank.sources.as_ptr();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let refusal = match begin(&mut custody, &project, bank) {
        Ok(_) => panic!("retained path must refuse"),
        Err(refusal) => refusal,
    };
    assert_eq!(refusal.reason, AudioStorageError::UnsupportedGraph);
    assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
    assert!(refusal.inputs.source_view(&token).is_ok());
}
#[test]
fn actual_arrangement_clips_preserve_source_handles_geometry_fades_phases_and_metadata_capacity() {
    if !super::session_source_tests::supported("arrangement_final_clips") {
        return;
    }
    for streamed in [false, true] {
        for nonneutral in [false, true] {
            let (p, b, old) = fixture(streamed, nonneutral, 2);
            let ptr = b.sources.as_ptr();
            let cap = b.sources.capacity();
            let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
            let t = begin(&mut c, &p, b).ok().unwrap();
            assert_eq!(
                c.phase(),
                AudioPreparationPhase::ArrangementAndSessionSources
            );
            assert_eq!(c.installed_source_bindings(), 2);
            assert!(c.arrangement_clips.validate(c.bank.as_ref().unwrap()));
            assert_eq!(c.arrangement_clips.handles()[0], Some(c.slots[0].handle));
            assert_ne!(
                c.arrangement_clips.handles()[0],
                c.arrangement_clips.handles()[1]
            );
            for (clip, original) in c.arrangement_clips.clips().zip(p.tracks()[0].clips()) {
                assert_eq!(
                    (
                        clip.start,
                        clip.end,
                        clip.source_offset,
                        clip.source_length,
                        clip.looping
                    ),
                    (
                        original.start(),
                        original.start() + original.duration(),
                        original.source_offset() as usize,
                        original.source_length() as usize,
                        original.looping()
                    )
                );
                assert_eq!(
                    clip.metadata.as_ref().map_or(0, |m| m.source_phase),
                    original.source_phase()
                );
                assert_eq!(clip.metadata.is_some(), streamed || nonneutral);
                if !streamed {
                    let a = match &c.bank.as_ref().unwrap().sources[0].1.kind {
                        SourceKind::Resident(a) => a.clone(),
                        _ => panic!("resident"),
                    };
                    let legacy = crate::Clip::region_with_fade(
                        a,
                        original.start(),
                        original.duration(),
                        original.source_offset(),
                        original.source_length(),
                        original.looping(),
                        original.fade(),
                    )
                    .unwrap()
                    .with_source_phase(original.source_phase())
                    .unwrap();
                    for position in 0..24 {
                        assert_eq!(
                            clip.at(position, original.source_phase()),
                            legacy.at(position, original.source_phase())
                        );
                    }
                    assert_eq!(
                        clip.metadata.as_ref().and_then(|m| m.fade),
                        legacy.metadata.as_ref().and_then(|m| m.fade)
                    );
                }
            }
            let report = c.capacity_report();
            assert_eq!(
                report.arrangement_clips,
                c.arrangement_clips.capacity(true).unwrap()
            );
            assert_eq!(
                report.arrangement_clips.metadata_pointee_bytes,
                if streamed || nonneutral {
                    2 * mem::size_of::<crate::ClipMetadata>()
                } else {
                    0
                }
            );
            assert!(c.session_inventory_metadata().is_some());
            assert!(c.cancel(&t));
            c.cleanup_on_control();
            let b = c.recover_on_control(&t).unwrap();
            assert_eq!(b.sources.as_ptr(), ptr);
            assert_eq!(b.sources.capacity(), cap);
            assert!(b.source_view(&old).is_ok());
        }
    }
    eprintln!(
        "hex preparation evidence: arrangement_final_clips; supported-controls=completed; resident-streamed-neutral-phase-fade=4"
    );
}
#[test]
fn whole_batch_pair_mismatch_refuses_before_source_moves_and_recovers_exact_bank() {
    if !super::session_source_tests::supported("arrangement_final_refusal") {
        return;
    }
    let (p, b, old) = fixture(true, true, 2);
    let ptr = b.sources.as_ptr();
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    c.arrangement_clips.mismatch = true;
    let t = begin(&mut c, &p, b).ok().unwrap();
    assert_eq!(c.phase(), AudioPreparationPhase::Cleaning);
    assert_eq!(c.reason(), Some(AudioStorageError::SourceMismatch));
    assert_eq!(c.arrangement_clips.installed(), 0);
    assert!(
        c.slots
            .iter()
            .all(|s| s.owner.is_some() && s.disposal == Disposal::Unstarted)
    );
    assert!(c.session_inventory_metadata().is_none());
    c.cleanup_on_control();
    let b = c.recover_on_control(&t).unwrap();
    assert_eq!(b.sources.as_ptr(), ptr);
    assert!(b.source_view(&old).is_ok());
    eprintln!(
        "hex preparation evidence: arrangement_final_refusal; supported-controls=completed; originals=retained"
    );
}
#[test]
fn every_partial_move_failure_and_construction_panic_retains_persistent_custody() {
    if !super::session_source_tests::supported("arrangement_final_move_failures") {
        return;
    }
    for phase in 0..2 {
        let (p, b, old) = fixture(true, true, 2);
        let ptr = b.sources.as_ptr();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        c.arrangement_clips.fail_after = Some(phase);
        let t = begin(&mut c, &p, b).ok().unwrap();
        assert_eq!(c.phase(), AudioPreparationPhase::Cleaning);
        assert_eq!(c.arrangement_clips.installed(), phase + 1);
        c.cleanup_on_control();
        let b = c.recover_on_control(&t).unwrap();
        assert_eq!(b.sources.as_ptr(), ptr);
        assert!(b.source_view(&old).is_ok());
        c.arrangement_clips.fail_after = None;
        let t = begin(&mut c, &p, b).ok().unwrap();
        assert_eq!(c.arrangement_clips.installed(), 2);
        assert!(c.cancel(&t));
        c.cleanup_on_control();
        assert!(c.recover_on_control(&t).is_some());
        let (p, b, _) = fixture(true, true, 2);
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        c.arrangement_clips.panic_after = Some(phase);
        let t = begin(&mut c, &p, b).ok().unwrap();
        assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
        assert!(c.first_panic.is_some());
        let installed = c.arrangement_clips.installed();
        c.cleanup_on_control();
        assert_eq!(c.arrangement_clips.installed(), installed);
        assert!(c.recover_on_control(&t).is_none());
    }
    eprintln!(
        "hex preparation evidence: arrangement_final_move_failures; supported-controls=completed; failures=2; panics=2"
    );
}
#[test]
fn actual_source_destructor_probe_keeps_metadata_remaining_pairs_bank_and_first_payload_no_retry() {
    if !super::session_source_tests::supported("arrangement_final_disposal_panic") {
        return;
    }
    let (p, b, _) = fixture(true, true, 2);
    let ptr = b.sources.as_ptr();
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let t = begin(&mut c, &p, b).ok().unwrap();
    let n = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&n);
    let id = std::thread::current().id();
    c.arrangement_clips.probe = Some(DropProbe(Some(Box::new(move || {
        assert_eq!(std::thread::current().id(), id);
        observed.fetch_add(1, Ordering::Relaxed);
        panic!("actual paired Arrangement source destructor probe");
    }))));
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(c.arrangement_clips.pending_metadata_present());
    assert!(c.arrangement_clips.pending_stream_empty());
    assert_eq!(c.arrangement_clips.installed(), 1);
    assert!(c.first_panic.is_some());
    assert_eq!(c.bank.as_ref().unwrap().sources.as_ptr(), ptr);
    c.cleanup_on_control();
    assert!(c.recover_on_control(&t).is_none());
    assert_eq!(n.load(Ordering::Relaxed), 1);
    eprintln!(
        "hex preparation evidence: arrangement_final_disposal_panic; supported-controls=completed; destructor-count=1"
    );
}
#[test]
fn actual_pair_and_box_spares_growth_peak_refusal_and_clone_conservation() {
    if !super::session_source_tests::supported("arrangement_final_capacity") {
        return;
    }
    let (p, b, old) = fixture(true, true, 2);
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    crate::owned_shared::reset_clone_count();
    let t = begin(&mut c, &p, b).ok().unwrap();
    let clones = crate::owned_shared::clone_count();
    assert!(clones > 0);
    let first = c.capacity_report();
    assert!(first.arrangement_clips.bytes().unwrap() > 0);
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    let b = c.recover_on_control(&t).unwrap();
    let t = begin(&mut c, &p, b).ok().unwrap();
    assert_eq!(
        c.capacity_report()
            .arrangement_clips
            .previous_growth_backing_bytes,
        0
    );
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    let b = c.recover_on_control(&t).unwrap();
    assert!(b.source_view(&old).is_ok());
    let t = c.begin_arrangement_on_control(&p, 11, b).ok().unwrap();
    assert_eq!(
        c.capacity_report().arrangement_clips.metadata_pointee_bytes,
        0
    );
    assert_eq!(
        c.capacity_report().arrangement_clips.pair_backing_bytes,
        first.arrangement_clips.pair_backing_bytes
    );
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    let _b = c.recover_on_control(&t).unwrap();
    let (grown, b, _) = fixture(true, true, 4);
    let t = begin(&mut c, &grown, b).ok().unwrap();
    let report = c.capacity_report();
    assert!(report.arrangement_clips.previous_growth_backing_bytes > 0);
    assert_eq!(
        report.construction_peak_bytes,
        report.retained_stage_bytes
            + report.previous_slot_backing_bytes
            + report.session.previous_backing_bytes
            + report.structure.previous_growth_backing_bytes
            + report.tempo.previous_growth_backing_bytes
            + report.assembly.previous_growth_backing_bytes
            + report.arrangement_clips.previous_growth_backing_bytes
    );
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&t).is_some());
    let (p, mut b, token) = fixture(true, true, 2);
    let ptr = b.sources.as_ptr();
    let cap = b.sources.capacity();
    b.reservation.max_prepared_data_bytes = first.retained_stage_bytes - 1;
    let mut fresh = AudioPreparationCustody::try_new_on_control().unwrap();
    let refusal = begin(&mut fresh, &p, b).err().unwrap();
    assert_eq!(refusal.reason, AudioStorageError::ReservationExceeded);
    assert_eq!(refusal.inputs.sources.as_ptr(), ptr);
    assert_eq!(refusal.inputs.sources.capacity(), cap);
    assert!(refusal.inputs.source_view(&token).is_ok());
    assert!(refusal.capacity.arrangement_clips.metadata_pointee_bytes > 0);
    let (p, b, _) = fixture(true, true, 2);
    let mut other = AudioPreparationCustody::try_new_on_control().unwrap();
    crate::owned_shared::reset_clone_count();
    let t = other
        .begin_input_on_control(AudioArrangementInput::new(&p, 11, b).with_session_inventory())
        .ok()
        .unwrap();
    assert_eq!(crate::owned_shared::clone_count(), clones);
    assert!(other.cancel(&t));
    other.cleanup_on_control();
    assert!(other.recover_on_control(&t).is_some());
    assert_eq!(
        ArrangementClipCapacity {
            pair_backing_bytes: usize::MAX,
            metadata_pointee_bytes: 1,
            ..Default::default()
        }
        .bytes(),
        Err(AudioStorageError::Overflow)
    );
    eprintln!(
        "hex preparation evidence: arrangement_final_capacity; supported-controls=completed; staging-clones={clones}"
    );
}

#[test]
fn occupied_custodian_returns_new_original_bank_without_touching_existing_final_clips() {
    if !super::session_source_tests::supported("arrangement_final_busy") {
        return;
    }
    let (p, b, _) = fixture(true, true, 2);
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let live = begin(&mut c, &p, b).ok().unwrap();
    let handles = c.arrangement_clips.handles();
    let capacity = c.arrangement_clips.capacity(false).unwrap();
    let (other, b, token) = fixture(true, true, 2);
    let pointer = b.sources.as_ptr();
    let source_capacity = b.sources.capacity();
    let reservation = b.reservation;
    let refusal = begin(&mut c, &other, b).err().unwrap();
    assert_eq!(refusal.reason, AudioStorageError::Capacity);
    assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
    assert_eq!(refusal.inputs.sources.capacity(), source_capacity);
    assert_eq!(refusal.inputs.reservation, reservation);
    assert!(refusal.inputs.source_view(&token).is_ok());
    assert!(c.matches(&live));
    assert_eq!(c.arrangement_clips.handles(), handles);
    assert_eq!(c.arrangement_clips.capacity(false).unwrap(), capacity);
    assert!(c.arrangement_clips.validate(c.bank.as_ref().unwrap()));
    assert!(c.cancel(&live));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&live).is_some());
    eprintln!(
        "hex preparation evidence: arrangement_final_busy; supported-controls=completed; originals=retained"
    );
}
#[test]
fn equal_pcm_values_in_distinct_actual_allocations_are_not_content_deduplicated() {
    if !super::session_source_tests::supported("arrangement_final_distinct") {
        return;
    }
    let (p, b) = super::tests::fixture(1);
    let first = p.assets().next().unwrap().id();
    let track = p.tracks()[0].id();
    let reservation = b.reservation;
    drop(b);
    let mut editor = hex_project::Editor::new(p, 16).unwrap();
    let hex_project::Created::Asset(second) = editor
        .execute(hex_project::Command::AddAsset {
            path: "equal-data.wav".into(),
            sample_rate: 48000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor
        .execute(hex_project::Command::PlaceClip {
            track,
            asset: second,
            start: 8,
            duration: 8,
            source_offset: 0,
            source_length: 8,
            looping: false,
        })
        .unwrap();
    let sources = [first, second]
        .into_iter()
        .map(|id| {
            (
                id,
                OwnedAudioSource::try_resident(48000, vec![[0.25, -0.25]; 8])
                    .ok()
                    .unwrap(),
            )
        })
        .collect();
    let b = OwnedAudioInputs::try_new(sources, reservation)
        .ok()
        .unwrap();
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut c, editor.project(), b).ok().unwrap();
    assert!(c.arrangement_clips.validate(c.bank.as_ref().unwrap()));
    {
        let mut clips = c.arrangement_clips.clips();
        let a = clips.next().unwrap().audio.as_ref().unwrap();
        let b = clips.next().unwrap().audio.as_ref().unwrap();
        assert!(!a.shares_pcm_with(b));
        assert_eq!(a.frames(), b.frames());
    }
    let sum = c
        .bank
        .as_ref()
        .unwrap()
        .sources
        .iter()
        .map(|(_, s)| {
            let (p, o) = crate::audio_input_storage::parts(s).unwrap();
            p + o
        })
        .sum::<usize>();
    assert_eq!(c.capacity_report().declared_source_bytes, sum);
    assert_ne!(
        c.arrangement_clips.handles()[0],
        c.arrangement_clips.handles()[1]
    );
    assert!(c.cancel(&token));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&token).is_some());
    eprintln!(
        "hex preparation evidence: arrangement_final_distinct; supported-controls=completed; equal-content-allocations=2"
    );
}

#[test]
fn endpoint_only_fades_construct_resident_metadata_and_recover_exact_source_bank() {
    for (start, end) in [(1, 0), (0, 2), (1, 2)] {
        let (p, mut bank, old) = fixture(false, false, 1);
        bank.reservation.handle_obligations = 64;
        let mut snapshot = p.snapshot();
        let clip = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
        clip.fade = hex_project::ClipFade::new(clip.duration, 0, 0, 0)
            .unwrap()
            .with_shape(start, end, 500, -500)
            .unwrap();
        let project = hex_project::Project::from_snapshot(snapshot).unwrap();
        let original = &project.tracks()[0].clips()[0];
        let source = match &bank.sources[0].1.kind {
            SourceKind::Resident(audio) => audio.frames().to_vec(),
            _ => panic!("resident fixture"),
        };
        let ptr = bank.sources.as_ptr();
        let cap = bank.sources.capacity();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_input_on_control(
                AudioArrangementInput::new(&project, 11, bank).with_audio_prepared_revision(),
            )
            .ok()
            .unwrap();
        assert_eq!(
            custody.phase(),
            AudioPreparationPhase::ArrangementAndSessionSources,
            "preparation refusal: {:?}",
            custody.reason()
        );
        let revision = custody.revision.owner_for_test().unwrap().as_ref();
        let clip = &revision.arrangement().tracks()[0].clips[0];
        assert_eq!(clip.metadata.as_ref().unwrap().fade, Some(original.fade()));
        let mut renderer = crate::Renderer::new(revision.arrangement());
        renderer.play();
        let mut actual = vec![[0.0; 2]; original.duration() as usize];
        for chunk in actual.chunks_mut(3) {
            renderer.render(chunk).unwrap();
        }
        let mut expected = source;
        expected[..start as usize].fill([0.0; 2]);
        expected[original.duration() as usize - end as usize..].fill([0.0; 2]);
        assert_eq!(actual, expected);
        renderer.seek(1);
        let mut seek = [[0.0; 2]; 5];
        renderer.render(&mut seek).unwrap();
        assert_eq!(seek, expected[1..6]);
        assert_eq!(
            custody
                .capacity_report()
                .arrangement_clips
                .metadata_pointee_bytes,
            mem::size_of::<crate::ClipMetadata>()
        );
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        let bank = custody.recover_on_control(&token).unwrap();
        assert_eq!(bank.sources.as_ptr(), ptr);
        assert_eq!(bank.sources.capacity(), cap);
        assert!(bank.source_view(&old).is_ok());
    }
}

#[test]
fn sparse_native_arrangement_retains_only_event_pcm_and_recovers_source_bank() {
    use hex_project::{AudioEventEdit, Command, Editor, TrimEdge};
    if !super::session_source_tests::supported("sparse_native_arrangement") {
        return;
    }
    let mut prior_quote = None;
    let mut prior_retained = None;
    for destination in [32_000, 4_000_000] {
        let (project, mut bank, original) = fixture(false, false, 1);
        bank.reservation.handle_obligations = 64;
        let source_bank = bank.sources.as_ptr();
        let clip = project.tracks()[0].clips()[0].id();
        let mut editor = Editor::new(project, 8).unwrap();
        for operation in [
            AudioEventEdit::Split { at: 4 },
            AudioEventEdit::Place {
                event: 1,
                start: destination,
            },
        ] {
            editor
                .execute(Command::EditAudioEvents { clip, operation })
                .unwrap();
        }
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Right,
                frame: destination as u64 + 4,
            })
            .unwrap();
        let project = editor.project();
        let quote = transformed_audio_quote(project).unwrap();
        assert_eq!(*prior_quote.get_or_insert(quote), quote);
        assert!(quote.0 < 1_024, "only eight frames and two span entries");
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_input_on_control(
                AudioArrangementInput::new(project, 11, bank).with_audio_prepared_revision(),
            )
            .ok()
            .unwrap();
        assert_eq!(
            custody.phase(),
            AudioPreparationPhase::ArrangementAndSessionSources,
            "{:?}",
            custody.reason()
        );
        let revision = custody.revision.owner_for_test().unwrap().as_ref();
        let compiled = &revision.arrangement().tracks()[0].clips[0];
        assert_eq!(compiled.audio.as_ref().unwrap().frames().len(), 8);
        assert!(compiled.metadata.as_ref().unwrap().sparse.is_some());
        let mut renderer = crate::Renderer::new(revision.arrangement());
        renderer.seek(destination as u64 - 2);
        renderer.play();
        let mut actual = [[0.0; 2]; 8];
        for chunk in actual.chunks_mut(3) {
            renderer.render(chunk).unwrap();
        }
        assert_eq!(
            actual,
            [
                [0.0; 2], [0.0; 2], [0.25; 2], [0.25; 2], [0.25; 2], [0.25; 2], [0.0; 2], [0.0; 2]
            ]
        );
        {
            let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
                crate::AudioStorageLedgerReservation {
                    data_rows: 1024,
                    handle_rows: 128,
                    root_rows: 8,
                    max_retained_bytes: usize::MAX,
                    max_construction_peak_bytes: usize::MAX,
                },
            )
            .unwrap();
            ledger
                .borrow_custody_on_control(&custody, crate::AudioStorageRole::Current)
                .unwrap();
            let summary = ledger.summary().unwrap();
            assert!(summary.require_known_storage().is_ok());
            assert_eq!(
                *prior_retained.get_or_insert(summary.retained_bytes),
                summary.retained_bytes
            );
        }
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        let mut bank = custody.recover_on_control(&token).unwrap();
        assert_eq!(bank.sources.as_ptr(), source_bank);
        assert!(bank.source_view(&original).is_ok());

        bank.reservation.max_prepared_data_bytes = quote.0 - 1;
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let refusal = custody
            .begin_input_on_control(
                AudioArrangementInput::new(project, 11, bank).with_audio_prepared_revision(),
            )
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStorageError::ReservationExceeded);
        assert_eq!(refusal.inputs.sources.as_ptr(), source_bank);
        assert!(refusal.inputs.source_view(&original).is_ok());
    }
}

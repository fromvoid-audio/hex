use super::*;
use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation};

#[test]
fn actual_session_clip_and_exported_views_conserve_paired_waveform_and_pcm_fields() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported constructor; no positive exported waveform ownership proof");
        return;
    }
    eprintln!("executing supported actual Session/clip/ViewState waveform ownership path");
    let mut session = Session::new(48_000, 8, 1024).unwrap();
    let source = hex_engine::OwnedAudioSource::try_resident(48_000, vec![[0.25, -0.5]; 4])
        .ok()
        .unwrap();
    let prepared = crate::PreparedImport::from_native_source(
        "waveform.wav".into(),
        source,
        48_000,
        hex_media::ResampleLimits {
            max_source_frames: 8,
            max_output_frames: 8,
        },
    )
    .unwrap();
    let (Created::Imported { asset, .. }, revision) =
        session.import_prepared_revision(prepared, None).unwrap()
    else {
        panic!("import")
    };
    drop(revision);
    let first = view_state(&session, Arc::default(), 1, 1, false, None);
    let second = view_state(&session, Arc::default(), 1, 1, false, None);
    assert_eq!(first.waveforms.len(), 1);
    assert_eq!(first.audio_clip_waveforms.len(), 1);
    assert!(!std::ptr::eq(
        first.waveforms.as_slice().as_ptr(),
        second.waveforms.as_slice().as_ptr()
    ));
    session
        .with_resident_pcm_inventory_on_control(|inventory| {
            let mut ledger =
                AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                    data_rows: 128,
                    handle_rows: 128,
                    root_rows: 32,
                    max_retained_bytes: 2_000_000,
                    max_construction_peak_bytes: 2_000_000,
                })?;
            inventory.visit_waveform_storage_on_control(&mut ledger)?;
            // Actual original/derivative fields are embedded in the already charged
            // resident entry. Snapshot bindings are actual additional PCM fields.
            ledger.borrow_embedded_native_audio_on_control(session.source_audio(asset).unwrap())?;
            ledger.borrow_embedded_native_audio_on_control(session.audio(asset).unwrap())?;
            for view in [&first, &second] {
                let snapshot = view.canonical_audio_source.as_ref().unwrap();
                ledger.borrow_metadata_entries_backing_on_control(snapshot.sources())?;
                for binding in snapshot.sources().values() {
                    let crate::AudioSourceBinding::Resident(audio) = binding else {
                        panic!("resident")
                    };
                    ledger.borrow_embedded_native_audio_on_control(audio)?;
                }
            }
            first.visit_waveform_storage_on_control(&mut ledger)?;
            // All actual PCM fields were observed above. The missing second view's
            // paired waveform aliases still independently prevent closure.
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            let before = ledger.summary()?.data_bytes;
            let omitted_maps = second.waveforms.backing_bytes().unwrap()
                + second.audio_clip_waveforms.backing_bytes().unwrap();
            assert!(omitted_maps > 0);
            second.visit_waveform_storage_on_control(&mut ledger)?;
            assert_eq!(ledger.summary()?.data_bytes, before + omitted_maps);
            ledger.validate_selected_shared_handles_on_control()?;
            // Two summary bodies, one shared PCM allocation; six outer handles and
            // six actual embedded/resident/snapshot PCM handles, not copied PCM.
            assert_eq!(ledger.selected_shared_counts_on_control(), (3, 12));
            let complete_selected = ledger.summary()?;
            inventory.visit_waveform_storage_on_control(&mut ledger)?;
            first.visit_waveform_storage_on_control(&mut ledger)?;
            second.visit_waveform_storage_on_control(&mut ledger)?;
            assert_eq!(ledger.summary()?.data_bytes, complete_selected.data_bytes);
            ledger.validate_selected_shared_handles_on_control()?;
            assert!(complete_selected.require_complete().is_err());
            Ok(())
        })
        .unwrap();
    drop(first);
    drop(session);
    assert_eq!(
        second
            .waveforms
            .get(&asset)
            .unwrap()
            .peaks_for_frames(0, 4)
            .unwrap()[1]
            .min,
        -0.5
    );
    // Old exports retain their original paired source after producer retirement.
    assert_eq!(
        second
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .len(),
        1
    );
}

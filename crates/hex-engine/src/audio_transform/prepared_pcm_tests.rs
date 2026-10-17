use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation, AudioStorageRole,
    OwnedAudioSource,
};
fn limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 16,
        handle_rows: 8,
        root_rows: 8,
        max_retained_bytes: 65_536,
        max_construction_peak_bytes: 65_536,
    }
}

#[test]
fn fresh_prepared_pcm_has_distinct_origin_exact_capacity_union_and_control_final_release() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported constructor; no prepared PCM ownership proof");
        return;
    }
    let mut frames = Vec::with_capacity(17);
    frames.extend([[0.25, -0.5], [0.75, -1.0], [-0.0, 0.0], [1.0, -0.25]]);
    let pointer = frames.as_ptr();
    let bytes = frames.capacity() * std::mem::size_of::<crate::Frame>();
    let released = budget::RELEASED_PRODUCTION_RESERVATIONS.with(|counter| counter.get());
    let audio = Audio::from_prepared_derived(
        48_000,
        frames,
        budget::production_budget().reserve(bytes).unwrap(),
    )
    .unwrap();
    assert_eq!(audio.frames().as_ptr(), pointer);
    assert_eq!(audio.pcm_capacity_frames(), 17);
    assert_eq!(audio.frames()[2][0].to_bits(), (-0.0_f32).to_bits());
    let alias = audio.clone();
    assert!(alias.shares_pcm_with(&audio));
    assert_ne!(alias.native_handle, audio.native_handle);
    // Prepared clip output never becomes a sampler's original source.
    assert!(matches!(
        OwnedAudioSource::alias_native_audio_on_control(&audio, 4096),
        Err(AudioStorageError::UnknownStorage)
    ));
    let refused = OwnedAudioSource::try_from_native_audio_on_control(audio.clone())
        .err()
        .unwrap();
    assert_eq!(refused.reason, AudioStorageError::UnknownStorage);
    drop(refused);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    ledger
        .borrow_native_audio_on_control(&audio, AudioStorageRole::Current)
        .unwrap();
    assert_eq!(
        ledger.validate_selected_shared_handles_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    ledger
        .borrow_native_audio_on_control(&alias, AudioStorageRole::Archive)
        .unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(ledger.selected_shared_counts_on_control(), (1, 2)); // no hidden budget Arc
    let retained = ledger.summary().unwrap().retained_bytes;
    drop(ledger);
    let mut small = limits();
    small.max_retained_bytes = retained - 1;
    let refused = (|| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(small)?;
        ledger.borrow_native_audio_on_control(&audio, AudioStorageRole::Current)?;
        ledger.borrow_native_audio_on_control(&alias, AudioStorageRole::Archive)?;
        ledger.summary()
    })();
    assert!(matches!(
        refused,
        Err(AudioStorageError::ReservationExceeded)
    ));
    drop(audio);
    assert_eq!(
        budget::RELEASED_PRODUCTION_RESERVATIONS.with(|counter| counter.get()),
        released
    );
    assert_eq!(alias.frames().as_ptr(), pointer);
    drop(alias); // actual last PCM owner and reservation retire on this control thread
    assert_eq!(
        budget::RELEASED_PRODUCTION_RESERVATIONS.with(|counter| counter.get()),
        (released.0 + 1, released.1 + bytes)
    );
}

#[test]
fn prepared_constructor_refuses_unreserved_capacity_and_unqualified_derived_stays_unknown() {
    let mut values = Vec::with_capacity(17);
    values.resize(4, [0.25, -0.5]);
    assert!(
        Audio::from_prepared_derived(
            48_000,
            values,
            budget::production_budget()
                .reserve(4 * std::mem::size_of::<crate::Frame>())
                .unwrap()
        )
        .is_err()
    );
    let local = budget::BudgetOwner::local(1024);
    let audio =
        Audio::from_prepared_derived(48_000, vec![[0.25; 2]; 4], local.reserve(32).unwrap())
            .unwrap();
    assert!(audio.pcm_arc_receipt.is_none());
    assert!(audio.native_handle.is_none());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    assert_eq!(
        ledger.borrow_native_audio_on_control(&audio, AudioStorageRole::Current),
        Err(AudioStorageError::UnknownStorage)
    );
    assert_eq!(local.used(), 32);
    drop(audio);
    assert_eq!(local.used(), 0);
    let reservation = budget::production_budget().reserve(32).unwrap();
    let raw = Audio::from_derived(48_000, vec![[0.5; 2]; 4], reservation).unwrap();
    assert!(raw.pcm_arc_receipt.is_none());
    assert!(matches!(
        OwnedAudioSource::alias_native_audio_on_control(&raw, 4096),
        Err(AudioStorageError::UnknownStorage)
    ));
}

fn visit_candidate_pcm<'a>(
    candidate: &'a crate::PreparedRevision,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    for track in &candidate.arrangement().tracks {
        for clip in &track.clips {
            if let Some(audio) = &clip.audio {
                ledger.borrow_native_audio_on_control(audio, AudioStorageRole::Current)?;
            }
        }
    }
    for view in candidate.arrangement().audio_clip_views() {
        ledger.borrow_native_audio_on_control(&view.audio, AudioStorageRole::Current)?;
    }
    Ok(())
}

#[test]
fn actual_pending_and_committed_cache_fields_are_required_and_refused_transaction_retires_pcm_on_control()
 {
    use hex_project::{Command, Created, Editor, Project, SourceGainPoint};
    use std::collections::BTreeMap;
    if !crate::audio_storage_constructor_layout_supported() {
        return;
    }
    let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "prepared".into(),
            path: "prepared.wav".into(),
            sample_rate: 16_000,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 32768,
                },
                SourceGainPoint {
                    frame: 512,
                    gain_q16: 32768,
                },
            ],
        })
        .unwrap();
    let original = OwnedAudioSource::try_resident(16_000, vec![[0.5, -0.25]; 512])
        .ok()
        .unwrap()
        .try_into_native_audio_on_control()
        .ok()
        .unwrap();
    let media = BTreeMap::from([(asset, original)]);
    let context = AudioTransformPreparation::default();
    {
        let busy = context.cache.borrow_mut();
        assert!(matches!(
            context.with_pcm_inventory_on_control(|_| Ok(())),
            Err(AudioStorageError::UnknownStorage)
        ));
        drop(busy);
        assert!(context.pending.borrow().is_none());
    }
    let prepare = || {
        crate::PreparedRevision::prepare_with_audio_transforms(
            editor.project(),
            &media,
            Arc::new(crate::SineTable::new()),
            None,
            &context,
        )
        .unwrap()
    };
    let released = budget::RELEASED_PRODUCTION_RESERVATIONS.with(|counter| counter.get());
    let mut limits = limits();
    limits.data_rows = 64;
    limits.handle_rows = 32;
    limits.root_rows = 32;
    let failed: Result<(), Invalid> = context.transaction(|| {
        let candidate = prepare();
        context
            .with_pcm_inventory_on_control(|inventory| {
                let mut ledger = AudioRootStorageLedger::try_new_on_control(limits)?;
                ledger.borrow_native_audio_on_control(&media[&asset], AudioStorageRole::Caller)?;
                visit_candidate_pcm(&candidate, &mut ledger)?;
                assert_eq!(
                    ledger.validate_selected_shared_handles_on_control(),
                    Err(AudioStorageError::UnknownStorage)
                );
                inventory.visit_on_control(&mut ledger, AudioStorageRole::FailedConstruction)?;
                ledger.validate_selected_shared_handles_on_control()?;
                assert_eq!(ledger.selected_shared_counts_on_control().0, 2);
                Ok(())
            })
            .unwrap();
        Err(Invalid("injected publication refusal"))
    });
    assert!(failed.is_err());
    assert!(context.pending.borrow().is_none());
    assert!(context.cache.borrow().entries.is_empty());
    assert_eq!(
        budget::RELEASED_PRODUCTION_RESERVATIONS.with(|counter| counter.get()),
        (released.0 + 1, released.1 + 512 * 8)
    );
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
    ledger
        .borrow_native_audio_on_control(&media[&asset], AudioStorageRole::Caller)
        .unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap(); // actual pending source aliases retired
    drop(ledger);
    let candidate = context.transaction(|| Ok(prepare())).unwrap();
    context
        .with_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits)?;
            ledger.borrow_native_audio_on_control(&media[&asset], AudioStorageRole::Caller)?;
            visit_candidate_pcm(&candidate, &mut ledger)?;
            assert_eq!(
                ledger.validate_selected_shared_handles_on_control(),
                Err(AudioStorageError::UnknownStorage)
            );
            inventory.visit_on_control(&mut ledger, AudioStorageRole::Caller)?;
            ledger.validate_selected_shared_handles_on_control()
        })
        .unwrap();
    drop(candidate);
    drop(context);
    assert_eq!(
        budget::RELEASED_PRODUCTION_RESERVATIONS.with(|counter| counter.get()),
        (released.0 + 2, released.1 + 2 * 512 * 8)
    );
}

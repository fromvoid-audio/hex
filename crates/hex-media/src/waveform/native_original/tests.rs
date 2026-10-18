use super::*;
use hex_engine::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation, AudioStorageRole,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
fn supported() -> bool {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported native original waveform");
        false
    } else {
        true
    }
}
fn limits(frames: usize, bins: usize) -> NativeOriginalWaveformLimits {
    NativeOriginalWaveformLimits {
        bins,
        scan_frames: frames,
        metadata_bytes: native_original_metadata_bytes(frames, bins).unwrap(),
    }
}
fn ledger_limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 32,
        root_rows: 16,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
#[test]
fn native_original_waveform_mixed_rate_layout_queries_retain_exact_original_without_bank() {
    if !supported() {
        return;
    }
    for stereo in [false, true] {
        eprintln!("SUPPORTED native original waveform stereo={stereo}");
        let values: Vec<_> = (0..1025).map(|i| (i % 19) as f32 / 20.0 - 0.5).collect();
        let expected: Vec<_> = values
            .iter()
            .map(|value| {
                if stereo {
                    [*value, -*value]
                } else {
                    [*value; 2]
                }
            })
            .collect();
        let source = if stereo {
            OwnedAudioSource::try_resident(44_100, expected.clone())
                .ok()
                .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(24_000, values)
                .ok()
                .unwrap()
        };
        let owner =
            summarize_native_source_on_worker(&source, limits(1025, 17), &AtomicBool::new(false))
                .unwrap();
        let reference =
            summarize(&Audio::new(source.sample_rate(), expected).unwrap(), 17).unwrap();
        let backing = owner.native_original_source().unwrap();
        assert_eq!(backing.sample_rate(), source.sample_rate());
        assert_eq!(backing.frames(), source.frames());
        if stereo {
            assert!(std::ptr::eq(
                backing.view().resident_frames().unwrap(),
                source.view().resident_frames().unwrap()
            ));
        } else {
            assert!(std::ptr::eq(
                backing.view().resident_mono_samples().unwrap(),
                source.view().resident_mono_samples().unwrap()
            ));
        }
        assert!(owner.audio_source().is_none() && owner.original_source().is_none());
        assert_eq!(owner.bins, reference.bins);
        for (start, end) in [(0, 1025), (1, 2), (37, 101), (65, 513), (1024, 1025)] {
            assert_eq!(
                read_shared(&owner).peaks_for_frames(start, end),
                reference.peaks_for_frames(start, end)
            );
        }
        let old_export = owner.clone();
        drop(owner);
        drop(source);
        assert_eq!(
            read_shared(&old_export).peaks_for_frames(13, 14),
            reference.peaks_for_frames(13, 14)
        );
    }
    let audio = OwnedAudioSource::try_resident(32_000, vec![[0.25, -0.5]; 9])
        .ok()
        .unwrap()
        .try_into_native_audio_on_control()
        .ok()
        .unwrap();
    let owner =
        summarize_native_audio_on_worker(&audio, limits(9, 3), &AtomicBool::new(false)).unwrap();
    assert!(owner.shares_audio_with(&audio));
    assert_eq!(
        actual_metadata_bytes(&owner).unwrap(),
        native_original_metadata_bytes(9, 3).unwrap()
    );
    assert_eq!(owner.levels.capacity(), PYRAMID_LEVEL_CAPACITY);
    drop(audio);
    assert_eq!(owner.read().peaks_for_frames(0, 1).unwrap()[1].min, -0.5);
}
struct CancelAt {
    calls: AtomicUsize,
    at: usize,
}
impl hex_engine::CancellationQuery for CancelAt {
    fn is_cancelled(&self) -> bool {
        self.calls.fetch_add(1, Ordering::Relaxed) + 1 == self.at
    }
}
#[test]
fn native_original_waveform_refuses_raw_derived_streamed_budget_and_midscan_cancel() {
    if !supported() {
        return;
    }
    eprintln!("SUPPORTED native original waveform refusal and cancellation");
    let raw = Audio::new(24_000, vec![[0.25, -0.5]; 1025]).unwrap();
    assert!(matches!(
        summarize_native_audio_on_worker(&raw, limits(1025, 3), &AtomicBool::new(false)),
        Err(DeviceSourceImportError::Storage(
            AudioStorageError::UnknownStorage
        ))
    ));
    let mut transform_editor =
        hex_project::Editor::new(hex_project::Project::new(24_000).unwrap(), 1).unwrap();
    let hex_project::Created::Imported { clip, asset, .. } = transform_editor
        .execute(hex_project::Command::ImportAudioTrack {
            name: "transformed".into(),
            path: "transformed.wav".into(),
            sample_rate: 24_000,
            frames: 1025,
        })
        .unwrap()
    else {
        panic!("import")
    };
    transform_editor
        .execute(hex_project::Command::SetAudioClipPitch { clip, cents: 100 })
        .unwrap();
    let transforms = hex_engine::audio_transform::AudioTransformPreparation::new(
        std::sync::Arc::new(AtomicBool::new(false)),
    );
    let prepared = hex_engine::PreparedRevision::prepare_with_audio_transforms(
        transform_editor.project(),
        &std::collections::BTreeMap::from([(asset, raw.clone())]),
        hex_engine::SharedSineTable::new_on_control(),
        None,
        &transforms,
    )
    .unwrap();
    let derived = &prepared.arrangement().audio_clip_views()[0].audio;
    assert!(
        summarize_native_audio_on_worker(
            derived,
            limits(derived.frames().len(), 3),
            &AtomicBool::new(false)
        )
        .is_err()
    );
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
    let hex_project::Created::Asset(asset) = editor
        .execute(hex_project::Command::AddAsset {
            path: "stream.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let metadata = hex_engine::StreamedSourceMetadata::new(
        hex_engine::StreamSourceKey::new(11, asset),
        48_000,
        48_000,
        4,
        4,
        hex_project::IDENTITY_CONVERSION,
        [9; 32],
    )
    .unwrap();
    let streamed = OwnedAudioSource::try_streamed(metadata).ok().unwrap();
    assert!(
        summarize_native_source_on_worker(&streamed, limits(4, 3), &AtomicBool::new(false))
            .is_err()
    );
    let source = OwnedAudioSource::try_resident_mono(24_000, vec![0.25; 1025])
        .ok()
        .unwrap();
    let mut budget = limits(1025, 3);
    budget.metadata_bytes -= 1;
    assert!(matches!(
        summarize_native_source_on_worker(&source, budget, &AtomicBool::new(false)),
        Err(DeviceSourceImportError::Resource(
            ResourcePreparationError::MetadataCapacity
        ))
    ));
    let cancel = CancelAt {
        calls: AtomicUsize::new(0),
        at: 4,
    };
    assert!(matches!(
        summarize_native_source_on_worker(&source, limits(1025, 3), &cancel),
        Err(DeviceSourceImportError::Media(ImportError::Cancelled))
    ));
    assert_eq!(cancel.calls.load(Ordering::Relaxed), 4);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits()).unwrap();
    ledger
        .borrow_native_source_on_control(&source, AudioStorageRole::Caller)
        .unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(
        source.view().resident_mono_samples().unwrap(),
        &[0.25; 1025]
    );
}
#[test]
fn native_original_waveform_actual_spare_backings_alias_omission_and_exact_budget() {
    if !supported() {
        return;
    }
    eprintln!("SUPPORTED native original waveform actual spare storage");
    let source = OwnedAudioSource::try_resident_mono(24_000, vec![0.25; 9])
        .ok()
        .unwrap();
    let mut bins = Vec::with_capacity(19);
    bins.extend(
        [[Peak {
            min: 0.25,
            max: 0.25,
        }; 2]; 3],
    );
    let mut value = Waveform::from_peaks(9, bins).unwrap();
    value.levels.try_reserve_exact(31).unwrap();
    for level in &mut value.levels {
        level.try_reserve_exact(13).unwrap();
    }
    value.samples = Some(SampleBacking::NativeOriginal(
        source
            .alias_for_resource_plan_on_control(size_of::<OwnedAudioSource>())
            .unwrap(),
    ));
    let expected = actual_metadata_bytes(&value).unwrap();
    let owner = SharedWaveform::new_on_control(value);
    let alias = owner.clone();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits()).unwrap();
    ledger
        .borrow_native_source_on_control(&source, AudioStorageRole::Caller)
        .unwrap();
    let pcm = ledger.summary().unwrap().data_bytes;
    ledger
        .borrow_shared_metadata_body_on_control(&owner)
        .unwrap();
    ledger
        .borrow_shared_metadata_body_on_control(&alias)
        .unwrap();
    assert!(
        ledger
            .validate_selected_shared_handles_on_control()
            .is_err(),
        "whole embedded source field omitted"
    );
    borrow_shared_storage_on_control(&owner, &mut ledger).unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, pcm + expected);
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    let before = ledger.summary().unwrap();
    borrow_shared_storage_on_control(&owner, &mut ledger).unwrap();
    borrow_shared_storage_on_control(&alias, &mut ledger).unwrap();
    assert_eq!(ledger.summary().unwrap(), before);
    drop(ledger);
    let mut exact = ledger_limits();
    exact.max_retained_bytes = before.retained_bytes;
    exact.max_construction_peak_bytes = before.construction_peak_bytes;
    let collect = |limit| {
        let mut budget = exact;
        budget.max_retained_bytes = limit;
        let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
        ledger.borrow_native_source_on_control(&source, AudioStorageRole::Caller)?;
        borrow_shared_storage_on_control(&owner, &mut ledger)?;
        borrow_shared_storage_on_control(&alias, &mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        Ok::<_, AudioStorageError>(ledger.summary()?.data_bytes)
    };
    assert_eq!(collect(exact.max_retained_bytes).unwrap(), pcm + expected);
    assert!(collect(exact.max_retained_bytes - 1).is_err());
}

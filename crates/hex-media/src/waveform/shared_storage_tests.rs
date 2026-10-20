use super::*;
use hex_engine::{AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation};
use std::{alloc::Layout, mem::size_of, sync::atomic::AtomicUsize};

fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 64,
        root_rows: 16,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
fn peaks() -> Waveform {
    let mut bins = Vec::with_capacity(19);
    bins.extend(
        [[Peak {
            min: -0.25,
            max: 0.5,
        }; 2]; 3],
    );
    let mut value = Waveform::from_peaks(9, bins).unwrap();
    value.levels.try_reserve_exact(31).unwrap();
    for level in &mut value.levels {
        level.try_reserve_exact(13).unwrap();
    }
    value
}
fn requested_bytes(value: &Waveform) -> usize {
    Layout::array::<AtomicUsize>(2)
        .unwrap()
        .extend(Layout::new::<Waveform>())
        .unwrap()
        .0
        .pad_to_align()
        .size()
        + value.bins.capacity() * size_of::<[Peak; 2]>()
        + value.levels.capacity() * size_of::<Vec<[Peak; 2]>>()
        + value
            .levels
            .iter()
            .map(|level| level.capacity() * size_of::<[Peak; 2]>())
            .sum::<usize>()
}

#[test]
fn paired_waveform_aliases_charge_body_pyramid_capacities_once_and_exact_limit_refuses() {
    let owner = SharedWaveform::new_on_control(peaks());
    let alias = owner.clone();
    let expected = requested_bytes(&owner);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        assert_eq!(
            borrow_shared_storage_on_control(&owner, &mut ledger),
            Err(AudioStorageError::UnknownStorage)
        );
        return;
    }
    // A prior body-only visit must not suppress the actual nested traversal.
    ledger
        .borrow_shared_metadata_body_on_control(&owner)
        .unwrap();
    assert!(ledger.summary().unwrap().data_bytes < expected);
    borrow_shared_storage_on_control(&owner, &mut ledger).unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, expected);
    assert!(
        ledger
            .validate_selected_shared_handles_on_control()
            .is_err()
    ); // omitted export
    borrow_shared_storage_on_control(&alias, &mut ledger).unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    borrow_shared_storage_on_control(&owner, &mut ledger).unwrap();
    let full = ledger.summary().unwrap();
    assert_eq!(full.data_bytes, expected);
    assert_eq!(ledger.selected_shared_counts_on_control(), (1, 2));
    assert!(full.require_complete().is_err()); // selected storage is not admission
    let mut limits = reservation();
    limits.max_retained_bytes = full.retained_bytes;
    limits.max_construction_peak_bytes = full.construction_peak_bytes;
    let mut exact = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
    borrow_shared_storage_on_control(&owner, &mut exact).unwrap();
    borrow_shared_storage_on_control(&alias, &mut exact).unwrap();
    assert_eq!(exact.summary().unwrap().data_bytes, expected);
    limits.max_retained_bytes -= 1;
    let mut refused = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
    assert_eq!(
        borrow_shared_storage_on_control(&owner, &mut refused),
        Err(AudioStorageError::ReservationExceeded)
    );
    assert!(refused.summary().is_err());
    assert_eq!(owner.peaks_for_frames(0, 9), alias.peaks_for_frames(0, 9));
    let equal = SharedWaveform::new_on_control(peaks());
    assert!(!owner.shares_owner_with(&equal));
    let mut two = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    borrow_shared_storage_on_control(&owner, &mut two).unwrap();
    borrow_shared_storage_on_control(&alias, &mut two).unwrap();
    let before = two.summary().unwrap().data_bytes;
    borrow_shared_storage_on_control(&equal, &mut two).unwrap();
    assert_eq!(
        two.summary().unwrap().data_bytes,
        before + requested_bytes(&equal)
    );
}

#[test]
fn paired_waveform_known_body_does_not_upgrade_unknown_pcm_child() {
    let audio = Audio::new(48_000, vec![[0.25, -0.5]; 8]).unwrap();
    let owner = SharedWaveform::new_on_control(summarize(&audio, 4).unwrap());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    if hex_engine::audio_storage_constructor_layout_supported() {
        ledger
            .borrow_shared_metadata_body_on_control(&owner)
            .unwrap();
        assert!(ledger.summary().unwrap().data_bytes >= size_of::<Waveform>());
    }
    assert_eq!(
        borrow_shared_storage_on_control(&owner, &mut ledger),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(ledger.summary().is_err());
    assert_eq!(owner.peaks_for_frames(0, 8).unwrap()[0].max, 0.25);
}

#[test]
fn paired_waveform_ten_exports_retain_one_actual_embedded_native_pcm_field() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported constructor; no positive waveform PCM qualification");
        return;
    }
    let audio = hex_engine::OwnedAudioSource::try_resident(48_000, vec![[0.25, -0.5]; 8])
        .ok()
        .unwrap()
        .try_into_native_audio_on_control()
        .ok()
        .unwrap();
    let owner = SharedWaveform::new_on_control(summarize(&audio, 4).unwrap());
    drop(audio);
    let exports: Vec<_> = (0..10).map(|_| owner.clone()).collect();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    borrow_shared_storage_on_control(&owner, &mut ledger).unwrap();
    let bytes = ledger.summary().unwrap().data_bytes;
    for export in &exports {
        borrow_shared_storage_on_control(export, &mut ledger).unwrap();
    }
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, bytes);
    assert_eq!(ledger.selected_shared_counts_on_control(), (2, 12));
    drop(ledger);
    drop(owner);
    assert_eq!(exports[0].peaks_for_frames(0, 8).unwrap()[1].min, -0.5);
}

#[test]
fn typed_waveform_projection_refuses_foreign_payload_bins_levels_and_pcm_fields() {
    use hex_engine::sampler_host_fields::{
        BodyRef, SharedMetadataRef, VecRef, WaveformFields, WaveformLevelsRef, WaveformPayload,
        WaveformSamples,
    };
    let actual = SharedWaveform::new_on_control(peaks());
    let mut foreign = peaks();
    foreign.samples = Some(SampleBacking::NativeOriginal(
        OwnedAudioSource::try_resident_mono(48_000, vec![0.25; 9])
            .ok()
            .unwrap(),
    ));
    let foreign = SharedWaveform::new_on_control(foreign);
    struct Wrong<'a> {
        actual: &'a Waveform,
        foreign: &'a Waveform,
        case: u8,
    }
    impl WaveformPayload for Wrong<'_> {
        fn body(&self) -> BodyRef<'_> {
            self.actual.body()
        }
        fn bins(&self) -> VecRef<'_> {
            if self.case == 0 {
                self.foreign.bins()
            } else {
                self.actual.bins()
            }
        }
        fn levels(&self) -> WaveformLevelsRef<'_> {
            if self.case == 1 {
                self.foreign.levels()
            } else {
                self.actual.levels()
            }
        }
        fn samples(&self) -> WaveformSamples<'_> {
            if self.case == 2 {
                self.foreign.samples()
            } else {
                self.actual.samples()
            }
        }
    }
    for case in 0..3 {
        let wrong = Wrong {
            actual: &actual,
            foreign: &foreign,
            case,
        };
        let fields = WaveformFields {
            owner: SharedMetadataRef::of(&actual),
            payload: &wrong,
        };
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        assert_eq!(
            ledger.borrow_sampler_waveform_fields_on_control(&fields),
            Err(AudioStorageError::SourceMismatch)
        );
        assert!(
            ledger.summary().is_err(),
            "failed partial observation is unusable"
        );
    }
    let fields = WaveformFields {
        owner: SharedMetadataRef::of(&actual),
        payload: foreign.as_ref(),
    };
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert_eq!(
        ledger.borrow_sampler_waveform_fields_on_control(&fields),
        Err(AudioStorageError::SourceMismatch)
    );
}

#[test]
fn typed_waveform_native_mono_and_stereo_aliases_keep_one_real_child_after_producer_drop() {
    use std::sync::atomic::AtomicBool;
    for stereo in [false, true] {
        let source = if stereo {
            OwnedAudioSource::try_resident(48_000, vec![[0.25, -0.5]; 9])
                .ok()
                .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(48_000, vec![0.25; 9])
                .ok()
                .unwrap()
        };
        let waveform = summarize_native_source_on_worker(
            &source,
            NativeOriginalWaveformLimits {
                bins: 4,
                scan_frames: 9,
                metadata_bytes: native_original_metadata_bytes(9, 4).unwrap(),
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        drop(source);
        let alias = waveform.clone();
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        borrow_shared_storage_on_control(&waveform, &mut ledger).unwrap();
        let bytes = ledger.summary().unwrap().data_bytes;
        borrow_shared_storage_on_control(&alias, &mut ledger).unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert_eq!(ledger.summary().unwrap().data_bytes, bytes);
        assert_eq!(ledger.selected_shared_counts_on_control(), (2, 3));
        assert!(ledger.summary().unwrap().require_complete().is_err());
        assert_eq!(waveform.peaks_for_frames(0, 9).unwrap()[0].max, 0.25);
        assert_eq!(
            waveform.peaks_for_frames(0, 9).unwrap()[1].min,
            if stereo { -0.5 } else { 0.25 }
        );
    }
}

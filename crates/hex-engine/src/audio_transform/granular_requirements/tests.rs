use super::*;
use crate::native_device_resources::{
    DeviceResourceAdmission, DeviceResourcePhase, inspect_device_resource_union,
};
use hex_project::{Command, Created, Editor, Project, SourceGainPoint, WarpMode};

fn fixture(mode: WarpMode) -> (Editor, hex_project::ClipId, crate::StreamedSourceMetadata) {
    fixture_frames(mode, 8_000)
}
fn fixture_frames(
    mode: WarpMode,
    frames: u64,
) -> (Editor, hex_project::ClipId, crate::StreamedSourceMetadata) {
    let mut editor = Editor::new(Project::new(16_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "audio".into(),
            path: "original.wav".into(),
            source_sample_rate: 8_000,
            source_frames: frames / 2,
            playback_frames: frames,
            conversion_version: hex_project::SINC_CONVERSION_V2,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    editor
        .execute(Command::SetClipSourcePhase { clip, phase: 137 })
        .unwrap();
    let warp = hex_project::default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        mode,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 40_000,
                },
                SourceGainPoint {
                    frame: 4_000,
                    gain_q16: 60_000,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 300 })
        .unwrap();
    let raw = crate::StreamedSourceMetadata::new(
        crate::StreamSourceKey::new(1, asset),
        8_000,
        16_000,
        frames / 2,
        frames,
        hex_project::SINC_CONVERSION_V2,
        [3; 32],
    )
    .unwrap();
    (editor, clip, raw)
}
fn requirement(
    editor: &Editor,
    clip: hex_project::ClipId,
    raw: &crate::StreamedSourceMetadata,
) -> StreamedGranularRequirement {
    StreamedGranularRequirement::new(
        raw,
        editor.project().clip(clip).unwrap().audio_edit().unwrap(),
        editor.project().tempo_map(),
        &AtomicBool::new(false),
    )
    .unwrap()
}
#[test]
fn complete_roots_survive_catalog_rebinding_copy_and_cancellation() {
    for (mode, frames) in [
        (WarpMode::Tones { grain_ms: 32 }, 8_000),
        (
            WarpMode::Texture {
                grain_ms: 32,
                fluctuation: 45,
                seed: 9,
            },
            8_000,
        ),
        (WarpMode::Tones { grain_ms: 32 }, 40_000_000),
        (
            WarpMode::Texture {
                grain_ms: 32,
                fluctuation: 45,
                seed: 9,
            },
            40_000_000,
        ),
    ] {
        let (editor, clip, raw) = fixture_frames(mode, frames);
        let requirement = requirement(&editor, clip, &raw);
        assert_eq!(requirement.asset(), raw.key().asset());
        let next = crate::StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(9, raw.key().asset()),
            raw.source_rate(),
            raw.project_rate(),
            raw.source_frames(),
            raw.playback_frames(),
            raw.conversion_version(),
            raw.fingerprint(),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let expected = streamed_granular_root(
            &next,
            editor.project().clip(clip).unwrap().audio_edit().unwrap(),
            editor.project().tempo_map(),
            &cancel,
        )
        .unwrap();
        let original = requirement.metadata_for(&raw).unwrap();
        assert_ne!(original.key(), expected.metadata().key());
        assert_eq!(
            requirement.metadata_for(&next).unwrap(),
            *expected.metadata()
        );
        let owned = requirement.try_owned_root(&next, &cancel).unwrap();
        assert!(
            owned.backing_bytes().unwrap() + std::mem::size_of::<StreamedGranularRoot>()
                <= requirement.owned_root_bytes().unwrap()
        );
        assert_eq!(owned.into_parts(), expected.into_parts());
        cancel.store(true, Ordering::Release);
        assert!(requirement.try_owned_root(&next, &cancel).is_err());
        assert!(
            StreamedGranularRequirement::new(
                &raw,
                editor.project().clip(clip).unwrap().audio_edit().unwrap(),
                editor.project().tempo_map(),
                &cancel
            )
            .is_err()
        );
        assert_eq!(requirement.metadata_for(&raw).unwrap(), original);
        assert!(requirement.retained_bytes().unwrap() > requirement.owned_root_bytes().unwrap());
    }
}
#[test]
fn source_mismatches_and_unsupported_edits_cannot_rebind_a_requirement() {
    let (mut editor, clip, raw) = fixture(WarpMode::Tones { grain_ms: 32 });
    let requirement = requirement(&editor, clip, &raw);
    let cancel = AtomicBool::new(false);
    let Created::Imported { asset: other, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "other".into(),
            path: "other.wav".into(),
            sample_rate: 16_000,
            frames: 8_000,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    let make = |asset, sr, pr, sf, pf, law, fingerprint| {
        crate::StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(2, asset),
            sr,
            pr,
            sf,
            pf,
            law,
            fingerprint,
        )
        .unwrap()
    };
    for wrong in [
        make(
            other,
            8_000,
            16_000,
            4_000,
            8_000,
            hex_project::SINC_CONVERSION_V2,
            [3; 32],
        ),
        make(
            raw.key().asset(),
            16_000,
            16_000,
            8_000,
            8_000,
            hex_project::IDENTITY_CONVERSION,
            [3; 32],
        ),
        make(
            raw.key().asset(),
            8_000,
            24_000,
            4_000,
            12_000,
            hex_project::SINC_CONVERSION_V2,
            [3; 32],
        ),
        make(
            raw.key().asset(),
            8_000,
            16_000,
            4_001,
            8_002,
            hex_project::SINC_CONVERSION_V2,
            [3; 32],
        ),
        make(
            raw.key().asset(),
            8_000,
            16_000,
            4_000,
            8_000,
            hex_project::SINC_CONVERSION_V1,
            [3; 32],
        ),
        make(
            raw.key().asset(),
            8_000,
            16_000,
            4_000,
            8_000,
            hex_project::SINC_CONVERSION_V2,
            [4; 32],
        ),
        raw.with_plain_pitch(100, hex_project::WarpResampler::V2)
            .unwrap(),
    ] {
        assert!(requirement.metadata_for(&wrong).is_err());
        assert!(requirement.try_owned_root(&wrong, &cancel).is_err());
    }
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 5,
            fade_out: 0,
        })
        .unwrap();
    assert!(
        StreamedGranularRequirement::new(
            &raw,
            editor.project().clip(clip).unwrap().audio_edit().unwrap(),
            editor.project().tempo_map(),
            &cancel
        )
        .is_err()
    );
}
fn limits() -> crate::AudioStorageLedgerReservation {
    crate::AudioStorageLedgerReservation {
        data_rows: 256,
        handle_rows: 256,
        root_rows: 64,
        max_retained_bytes: 2_000_000,
        max_construction_peak_bytes: 2_000_000,
    }
}
fn phase<'a>(registered: &'a [DeviceResourceAdmission<'a>]) -> DeviceResourcePhase<'a> {
    DeviceResourcePhase {
        stores: &[],
        registered,
        selections: &[],
        outbound: &[],
        legacy: &[],
        checked_runtime_bytes: 0,
        construction_bytes: 0,
    }
}
#[test]
fn native_admission_charges_real_capacity_and_deduplicates_only_actual_owners() {
    let (editor, clip, raw) = fixture(WarpMode::Texture {
        grain_ms: 32,
        fluctuation: 35,
        seed: 42,
    });
    let owner = requirement(&editor, clip, &raw);
    let mut handles = Vec::with_capacity(4);
    handles.push(owner.clone());
    let set = StreamedGranularRequirements::new(handles).unwrap();
    assert_eq!(
        set.handle_capacity_bytes().unwrap(),
        4 * std::mem::size_of::<StreamedGranularRequirement>()
    );
    let registered = [DeviceResourceAdmission::default().with_granular_requirements(&set)];
    let actual =
        inspect_device_resource_union(&phase(&registered), &phase(&registered), limits()).unwrap();
    let baseline = inspect_device_resource_union(&phase(&[]), &phase(&[]), limits()).unwrap();
    assert_eq!(
        actual.retained.retained_bytes - baseline.retained.retained_bytes,
        owner.retained_bytes().unwrap()
            + std::mem::size_of::<StreamedGranularRequirements>()
            + set.handle_capacity_bytes().unwrap()
    );
    assert_eq!(actual.retained_frames, 0);
    let repeated = [registered[0], registered[0]];
    assert_eq!(
        inspect_device_resource_union(&phase(&repeated), &phase(&repeated), limits())
            .unwrap()
            .retained
            .retained_bytes,
        actual.retained.retained_bytes
    );
    let shared = set.clone();
    let shared_views = [
        registered[0],
        DeviceResourceAdmission::default().with_granular_requirements(&shared),
    ];
    let shared_bytes =
        inspect_device_resource_union(&phase(&shared_views), &phase(&shared_views), limits())
            .unwrap()
            .retained
            .retained_bytes;
    assert_eq!(
        shared_bytes,
        actual.retained.retained_bytes
            + std::mem::size_of::<StreamedGranularRequirements>()
            + shared.handle_capacity_bytes().unwrap()
    );
    let separate_owner = requirement(&editor, clip, &raw);
    let separate_bytes = separate_owner.retained_bytes().unwrap();
    let separate = StreamedGranularRequirements::new(vec![separate_owner]).unwrap();
    let distinct = [
        registered[0],
        DeviceResourceAdmission::default().with_granular_requirements(&separate),
    ];
    assert_eq!(
        inspect_device_resource_union(&phase(&distinct), &phase(&distinct), limits())
            .unwrap()
            .retained
            .retained_bytes,
        actual.retained.retained_bytes
            + separate_bytes
            + std::mem::size_of::<StreamedGranularRequirements>()
            + separate.handle_capacity_bytes().unwrap()
    );
    let mut short = limits();
    short.max_retained_bytes = actual.retained.retained_bytes - 1;
    assert!(
        inspect_device_resource_union(&phase(&registered), &phase(&registered), short).is_err()
    );
    let mut construction = phase(&registered);
    construction.construction_bytes = 128;
    let peak = inspect_device_resource_union(&phase(&registered), &construction, limits())
        .unwrap()
        .construction
        .construction_peak_bytes;
    short = limits();
    short.max_construction_peak_bytes = peak - 1;
    assert!(inspect_device_resource_union(&phase(&registered), &construction, short).is_err());
}
#[test]
fn handle_sets_refuse_oversized_capacity_and_full_push() {
    assert!(
        StreamedGranularRequirements::new(Vec::with_capacity(hex_project::MAX_CLIPS + 1)).is_err()
    );
    let (editor, clip, raw) = fixture(WarpMode::Tones { grain_ms: 32 });
    let owner = requirement(&editor, clip, &raw);
    let mut set = StreamedGranularRequirements::default();
    set.try_push(owner.clone()).unwrap();
    assert_eq!(set.requirements().len(), 1);
    let mut full =
        StreamedGranularRequirements::new(vec![owner.clone(); hex_project::MAX_CLIPS]).unwrap();
    assert!(full.try_push(owner).is_err());
    assert_eq!(full.requirements().len(), hex_project::MAX_CLIPS);
}

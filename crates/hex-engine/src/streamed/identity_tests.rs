use super::*;
use hex_project::{Command, Created, Editor, Project};
use std::collections::{BTreeSet, HashSet};

#[test]
fn repitch_identity_covers_complete_map_law_and_verified_source_facts() {
    const Q32: i128 = 1_i128 << 32;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "identity.wav".into(),
            sample_rate: 48_000,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let source = |generation, fingerprint| {
        StreamedSourceMetadata::new(
            StreamSourceKey::new(generation, asset),
            48_000,
            48_000,
            4096,
            4096,
            IDENTITY_CONVERSION,
            fingerprint,
        )
        .unwrap()
    };
    let map = |split, output| {
        hex_warp::TimeMap::new(
            vec![
                hex_warp::MapSegment {
                    output_start: 0,
                    output_end: split,
                    source_start_q32: 0,
                    source_end_q32: 2048 * Q32,
                },
                hex_warp::MapSegment {
                    output_start: split,
                    output_end: output,
                    source_start_q32: 2048 * Q32,
                    source_end_q32: 4096 * Q32,
                },
            ],
            output as usize,
        )
        .unwrap()
    };
    let raw = source(1, [7; 32]);
    let base = RepitchStreamIdentity::new(&raw, &map(2048, 4096), hex_warp::Resampler::V2).unwrap();
    let key = StreamSourceKey::repitch(base);
    assert!(!key.is_raw());
    assert_eq!(key.pitch(), None);
    assert_eq!(key.repitch_identity(), Some(base));
    assert!(raw.key().is_raw());
    assert_eq!(raw.key().repitch_identity(), None);
    assert_ne!(key, raw.key());
    assert!(
        StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            4096,
            4096,
            IDENTITY_CONVERSION,
            [7; 32]
        )
        .is_err()
    );
    let mut forged = raw.clone();
    forged.key = key;
    assert!(forged.with_plain_pitch(700, WarpResampler::V2).is_err());
    assert!(crate::OwnedAudioSource::try_streamed(forged.clone()).is_err());
    assert!(crate::OwnedAudioSource::try_pitched_streamed(forged.clone()).is_err());
    assert!(
        RepitchStreamIdentity::new(&forged, &map(2048, 4096), hex_warp::Resampler::V2).is_err()
    );
    assert_eq!(
        base,
        RepitchStreamIdentity::new(&raw, &map(2048, 4096), hex_warp::Resampler::V2).unwrap()
    );
    assert_eq!(base.raw_key(), raw.key());
    let alternatives = [
        RepitchStreamIdentity::new(&raw, &map(2049, 4096), hex_warp::Resampler::V2).unwrap(),
        RepitchStreamIdentity::new(&raw, &map(2048, 4097), hex_warp::Resampler::V2).unwrap(),
        RepitchStreamIdentity::new(&raw, &map(2048, 4096), hex_warp::Resampler::V1).unwrap(),
        RepitchStreamIdentity::new(
            &source(1, [8; 32]),
            &map(2048, 4096),
            hex_warp::Resampler::V2,
        )
        .unwrap(),
        RepitchStreamIdentity::new(
            &source(2, [7; 32]),
            &map(2048, 4096),
            hex_warp::Resampler::V2,
        )
        .unwrap(),
        RepitchStreamIdentity::new(
            &StreamedSourceMetadata::new(
                raw.key(),
                48_000,
                48_000,
                4095,
                4095,
                IDENTITY_CONVERSION,
                [7; 32],
            )
            .unwrap(),
            &map(2048, 4096),
            hex_warp::Resampler::V2,
        )
        .unwrap(),
        RepitchStreamIdentity::new(
            &raw,
            &hex_warp::TimeMap::new(
                vec![
                    hex_warp::MapSegment {
                        output_start: 0,
                        output_end: 2048,
                        source_start_q32: 0,
                        source_end_q32: 2049 * Q32,
                    },
                    hex_warp::MapSegment {
                        output_start: 2048,
                        output_end: 4096,
                        source_start_q32: 2049 * Q32,
                        source_end_q32: 4096 * Q32,
                    },
                ],
                4096,
            )
            .unwrap(),
            hex_warp::Resampler::V2,
        )
        .unwrap(),
    ];
    assert!(alternatives.iter().all(|identity| *identity != base));
    let mut keys = alternatives
        .iter()
        .copied()
        .map(StreamSourceKey::repitch)
        .collect::<BTreeSet<_>>();
    keys.insert(key);
    keys.insert(raw.key());
    keys.insert(StreamSourceKey::plain_pitch(1, asset, 700, WarpResampler::V2).unwrap());
    assert_eq!(keys.len(), 10);
    assert_eq!(keys.iter().copied().collect::<HashSet<_>>().len(), 10);
    assert_eq!(alternatives.into_iter().collect::<BTreeSet<_>>().len(), 7);
    let pitched = raw.with_plain_pitch(700, WarpResampler::V2).unwrap();
    assert!(
        RepitchStreamIdentity::new(&pitched, &map(2048, 4096), hex_warp::Resampler::V2).is_err()
    );
    let invalid = hex_warp::TimeMap::new(
        vec![hex_warp::MapSegment {
            output_start: 0,
            output_end: 1,
            source_start_q32: 0,
            source_end_q32: 100 * Q32,
        }],
        1,
    )
    .unwrap();
    assert!(RepitchStreamIdentity::new(&raw, &invalid, hex_warp::Resampler::V2).is_err());
}

#[test]
fn raw_compiler_refuses_forged_repitch_metadata() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let raw = StreamedSourceMetadata::new(
        StreamSourceKey::new(1, asset),
        48_000,
        48_000,
        4096,
        4096,
        IDENTITY_CONVERSION,
        [7; 32],
    )
    .unwrap();
    let map = hex_warp::TimeMap::new(
        vec![hex_warp::MapSegment {
            output_start: 0,
            output_end: 4096,
            source_start_q32: 0,
            source_end_q32: 4096_i128 << 32,
        }],
        4096,
    )
    .unwrap();
    let identity = RepitchStreamIdentity::new(&raw, &map, hex_warp::Resampler::V2).unwrap();
    let mut forged = raw;
    forged.key = StreamSourceKey::repitch(identity);
    let sources = std::collections::BTreeMap::from([(
        asset,
        crate::compile::PreparedAudioSource::Streamed(std::sync::Arc::new(forged)),
    )]);
    assert!(
        crate::compile::project_with_prepared_audio_sources(editor.project(), &sources, 1).is_err()
    );
}

#[test]
fn pitch_identity_distinguishes_source_generation_asset_and_law() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let mut assets = Vec::new();
    for path in ["a.wav", "b.wav"] {
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: path.into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("asset fixture")
        };
        assets.push(asset);
    }
    let raw = StreamSourceKey::new(1, assets[0]);
    let pitched = StreamSourceKey::plain_pitch(1, assets[0], 700, WarpResampler::V2).unwrap();
    assert_eq!(
        pitched,
        StreamSourceKey::plain_pitch(1, assets[0], 700, WarpResampler::V2).unwrap()
    );
    assert_eq!(pitched.pitch(), Some((700, WarpResampler::V2)));
    assert_eq!(raw.pitch(), None);
    assert_eq!(pitched.generation(), 1);
    assert_eq!(pitched.asset(), assets[0]);
    let keys = [
        raw,
        pitched,
        StreamSourceKey::plain_pitch(1, assets[0], -700, WarpResampler::V2).unwrap(),
        StreamSourceKey::plain_pitch(1, assets[0], 700, WarpResampler::V1).unwrap(),
        StreamSourceKey::plain_pitch(2, assets[0], 700, WarpResampler::V2).unwrap(),
        StreamSourceKey::plain_pitch(1, assets[1], 700, WarpResampler::V2).unwrap(),
    ];
    assert_eq!(
        keys.iter().copied().collect::<BTreeSet<_>>().len(),
        keys.len()
    );
    assert_eq!(
        keys.iter().copied().collect::<HashSet<_>>().len(),
        keys.len()
    );
    assert!(StreamSourceKey::plain_pitch(0, assets[0], 0, WarpResampler::V1).is_err());
    assert!(StreamSourceKey::plain_pitch(1, assets[0], 4801, WarpResampler::V1).is_err());
    assert!(StreamSourceKey::plain_pitch(1, assets[0], -4801, WarpResampler::V1).is_err());

    let metadata = |key| {
        StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            4096,
            4096,
            IDENTITY_CONVERSION,
            [7; 32],
        )
    };
    assert!(metadata(raw).is_ok());
    assert!(
        metadata(pitched).is_err(),
        "raw worker must never publish source PCM under a pitch key"
    );
}

use super::*;
use hex_project::{Command, Created, Editor, Project};
use std::collections::{BTreeSet, HashSet};

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

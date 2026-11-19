use super::*;
use crate::{decode, encode, schema_version_for};

const SCHEMA64: &[u8] = include_bytes!("../../tests/fixtures/warp-resampler/schema64.json");

#[test]
fn historical_domains_including_retained_roots_keep_legacy_law_and_complete_state() {
    let project = decode(SCHEMA64).unwrap();
    let mut count = 0;
    for clip in project.tracks().iter().flat_map(|track| track.clips()) {
        for clip in std::iter::once(clip).chain(clip.retained_audio_roots().map(|root| &root.clip))
        {
            if let Some(edit) = clip.audio_edit() {
                assert_eq!(edit.domain.fade_clock, AudioClipFadeClock::LegacyRoot);
                let dto = AudioEditDto::try_from(edit.clone()).unwrap();
                // Events are owned by their separate wrapper, not AudioEditDto.
                let restored = AudioClipEdit::try_from(dto).unwrap();
                assert_eq!(restored.domain.fade_clock, AudioClipFadeClock::LegacyRoot);
                count += 1;
            }
        }
    }
    assert!(count >= 5, "fixture must retain nested processing roots");
    let bytes = encode(&project).unwrap();
    assert_eq!(schema_version_for(&project), 64);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        serde_json::from_slice::<serde_json::Value>(SCHEMA64).unwrap()
    );
    assert_eq!(decode(&bytes).unwrap(), project);
}

#[test]
fn dto_encoder_refuses_outer_clock_before_it_can_drop_the_law() {
    let project = decode(SCHEMA64).unwrap();
    let edit = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .find_map(|clip| clip.audio_edit())
        .unwrap();
    for neutral in [false, true] {
        let mut edit = edit.clone();
        let domain = Arc::make_mut(&mut edit.domain);
        domain.fade_clock = AudioClipFadeClock::Outer;
        if neutral {
            domain.fade = ClipFade::neutral(domain.fade.span()).unwrap();
        }
        assert!(matches!(
            AudioEditDto::try_from(edit),
            Err(StorageError::Invalid(
                "outer clip fades are not supported yet"
            ))
        ));
    }
}

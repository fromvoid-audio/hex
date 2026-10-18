//! Regression coverage for the independent mixed-route profiling fixture.
#[path = "../examples/support/session_fixture.rs"]
mod fixture;

use hex_engine::{
    LaunchQuantization, PlaybackAction, PlaybackCommand, PreparedRevision, Renderer,
    SessionPlaybackCommand, playback::Playback,
};
use std::{collections::BTreeMap, sync::Arc};

fn assert_pcm_matches(actual: &[[f32; 2]], expected: &[[f32; 2]], label: &str) {
    assert_eq!(actual.len(), expected.len());
    let mismatch = actual
        .iter()
        .zip(expected)
        .enumerate()
        .find(|(_, (actual, expected))| actual != expected);
    assert!(
        mismatch.is_none(),
        "{label}: first PCM mismatch {mismatch:?}"
    );
}

fn check_fixture(loop_tempo: bool) {
    let project = fixture::project(loop_tempo).unwrap();
    let finite = fixture::finite_oracle(&project, loop_tempo).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            &project,
            &BTreeMap::new(),
            hex_engine::SharedSineTable::new_on_control(),
            None,
        )
        .unwrap(),
    );
    let frames = if loop_tempo { 260_000 } else { 160_000 };
    let mut expected = vec![[0.0; 2]; frames];
    let mut renderer = Renderer::new(&finite);
    renderer.play();
    renderer.render(&mut expected).unwrap();

    let mut direct = fixture::kernel(Arc::clone(&revision)).unwrap();
    let mut actual = vec![[0.0; 2]; frames];
    direct.render(&mut actual).unwrap();
    assert_pcm_matches(&actual, &expected, "direct kernel");

    let mut partitioned = fixture::kernel(Arc::clone(&revision)).unwrap();
    for block in actual.chunks_mut(131) {
        partitioned.render(block).unwrap();
    }
    assert_pcm_matches(&actual, &expected, "partitioned kernel");
    assert_eq!(direct.performance_frame(), partitioned.performance_frame());
    assert_eq!(direct.arrangement_frame(), partitioned.arrangement_frame());

    let scene = revision.session().scenes()[0].id;
    let mut wrapper = Playback::try_from_prepared_with_session(revision).unwrap();
    wrapper
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    wrapper
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    wrapper.render(&mut actual).unwrap();
    assert_pcm_matches(&actual, &expected, "prepared playback wrapper");
    assert_eq!(direct.performance_frame(), wrapper.performance_frame());
    assert_eq!(direct.arrangement_frame(), wrapper.position());
}

#[test]
fn mixed_profile_oracle_matches_canonical_hybrid_tracks_across_transport_wraps() {
    check_fixture(false);
}

#[test]
fn mixed_profile_oracle_matches_repeating_tempo_map_across_transport_wraps() {
    check_fixture(true);
}

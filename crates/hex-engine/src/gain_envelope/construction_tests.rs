//! Independent actual prepared capacities, including touch and tempo splitting.
use super::*;
use hex_project::{
    BeatTime, Command, Created, Editor, GainTouchContext, GainTouchSpan, GainTouchStep, Tempo,
    TempoPoint,
};
#[test]
fn compiler_gain_cost_includes_real_tempo_knots_and_touch_backings_without_births() {
    for automation in [false, true] {
        for touch in [false, true] {
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Track(_) = editor
                .execute(Command::AddTrack {
                    name: "gain".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            let mut snapshot = editor.project().snapshot();
            snapshot.tempo_points = vec![
                TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
                TempoPoint::new(BeatTime::ONE, Tempo::new(90_000_000).unwrap()),
            ];
            if automation {
                snapshot.tracks[0].gain_envelope = vec![
                    GainPoint {
                        beat: BeatTime::ZERO,
                        gain: 0.25,
                        outgoing: GainInterpolation::Linear,
                    },
                    GainPoint {
                        beat: BeatTime::from_quarters(4).unwrap(),
                        gain: 1.5,
                        outgoing: GainInterpolation::Step,
                    },
                ];
            }
            if touch {
                snapshot.tracks[0].gain_touch_epoch = 1;
                snapshot.tracks[0].gain_touch_spans = vec![GainTouchSpan {
                    start: BeatTime::ONE,
                    end: BeatTime::from_quarters(3).unwrap(),
                    context: GainTouchContext {
                        recording_epoch: 1,
                        project_generation: 0,
                        canonical_revision: 1,
                        audio_revision: 1,
                        transport_epoch: 1,
                        gesture_epoch: 1,
                        gesture_serial: 1,
                        sample_rate: 48_000,
                        loop_pass: 0,
                    },
                    steps: vec![GainTouchStep {
                        beat: BeatTime::ONE,
                        gain: 0.75,
                        origin_beat: BeatTime::ONE,
                        source_sequence: 1,
                    }],
                }];
            }
            let project = Project::from_snapshot(snapshot).unwrap();
            let lane = project.tracks()[0].gain_lane();
            crate::owned_shared::reset_birth_count();
            crate::owned_shared::reset_clone_count();
            let cost = PreparedGainEnvelope::construction_cost(lane, project.tempo_map()).unwrap();
            assert_eq!(crate::owned_shared::birth_count(), 0);
            assert_eq!(crate::owned_shared::clone_count(), 0);
            let actual = PreparedGainEnvelope::prepare(
                lane,
                project.tempo_map(),
                PreparedTrackMix::identity(),
                1.0,
            )
            .unwrap();
            match actual {
                None => {
                    assert!(!automation && !touch);
                    assert_eq!(cost.backing_bytes, 0);
                }
                Some(actual) => {
                    let independently_observed = actual.knots.capacity()
                        * size_of::<GainEnvelopeKnot>()
                        + actual.touch.allocated_bytes();
                    assert_eq!(cost.backing_bytes, independently_observed);
                    assert_eq!(actual.knots.len(), if automation { 3 } else { 0 });
                    assert_eq!(actual.value_at(0), if automation { 0.25 } else { 1.0 });
                    if touch {
                        assert_eq!(actual.value_at(24_000), 0.75);
                    }
                }
            }
            eprintln!(
                "SUPPORTED gain constructor automation={automation} touch={touch} bytes={}",
                cost.backing_bytes
            );
        }
    }
}

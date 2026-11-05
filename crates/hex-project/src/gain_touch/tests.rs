use super::*;
use crate::{Command, Created, Editor, GainInterpolation, Project};
fn beat(raw: u64) -> BeatTime {
    BeatTime::from_raw(raw)
}
fn point(raw: u64, gain: f32) -> GainPoint {
    GainPoint {
        incoming_gain: None,
        beat: beat(raw),
        gain,
        outgoing: GainInterpolation::Linear,
    }
}
fn span(start: u64, end: u64, epoch: u64, events: &[(u64, f32)]) -> GainTouchSpan {
    GainTouchSpan {
        source_start: beat(start),
        start: beat(start),
        end: beat(end),
        context: GainTouchContext {
            recording_epoch: epoch,
            project_generation: 0,
            canonical_revision: 1,
            audio_revision: 1,
            transport_epoch: 1,
            gesture_epoch: 1,
            gesture_serial: epoch,
            sample_rate: 48000,
            loop_pass: 0,
        },
        steps: events
            .iter()
            .enumerate()
            .map(|(i, (raw, gain))| GainTouchStep {
                beat: beat(*raw),
                gain: *gain,
                origin_beat: beat(*raw),
                source_sequence: i as u64 + 1,
            })
            .collect(),
    }
}
fn fixture() -> (Editor, crate::TrackId) {
    let mut e = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(id) = e
        .execute(Command::AddTrack {
            name: "touch".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    (e, id)
}
#[test]
fn independent_ramp_and_pcm_oracle_preserves_base_outside_half_open_span() {
    let base = vec![point(0, 0.0), point(400, 1.0)];
    let lane = GainLane::new(base.clone(), vec![], 0)
        .unwrap()
        .with_touch(0, span(200, 300, 1, &[(200, 0.9), (250, 0.4)]))
        .unwrap();
    assert_eq!(lane.base(), base);
    // Independent analytic oracle; this is model math, not a runtime/audio ACK test.
    for raw in 0..=450 {
        let original = (raw as f32 / 400.0).min(1.0);
        let expected = if (200..250).contains(&raw) {
            0.9
        } else if (250..300).contains(&raw) {
            0.4
        } else {
            original
        };
        let read = lane.touch_gain_at(beat(raw)).unwrap_or(original);
        assert_eq!(read.to_bits(), expected.to_bits(), "beat {raw}");
        let source = if raw % 2 == 0 { 0.125_f32 } else { -0.75 };
        assert_eq!((source * read).to_bits(), (source * expected).to_bits());
    }
    assert_eq!(lane.touch_gain_at(beat(300)), None);
    // A naive inserted point would incorrectly change this untouched ramp value.
    assert_eq!(lane.touch_gain_at(beat(100)), None);
}
#[test]
fn replacements_trim_flat_fragments_preserve_origin_and_resume_base_at_exact_end() {
    let lane = GainLane::default()
        .with_touch(0, span(10, 100, 1, &[(10, 0.2), (30, 0.4), (70, 0.8)]))
        .unwrap();
    let lane = lane
        .with_touch(1, span(40, 60, 2, &[(40, 1.1), (50, 1.2)]))
        .unwrap();
    assert_eq!(
        lane.spans()
            .iter()
            .map(|s| (s.start.raw(), s.end.raw()))
            .collect::<Vec<_>>(),
        vec![(10, 40), (40, 60), (60, 100)]
    );
    assert_eq!(
        lane.spans()[2].steps[0],
        GainTouchStep {
            beat: beat(60),
            gain: 0.4,
            origin_beat: beat(30),
            source_sequence: 2
        }
    );
    assert_eq!(lane.touch_gain_at(beat(60)), Some(0.4));
    assert_eq!(lane.touch_gain_at(beat(70)), Some(0.8));
    assert_eq!(lane.touch_gain_at(beat(100)), None);
    let lane = lane.with_touch(2, span(0, 110, 3, &[(0, 1.0)])).unwrap();
    assert_eq!(lane.spans().len(), 1);
    assert!(
        lane.with_touch(2, span(110, 120, 4, &[(110, 1.0)]))
            .is_err()
    );
    let lane = lane
        .with_touch(3, span(110, 120, 4, &[(110, 0.5)]))
        .unwrap();
    assert_eq!(lane.touch_gain_at(beat(110)), Some(0.5));
    assert_eq!(lane.touch_gain_at(beat(120)), None);
}
#[test]
fn malformed_events_context_order_overlap_and_fragment_provenance_refuse() {
    let valid = span(10, 20, 1, &[(10, 1.0), (15, 0.5)]);
    for which in 0..12 {
        let mut bad = valid.clone();
        match which {
            0 => bad.end = bad.start,
            1 => bad.steps.clear(),
            2 => bad.steps[0].beat = beat(11),
            3 => bad.steps[1].beat = beat(20),
            4 => bad.steps[1].gain = f32::NAN,
            5 => bad.steps[1].gain = 2.01,
            6 => bad.context.transport_epoch = 0,
            7 => bad.context.sample_rate = 1,
            8 => bad.steps[1].source_sequence = 1,
            9 => bad.steps[1].origin_beat = beat(14),
            10 => bad.steps[1].beat = beat(10),
            _ => bad.context.recording_epoch = 0,
        }
        assert!(bad.validate().is_err(), "case {which}");
    }
    assert!(GainLane::new(vec![], vec![valid.clone(), valid.clone()], 1).is_err());
    assert!(GainLane::new(vec![], vec![valid.clone()], 0).is_err());
    let mut later = span(30, 40, 1, &[(30, 0.5)]);
    later.context = valid.context;
    // Reused sequence with changed provenance is not a retained held event.
    assert!(GainLane::new(vec![], vec![valid.clone(), later.clone()], 1).is_err());
    later.steps[0].source_sequence = 3;
    assert!(GainLane::new(vec![], vec![later.clone(), valid.clone()], 1).is_err());
    later.context.gesture_serial = 99;
    assert!(GainLane::new(vec![], vec![valid, later], 1).is_err());
}
#[test]
fn endpoints_and_fragment_costs_are_charged_before_replacement() {
    let base = (0..1021).map(|i| point(i, 1.0)).collect::<Vec<_>>();
    let lane = GainLane::new(base, vec![], 0)
        .unwrap()
        .with_touch(0, span(20, 80, 1, &[(20, 0.5)]))
        .unwrap();
    assert_eq!(lane.units().unwrap(), 1024);
    assert!(lane.with_touch(1, span(30, 40, 2, &[(30, 0.9)])).is_err());
    let mut lane = GainLane::default();
    for i in 0..128 {
        lane = lane
            .with_touch(i, span(i * 2, i * 2 + 1, i + 1, &[(i * 2, 1.0)]))
            .unwrap();
    }
    assert!(
        lane.with_touch(128, span(256, 257, 129, &[(256, 1.0)]))
            .is_err()
    );
    // Replacing the complete domain shrinks instead of exhausting the span bound.
    assert_eq!(
        lane.with_touch(128, span(0, 257, 129, &[(0, 1.0)]))
            .unwrap()
            .spans()
            .len(),
        1
    );
}
#[test]
fn refused_commands_never_prepare_mutate_history_clear_redo_or_consume_ids() {
    let (mut e, id) = fixture();
    e.execute(Command::Gain {
        track: id,
        gain: 0.5,
    })
    .unwrap();
    assert!(e.undo());
    let before = e.project().clone();
    let mut called = false;
    let mut invalid = span(10, 20, 1, &[(10, 1.0)]);
    invalid.context.sample_rate = 44100;
    let result: Result<_, Invalid> = e.execute_prepared(
        Command::SpliceGainTouch {
            track: id,
            expected_epoch: 0,
            span: invalid,
        },
        |_| {
            called = true;
            Ok(())
        },
    );
    assert_eq!(
        result.unwrap_err().0,
        "gain touch sample rate differs from project"
    );
    assert!(!called);
    assert_eq!(e.project(), &before);
    assert!(e.can_redo());
    let mut snapshot = before.snapshot();
    snapshot.tracks[0].gain_touch_spans = vec![span(10, 20, 1, &[(10, 1.0)])];
    snapshot.tracks[0].gain_touch_epoch = 1;
    assert!(snapshot.validate_gain_lanes().is_ok());
    assert_eq!(
        Project::from_snapshot(snapshot.clone()).unwrap().snapshot(),
        snapshot
    );
    snapshot.tracks[0].gain_touch_spans[0].context.sample_rate = 44100;
    assert!(snapshot.validate_gain_lanes().is_err());
}
#[test]
fn epoch_only_history_persists_high_water_and_removal_restores_owner_value() {
    let (e, id) = fixture();
    let mut snapshot = e.project().snapshot();
    snapshot.tracks[0].gain_envelope = vec![point(0, 0.25), point(100, 0.75)];
    snapshot.tracks[0].gain_touch_epoch = 7;
    let project = Project::from_snapshot(snapshot).unwrap();
    let mut e = Editor::new(project.clone(), 16).unwrap();
    e.execute(Command::ClearTrackGainEnvelope { track: id })
        .unwrap();
    assert_eq!(
        e.project().track(id).unwrap().gain_lane().recording_epoch(),
        7
    );
    assert!(e.undo());
    assert_eq!(e.project(), &project);
    e.execute(Command::RemoveTrack(id)).unwrap();
    assert!(e.undo());
    assert_eq!(e.project(), &project);
    assert!(e.redo());
    assert!(e.project().track(id).is_none());
}
#[test]
fn unsupported_fixture_transforms_refuse_without_losing_complete_lane() {
    let (e, id) = fixture();
    let mut project = e.project().clone();
    // Editing the base remains explicit while recorded overlays are present.
    project.tracks[0].gain_lane = GainLane::default()
        .with_touch(0, span(10, 20, 1, &[(10, 1.0)]))
        .unwrap();
    let snapshot = project.snapshot();
    for cmd in [
        Command::ReplaceTrackGainEnvelope {
            track: id,
            points: vec![point(0, 1.0)],
        },
        Command::Gain {
            track: id,
            gain: 1.0,
        },
    ] {
        let mut editor = Editor::new(project.clone(), 16).unwrap();
        assert!(editor.execute(cmd).is_err());
        assert_eq!(editor.project().snapshot(), snapshot);
    }
    let mut editor = Editor::new(project.clone(), 16).unwrap();
    editor.execute(Command::RemoveTrack(id)).unwrap();
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), snapshot);
}
#[test]
fn project_capacity_counts_base_steps_and_endpoints_before_adoption() {
    let (e, _) = fixture();
    let mut snapshot = e.project().snapshot();
    let mut template = snapshot.tracks[0].clone();
    template.gain_envelope = (0..1021).map(|i| point(i, 1.0)).collect();
    template.gain_touch_spans = vec![span(0, 1, 1, &[(0, 1.0)])];
    template.gain_touch_epoch = 1;
    snapshot.tracks = vec![template; 64];
    assert!(snapshot.validate_gain_lanes().is_ok());
    snapshot.tracks.push(snapshot.tracks[0].clone());
    assert_eq!(
        snapshot.validate_gain_lanes().unwrap_err().0,
        "project gain lane limit reached"
    );
}
#[test]
fn tiny_distinct_beats_remain_authored_without_frame_quantization() {
    let lane = GainLane::default()
        .with_touch(0, span(1, 4, 1, &[(1, 0.2), (2, 0.6), (3, 1.0)]))
        .unwrap();
    for (raw, gain) in [(1, 0.2), (2, 0.6), (3, 1.0)] {
        assert_eq!(lane.touch_gain_at(beat(raw)), Some(gain));
    }
    assert_eq!(lane.spans()[0].steps.len(), 3);
    assert_eq!(lane.touch_gain_at(beat(4)), None);
}

#[test]
fn splice_clear_and_history_adopt_the_complete_lane_and_keep_epoch_high_water() {
    let (mut editor, id) = fixture();
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track: id,
            points: vec![point(0, 0.2), point(100, 0.8)],
        })
        .unwrap();
    let base = editor.project().track(id).unwrap().gain_envelope().to_vec();
    editor
        .execute(Command::SpliceGainTouch {
            track: id,
            expected_epoch: 0,
            span: span(20, 80, 1, &[(20, 1.1), (50, 0.3)]),
        })
        .unwrap();
    let recorded = editor.project().snapshot();
    assert_eq!(editor.project().track(id).unwrap().gain_envelope(), base);
    assert!(editor.undo());
    assert!(
        editor
            .project()
            .track(id)
            .unwrap()
            .gain_lane()
            .spans()
            .is_empty()
    );
    assert_eq!(
        editor
            .project()
            .track(id)
            .unwrap()
            .gain_lane()
            .recording_epoch(),
        1
    );
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), recorded);
    editor
        .execute(Command::ClearTrackGainEnvelope { track: id })
        .unwrap();
    assert!(
        editor
            .project()
            .track(id)
            .unwrap()
            .gain_envelope()
            .is_empty()
    );
    assert!(
        editor
            .project()
            .track(id)
            .unwrap()
            .gain_lane()
            .spans()
            .is_empty()
    );
    assert_eq!(
        editor
            .project()
            .track(id)
            .unwrap()
            .gain_lane()
            .recording_epoch(),
        1
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), recorded);
    let mut stale = span(20, 80, 1, &[(20, 0.1)]);
    stale.context.gesture_serial = 2;
    assert!(
        editor
            .execute(Command::SpliceGainTouch {
                track: id,
                expected_epoch: 1,
                span: stale
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), recorded);
}

#[test]
fn splice_refuses_project_weighted_overflow_before_candidate_preparation() {
    let (editor, _) = fixture();
    let mut snapshot = editor.project().snapshot();
    let mut template = snapshot.tracks[0].clone();
    template.gain_envelope = (0..1024).map(|i| point(i, 1.0)).collect();
    template.instrument = None;
    template.instrument_position = None;
    let first_id = snapshot.next_id;
    snapshot.tracks = (first_id..first_id + 64)
        .map(|id| {
            let mut track = template.clone();
            track.id = id;
            track
        })
        .collect();
    template.id = first_id + 64;
    template.gain_envelope.clear();
    snapshot.tracks.push(template);
    snapshot.next_id = first_id + 65;
    let project = Project::from_snapshot(snapshot).unwrap();
    let track = project.tracks()[64].id();
    let mut editor = Editor::new(project.clone(), 1).unwrap();
    let mut prepared = false;
    let result = editor.execute_prepared(
        Command::SpliceGainTouch {
            track,
            expected_epoch: 0,
            span: span(10, 20, 1, &[(10, 0.5)]),
        },
        |_| {
            prepared = true;
            Ok::<(), Invalid>(())
        },
    );
    assert_eq!(result.unwrap_err().0, "project gain lane limit reached");
    assert!(!prepared);
    assert_eq!(editor.project(), &project);
    assert!(!editor.can_undo());
}

#[test]
fn two_loop_passes_splice_as_one_undo_step_with_later_pass_winning_overlap() {
    let (mut editor, track) = fixture();
    let original = editor.project().snapshot();
    let mut later = span(15, 25, 2, &[(15, 0.8), (20, 1.1)]);
    later.context.loop_pass = 1;
    editor
        .execute(Command::SpliceGainTouchBatch {
            track,
            expected_epoch: 0,
            spans: vec![span(10, 20, 1, &[(10, 0.3)]), later],
        })
        .unwrap();
    let recorded = editor.project().snapshot();
    let lane = editor.project().track(track).unwrap().gain_lane();
    assert_eq!(lane.touch_gain_at(beat(10)), Some(0.3));
    assert_eq!(lane.touch_gain_at(beat(15)), Some(0.8));
    assert_eq!(lane.touch_gain_at(beat(20)), Some(1.1));
    assert_eq!(lane.touch_gain_at(beat(25)), None);
    assert_eq!(lane.recording_epoch(), 2);
    assert!(editor.undo());
    let mut expected = original;
    expected.tracks[0].gain_touch_epoch = 2;
    assert_eq!(editor.project().snapshot(), expected);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), recorded);
    for (expected_epoch, spans) in [
        (0, vec![span(30, 40, 3, &[(30, 0.2)])]),
        (2, vec![]),
        (
            2,
            vec![span(30, 40, 3, &[(30, 0.2)]), span(40, 50, 3, &[(40, 0.4)])],
        ),
    ] {
        assert!(
            editor
                .execute(Command::SpliceGainTouchBatch {
                    track,
                    expected_epoch,
                    spans
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), recorded);
    }
}

#[test]
fn later_pass_capacity_refusal_leaves_original_lane_history_and_redo_intact() {
    let (editor, track) = fixture();
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].gain_envelope = (0..1021).map(|i| point(i, 1.0)).collect();
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    editor
        .execute(Command::ClearTrackGainEnvelope { track })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    let mut prepared = false;
    let result = editor.execute_prepared(
        Command::SpliceGainTouchBatch {
            track,
            expected_epoch: 0,
            spans: vec![span(10, 20, 1, &[(10, 0.3)]), span(30, 40, 2, &[(30, 0.8)])],
        },
        |_| {
            prepared = true;
            Ok::<(), Invalid>(())
        },
    );
    assert!(result.is_err());
    assert!(!prepared);
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.can_redo());
    assert!(!editor.can_undo());
}

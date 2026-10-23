use super::*;
use hex_project::{Command, Created, Editor, GainLane};
fn fixture() -> Project {
    let mut e = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "touch".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    e.execute(Command::ReplaceTrackGainEnvelope {
        track,
        points: vec![
            GainPoint {
                incoming_gain: None,
                beat: BeatTime::ZERO,
                gain: 0.2,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                incoming_gain: None,
                beat: BeatTime::from_raw(100),
                gain: 0.8,
                outgoing: GainInterpolation::Step,
            },
        ],
    })
    .unwrap();
    e.project().clone()
}
fn touch() -> GainTouchSpan {
    GainTouchSpan {
        source_start: BeatTime::from_raw(20),
        start: BeatTime::from_raw(20),
        end: BeatTime::from_raw(80),
        context: GainTouchContext {
            recording_epoch: 1,
            project_generation: 0,
            canonical_revision: 1,
            audio_revision: 1,
            transport_epoch: 1,
            gesture_epoch: 1,
            gesture_serial: 1,
            sample_rate: 48000,
            loop_pass: 0,
        },
        steps: vec![
            GainTouchStep {
                beat: BeatTime::from_raw(20),
                gain: 1.1,
                origin_beat: BeatTime::from_raw(20),
                source_sequence: 1,
            },
            GainTouchStep {
                beat: BeatTime::from_raw(50),
                gain: 0.3,
                origin_beat: BeatTime::from_raw(50),
                source_sequence: 2,
            },
        ],
    }
}
fn proposed_doc() -> DocumentV22 {
    let mut doc = DocumentV22::from_project(&fixture()).unwrap();
    doc.tracks[0].gain_touch_epoch = 1;
    doc.tracks[0].gain_touch_spans = vec![SpanDto::from_model(&touch())];
    doc
}
#[test]
fn span_roundtrip_preserves_canonical_value_through_current_writer() {
    let doc = proposed_doc();
    let bytes = serde_json::to_vec(&doc).unwrap();
    let snapshot = serde_json::from_slice::<DocumentV22>(&bytes)
        .unwrap()
        .into_snapshot()
        .unwrap();
    assert_eq!(
        snapshot.tracks[0].gain_envelope,
        fixture().snapshot().tracks[0].gain_envelope
    );
    assert_eq!(snapshot.tracks[0].gain_touch_spans, vec![touch()]);
    assert_eq!(snapshot.tracks[0].gain_touch_epoch, 1);
    assert!(snapshot.validate_gain_lanes().is_ok());
    let restored = Project::from_snapshot(snapshot.clone()).unwrap();
    assert_eq!(restored.snapshot(), snapshot);
    let current = crate::encode(&restored).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&current).unwrap()["version"],
        crate::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(crate::decode(&current).unwrap(), restored);
    assert_eq!(
        crate::decode(&crate::encode_schema22(&restored).unwrap()).unwrap(),
        restored
    );
    assert!(super::super::gain_envelopes::DocumentV21::from_project(&restored).is_err());
    let lane = GainLane::new(
        snapshot.tracks[0].gain_envelope.clone(),
        snapshot.tracks[0].gain_touch_spans.clone(),
        1,
    )
    .unwrap();
    assert_eq!(lane.touch_gain_at(BeatTime::from_raw(80)), None);
    assert_eq!(lane.touch_gain_at(BeatTime::from_raw(50)), Some(0.3));
}
#[test]
fn strict_fields_context_events_epochs_and_order_refuse_before_adoption() {
    let original = serde_json::to_value(proposed_doc()).unwrap();
    for case in 0..10 {
        let mut bad = original.clone();
        let track = &mut bad["tracks"][0];
        match case {
            0 => {
                track.as_object_mut().unwrap().remove("gain_touch_epoch");
            }
            1 => {
                track.as_object_mut().unwrap().remove("gain_touch_spans");
            }
            2 => track["gain_touch_epoch"] = 0.into(),
            3 => track["gain_touch_spans"][0]["context"]["sample_rate"] = 44100.into(),
            4 => track["gain_touch_spans"][0]["context"]["canonical_revision"] = 0.into(),
            5 => track["gain_touch_spans"][0]["steps"][1]["source_sequence"] = 1.into(),
            6 => track["gain_touch_spans"][0]["steps"][1]["beat_raw"] = 80.into(),
            7 => track["gain_touch_spans"][0]["steps"][0]["surprise"] = true.into(),
            8 => track["gain_touch_spans"]
                .as_array_mut()
                .unwrap()
                .push(original["tracks"][0]["gain_touch_spans"][0].clone()),
            _ => track["gain_touch_spans"][0]["steps"][1]["origin_beat_raw"] = 40.into(),
        }
        let bytes = serde_json::to_vec(&bad).unwrap();
        let result = serde_json::from_slice::<DocumentV22>(&bytes)
            .map_err(StorageError::from)
            .and_then(DocumentV22::into_snapshot);
        assert!(result.is_err(), "case {case}");
    }
}
#[test]
fn decoder_limits_count_endpoints_and_base_points_together() {
    let original = serde_json::to_value(proposed_doc()).unwrap();
    for case in 0..3 {
        let mut bad = original.clone();
        let span = original["tracks"][0]["gain_touch_spans"][0].clone();
        let step = span["steps"][0].clone();
        if case == 0 {
            bad["tracks"][0]["gain_touch_spans"][0]["steps"] =
                vec![step; MAX_GAIN_TOUCH_STEPS_PER_SPAN + 1].into();
        } else if case == 1 {
            bad["tracks"][0]["gain_touch_spans"] =
                vec![span; MAX_GAIN_TOUCH_SPANS_PER_TRACK + 1].into();
        } else {
            bad["tracks"][0]["gain_envelope"] = (0..1024)
                .map(|i| serde_json::json!({"beat_raw":i,"gain":1.0,"outgoing":"linear"}))
                .collect::<Vec<_>>()
                .into();
        }
        let bytes = serde_json::to_vec(&bad).unwrap();
        assert!(
            serde_json::from_slice::<DocumentV22>(&bytes)
                .map_err(StorageError::from)
                .and_then(DocumentV22::into_snapshot)
                .is_err()
        );
    }
}
#[test]
fn empty_spans_preserve_existing_base_and_high_water_old_writers_refuse_loss() {
    let project = fixture();
    // This case intentionally authors old-writer-compatible input; ordinary
    // new-track fixtures keep their automatically assigned non-gray colors.
    let track = project.tracks()[0].id();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::SetTrackColor {
            track,
            color: hex_project::ElementColor::Gray,
        })
        .unwrap();
    let project = editor.project().clone();
    let restored = crate::decode(&crate::encode(&project).unwrap()).unwrap();
    assert_eq!(restored, project);
    assert!(
        restored
            .track(project.tracks()[0].id())
            .unwrap()
            .gain_lane()
            .spans()
            .is_empty()
    );
    let mut snapshot = project.snapshot();
    snapshot.tracks[0].gain_touch_epoch = 9;
    let project = Project::from_snapshot(snapshot).unwrap();
    let encoded = crate::encode(&project).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&encoded).unwrap()["version"],
        crate::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(crate::decode(&encoded).unwrap(), project);
    assert!(super::super::gain_envelopes::DocumentV21::from_project(&project).is_err());
    assert!(crate::encode_schema20(&project).is_err());
    assert!(super::super::provenance::DocumentV18::from_project(&project).is_err());
}
#[test]
fn proposed_trim_fragment_roundtrip_retains_original_held_event_identity() {
    let base = fixture().tracks()[0].gain_envelope().to_vec();
    let lane = GainLane::new(base, vec![touch()], 1).unwrap();
    let mut replacement = touch();
    replacement.start = BeatTime::from_raw(30);
    replacement.source_start = replacement.start;
    replacement.end = BeatTime::from_raw(40);
    replacement.context.recording_epoch = 2;
    replacement.context.gesture_serial = 2;
    replacement.steps = vec![GainTouchStep {
        beat: replacement.start,
        gain: 0.9,
        origin_beat: replacement.start,
        source_sequence: 1,
    }];
    let lane = lane.with_touch(1, replacement).unwrap();
    let mut doc = DocumentV22::from_project(&fixture()).unwrap();
    doc.tracks[0].gain_touch_epoch = 2;
    doc.tracks[0].gain_touch_spans = lane.spans().iter().map(SpanDto::from_model).collect();
    let bytes = serde_json::to_vec(&doc).unwrap();
    let snapshot = serde_json::from_slice::<DocumentV22>(&bytes)
        .unwrap()
        .into_snapshot()
        .unwrap();
    assert_eq!(snapshot.tracks[0].gain_touch_spans, lane.spans());
    assert_eq!(
        snapshot.tracks[0].gain_touch_spans[2].steps[0].origin_beat,
        BeatTime::from_raw(20)
    );
    assert_eq!(
        snapshot.tracks[0].gain_touch_spans[2].steps[0].source_sequence,
        1
    );
    let project = Project::from_snapshot(snapshot.clone()).unwrap();
    assert_eq!(
        crate::decode(&crate::encode(&project).unwrap())
            .unwrap()
            .snapshot(),
        snapshot
    );
}

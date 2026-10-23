use hex_project::{BeatTime, Command, Created, Editor, ElementColor, Project};
use serde_json::json;
fn b(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn fixture() -> Project {
    let mut e = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Imported { track, clip, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "Original.wav".into(),
            sample_rate: 8_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    e.execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    e.execute(Command::SetAudioClipMusical {
        clip,
        enabled: true,
    })
    .unwrap();
    e.execute(Command::ReplaceTrackGainEnvelope {
        track,
        points: [(0, 0.2), (6, 1.7), (12, 0.4)]
            .into_iter()
            .map(|(beat, gain)| hex_project::GainPoint {
                beat: b(beat),
                gain,
                incoming_gain: None,
                outgoing: hex_project::GainInterpolation::Linear,
            })
            .collect(),
    })
    .unwrap();
    let context = hex_project::GainTouchContext {
        recording_epoch: 1,
        project_generation: 0,
        canonical_revision: 1,
        audio_revision: 1,
        transport_epoch: 1,
        gesture_epoch: 1,
        gesture_serial: 1,
        sample_rate: 8_000,
        loop_pass: 0,
    };
    e.execute(Command::SpliceGainTouch {
        track,
        expected_epoch: 0,
        span: hex_project::GainTouchSpan {
            source_start: b(1),
            start: b(1),
            end: b(9),
            context,
            steps: [(1, 0.8), (3, 0.5), (8, 1.1)]
                .into_iter()
                .enumerate()
                .map(|(i, (beat, gain))| hex_project::GainTouchStep {
                    beat: b(beat),
                    gain,
                    origin_beat: b(beat),
                    source_sequence: i as u64 + 1,
                })
                .collect(),
        },
    })
    .unwrap();
    let mut add = |name: &str, start, end| {
        let Created::ArrangementSection(id) = e
            .execute(Command::AddArrangementSection {
                name: name.into(),
                color: ElementColor::Rose,
                start: b(start),
                end: b(end),
            })
            .unwrap()
        else {
            panic!("section")
        };
        id
    };
    let a = add("Introduction", 2, 4);
    let c = add("Chorus", 7, 10);
    e.execute(Command::ReorderSectionBefore {
        section: c,
        before: a,
    })
    .unwrap();
    e.project().clone()
}
#[test]
fn current_format_roundtrips_section_ids_case_colors_ranges_frozen_processing_and_automation() {
    let project = fixture();
    let bytes = hex_storage::encode(&project).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(doc["arrangement_sections"][0]["name"], "Chorus");
    assert_eq!(doc["arrangement_sections"][0]["color"], "rose");
    assert!(
        doc["audio_clip_state"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| !row["audio_edit"]["processing_projection"].is_null())
    );
    assert!(
        project.tracks()[0]
            .gain_envelope()
            .iter()
            .any(|point| point.incoming_gain.is_some_and(|left| left != point.gain))
    );
    assert!(
        project.tracks()[0]
            .gain_lane()
            .spans()
            .windows(2)
            .any(|pair| pair[0].source_start > pair[1].source_start)
    );
    assert_eq!(
        hex_storage::decode(&bytes).unwrap().snapshot(),
        project.snapshot()
    );
}
#[test]
fn malformed_missing_overlap_duplicate_ids_and_unbounded_residuals_are_refused() {
    let project = fixture();
    let base: serde_json::Value =
        serde_json::from_slice(&hex_storage::encode(&project).unwrap()).unwrap();
    let mut cases = Vec::new();
    let mut missing = base.clone();
    missing
        .as_object_mut()
        .unwrap()
        .remove("arrangement_sections");
    cases.push(missing);
    for (field, value) in [
        ("id", json!(0)),
        ("color", json!("unknown")),
        ("end_raw", json!(0)),
        ("name", json!("")),
    ] {
        let mut doc = base.clone();
        doc["arrangement_sections"][0][field] = value;
        cases.push(doc);
    }
    let mut overlap = base.clone();
    overlap["arrangement_sections"][1]["start_raw"] = json!(b(3).raw());
    cases.push(overlap);
    let mut duplicate = base.clone();
    duplicate["arrangement_sections"][1]["id"] = duplicate["arrangement_sections"][0]["id"].clone();
    cases.push(duplicate);
    let mut residual = base.clone();
    residual["audio_clip_state"][0]["audio_edit"]["placement_start_offset"] = json!(2);
    cases.push(residual);
    let mut missing_projection = base.clone();
    missing_projection["audio_clip_state"][0]["audio_edit"]
        .as_object_mut()
        .unwrap()
        .remove("processing_projection");
    cases.push(missing_projection);
    let mut wrong_processing = base.clone();
    wrong_processing["audio_clip_state"][0]["audio_edit"]["processing_projection"]["tempo_points"]
        [0]["micro_bpm"] = json!(240_000_000);
    cases.push(wrong_processing);
    for doc in cases {
        assert!(
            hex_storage::decode(&serde_json::to_vec(&doc).unwrap()).is_err(),
            "{doc}"
        );
    }
}

#[test]
fn global_tempo_absorbed_sections_roundtrip_and_reject_competing_processing_clocks() {
    let mut editor = Editor::new(fixture(), 16).unwrap();
    editor
        .execute(Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let bytes = hex_storage::encode(editor.project()).unwrap();
    let decoded = hex_storage::decode(&bytes).unwrap();
    assert_eq!(decoded.snapshot(), editor.project().snapshot());
    assert!(decoded.tracks()[0].clips().iter().all(|c| {
        let edit = c.audio_edit().unwrap();
        matches!(edit.timing, hex_project::AudioClipTiming::Frozen { .. })
            && edit.processing_projection.is_none()
    }));
    let base: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let retained: serde_json::Value =
        serde_json::from_slice(&hex_storage::encode(&fixture()).unwrap()).unwrap();
    let mut cases = Vec::new();
    let mut competing = base.clone();
    competing["audio_clip_state"][0]["audio_edit"]["processing_projection"] =
        retained["audio_clip_state"][0]["audio_edit"]["processing_projection"].clone();
    cases.push(competing);
    let mut residual = base.clone();
    residual["audio_clip_state"][0]["audio_edit"]["placement_start_offset"] = json!(1);
    cases.push(residual);
    let mut wrong_rate = base.clone();
    wrong_rate["audio_clip_state"][0]["audio_edit"]["timing"]["sample_rate"] = json!(16000);
    cases.push(wrong_rate);
    for field in ["timing", "placement_end_offset"] {
        let mut missing = base.clone();
        missing["audio_clip_state"][0]["audio_edit"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        cases.push(missing);
    }
    for doc in cases {
        assert!(
            hex_storage::decode(&serde_json::to_vec(&doc).unwrap()).is_err(),
            "{doc}"
        );
    }
}

#[test]
fn structural_section_edits_roundtrip_the_complete_project_and_one_history_edge() {
    for operation in [
        "resize",
        "duplicate",
        "delete",
        "merge",
        "shared",
        "shared-marker",
        "isolated-left",
        "isolated-right",
    ] {
        let mut editor = Editor::new(fixture(), 16).unwrap();
        let sections = editor.project().arrangement_sections().to_vec();
        assert_eq!(sections.len(), 2);
        let ids: Vec<_> = sections.iter().map(|section| section.id()).collect();
        let before = editor.project().snapshot();
        assert!(
            editor.project().tracks()[0].clips()[0]
                .audio_edit()
                .unwrap()
                .processing_projection
                .is_some()
        );
        assert!(!editor.project().tracks()[0].gain_lane().spans().is_empty());

        match operation {
            "resize" => {
                let marker = editor.project().arrangement_section(ids[0]).unwrap();
                let new_length = BeatTime::from_raw(
                    marker.end().raw() - marker.start().raw() + BeatTime::ONE.raw(),
                );
                editor
                    .execute(Command::ResizeArrangementSection {
                        section: ids[0],
                        edge: hex_project::TrimEdge::Left,
                        length: new_length,
                    })
                    .unwrap();
                let resized = editor.project().arrangement_section(ids[0]).unwrap();
                assert_eq!(resized.start(), sections[0].start());
                assert_eq!(
                    resized.end().raw() - resized.start().raw(),
                    new_length.raw()
                );
            }
            "isolated-left" | "isolated-right" => {
                let marker = editor.project().arrangement_section(ids[0]).unwrap();
                let (edge, boundary) = if operation == "isolated-left" {
                    (
                        hex_project::TrimEdge::Left,
                        BeatTime::from_raw(marker.start().raw() + BeatTime::ONE.raw()),
                    )
                } else {
                    (
                        hex_project::TrimEdge::Right,
                        BeatTime::from_raw(marker.end().raw() - BeatTime::ONE.raw()),
                    )
                };
                editor
                    .execute(Command::ResizeArrangementSectionEdge {
                        section: ids[0],
                        edge,
                        boundary,
                    })
                    .unwrap();
            }
            "shared" | "shared-marker" => {
                let old = editor.project().arrangement_section(ids[0]).unwrap().end();
                editor
                    .execute(Command::ResizeArrangementSectionBoundary {
                        left: ids[0],
                        right: ids[1],
                        boundary: BeatTime::from_raw(old.raw() + BeatTime::ONE.raw()),
                        marker_only: operation == "shared-marker",
                    })
                    .unwrap();
            }
            "duplicate" => {
                let Created::ArrangementSection(copy) = editor
                    .execute(Command::DuplicateArrangementSection(ids[0]))
                    .unwrap()
                else {
                    panic!("duplicate section result")
                };
                let source = editor.project().arrangement_section(ids[0]).unwrap();
                let duplicate = editor.project().arrangement_section(copy).unwrap();
                assert_eq!(duplicate.start(), source.end());
                assert_eq!(duplicate.name(), source.name());
                assert_eq!(duplicate.color(), source.color());
            }
            "delete" => {
                editor
                    .execute(Command::DeleteArrangementSection(ids[0]))
                    .unwrap();
                assert!(editor.project().arrangement_section(ids[0]).is_none());
            }
            "merge" => {
                editor
                    .execute(Command::MergeArrangementSections(ids.clone()))
                    .unwrap();
                assert_eq!(editor.project().arrangement_sections().len(), 1);
                assert_eq!(editor.project().arrangement_sections()[0].id(), ids[0]);
            }
            _ => unreachable!(),
        }

        let after = editor.project().snapshot();
        let bytes = hex_storage::encode(editor.project()).unwrap();
        assert_eq!(hex_storage::decode(&bytes).unwrap().snapshot(), after);
        assert!(editor.undo());
        assert!(!editor.can_undo());
        let mut undone = editor.project().snapshot();
        undone.next_id = before.next_id;
        assert_eq!(undone, before, "{operation} undo");
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after, "{operation} redo");
    }
}

#[test]
fn split_markers_roundtrip_complete_current_state_and_stable_history() {
    let mut editor = Editor::new(fixture(), 8).unwrap();
    let source = editor.project().arrangement_sections()[0].clone();
    let before = editor.project().snapshot();
    let at =
        BeatTime::from_raw(source.start().raw() + (source.end().raw() - source.start().raw()) / 2);
    let Created::ArrangementSection(right) = editor
        .execute(Command::SplitArrangementSection {
            section: source.id(),
            at,
        })
        .unwrap()
    else {
        panic!("split")
    };
    let after = editor.project().snapshot();
    let encoded = hex_storage::encode(editor.project()).unwrap();
    let reopened = hex_storage::decode(&encoded).unwrap();
    assert_eq!(reopened.snapshot(), after);
    assert_eq!(reopened.arrangement_section(right).unwrap().start(), at);
    assert_eq!(reopened.arrangement_section(source.id()).unwrap().end(), at);
    assert!(editor.undo());
    let mut undone = hex_storage::decode(&hex_storage::encode(editor.project()).unwrap())
        .unwrap()
        .snapshot();
    undone.next_id = before.next_id;
    assert_eq!(undone, before);
    assert!(editor.redo());
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(editor.project()).unwrap())
            .unwrap()
            .snapshot(),
        after
    );
}

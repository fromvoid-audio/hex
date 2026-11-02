use hex_project::{BeatTime, Command, Created, Editor, ElementColor, Project};
use serde_json::json;
#[allow(dead_code)]
mod common;
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
    assert_eq!(
        e.project().meter_map(),
        &hex_project::MeterMap::common_time(),
        "section reorder must not persist synthetic same-meter seams"
    );
    // Its envelopes are relabelled as historical schemas below.
    implied_warp_laws(e.project())
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
fn schema49_roundtrips_retained_musical_repeat_after_section_reorder() {
    let project = fixture();
    let tempo = project.tempo_map().clone();
    let mut snapshot = project.snapshot();
    let clip = snapshot.tracks[0]
        .audio_clips_mut()
        .unwrap()
        .iter()
        .max_by_key(|clip| clip.start)
        .unwrap();
    let id = clip.id;
    let audio = snapshot.tracks[0]
        .audio_clips_mut()
        .unwrap()
        .iter_mut()
        .find(|clip| clip.id == id)
        .unwrap();
    let edit = audio.edit.as_mut().unwrap();
    let start = match &edit.timing {
        hex_project::AudioClipTiming::Musical(start) => *start,
        _ => panic!("reordered section audio retains musical placement"),
    };
    assert!(edit.processing_projection.is_some());
    let end_offset = i128::from(edit.placement_end_offset);
    let cycle = edit.view_end - edit.view_start;
    let outer_length = cycle.checked_mul(2).unwrap();
    let end = BeatTime::from_raw(start.raw().checked_add(outer_length).unwrap());
    let end_frame = i128::from(tempo.beat_to_frame(end).unwrap()) + end_offset;
    audio.duration = u64::try_from(end_frame - i128::from(audio.start)).unwrap();
    audio.fade = hex_project::ClipFade::neutral(audio.duration).unwrap();
    edit.placement = Some(hex_project::AudioViewPlacement {
        mode: hex_project::AudioViewMode::Repeat,
        outer_length: hex_project::AudioDomainClock::Beats(BeatTime::from_raw(outer_length)),
        cycle_phase: cycle / 3,
    });
    let project = Project::from_snapshot(snapshot).unwrap();
    let bytes = hex_storage::encode(&project).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(document["version"], hex_storage::CURRENT_SCHEMA_VERSION);

    assert_eq!(
        hex_storage::decode(&bytes).unwrap().snapshot(),
        project.snapshot()
    );

    let mut prior = document;
    common::remove_audio_cycles(&mut prior);
    prior["version"] = 47.into();
    assert_eq!(
        hex_storage::decode(&serde_json::to_vec(&prior).unwrap()).unwrap(),
        project
    );
    prior["version"] = 46.into();
    let error = hex_storage::decode(&serde_json::to_vec(&prior).unwrap()).unwrap_err();
    assert!(error.to_string().contains("historical project schema"));
}

#[test]
fn schema47_accepts_but_historical_schemas_reject_musical_directed_empty_base_intro() {
    let project = fixture();
    let tempo = project.tempo_map().clone();
    let mut snapshot = project.snapshot();
    let audio = snapshot.tracks[0]
        .audio_clips_mut()
        .unwrap()
        .iter_mut()
        .max_by_key(|clip| clip.start)
        .unwrap();
    let edit = audio.edit.as_mut().unwrap();
    let start = match &edit.timing {
        hex_project::AudioClipTiming::Musical(start) => *start,
        _ => panic!("section reorder retains musical timing"),
    };
    let cycle = edit.view_end - edit.view_start;
    assert!(edit.processing_projection.is_some());
    assert!(edit.placement.is_none());
    assert!(cycle > 1);
    let outer_length = cycle.checked_mul(2).unwrap();
    edit.placement_start_offset = 0;
    edit.placement_end_offset = 0;
    edit.content_view = Some(hex_project::AudioContentView {
        restore_plain: false,
        base_view_start: edit.view_start,
        base_view_end: edit.view_end,
        base_placement: Some(hex_project::AudioViewPlacement {
            mode: hex_project::AudioViewMode::Repeat,
            outer_length: hex_project::AudioDomainClock::Beats(BeatTime::from_raw(outer_length)),
            cycle_phase: cycle - 1,
        }),
        base_start_delta: 0,
        window: hex_project::ContentWindow::new(0, cycle, false).unwrap(),
        // Historical musical views carry the implied frozen v1 read law.
        read_resampler: hex_project::WarpResampler::V1,
    });
    let displacement = i128::from(audio.start)
        - hex_project::audio_projection_frame(&tempo, i128::from(start.raw())).unwrap();
    let end =
        hex_project::audio_projection_frame(&tempo, i128::from(start.raw()) + i128::from(cycle))
            .unwrap()
            + displacement;
    audio.duration = u64::try_from(end - i128::from(audio.start)).unwrap();
    audio.fade = hex_project::ClipFade::neutral(audio.duration).unwrap();

    let project = Project::from_snapshot(snapshot).unwrap();
    let clip = project.tracks()[0]
        .clips()
        .iter()
        .max_by_key(|clip| clip.start())
        .unwrap();
    let projection = hex_project::AudioViewProjection::new(clip, project.tempo_map()).unwrap();
    assert!(projection.has_subframe_intro());

    let bytes = hex_storage::encode(&project).unwrap();
    assert_eq!(hex_storage::decode(&bytes).unwrap(), project);
    let mut historical: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    common::remove_audio_cycles(&mut historical);
    historical["version"] = 47.into();
    assert_eq!(
        hex_storage::decode(&serde_json::to_vec(&historical).unwrap()).unwrap(),
        project
    );
    historical["version"] = 46.into();
    let error = hex_storage::decode(&serde_json::to_vec(&historical).unwrap()).unwrap_err();
    assert!(error.to_string().contains("historical project schema"));
}

#[test]
fn schema47_accepts_but_historical_schemas_reject_empty_intro_for_both_timing_modes() {
    for frozen in [true, false] {
        let project = fixture();
        let tempo = project.tempo_map().clone();
        let mut snapshot = project.snapshot();
        let audio = snapshot.tracks[0]
            .audio_clips_mut()
            .unwrap()
            .iter_mut()
            .max_by_key(|clip| clip.start)
            .unwrap();
        let edit = audio.edit.as_mut().unwrap();
        let start = edit.reference_start_raw().unwrap();
        let cycle = edit.view_end - edit.view_start;
        assert!(cycle > 1);
        if frozen {
            edit.timing = hex_project::AudioClipTiming::Frozen {
                reference_start_raw: start,
                tempo: std::sync::Arc::new(tempo.clone()),
            };
        } else {
            assert!(matches!(
                &edit.timing,
                hex_project::AudioClipTiming::Musical(_)
            ));
        }
        edit.processing_projection = None;
        edit.placement_start_offset = 0;
        edit.placement_end_offset = 0;
        edit.placement = Some(hex_project::AudioViewPlacement {
            mode: hex_project::AudioViewMode::Repeat,
            outer_length: hex_project::AudioDomainClock::Beats(BeatTime::from_raw(cycle * 2)),
            cycle_phase: cycle - 1,
        });
        let displacement =
            i128::from(audio.start) - hex_project::audio_projection_frame(&tempo, start).unwrap();
        let end = hex_project::audio_projection_frame(&tempo, start + i128::from(cycle * 2))
            .unwrap()
            + displacement;
        audio.duration = u64::try_from(end - i128::from(audio.start)).unwrap();
        audio.fade = hex_project::ClipFade::neutral(audio.duration).unwrap();

        let project = Project::from_snapshot(snapshot).unwrap();
        let clip = project.tracks()[0]
            .clips()
            .iter()
            .max_by_key(|clip| clip.start())
            .unwrap();
        let projection = hex_project::AudioViewProjection::new(clip, project.tempo_map()).unwrap();
        assert!(projection.has_subframe_intro());
        let bytes = hex_storage::encode(&project).unwrap();
        let current: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(current["version"], hex_storage::CURRENT_SCHEMA_VERSION);

        assert_eq!(hex_storage::decode(&bytes).unwrap(), project);

        let mut prior = current;
        common::remove_audio_cycles(&mut prior);
        prior["version"] = 47.into();
        assert_eq!(
            hex_storage::decode(&serde_json::to_vec(&prior).unwrap()).unwrap(),
            project
        );
        prior["version"] = 46.into();
        let error = hex_storage::decode(&serde_json::to_vec(&prior).unwrap()).unwrap_err();
        assert!(error.to_string().contains("historical project schema"));
    }
}

#[test]
fn schema46_accepts_frozen_directed_repeat_trimmed_to_its_base_seam() {
    let project = fixture();
    let tempo = project.tempo_map().clone();
    let mut snapshot = project.snapshot();
    let audio = snapshot.tracks[0]
        .audio_clips_mut()
        .unwrap()
        .iter_mut()
        .max_by_key(|clip| clip.start)
        .unwrap();
    let edit = audio.edit.as_mut().unwrap();
    let start = edit.reference_start_raw().unwrap();
    let cycle = edit.view_end - edit.view_start;
    let phase = cycle / 3;
    let base_length = cycle * 2;
    let seam = cycle - phase;
    let window_length = cycle;
    edit.timing = hex_project::AudioClipTiming::Frozen {
        reference_start_raw: start,
        tempo: std::sync::Arc::new(tempo.clone()),
    };
    edit.processing_projection = None;
    edit.placement_start_offset = 0;
    edit.placement_end_offset = 0;
    edit.placement = Some(hex_project::AudioViewPlacement {
        mode: hex_project::AudioViewMode::Repeat,
        outer_length: hex_project::AudioDomainClock::Beats(BeatTime::from_raw(window_length)),
        cycle_phase: 0,
    });
    edit.content_view = Some(hex_project::AudioContentView {
        restore_plain: false,
        base_view_start: edit.view_start,
        base_view_end: edit.view_end,
        base_placement: Some(hex_project::AudioViewPlacement {
            mode: hex_project::AudioViewMode::Repeat,
            outer_length: hex_project::AudioDomainClock::Beats(BeatTime::from_raw(base_length)),
            cycle_phase: phase,
        }),
        base_start_delta: 0,
        window: hex_project::ContentWindow::new(seam, seam + window_length, false).unwrap(),
        // Historical musical views carry the implied frozen v1 read law.
        read_resampler: hex_project::WarpResampler::V1,
    });
    let displacement =
        i128::from(audio.start) - hex_project::audio_projection_frame(&tempo, start).unwrap();
    let end = hex_project::audio_projection_frame(&tempo, start + i128::from(window_length))
        .unwrap()
        + displacement;
    audio.duration = u64::try_from(end - i128::from(audio.start)).unwrap();
    audio.fade = hex_project::ClipFade::neutral(audio.duration).unwrap();

    let project = Project::from_snapshot(snapshot).unwrap();
    let clip = project.tracks()[0]
        .clips()
        .iter()
        .max_by_key(|clip| clip.start())
        .unwrap();
    let projection = hex_project::AudioViewProjection::new(clip, project.tempo_map()).unwrap();
    assert!(!projection.has_subframe_intro());
    let mut historical: serde_json::Value =
        serde_json::from_slice(&hex_storage::encode(&project).unwrap()).unwrap();
    common::remove_audio_cycles(&mut historical);
    historical["version"] = 47.into();
    assert_eq!(
        hex_storage::decode(&serde_json::to_vec(&historical).unwrap()).unwrap(),
        project
    );
    historical["version"] = 46.into();
    let decoded = hex_storage::decode(&serde_json::to_vec(&historical).unwrap()).unwrap();
    assert_eq!(decoded.snapshot(), project.snapshot());
}

#[test]
fn marker_move_split_roundtrips_without_changing_music_or_processing() {
    let mut editor = Editor::new(fixture(), 8).unwrap();
    let before = editor.project().snapshot();
    let moving = editor.project().arrangement_sections()[1].id();
    editor
        .execute(Command::MoveArrangementSectionMarker {
            section: moving,
            start: BeatTime::from_ratio(5, 2).unwrap(),
        })
        .unwrap();
    let after = editor.project().snapshot();
    assert_eq!(after.arrangement_sections.len(), 3);
    let mut expected = before;
    expected.arrangement_sections = after.arrangement_sections.clone();
    expected.next_id = after.next_id;
    assert_eq!(after, expected);
    let bytes = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&bytes).unwrap().snapshot(), after);
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
        "duplicate-before",
        "duplicate-after",
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
            "duplicate-before" | "duplicate-after" => {
                let after_target = operation == "duplicate-after";
                let Created::ArrangementSection(copy) = editor
                    .execute(Command::DuplicateArrangementSectionAt {
                        section: ids[0],
                        target: ids[1],
                        after: after_target,
                    })
                    .unwrap()
                else {
                    panic!("destination copy identity");
                };
                let source = editor.project().arrangement_section(ids[0]).unwrap();
                let target = editor.project().arrangement_section(ids[1]).unwrap();
                let copied = editor.project().arrangement_section(copy).unwrap();
                assert_eq!(
                    (copied.name(), copied.color()),
                    (source.name(), source.color())
                );
                if after_target {
                    assert_eq!(copied.start(), target.end());
                } else {
                    assert_eq!(copied.end(), target.start());
                }
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

#[test]
fn marker_resize_overlap_roundtrips_with_unchanged_music() {
    let mut editor = Editor::new(fixture(), 8).unwrap();
    let before = editor.project().snapshot();
    let sections = editor.project().arrangement_sections();
    let id = sections[0].id();
    let boundary = sections[1].end();
    editor
        .execute(Command::ResizeArrangementSectionMarker {
            section: id,
            edge: hex_project::TrimEdge::Right,
            boundary,
        })
        .unwrap();
    let after = editor.project().snapshot();
    assert_eq!(after.arrangement_sections[0].end, boundary);
    let mut music = after.clone();
    music.arrangement_sections = before.arrangement_sections.clone();
    assert_eq!(music, before);
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(editor.project()).unwrap())
            .unwrap()
            .snapshot(),
        after
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

/// Model a project authored before schema65 warp resampler laws: visible clip
/// content takes the law its pitch/warp state implies (v1 when it reads
/// fractional positions). This fixture relabels current envelopes as historical
/// schemas; it has no retained roots or aliases.
fn implied_warp_laws(project: &hex_project::Project) -> hex_project::Project {
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        if let hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut track.content {
            for edit in audio_clips.iter_mut().filter_map(|clip| clip.edit.as_mut()) {
                let domain = std::sync::Arc::make_mut(&mut edit.domain);
                domain.warp_resampler =
                    hex_project::WarpResampler::implied(domain.pitch_cents, domain.warp_enabled);
            }
        }
    }
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    assert!(
        hex_storage::schema_version_for(&project) < hex_storage::WARP_RESAMPLER_SCHEMA_VERSION,
        "only visible clip content is converted"
    );
    project
}

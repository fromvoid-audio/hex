//! Current-format retained sources, strict inventories and prior-schema migration.
use hex_project::{AudioEventEdit, ClipId, Command, Created, Editor, Project};
use hex_storage::{decode, encode};
use serde_json::{Value, json};
mod common;
fn fixture() -> (Editor, Vec<ClipId>) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { track, clip: a, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "a".into(),
            path: "Media/A.wav".into(),
            sample_rate: 48_000,
            frames: 200,
        })
        .unwrap()
    else {
        panic!()
    };
    e.execute(Command::EditAudioEvents {
        clip: a,
        operation: AudioEventEdit::Split { at: 70 },
    })
    .unwrap();
    e.execute(Command::SetAudioClipGain {
        clip: a,
        millidb: -3000,
    })
    .unwrap();
    let Created::Asset(asset) = e
        .execute(Command::AddAsset {
            path: "Media/B.wav".into(),
            sample_rate: 48_000,
            frames: 150,
        })
        .unwrap()
    else {
        panic!()
    };
    let Created::Clip(b) = e
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 400,
            duration: 200,
            source_offset: 20,
            source_length: 100,
            looping: true,
        })
        .unwrap()
    else {
        panic!()
    };
    e.execute(Command::SetClipFades {
        clip: b,
        fade_in: 30,
        fade_out: 40,
    })
    .unwrap();
    (Editor::new(e.project().clone(), 16).unwrap(), vec![a, b])
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn strip_flat_hierarchy(value: &mut Value) {
    common::restore_pre_binding_instruments(value);
    let expected: Vec<_> = value["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| json!({"track":track["id"],"group":false,"parent":null}))
        .collect();
    assert_eq!(
        value.as_object_mut().unwrap().remove("track_hierarchy"),
        Some(json!(expected))
    );
}

#[test]
fn nested_repeated_group_roundtrips_and_rejects_missing_or_invalid_sources() {
    let (mut editor, ids) = fixture();
    editor
        .execute(Command::SetClipActive {
            clip: ids[1],
            active: false,
        })
        .unwrap();
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    editor
        .execute(Command::ResizeAudioClipView {
            clip: group,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame: 700 },
        })
        .unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().clip(group).unwrap().asset();
    let Created::Clip(third) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 900,
            duration: 100,
            source_offset: 0,
            source_length: 100,
            looping: false,
        })
        .unwrap()
    else {
        panic!("third")
    };
    let Created::Clip(outer) = editor
        .execute(Command::GroupClips(vec![group, third]))
        .unwrap()
    else {
        panic!("regroup")
    };
    let encoded = encode(editor.project()).unwrap();
    let mut value: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decode(&encoded).unwrap(), *editor.project());
    let nested = decode(&encoded)
        .unwrap()
        .clip(outer)
        .unwrap()
        .audio_events()
        .sources;
    assert!(nested.iter().any(|root| {
        root.clip
            .audio_events()
            .events
            .iter()
            .any(|event| !event.active)
    }));
    let rows = value["nested_audio"].as_array_mut().unwrap();
    let row = rows.iter().position(|row| !row["state"].is_null()).unwrap();
    let original = rows[row].clone();
    rows.remove(row);
    assert!(decode(&bytes(&value)).is_err());
    value["nested_audio"]
        .as_array_mut()
        .unwrap()
        .push(original.clone());
    value["nested_audio"].as_array_mut().unwrap().push(original);
    assert!(decode(&bytes(&value)).is_err());
    value["nested_audio"].as_array_mut().unwrap().pop();
    let row = value["nested_audio"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| !row["state"].is_null())
        .unwrap();
    row["state"]["sources"][0]["document"]["version"] = 56.into();
    assert!(decode(&bytes(&value)).is_err());
}

#[test]
fn nested_audio_aliases_roundtrip_and_reject_conflicting_member_projection() {
    let (mut editor, ids) = fixture();
    editor
        .execute(Command::SetClipActive {
            clip: ids[1],
            active: false,
        })
        .unwrap();
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    editor
        .execute(Command::ResizeAudioClipView {
            clip: group,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame: 700 },
        })
        .unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().clip(group).unwrap().asset();
    let Created::Clip(third) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 900,
            duration: 100,
            source_offset: 0,
            source_length: 100,
            looping: false,
        })
        .unwrap()
    else {
        panic!("third")
    };
    let Created::Clip(outer) = editor
        .execute(Command::GroupClips(vec![group, third]))
        .unwrap()
    else {
        panic!("regroup")
    };
    editor
        .execute(Command::DuplicateAliasClips(vec![outer]))
        .unwrap();
    let alias = editor.project().clip_alias(outer).unwrap();
    let copy = editor
        .project()
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .find(|clip| clip.id() != outer && editor.project().clip_alias(clip.id()) == Some(alias))
        .unwrap()
        .id();
    editor
        .execute(Command::SetAudioClipGain {
            clip: copy,
            millidb: -4_500,
        })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip: copy,
            fade_in: 15,
            fade_out: 30,
        })
        .unwrap();
    let (path, inner_event) = {
        let roots = editor
            .project()
            .clip(outer)
            .unwrap()
            .editable_audio_events()
            .unwrap();
        let parent = roots
            .sources
            .iter()
            .find(|root| {
                !root
                    .clip
                    .editable_audio_events()
                    .unwrap()
                    .sources
                    .is_empty()
            })
            .unwrap();
        let nested = parent.clip.editable_audio_events().unwrap();
        let child = &nested.sources[0];
        let inner = child.clip.editable_audio_events().unwrap().events[0].id;
        (vec![parent.id, child.id], inner)
    };
    editor
        .execute(Command::EditAudioEvents {
            clip: outer,
            operation: AudioEventEdit::SetRetainedGain {
                sources: path,
                event: inner_event,
                gain: hex_project::ClipGain::new(-6_000).unwrap(),
            },
        })
        .unwrap();
    let original = editor.project().snapshot();
    let encoded = encode(editor.project()).unwrap();
    let loaded = decode(&encoded).unwrap();
    assert_eq!(loaded.snapshot(), original);
    let source = loaded.clip_alias(outer).unwrap();
    let members: Vec<_> = loaded
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter(|clip| loaded.clip_alias(clip.id()) == Some(source))
        .collect();
    assert_eq!(members.len(), 2);
    assert_ne!(members[0].gain(), members[1].gain());
    assert_ne!(
        members[0].audio_edit().unwrap().domain.fade,
        members[1].audio_edit().unwrap().domain.fade
    );
    assert!(std::sync::Arc::ptr_eq(
        &members[0].audio_edit().unwrap().domain.content,
        &members[1].audio_edit().unwrap().domain.content
    ));
    let dir = std::env::temp_dir().join(format!(
        "hex-nested-alias-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("group.hex");
    hex_storage::save_new(&path, &loaded).unwrap();
    assert_eq!(hex_storage::load(&path).unwrap().snapshot(), original);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
    let mut value: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["alias_sources"].as_array().unwrap().len(), 1);
    assert_eq!(value["audio_event_gains"].as_array().unwrap().len(), 1);
    let row = value["nested_audio"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| !row["state"].is_null())
        .unwrap();
    let active = row["state"]["events"][0]["active"].as_bool().unwrap();
    row["state"]["events"][0]["active"] = (!active).into();
    assert!(decode(&bytes(&value)).is_err());
}

#[test]
fn previous_event_activation_schema_migrates_without_nested_sources() {
    let (mut editor, ids) = fixture();
    editor
        .execute(Command::EditAudioEvents {
            clip: ids[0],
            operation: AudioEventEdit::SetActive {
                events: vec![1],
                active: false,
            },
        })
        .unwrap();
    editor.execute(Command::GroupClips(ids)).unwrap();
    let mut value: Value = serde_json::from_slice(&encode(editor.project()).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("nested_audio");
    assert_eq!(
        value.as_object_mut().unwrap().remove("audio_event_gains"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        value.as_object_mut().unwrap().remove("midi_clip_gains"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        value
            .as_object_mut()
            .unwrap()
            .remove("automatic_track_names"),
        Some(serde_json::json!([]))
    );
    value["version"] = 57.into();
    assert_eq!(decode(&bytes(&value)).unwrap(), *editor.project());
}
#[test]
fn grouped_sources_roundtrip_complete_original_state_and_missing_media_paths() {
    let (mut e, ids) = fixture();
    let original = e.project().clip(ids[0]).unwrap().retained_snapshot();
    let Created::Clip(group) = e.execute(Command::GroupClips(ids)).unwrap() else {
        panic!()
    };
    let encoded = encode(e.project()).unwrap();
    let json: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(json["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    let reopened = decode(&encoded).unwrap();
    assert_eq!(reopened, *e.project());
    let roots = reopened.clip(group).unwrap().audio_events().sources;
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0].clip.retained_snapshot(), original);
    assert_eq!(reopened.assets().count(), 2);
    assert!(hex_storage::encode_schema22(&reopened).is_err());
    let dir = std::env::temp_dir().join(format!(
        "hex-grouping-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("group.hex");
    {
        hex_storage::save_new(&path, &reopened).unwrap();
        assert_eq!(hex_storage::load(&path).unwrap(), reopened);
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(dir).unwrap();
}
#[test]
fn schema40_migrates_in_memory_with_single_root_defaults() {
    let (e, _) = fixture();
    let mut value: Value = serde_json::from_slice(&encode(e.project()).unwrap()).unwrap();
    strip_flat_hierarchy(&mut value);
    value["version"] = 40.into();
    value.as_object_mut().unwrap().remove("grouped_audio");
    value.as_object_mut().unwrap().remove("content_views");
    value.as_object_mut().unwrap().remove("audio_event_fades");
    let original = bytes(&value);
    let loaded = decode(&original).unwrap();
    assert_eq!(loaded, *e.project());
    assert_eq!(bytes(&value), original);
    for clip in loaded.tracks().iter().flat_map(|t| t.clips()) {
        let events = clip.audio_events();
        assert!(events.sources.is_empty());
        assert!(events.events.iter().all(|e| e.source == 0));
    }
    assert_eq!(decode(&encode(&loaded).unwrap()).unwrap(), loaded);
}
#[test]
fn malformed_group_sources_refuse_missing_foreign_or_unrelated_state() {
    let (mut e, ids) = fixture();
    e.execute(Command::GroupClips(ids)).unwrap();
    let value: Value = serde_json::from_slice(&encode(e.project()).unwrap()).unwrap();
    for kind in 0..8 {
        let mut bad = value.clone();
        let state = &mut bad["grouped_audio"][0]["group"];
        match kind {
            0 => state["events"][0]["source"] = 999.into(),
            1 => state["events"][0]["source_start"] = u64::MAX.into(),
            2 => state["sources"][1]["id"] = state["sources"][0]["id"].clone(),
            3 => state["sources"][0]["document"]["assets"][0]["path"] = json!("other.wav"),
            4 => state["sources"][0]["document"]["master_gain"] = json!(0.5),
            5 => state["sources"][0]["document"]["version"] = 43.into(),
            6 => state["events"][0]["source"] = 0.into(),
            _ => state["extra"] = true.into(),
        }
        assert!(
            decode(&bytes(&bad)).is_err(),
            "accepted malformed inventory {kind}"
        );
    }
}

#[test]
fn grouping_preserves_directed_child_and_container_views_through_current_codec() {
    let (mut e, ids) = fixture();
    e.execute(Command::ReverseClips(ids.clone())).unwrap();
    let original = e.project().snapshot();
    let Created::Clip(group) = e.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group");
    };
    e.execute(Command::ReverseClips(vec![group])).unwrap();
    let data = encode(e.project()).unwrap();
    assert_eq!(decode(&data).unwrap().snapshot(), e.project().snapshot());
    assert!(e.undo());
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = original.next_id; // IDs remain monotonic across Undo.
    assert_eq!(restored, original);
}

#[test]
fn both_evidenced_schema41_producers_migrate_without_losing_inventory() {
    for data in [
        include_bytes!("fixtures/grouped-audio/schema41-group-04d5ee56.json").as_slice(),
        include_bytes!("fixtures/grouped-audio/schema41-content-08dfb3a0.json").as_slice(),
    ] {
        let original = data.to_vec();
        let old: Value = serde_json::from_slice(data).unwrap();
        let project = decode(data).unwrap();
        let current = encode(&project).unwrap();
        assert_eq!(decode(&current).unwrap(), project);
        let mut upgraded: Value = serde_json::from_slice(&current).unwrap();
        strip_flat_hierarchy(&mut upgraded);
        upgraded["version"] = 41.into();
        upgraded
            .as_object_mut()
            .unwrap()
            .remove("audio_event_fades");
        if old.get("grouped_audio").is_some() {
            upgraded.as_object_mut().unwrap().remove("content_views");
            // Current retained roots use strict42; the historical writer used
            // strict40. The actual original fixtures validate both envelopes.
            let mut sparse = Vec::new();
            for row in upgraded["grouped_audio"].as_array_mut().unwrap() {
                let Some(mut group) = row
                    .as_object_mut()
                    .unwrap()
                    .remove("group")
                    .filter(|v| !v.is_null())
                else {
                    continue;
                };
                group
                    .as_object_mut()
                    .unwrap()
                    .insert("clip".into(), row["clip"].clone());
                for event in group["events"].as_array_mut().unwrap() {
                    assert_eq!(
                        event.as_object_mut().unwrap().remove("fade"),
                        Some(Value::Null)
                    );
                }
                for root in group["sources"].as_array_mut().unwrap() {
                    root["document"]["version"] = 40.into();
                    root["document"]
                        .as_object_mut()
                        .unwrap()
                        .remove("content_views");
                    root["document"]
                        .as_object_mut()
                        .unwrap()
                        .remove("audio_event_fades");
                }
                sparse.push(group);
            }
            upgraded["grouped_audio"] = Value::Array(sparse);
        } else {
            upgraded.as_object_mut().unwrap().remove("grouped_audio");
        }
        assert_eq!(upgraded, old, "migration changed existing saved state");
        assert_eq!(data, original);
        let mut ambiguous = old.clone();
        if ambiguous.get("grouped_audio").is_some() {
            ambiguous["content_views"] = json!([]);
        } else {
            ambiguous["grouped_audio"] = json!([]);
        }
        assert!(decode(&bytes(&ambiguous)).is_err());
    }
}

#[test]
fn embedded_root_schema_matches_the_actual_outer_writer_lineage() {
    let (mut e, ids) = fixture();
    e.execute(Command::GroupClips(ids)).unwrap();
    let current: Value = serde_json::from_slice(&encode(e.project()).unwrap()).unwrap();
    let mut old_root_in_current = current.clone();
    let document = &mut old_root_in_current["grouped_audio"][0]["group"]["sources"][0]["document"];
    document["version"] = 40.into();
    document.as_object_mut().unwrap().remove("content_views");
    document
        .as_object_mut()
        .unwrap()
        .remove("audio_event_fades");
    assert!(decode(&bytes(&old_root_in_current)).is_err());
    let mut old: Value = serde_json::from_slice(include_bytes!(
        "fixtures/grouped-audio/schema41-group-04d5ee56.json"
    ))
    .unwrap();
    // Construct a valid published42 child using the complete decoded historical root.
    // It remains invalid in the draft41 outer envelope, whose writer used40.
    let old_document = &old["grouped_audio"][0]["sources"][0]["document"];
    let root = decode(&bytes(old_document)).unwrap();
    let mut newer_document: Value = serde_json::from_slice(&encode(&root).unwrap()).unwrap();
    strip_flat_hierarchy(&mut newer_document);
    newer_document["version"] = 42.into();
    newer_document
        .as_object_mut()
        .unwrap()
        .remove("grouped_audio");
    assert!(decode(&bytes(&newer_document)).is_ok());
    old["grouped_audio"][0]["sources"][0]["document"] = newer_document;
    assert!(decode(&bytes(&old)).is_err());
}

#[test]
fn grouped_and_retained_child_fades_roundtrip_with_independent_captured_tempo() {
    use hex_project::{AudioEventFade, BeatTime, ClipFade, Tempo, WarpMode};
    let (mut e, ids) = fixture();
    let first = ids[0];
    let event = e.project().clip(first).unwrap().audio_events().events[0];
    let fade = AudioEventFade::new(
        0,
        ClipFade::new(200, 0, 19, 37)
            .unwrap()
            .with_shape(3, 7, 123, -234)
            .unwrap()
            .with_curve_positions(177, 823)
            .unwrap(),
    )
    .unwrap();
    e.execute(Command::EditAudioEvents {
        clip: first,
        operation: AudioEventEdit::SetFade {
            event: event.id,
            fade: Some(fade),
        },
    })
    .unwrap();
    let settings = hex_project::default_audio_warp(
        e.project().clip(first).unwrap(),
        e.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    e.execute(Command::SetAudioClipWarp {
        clip: first,
        settings: Some(settings),
    })
    .unwrap();
    let original = e.project().clip(first).unwrap().retained_snapshot();
    let Created::Clip(group) = e.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    let event = e.project().clip(group).unwrap().audio_events().events[0];
    let outer = AudioEventFade::new(
        0,
        ClipFade::new(event.length(), 0, 17, 31)
            .unwrap()
            .with_shape(3, 9, 111, -222)
            .unwrap()
            .with_curve_positions(173, 827)
            .unwrap(),
    )
    .unwrap();
    e.execute(Command::EditAudioEvents {
        clip: group,
        operation: AudioEventEdit::SetFade {
            event: event.id,
            fade: Some(outer),
        },
    })
    .unwrap();
    e.execute(Command::SetTempoPoint {
        beat: BeatTime::ZERO,
        tempo: Tempo::new(180_000_000).unwrap(),
    })
    .unwrap();
    let data = encode(e.project()).unwrap();
    let value: Value = serde_json::from_slice(&data).unwrap();
    // Its retained musical roots render with the v2 warp resampler.
    assert_eq!(value["version"], hex_storage::WARP_RESAMPLER_SCHEMA_VERSION);
    assert_eq!(
        value["grouped_audio"][0]["group"]["sources"][0]["document"]["version"],
        42
    );
    assert!(!value["grouped_audio"][0]["group"]["events"][0]["fade"].is_null());
    assert!(
        !value["grouped_audio"][0]["group"]["sources"][0]["document"]["audio_event_fades"][0]
            ["events"][0]["fade"]
            .is_null()
    );
    let restored = decode(&data).unwrap();
    assert_eq!(restored, *e.project());
    let roots = restored.clip(group).unwrap().audio_events().sources;
    assert_eq!(roots[0].clip.retained_snapshot(), original);
    assert_ne!(roots[0].tempo.as_ref(), restored.tempo_map());
    assert_eq!(decode(&encode(&restored).unwrap()).unwrap(), restored);
    let mut missing = value.clone();
    missing["grouped_audio"][0]["group"]["events"][0]
        .as_object_mut()
        .unwrap()
        .remove("fade");
    assert!(decode(&bytes(&missing)).is_err());
}

#[test]
fn published_schema42_event_fades_migrate_with_every_predecessor_field_intact() {
    let data = include_bytes!("fixtures/grouped-audio/schema42-event-fades-31545bb9.json");
    let old: Value = serde_json::from_slice(data).unwrap();
    let project = decode(data).unwrap();
    let encoded = encode(&project).unwrap();
    let mut current: Value = serde_json::from_slice(&encoded).unwrap();
    let groups = current
        .as_object_mut()
        .unwrap()
        .remove("grouped_audio")
        .unwrap();
    assert!(
        groups
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["group"].is_null())
    );
    strip_flat_hierarchy(&mut current);
    current["version"] = 42.into();
    assert_eq!(current, old);
    assert_eq!(decode(&encoded).unwrap(), project);
    assert!(
        project.tracks().iter().flat_map(|t| t.clips()).any(|c| c
            .audio_events()
            .events
            .iter()
            .any(|e| e.fade.is_some()))
    );
}

#[test]
fn current_group_inventory_requires_each_clip_and_explicit_nullable_state() {
    let (mut e, ids) = fixture();
    for grouped in [false, true] {
        if grouped {
            e.execute(Command::GroupClips(ids.clone())).unwrap();
        }
        let value: Value = serde_json::from_slice(&encode(e.project()).unwrap()).unwrap();
        let mut missing = value.clone();
        missing["grouped_audio"].as_array_mut().unwrap().remove(0);
        assert!(decode(&bytes(&missing)).is_err());
        let mut absent = value.clone();
        absent["grouped_audio"][0]
            .as_object_mut()
            .unwrap()
            .remove("group");
        assert!(decode(&bytes(&absent)).is_err());
        let mut duplicate = value.clone();
        let row = duplicate["grouped_audio"][0].clone();
        duplicate["grouped_audio"].as_array_mut().unwrap().push(row);
        assert!(decode(&bytes(&duplicate)).is_err());
    }
}

#[test]
fn time_group_midi_empty_margins_and_note_timing_survive_current_codec() {
    use hex_project::{BeatTime, TimeRange};
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "notes".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = e
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    e.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })
    .unwrap();
    let end = e
        .project()
        .tempo_map()
        .beat_to_frame(BeatTime::from_quarters(3).unwrap())
        .unwrap();
    let Created::Clip(group) = e
        .execute(Command::GroupTimeRange(
            TimeRange::new(vec![track], 0, end).unwrap(),
        ))
        .unwrap()
    else {
        panic!("group")
    };
    let reopened = decode(&encode(e.project()).unwrap()).unwrap();
    assert_eq!(reopened, *e.project());
    let clip = reopened.midi_clip(group).unwrap();
    assert_eq!(
        (clip.start(), clip.length()),
        (BeatTime::ZERO, BeatTime::from_quarters(3).unwrap())
    );
    assert_eq!(clip.notes()[0].start(), BeatTime::ONE);
}

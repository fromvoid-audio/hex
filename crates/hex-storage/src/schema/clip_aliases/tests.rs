//! Complete source/member persistence and predecessor-format admission.
use super::*;
use crate::{decode, encode};
use hex_project::{AudioEventEdit, BeatTime, ClipId, Command, Created, Editor, TrackId};
use serde_json::json;
fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn independent() -> (Editor, TrackId, ClipId, ClipId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
    let Created::Imported {
        track, clip: audio, ..
    } = e
        .execute(Command::ImportAudioTrack {
            name: "original audio".into(),
            path: "Media/Missing.WAV".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    e.execute(Command::EditAudioEvents {
        clip: audio,
        operation: AudioEventEdit::Split { at: 24_000 },
    })
    .unwrap();
    e.execute(Command::SetAudioClipPitch {
        clip: audio,
        cents: 100,
    })
    .unwrap();
    let Created::Track(midi_track) = e
        .execute(Command::AddTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("midi track")
    };
    let Created::Clip(midi) = e
        .execute(Command::PlaceMidiClip {
            track: midi_track,
            start: beat(0),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    e.execute(Command::AddMidiNote {
        clip: midi,
        start: beat(1),
        duration: beat(1),
        pitch: 60,
        channel: 0,
        velocity: 100,
    })
    .unwrap();
    (e, track, audio, midi)
}
fn alias(e: &mut Editor, clip: ClipId) -> ClipId {
    let Created::Clips { first, last } =
        e.execute(Command::DuplicateAliasClips(vec![clip])).unwrap()
    else {
        panic!("alias")
    };
    assert_eq!(first, last);
    first
}
fn fixture() -> (Editor, ClipId, ClipId, ClipId, ClipId) {
    let (mut e, track, audio, midi) = independent();
    let audio_alias = alias(&mut e, audio);
    let midi_alias = alias(&mut e, midi);
    e.execute(Command::SetClipFades {
        clip: audio_alias,
        fade_in: 120,
        fade_out: 240,
    })
    .unwrap();
    e.execute(Command::TrimClipEdge {
        clip: audio_alias,
        edge: hex_project::TrimEdge::Right,
        frame: 168_000,
    })
    .unwrap();
    e.execute(Command::SetMidiClipSource {
        clip: midi_alias,
        source_length: beat(4),
        source_start: beat(1),
        loop_start: beat(1),
        loop_end: beat(3),
        looping: true,
        length: beat(3),
    })
    .unwrap();
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(console_midi) = e
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "console midi".into(),
            source_length: beat(4),
        })
        .unwrap()
    else {
        panic!("console midi")
    };
    e.execute(Command::AddMidiNote {
        clip: console_midi,
        start: beat(1),
        duration: beat(1),
        pitch: 64,
        channel: 1,
        velocity: 110,
    })
    .unwrap();
    e.execute(Command::DuplicateSessionCell {
        track,
        scene,
        alias: true,
    })
    .unwrap();
    let Created::Scene(audio_scene) = e
        .execute(Command::AddScene {
            name: "audio scene".into(),
        })
        .unwrap()
    else {
        panic!("audio scene")
    };
    // Console one-shot capability stays independent of Arrangement event roots.
    let asset = e.project().clip(audio).unwrap().asset();
    e.execute(Command::CreateSessionAudioClip {
        track,
        scene: audio_scene,
        name: "console audio".into(),
        asset,
        source_start: 100,
        source_end: 24_000,
    })
    .unwrap();
    e.execute(Command::DuplicateSessionCell {
        track,
        scene: audio_scene,
        alias: true,
    })
    .unwrap();
    (e, audio, audio_alias, midi, midi_alias)
}
fn value(project: &Project) -> Value {
    serde_json::from_slice(&encode(project).unwrap()).unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

#[test]
fn complete_arrangement_console_aliases_roundtrip_with_local_windows_and_history() {
    let (mut e, audio, audio_alias, midi, midi_alias) = fixture();
    let encoded = encode(e.project()).unwrap();
    let doc: Value = serde_json::from_slice(&encoded).unwrap();
    // The pitched source renders with the v2 warp resampler.
    assert_eq!(doc["version"], crate::WARP_RESAMPLER_SCHEMA_VERSION);
    assert_eq!(doc["alias_sources"].as_array().unwrap().len(), 4);
    assert_eq!(doc["alias_members"].as_array().unwrap().len(), 8);
    let loaded = decode(&encoded).unwrap();
    assert_eq!(loaded, *e.project());
    assert_eq!(
        loaded.midi_source(midi).unwrap().notes(),
        loaded.midi_source(midi_alias).unwrap().notes()
    );
    assert!(!loaded.midi_clip(midi).unwrap().looping());
    assert!(loaded.midi_clip(midi_alias).unwrap().looping());
    assert_ne!(
        loaded.clip(audio).unwrap().retained_fade(),
        loaded.clip(audio_alias).unwrap().retained_fade()
    );
    assert!(Arc::ptr_eq(
        &loaded
            .clip(audio)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .content,
        &loaded
            .clip(audio_alias)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .content
    ));
    assert!(crate::encode_schema20(&loaded).is_err());
    assert!(crate::encode_schema22(&loaded).is_err());
    let source = e.project().clone();
    e.execute(Command::MakeClipsUnique(vec![audio_alias, midi_alias]))
        .unwrap();
    assert_eq!(decode(&encode(e.project()).unwrap()).unwrap(), *e.project());
    assert!(e.undo());
    let mut expected = source.snapshot();
    expected.next_id = e.project().snapshot().next_id;
    assert_eq!(
        decode(&encode(e.project()).unwrap()).unwrap(),
        Project::from_snapshot(expected).unwrap()
    );
}

#[test]
fn shared_events_and_notes_are_saved_once_and_member_fields_are_explicit_markers() {
    let (mut e, audio, mut next_audio, midi, mut next_midi) = fixture();
    let first = value(e.project());
    let audio_source = e.project().clip_alias(audio).unwrap().value();
    let midi_source = e.project().clip_alias(midi).unwrap().value();
    for _ in 0..8 {
        next_audio = alias(&mut e, next_audio);
        next_midi = alias(&mut e, next_midi);
    }
    let many = value(e.project());
    let source = |doc: &Value, id| {
        doc["alias_sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(source(&first, audio_source), source(&many, audio_source));
    assert_eq!(source(&first, midi_source), source(&many, midi_source));
    let aliases = e.project().snapshot().aliases;
    let index = CodecIndex::new(&many).unwrap();
    for member in aliases.members {
        match index.location(member.clip).unwrap() {
            Location::Audio(..) => {
                for key in SHARED_DOMAIN_KEYS {
                    assert!(domain(&many, &index, member.clip).unwrap()[key].is_null());
                }
                assert!(
                    row(&many, &index, "audio_event_state", member.clip).unwrap()["events"]
                        .is_null()
                );
                assert!(
                    row(&many, &index, "grouped_audio", member.clip).unwrap()["group"].is_null()
                );
            }
            Location::Midi(..) | Location::ConsoleMidi(..) => {
                let mut copied = many.clone();
                let wrapper =
                    wrapper_mut(&mut copied, index.location(member.clip).unwrap()).unwrap();
                assert!(wrapper["notes"].is_null());
                assert!(wrapper["source_length_raw"].is_null());
            }
            Location::ConsoleAudio(..) => {}
        }
    }
    assert_eq!(decode(&bytes(&many)).unwrap(), *e.project());
}

#[test]
fn alias_source_member_inventories_and_inline_content_are_strict_and_atomic() {
    let (e, _, _, _, _) = fixture();
    let original = value(e.project());
    let mut cases = Vec::new();
    for key in ["alias_sources", "alias_members"] {
        let mut v = original.clone();
        v.as_object_mut().unwrap().remove(key);
        cases.push(v);
    }
    let mut v = original.clone();
    v["alias_sources"][0]["unknown"] = true.into();
    cases.push(v);
    let mut v = original.clone();
    v["alias_sources"][0]["content"]["unknown"] = true.into();
    cases.push(v);
    let mut v = original.clone();
    v["alias_sources"][0]["content"]["kind"] = "future".into();
    cases.push(v);
    let mut v = original.clone();
    let duplicate = v["alias_sources"][0].clone();
    v["alias_sources"].as_array_mut().unwrap().push(duplicate);
    cases.push(v);
    let mut v = original.clone();
    let duplicate = v["alias_members"][0].clone();
    v["alias_members"].as_array_mut().unwrap().push(duplicate);
    cases.push(v);
    let mut v = original.clone();
    v["alias_members"][0]["source"] = u64::MAX.into();
    cases.push(v);
    let mut v = original.clone();
    v["alias_members"][0]["clip"] = u64::MAX.into();
    cases.push(v);
    let mut v = original.clone();
    v["alias_members"].as_array_mut().unwrap().pop();
    cases.push(v);
    let orphan_id = original["alias_sources"][0]["id"].clone();
    let mut v = original.clone();
    v["alias_members"]
        .as_array_mut()
        .unwrap()
        .retain(|m| m["source"] != orphan_id);
    cases.push(v);
    let mut v = original.clone();
    v["alias_sources"][0]["id"] = v["tracks"][0]["id"].clone();
    let wrong = v["alias_sources"][0]["id"].clone();
    for m in v["alias_members"].as_array_mut().unwrap() {
        if m["source"] == orphan_id {
            m["source"] = wrong.clone();
        }
    }
    cases.push(v);
    let midi = original["alias_sources"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| s["content"]["kind"] == "midi")
        .unwrap();
    let mut v = original.clone();
    v["alias_sources"][midi]["content"]["notes"][0]["velocity"] = 0.into();
    cases.push(v);
    let mut v = original.clone();
    v["alias_sources"][midi]["content"]["notes"][0]["id"] = v["tracks"][0]["id"].clone();
    cases.push(v);
    let mut v = original.clone();
    v["alias_sources"][midi]["content"]["notes"][0]["extra"] = true.into();
    cases.push(v);
    let index = CodecIndex::new(&original).unwrap();
    let audio = original["alias_members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["clip"].as_u64().unwrap())
        .find(|id| matches!(index.location(*id), Ok(Location::Audio(..))))
        .unwrap();
    let mut v = original.clone();
    domain_mut(&mut v, &index, audio).unwrap()["pitch_cents"] = 100.into();
    cases.push(v);
    let mut v = original.clone();
    domain_mut(&mut v, &index, audio)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("warp");
    cases.push(v);
    let mut v = original.clone();
    row_mut(&mut v, &index, "audio_event_state", audio).unwrap()["events"] = json!([]);
    cases.push(v);
    for (i, malformed) in cases.into_iter().enumerate() {
        let before = bytes(&malformed);
        assert!(decode(&before).is_err(), "malformed alias case {i}");
        assert_eq!(
            before,
            bytes(&malformed),
            "decode never mutates source bytes"
        );
    }
    let encoded = String::from_utf8(bytes(&original)).unwrap();
    let duplicate_key = encoded.replacen(
        "\"alias_sources\":",
        "\"alias_sources\":[],\"alias_sources\":",
        1,
    );
    assert!(decode(duplicate_key.as_bytes()).is_err());
}

#[test]
fn strict_schema51_writer_migrates_complete_state_and_original_bytes_are_unchanged() {
    let (e, _, _, _) = independent();
    // A schema51-era project rendered its pitched clip with the v1 law.
    let project = super::super::warp_resampler::with_implied_laws(e.project());
    let prior = super::super::crossfades::encode(&project).unwrap();
    assert_eq!(prior["version"], 51);
    let original = bytes(&prior);
    let migrated = decode(&original).unwrap();
    assert!(!migrated.has_clip_aliases());
    assert_eq!(migrated, project);
    let mut current = value(&migrated);
    assert_eq!(
        current.as_object_mut().unwrap().remove("alias_sources"),
        Some(json!([]))
    );
    assert_eq!(
        current.as_object_mut().unwrap().remove("alias_members"),
        Some(json!([]))
    );
    assert_eq!(
        current.as_object_mut().unwrap().remove("host_automation"),
        Some(json!([]))
    );
    assert_eq!(
        current
            .as_object_mut()
            .unwrap()
            .remove("inactive_midi_notes"),
        Some(json!([]))
    );
    assert_eq!(
        current
            .as_object_mut()
            .unwrap()
            .remove("inactive_audio_events"),
        Some(json!([]))
    );
    assert_eq!(
        current.as_object_mut().unwrap().remove("nested_audio"),
        Some(json!([]))
    );
    assert_eq!(
        current.as_object_mut().unwrap().remove("audio_event_gains"),
        Some(json!([]))
    );
    assert_eq!(
        current.as_object_mut().unwrap().remove("midi_clip_gains"),
        Some(json!([]))
    );
    assert_eq!(
        current
            .as_object_mut()
            .unwrap()
            .remove("automatic_track_names"),
        Some(json!([]))
    );
    current["version"] = 51.into();
    assert_eq!(current, prior, "every predecessor field survives");
    assert_eq!(bytes(&prior), original);
}

#[test]
fn aliases_coexist_with_native_sources_automation_crossfades_and_track_groups() {
    let (mut e, _, _, _, _) = fixture();
    let Created::Track(instrument) = e
        .execute(Command::InsertTrackWithDevice {
            name: "native source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument")
    };
    let device = e
        .project()
        .track(instrument)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    e.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter: hex_lattice_contract::ParameterId::from_static(7),
        points: vec![hex_project::AutomationPoint {
            beat: BeatTime::ZERO,
            value: hex_project::AutomationValue::new(0.25).unwrap(),
            outgoing: hex_project::AutomationInterpolation::Linear,
        }],
    })
    .unwrap();
    let Created::Imported { track, clip, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "crossfade".into(),
            path: "missing-crossfade.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    e.execute(Command::SetClipLooping {
        clip,
        looping: true,
    })
    .unwrap();
    e.execute(Command::SetClipSourcePhase { clip, phase: 16 })
        .unwrap();
    e.execute(Command::CopyClipTo {
        clip,
        track,
        start: 96_000,
    })
    .unwrap();
    e.execute(Command::CreateArrangementFades {
        range: hex_project::TimeRange::new(vec![track], 92_000, 100_000).unwrap(),
    })
    .unwrap();
    e.execute(Command::GroupTracks {
        tracks: hex_project::TrackSelectionIds::new(&[track, instrument]).unwrap(),
        name: "group".into(),
    })
    .unwrap();
    assert!(!e.project().snapshot().native_sources.is_empty());
    assert!(e.project().has_group_tracks());
    assert!(e.project().has_clip_aliases());
    let encoded = encode(e.project()).unwrap();
    assert_eq!(decode(&encoded).unwrap(), *e.project());
    assert!(e.undo());
    assert_eq!(decode(&encode(e.project()).unwrap()).unwrap(), *e.project());
    assert!(e.redo());
    assert_eq!(encode(e.project()).unwrap(), encoded);
}

#[test]
fn grouped_audio_aliases_retain_child_roots_once_and_preserve_hierarchy() {
    let (mut e, track, audio, midi) = independent();
    let asset = e.project().clip(audio).unwrap().asset();
    let Created::Clip(second) = e
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 240_000,
            duration: 48_000,
            source_offset: 24_000,
            source_length: 48_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("second")
    };
    let Created::Clip(grouped) = e.execute(Command::GroupClips(vec![audio, second])).unwrap()
    else {
        panic!("grouped")
    };
    let copy = alias(&mut e, grouped);
    e.execute(Command::SetClipFades {
        clip: copy,
        fade_in: 12_000,
        fade_out: 24_000,
    })
    .unwrap();
    let midi_track = e
        .project()
        .tracks()
        .iter()
        .find(|t| t.midi_clips().iter().any(|c| c.id() == midi))
        .unwrap()
        .id();
    e.execute(Command::GroupTracks {
        tracks: hex_project::TrackSelectionIds::new(&[track, midi_track]).unwrap(),
        name: "nested submix".into(),
    })
    .unwrap();
    let encoded = encode(e.project()).unwrap();
    let loaded = decode(&encoded).unwrap();
    assert_eq!(loaded, *e.project());
    assert!(loaded.has_group_tracks());
    let source = loaded.clip(grouped).unwrap().audio_edit().unwrap();
    let copied = loaded.clip(copy).unwrap().audio_edit().unwrap();
    assert!(Arc::ptr_eq(&source.domain.content, &copied.domain.content));
    assert_eq!(source.domain.events.as_ref().unwrap().sources.len(), 2);
    assert_ne!(source.domain.fade, copied.domain.fade);
    let doc: Value = serde_json::from_slice(&encoded).unwrap();
    let sources = doc["alias_sources"].as_array().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(
        sources[0]["content"]["payload"]["group"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

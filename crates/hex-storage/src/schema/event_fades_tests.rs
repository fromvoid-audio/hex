use super::*;
use hex_project::{AudioEventEdit, BeatTime, ClipId, Command, Created, Editor};
fn fixture() -> (Project, ClipId) {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, track, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "original".into(),
            path: "Original.wav".into(),
            sample_rate: 8_000,
            frames: 128,
        })
        .unwrap()
    else {
        panic!("fixture");
    };
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 64 },
        })
        .unwrap();
    for (event, start, span) in [(1, 0, 128), (2, 64, 64)] {
        let shape = ClipFade::new(span, 0, 16, 24)
            .unwrap()
            .with_shape(2, 4, -300, 400)
            .unwrap()
            .with_curve_positions(250, 700)
            .unwrap();
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::SetFade {
                    event,
                    fade: Some(AudioEventFade::new(start, shape).unwrap()),
                },
            })
            .unwrap();
    }
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    else {
        panic!("fixture");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 2,
            velocity: 91,
        })
        .unwrap();
    editor
        .execute(Command::ReverseClips(vec![clip, midi]))
        .unwrap();
    (editor.project().clone(), clip)
}
#[test]
fn schema42_roundtrips_complete_fades_and_both_directed_clip_families() {
    let (project, clip) = fixture();
    let bytes = crate::encode_document(&super::encode(&project).unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        42
    );
    let restored = super::decode(&bytes).unwrap();
    assert_eq!(restored.snapshot(), project.snapshot());
    assert_eq!(
        crate::encode_document(&super::encode(&restored).unwrap()).unwrap(),
        bytes
    );
    let fades: Vec<_> = restored
        .clip(clip)
        .unwrap()
        .audio_events()
        .events
        .iter()
        .map(|e| e.fade)
        .collect();
    assert!(fades.iter().all(Option::is_some));
    assert_eq!(fades[0].unwrap().shape().span(), 128);
    assert_eq!(fades[1].unwrap().source_start(), 64);
    assert!(
        super::super::content_views::encode(&project).is_err(),
        "older writer must never drop event envelopes"
    );
    assert!(crate::encode_schema20(&project).is_err());
    assert!(crate::encode_schema22(&project).is_err());
}
#[test]
fn schema41_migration_acquires_absent_fades_and_retains_the_entire_predecessor() {
    let (project, _) = fixture();
    let mut snapshot = project.snapshot();
    visit(&mut snapshot, |clip| {
        if let Some(edit) = clip.edit.as_mut()
            && let Some(events) = Arc::make_mut(&mut edit.domain).events.as_mut()
        {
            for event in &mut Arc::make_mut(events).events {
                event.fade = None;
            }
        }
        Ok(())
    })
    .unwrap();
    let original = Project::from_legacy_snapshot(snapshot).unwrap();
    // The actual published schema41 codec, never a manually invented layout.
    let genuine41 =
        serde_json::to_vec(&super::super::content_views::encode(&original).unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&genuine41).unwrap()["version"],
        41
    );
    let migrated = crate::decode(&genuine41).unwrap();
    assert_eq!(migrated.snapshot(), original.snapshot());
    let current = crate::encode(&migrated).unwrap();
    assert_eq!(
        crate::decode(&current).unwrap().snapshot(),
        original.snapshot()
    );
    let value: serde_json::Value = serde_json::from_slice(&current).unwrap();
    assert_eq!(
        value["audio_event_fades"][0]["events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        value["audio_event_fades"][0]["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["fade"].is_null())
    );
}
#[test]
fn schema42_requires_complete_unique_known_inventories_and_valid_all_knots() {
    let (project, _) = fixture();
    let bytes = crate::encode(&project).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for case in 0..15 {
        let mut bad = original.clone();
        match case {
            0 => {
                bad.as_object_mut().unwrap().remove("audio_event_fades");
            }
            1 => {
                bad["audio_event_fades"].as_array_mut().unwrap().clear();
            }
            2 => {
                let row = bad["audio_event_fades"][0].clone();
                bad["audio_event_fades"].as_array_mut().unwrap().push(row);
            }
            3 => bad["audio_event_fades"][0]["clip"] = u64::MAX.into(),
            4 => {
                bad["audio_event_fades"][0]["events"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            5 => {
                let row = bad["audio_event_fades"][0]["events"][0].clone();
                bad["audio_event_fades"][0]["events"][1] = row;
            }
            6 => bad["audio_event_fades"][0]["events"][0]["event"] = u64::MAX.into(),
            7 => {
                bad["audio_event_fades"][0]["events"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("fade");
            }
            8 => bad["audio_event_fades"][0]["events"][0]["fade"]["span"] = 0.into(),
            9 => bad["audio_event_fades"][0]["events"][0]["fade"]["source_start"] = u64::MAX.into(),
            10 => bad["audio_event_fades"][0]["events"][0]["fade"]["fade_in"] = u64::MAX.into(),
            11 => bad["audio_event_fades"][0]["events"][0]["fade"]["in_curve"] = 1001.into(),
            12 => bad["audio_event_fades"][0]["events"][0]["fade"]["in_position"] = 0.into(),
            13 => bad["audio_event_fades"][0]["events"][0]["fade"]["offset"] = 1.into(),
            _ => {
                let row = bad["audio_event_fades"][0]["events"][0].clone();
                bad["audio_event_fades"][0]["events"] =
                    serde_json::Value::Array(vec![row; hex_project::MAX_AUDIO_EVENTS + 1]);
            }
        }
        assert!(
            crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "case {case}"
        );
    }
    let text = String::from_utf8(bytes.clone()).unwrap();
    let duplicate = text.replacen("\"fade_in\":16", "\"fade_in\":16,\"fade_in\":16", 1);
    assert_ne!(duplicate, text);
    assert!(crate::decode(duplicate.as_bytes()).is_err());
    assert_eq!(
        crate::decode(&bytes).unwrap().snapshot(),
        project.snapshot()
    );
}

#[test]
fn frozen_published_schema41_projects_migrate_without_losing_predecessor_state() {
    for bytes in [
        include_bytes!("../../tests/fixtures/event-fades/schema41-audio-600ea714.json").as_slice(),
        include_bytes!("../../tests/fixtures/event-fades/schema41-midi-600ea714.json").as_slice(),
        include_bytes!("../../tests/fixtures/event-fades/schema41-session-audio-600ea714.json")
            .as_slice(),
    ] {
        let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(value["version"], 41);
        let predecessor = super::super::content_views::decode(bytes).unwrap();
        let migrated = crate::decode(bytes).unwrap();
        assert_eq!(migrated.snapshot(), predecessor.snapshot());
        assert!(
            migrated
                .tracks()
                .iter()
                .flat_map(|track| track.clips())
                .all(|clip| clip
                    .audio_events()
                    .events
                    .iter()
                    .all(|event| event.fade.is_none()))
        );
        let current = crate::encode(&migrated).unwrap();
        assert_eq!(
            crate::decode(&current).unwrap().snapshot(),
            predecessor.snapshot()
        );
    }
}

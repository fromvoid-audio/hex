use hex_project::{AudioEventEdit, Command, Created, Editor, Project};
#[test]
fn events_roundtrip_and_strict_complete_overlay_rejects_loss_and_bad_references() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48000,
            frames: 100,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 30 },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Move {
                event: 1,
                start: 50,
            },
        })
        .unwrap();
    editor
        .execute(Command::RenameClip {
            clip,
            name: "edited audio".into(),
        })
        .unwrap();
    editor
        .execute(Command::SetTrackColor {
            track: editor.project().tracks()[0].id(),
            color: hex_project::ElementColor::Cyan,
        })
        .unwrap();
    editor
        .execute(Command::SetClipFlavor {
            clip,
            flavor: hex_project::ClipFlavor::Accent,
        })
        .unwrap();
    let bytes = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&bytes).unwrap(), *editor.project());
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    for mutate in 0..4 {
        let mut bad = value.clone();
        match mutate {
            0 => {
                bad["audio_event_state"] = serde_json::json!([]);
            }
            1 => {
                bad["audio_event_state"][0]["clip"] = 999999.into();
            }
            2 => {
                bad["audio_event_state"][0]["events"][0]["source_start"] = u64::MAX.into();
            }
            _ => {
                bad["audio_event_state"][0]["unexpected"] = true.into();
            }
        }
        assert!(hex_storage::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let original = String::from_utf8(bytes).unwrap();
    let duplicate = format!("{{\"audio_event_state\":[],{}", &original[1..]);
    assert!(hex_storage::decode(duplicate.as_bytes()).is_err());
}

/// Dense per-clip event inventories survive encode/decode exactly. Documents
/// above the former 8 MiB reader limit are covered once, through save, replace,
/// recovery and journal, by `tests/provenance.rs`.
#[test]
fn dense_event_serialization_roundtrips_without_loss() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48000,
            frames: 2048,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    let mut root = editor.project().clip(clip).unwrap().audio_event_root();
    std::sync::Arc::make_mut(&mut root.domain).events =
        Some(std::sync::Arc::new(hex_project::AudioEvents {
            sources: Vec::new(),
            next_id: 1025,
            events: (0..1024)
                .map(|i| hex_project::AudioEvent {
                    active: true,
                    gain: hex_project::ClipGain::UNITY,
                    source: 0,
                    id: i + 1,
                    start: i * 2,
                    end: i * 2 + 1,
                    source_start: i * 2,
                    fade: None,
                })
                .collect(),
        }));
    let mut snapshot = editor.project().snapshot();
    let hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } =
        &mut snapshot.tracks[0].content
    else {
        panic!("hybrid")
    };
    let mut template = audio_clips[0].clone();
    template.edit = Some(root);
    *audio_clips = (0..16)
        .map(|i| {
            let mut clip = template.clone();
            clip.id = 1000 + i;
            clip.start = i * 2048;
            clip
        })
        .collect();
    snapshot.next_id = 2000;
    let project = Project::from_snapshot(snapshot).unwrap();
    let bytes = hex_storage::encode(&project).unwrap();
    assert_eq!(hex_storage::decode(&bytes).unwrap(), project);
}

#[test]
fn edited_events_warp_and_frozen_frame_placement_roundtrip_without_losing_source() {
    use hex_project::{Tempo, WarpMode, default_audio_warp};
    for (rate, bpm) in [
        (44_100, 123_456_789),
        (48_000, 97_333_331),
        (96_000, 177_777_779),
    ] {
        let mut editor = Editor::new(Project::new(rate).unwrap(), 32).unwrap();
        editor
            .execute(Command::SetProjectTempo {
                tempo: Tempo::new(bpm).unwrap(),
            })
            .unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "edited".into(),
                path: "original.wav".into(),
                sample_rate: rate,
                frames: 997,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 13,
                fade_out: 17,
            })
            .unwrap();
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 137 },
            })
            .unwrap();
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Move { event: 2, start: 0 },
            })
            .unwrap();
        let track = editor.project().tracks()[0].id();
        editor
            .execute(Command::MoveClip {
                clip,
                track,
                start: 4003,
            })
            .unwrap();
        let before = editor.project().clip(clip).unwrap().clone();
        let settings =
            default_audio_warp(&before, editor.project().tempo_map(), WarpMode::RePitch).unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            })
            .unwrap();
        let warped = editor.project().clip(clip).unwrap();
        let events = warped.audio_events();
        assert_eq!(events.next_id, before.audio_events().next_id);
        let edit = warped.audio_edit().unwrap();
        for (event, old) in events.events.iter().zip(before.audio_events().events) {
            assert_eq!(event.id, old.id);
            assert_eq!(
                edit.root_frame_at(event.source_start, editor.project().tempo_map())
                    .unwrap(),
                old.source_start
            );
        }
        let bytes = hex_storage::encode(editor.project()).unwrap();
        assert_eq!(hex_storage::decode(&bytes).unwrap(), *editor.project());
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: None,
            })
            .unwrap();
        let musical = editor.project().clip(clip).unwrap().clone();
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: false,
            })
            .unwrap();
        let frozen = editor.project().clip(clip).unwrap().clone();
        assert!(frozen.audio_edit().unwrap().live_musical_start().is_none());
        assert_eq!(frozen.audio_events(), events);
        assert!(editor.undo());
        assert_eq!(editor.project().clip(clip).unwrap(), &musical);
        assert!(editor.redo());
        assert_eq!(editor.project().clip(clip).unwrap(), &frozen);
        editor
            .execute(Command::SetProjectTempo {
                tempo: Tempo::new(180_000_000).unwrap(),
            })
            .unwrap();
        let bytes = hex_storage::encode(editor.project()).unwrap();
        let reopened = hex_storage::decode(&bytes).unwrap();
        assert_eq!(reopened, *editor.project());
        assert_eq!(reopened.clip(clip).unwrap().audio_events(), events);
        assert_eq!(reopened.clip(clip).unwrap().duration(), before.duration());
    }
}

#[test]
fn non_affine_event_clock_refusal_is_atomic_and_global_tempo_recovers_without_undoing_edits() {
    use hex_project::{BeatTime, Tempo, WarpMode, default_audio_warp};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 997,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 137 },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Move { event: 2, start: 0 },
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 100),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let before = editor.project().clone();
    assert!(
        default_audio_warp(
            before.clip(clip).unwrap(),
            before.tempo_map(),
            WarpMode::RePitch
        )
        .unwrap_err()
        .to_string()
        .contains("unequal tempo segments")
    );
    assert_eq!(editor.project(), &before);
    let events = before.clip(clip).unwrap().audio_events();
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(120_000_000).unwrap(),
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    assert_eq!(
        editor.project().clip(clip).unwrap().audio_events().next_id,
        events.next_id
    );
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(editor.project()).unwrap()).unwrap(),
        *editor.project()
    );
}

#[test]
fn freely_placed_events_roundtrip_with_and_without_a_source_loop() {
    for (looping, directed) in [(false, false), (true, false), (false, true)] {
        roundtrip_placed_events(looping, directed);
    }
}

fn roundtrip_placed_events(looping: bool, directed: bool) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48000,
            frames: 100,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    if looping {
        editor
            .execute(Command::SetClipLooping { clip, looping })
            .unwrap();
        editor
            .execute(Command::SetClipSourcePhase { clip, phase: 65 })
            .unwrap();
    }
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 40 },
        })
        .unwrap();
    if directed {
        editor
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: hex_project::AudioViewResizeOperation::CycleRange {
                    start: 0,
                    end: 80,
                    looping: true,
                },
            })
            .unwrap();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    }
    for operation in [
        AudioEventEdit::Place {
            event: 2,
            start: 130,
        },
        AudioEventEdit::Place {
            event: 1,
            start: -25,
        },
    ] {
        editor
            .execute(Command::EditAudioEvents { clip, operation })
            .unwrap();
    }
    let project = editor.project();
    let placed = project.clip(clip).unwrap();
    assert_eq!(
        (placed.looping(), placed.source_phase()),
        (looping, if looping { 65 } else { 0 })
    );
    let edit = placed.audio_edit().unwrap();
    assert_eq!(edit.view_start, 25);
    assert_eq!(edit.domain.clock.length(), if directed { 195 } else { 215 });
    let saved = hex_storage::encode(project).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(value["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(
        hex_storage::decode(&saved).unwrap().snapshot(),
        project.snapshot()
    );
}

#[test]
fn musical_event_source_schema_preserves_cropped_and_nested_event_spans() {
    use hex_project::{BeatTime, TrimEdge, WarpMode};
    let old = hex_storage::decode(include_bytes!("fixtures/warp-resampler/schema64.json")).unwrap();
    assert!(
        old.tracks()
            .iter()
            .flat_map(|track| track.clips())
            .all(|clip| {
                clip.audio_event_root()
                    .domain
                    .musical_event_source
                    .is_none()
            })
    );
    assert!(
        hex_storage::schema_version_for(&old) < hex_storage::MUSICAL_EVENT_SOURCE_SCHEMA_VERSION
    );

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, track, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 24_000,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 24_000,
        })
        .unwrap();
    let settings = hex_project::default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 27_000,
        })
        .unwrap();
    let before = editor.project().clone();
    let original = before.clip(clip).unwrap();
    let root = original.audio_event_root();
    let event = original
        .editable_audio_events()
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.start == root.view_start)
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Place {
                event: event.id,
                start: -(BeatTime::ONE.raw() as i64),
            },
        })
        .unwrap();
    let left = editor.project().clone();
    let candidate = left.clip(clip).unwrap();
    assert_eq!(
        (candidate.start(), candidate.duration()),
        (original.start(), original.duration())
    );
    assert_eq!(
        candidate
            .audio_event_root()
            .domain
            .musical_event_source
            .unwrap()
            .zero_in_destination,
        BeatTime::ONE.raw()
    );
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Place {
                event: event.id,
                start: (3 * BeatTime::ONE.raw()) as i64,
            },
        })
        .unwrap();
    let both = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project(), &left);
    assert!(editor.redo());
    assert_eq!(editor.project(), &both);

    for nested in [false, true] {
        if nested {
            let Created::Clip(copy) = editor.execute(Command::DuplicateClip(clip)).unwrap() else {
                panic!("duplicate")
            };
            editor
                .execute(Command::GroupClips(vec![clip, copy]))
                .unwrap();
        }
        let bytes = hex_storage::encode(editor.project()).unwrap();
        assert_eq!(
            hex_storage::schema_version_for(editor.project()),
            hex_storage::MUSICAL_EVENT_SOURCE_SCHEMA_VERSION
        );
        assert_eq!(hex_storage::decode(&bytes).unwrap(), *editor.project());
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for mutation in 0..4 {
            let mut malformed = value.clone();
            match mutation {
                0 => malformed["version"] = hex_storage::SIDECHAIN_SCHEMA_VERSION.into(),
                1 => malformed["musical_event_sources"][0]["length_raw"] = u64::MAX.into(),
                2 => malformed["musical_event_sources"][0]["path"][0] = u64::MAX.into(),
                _ => {
                    let row = malformed["musical_event_sources"][0].clone();
                    malformed["musical_event_sources"]
                        .as_array_mut()
                        .unwrap()
                        .push(row);
                }
            }
            assert!(hex_storage::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
        }
    }
}

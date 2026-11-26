use super::*;
use hex_lattice_contract::{
    DeviceAutomationTarget, DeviceTypeId, MAX_VST3_DEVICE_STATE_BYTES, NativeDeviceState,
};
use hex_project::{
    AudioEventEdit, AudioViewResizeOperation, AutomationInterpolation, AutomationPoint,
    AutomationValue, BeatTime, ClipFade, Command, Created, Editor, FadeCurveLaw,
    MAX_AUDIO_DOMAIN_FRAMES, Project, TrackTag,
};
use serde_json::Value;
use std::time::Instant;

/// Build a rich project exercising 9 post-112 features simultaneously:
/// 1. s_curve_fades (schema 113)
/// 2. arrangement_locators (schema 114)
/// 3. coincident_automation (schema 115)
/// 4. long_reverse (schema 116)
/// 5. long_pitch (schema 117)
/// 6. silent_repeat (schema 118)
/// 7. outer_clip_fades (schema 122)
/// 8. scoped_musical_events (schema 123)
/// 9. event_warp_sources (schema 124)
/// 10. large_vst3_state (schema 125)
///
/// Plus post-63 features: track_tags, etc.
fn build_rich_multi_feature_project() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
    let beat = BeatTime::ONE.raw();

    // Feature 1: arrangement_locators (114)
    let Created::ArrangementLocator(loc) = editor
        .execute(Command::AddArrangementLocator { frame: 48_000 })
        .unwrap()
    else {
        panic!("expected locator");
    };
    editor
        .execute(Command::RenameArrangementLocator {
            locator: loc,
            name: Some("Chorus".into()),
        })
        .unwrap();
    editor
        .execute(Command::AddArrangementLocator { frame: 96_000 })
        .unwrap();

    // Track 1: scoped musical event (123) + outer S-curve fade (113 + 122)
    let Created::Imported {
        track: t1,
        clip: c_scoped,
        asset: short_asset,
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "vocals".into(),
            path: "vocals.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("expected imported clip");
    };
    editor
        .execute(Command::SetTrackTags {
            track: t1,
            tags: vec![TrackTag::new("lead").unwrap()],
        })
        .unwrap();
    for command in [
        Command::SetAudioClipMusical {
            clip: c_scoped,
            enabled: true,
        },
        Command::ResizeAudioClipView {
            clip: c_scoped,
            operation: AudioViewResizeOperation::ScopeRange {
                start: beat,
                end: 3 * beat,
                looping: true,
            },
        },
        Command::EditAudioEvents {
            clip: c_scoped,
            operation: AudioEventEdit::Place {
                event: 1,
                start: -(beat as i64),
            },
        },
    ] {
        editor.execute(command).unwrap();
    }
    let row = editor.project().clip(c_scoped).unwrap();
    let fade = ClipFade::new(row.duration(), 0, 12_000, 12_000)
        .unwrap()
        .with_curve_laws([FadeCurveLaw::SmoothStep, FadeCurveLaw::Linear]);
    editor
        .execute(Command::SetClipFadeEnvelope {
            clip: c_scoped,
            fade,
        })
        .unwrap();

    // Track 1b: event_warp_sources (124)
    let Created::Imported { clip: c_warp, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "guitar".into(),
            path: "guitar.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("expected warp clip");
    };
    let mut warp = editor
        .project()
        .audio_warp_settings_for_enable(c_warp)
        .unwrap();
    warp.insert_marker(24_000).unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip: c_warp,
            settings: Some(warp.clone()),
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip: c_warp,
            operation: AudioEventEdit::Split { at: 2 * beat },
        })
        .unwrap();
    warp.move_marker(24_000, BeatTime::from_raw(beat + beat / 4))
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip: c_warp,
            operation: AudioEventEdit::SetRetainedWarp {
                event: 1,
                settings: warp,
            },
        })
        .unwrap();

    // Feature: silent_repeat (118) on a second clip on track 1
    let Created::Clip(c_repeat) = editor
        .execute(Command::PlaceClip {
            track: t1,
            asset: short_asset,
            start: 600_000,
            duration: 24_000,
            source_offset: 0,
            source_length: 24_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("expected repeat clip");
    };
    editor
        .execute(Command::ResizeAudioClipView {
            clip: c_repeat,
            operation: AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 660_000,
            },
        })
        .unwrap();
    editor
        .execute(Command::ResizeAudioClipView {
            clip: c_repeat,
            operation: AudioViewResizeOperation::CycleEnd {
                project_frame: 630_000,
            },
        })
        .unwrap();

    // Feature: long_reverse (116) on track 2
    let long_frames = MAX_AUDIO_DOMAIN_FRAMES + 48_000;
    let Created::Imported {
        track: t2,
        clip: c_rev,
        asset: long_asset,
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "stem-rev".into(),
            path: "long.wav".into(),
            sample_rate: 48_000,
            frames: long_frames,
        })
        .unwrap()
    else {
        panic!("expected long clip");
    };
    editor
        .execute(Command::SetClipFades {
            clip: c_rev,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor.execute(Command::ReverseClips(vec![c_rev])).unwrap();

    // Feature: long_pitch (117) on track 3
    let Created::Track(t3) = editor
        .execute(Command::AddTrack {
            name: "stem-pitch".into(),
        })
        .unwrap()
    else {
        panic!("expected track");
    };
    let Created::Clip(c_pitch) = editor
        .execute(Command::PlaceClip {
            track: t3,
            asset: long_asset,
            start: 0,
            duration: long_frames,
            source_offset: 0,
            source_length: long_frames,
            looping: false,
        })
        .unwrap()
    else {
        panic!("expected long pitch clip");
    };
    editor
        .execute(Command::SetClipFades {
            clip: c_pitch,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch {
            clip: c_pitch,
            cents: 200,
        })
        .unwrap();

    // Feature: coincident_automation (115) on a device on track 2
    let Created::Device(dev) = editor
        .execute(Command::InsertDevice {
            track: t2,
            before: None,
            state: NativeDeviceState::new(
                DeviceTypeId::from_static("hex.utility"),
                1,
                vec![1, 2, 3],
            )
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("expected device");
    };
    let p_out = AutomationPoint {
        beat: BeatTime::ONE,
        value: AutomationValue::new(0.25).unwrap(),
        outgoing: AutomationInterpolation::Linear,
    };
    let p_in = AutomationPoint {
        beat: BeatTime::ONE,
        value: AutomationValue::new(0.85).unwrap(),
        outgoing: AutomationInterpolation::Step,
    };
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device: dev,
            target: DeviceAutomationTarget::HostInputGain,
            points: vec![p_in, p_out],
        })
        .unwrap();

    // Feature: large_vst3_state (125) on track 3 (just above 512 KiB threshold)
    let vst3_payload = vec![0x5Au8; MAX_VST3_DEVICE_STATE_BYTES + 4096];
    let vst3_state = NativeDeviceState::new(hex_plugin_state::TYPE_ID, 1, vst3_payload).unwrap();
    editor
        .execute(Command::InsertDevice {
            track: t3,
            before: None,
            state: vst3_state,
        })
        .unwrap();

    // Add realistic arrangement density (16 tracks x 32 notes = 512 MIDI notes)
    for idx in 0..16 {
        let Created::Track(t) = editor
            .execute(Command::AddTrack {
                name: format!("synth {idx}"),
            })
            .unwrap()
        else {
            panic!("expected synth track");
        };
        let Created::Clip(mc) = editor
            .execute(Command::PlaceMidiClip {
                track: t,
                start: BeatTime::from_raw(idx * 4 * beat),
                length: BeatTime::from_raw(16 * beat),
            })
            .unwrap()
        else {
            panic!("expected midi clip");
        };
        for step in 0..32 {
            editor
                .execute(Command::AddMidiNote {
                    clip: mc,
                    start: BeatTime::from_raw(step * (beat / 2)),
                    duration: BeatTime::from_raw(beat / 4),
                    pitch: 48 + ((step % 24) as u8),
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
    }

    let project = editor.project().clone();
    assert!(
        post_112_feature_count(&project) >= 6,
        "expected >= 6 post-112 features, got {} ({:?})",
        post_112_feature_count(&project),
        capabilities_for(&project)
    );
    project
}

#[test]
fn supported_capabilities_table_is_strictly_sorted_and_unique() {
    for pair in SUPPORTED_CAPABILITIES.windows(2) {
        assert!(
            pair[0] < pair[1],
            "SUPPORTED_CAPABILITIES out of order: {} >= {}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn unified_schema126_round_trips_rich_multi_feature_project_losslessly() {
    let project = build_rich_multi_feature_project();
    assert_eq!(
        crate::schema_version_for(&project),
        CAPABILITY_MANIFEST_SCHEMA_VERSION
    );

    let encoded = crate::encode(&project).unwrap();
    let json: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(json["format"], "hex-project");
    assert_eq!(json["version"], CAPABILITY_MANIFEST_SCHEMA_VERSION);
    let caps: Vec<&str> = json["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(caps, crate::capabilities_for(&project));
    assert!(caps.contains(&"arrangement_locators"));
    assert!(caps.contains(&"coincident_automation"));
    assert!(caps.contains(&"event_warp_sources"));
    assert!(caps.contains(&"large_vst3_state"));
    assert!(caps.contains(&"long_pitch"));
    assert!(caps.contains(&"long_reverse"));
    assert!(caps.contains(&"s_curve_fades"));
    assert!(caps.contains(&"silent_repeat"));

    let decoded = crate::decode(&encoded).unwrap();
    assert_eq!(decoded.snapshot(), project.snapshot());
    assert_eq!(decoded, project);
    assert_eq!(crate::encode(&decoded).unwrap(), encoded);
}

#[test]
fn explicit_encode_capability_manifest_round_trips_empty_and_simple_projects() {
    let empty = Project::new(48_000).unwrap();
    assert_eq!(
        crate::schema_version_for(&empty),
        crate::CURRENT_SCHEMA_VERSION
    );
    let encoded = crate::encode_capability_manifest(&empty).unwrap();
    let json: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(json["version"], CAPABILITY_MANIFEST_SCHEMA_VERSION);
    assert_eq!(json["capabilities"], serde_json::json!([]));
    let decoded = crate::decode(&encoded).unwrap();
    assert_eq!(decoded, empty);
}

#[test]
fn schema126_rejects_unknown_unsorted_duplicate_mismatched_capabilities_and_unknown_fields() {
    let project = build_rich_multi_feature_project();
    let encoded = crate::encode(&project).unwrap();
    let base: Value = serde_json::from_slice(&encoded).unwrap();

    // 1. Unknown future capability -> explicit newer-capability refusal
    let mut future_cap = base.clone();
    future_cap["capabilities"]
        .as_array_mut()
        .unwrap()
        .push(Value::String("zz_future_capability_v9".into()));
    let err = crate::decode(&serde_json::to_vec(&future_cap).unwrap()).unwrap_err();
    assert!(
        err.to_string().contains("newer hex capability"),
        "unexpected error: {err}"
    );

    // 2. Unsorted capabilities
    let mut unsorted = base.clone();
    unsorted["capabilities"].as_array_mut().unwrap().reverse();
    assert!(crate::decode(&serde_json::to_vec(&unsorted).unwrap()).is_err());

    // 3. Duplicate capability
    let mut dup = base.clone();
    let first = dup["capabilities"][0].clone();
    dup["capabilities"].as_array_mut().unwrap().insert(0, first);
    assert!(crate::decode(&serde_json::to_vec(&dup).unwrap()).is_err());

    // 4. Missing required capability (manifest disagrees with body)
    let mut missing = base.clone();
    missing["capabilities"].as_array_mut().unwrap().pop();
    assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());

    // 5. Extra unused supported capability (manifest disagrees with body)
    let mut extra = base.clone();
    let arr = extra["capabilities"].as_array_mut().unwrap();
    arr.push(Value::String("sinc_v2".into()));
    arr.sort_by(|a, b| a.as_str().unwrap().cmp(b.as_str().unwrap()));
    assert!(crate::decode(&serde_json::to_vec(&extra).unwrap()).is_err());

    // 6. Unknown top-level field
    let mut unknown_field = base;
    unknown_field["unexpected"] = Value::Bool(true);
    assert!(crate::decode(&serde_json::to_vec(&unknown_field).unwrap()).is_err());
}

#[test]
fn overwrite_consent_tracks_schema126_upgrade_and_preserves_original_bytes() {
    let dir = std::env::temp_dir().join(format!("hex-schema126-consent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Start with a historical schema 114 (single post-112 feature) file on disk
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    editor
        .execute(Command::AddArrangementLocator { frame: 48_000 })
        .unwrap();
    let old_project = editor.project().clone();
    // Default saves with any post-112 feature now target schema 126 directly.
    assert_eq!(
        crate::schema_version_for(&old_project),
        CAPABILITY_MANIFEST_SCHEMA_VERSION
    );

    let path = dir.join("song.hex");
    let legacy_bytes = crate::encode_stacked_legacy(&old_project).unwrap();
    std::fs::write(&path, &legacy_bytes).unwrap();
    let original_bytes = std::fs::read(&path).unwrap();
    let (loaded_old, old_token) = crate::load_with_token(&path).unwrap();
    assert_eq!(loaded_old, old_project);

    // Re-saving an existing schema 114 file as schema 126 requires explicit upgrade confirmation
    assert!(old_token.requires_schema_upgrade_for(&old_project));
    assert!(old_token.requires_save_confirmation(&old_project).unwrap());

    // Upgrading to schema 126 requires explicit confirmation and preserves original bytes
    let rich = build_rich_multi_feature_project();
    assert!(old_token.requires_schema_upgrade_for(&rich));
    assert!(old_token.requires_save_confirmation(&rich).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes);

    // Save As under a new file publishes schema 126 cleanly
    let v126_path = dir.join("song-v126.hex");
    crate::save_new(&v126_path, &rich).unwrap();
    let (reopened, v126_token) = crate::load_with_token(&v126_path).unwrap();
    assert_eq!(reopened, rich);
    // Re-saving schema 126 with same capabilities needs no confirmation
    assert!(!v126_token.requires_schema_upgrade_for(&reopened));
    assert!(!v126_token.requires_save_confirmation(&reopened).unwrap());

    // Adding a new capability (sample_rate_mode) to an existing schema 126 file requires confirmation
    let mut richer_snapshot = reopened.snapshot();
    richer_snapshot.sample_rate_mode = hex_project::SampleRateMode::FollowInterface;
    let richer = Project::from_snapshot(richer_snapshot).unwrap();
    assert!(crate::capabilities_for(&richer).contains(&"sample_rate_mode"));
    assert!(v126_token.requires_schema_upgrade_for(&richer));

    // Original file remains byte-for-byte untouched
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stacked_legacy_and_schema126_decode_to_identical_project() {
    let project = build_rich_multi_feature_project();
    let stacked_bytes = crate::encode_stacked_legacy(&project).unwrap();
    let unified_bytes = crate::encode(&project).unwrap();

    let stacked_json: Value = serde_json::from_slice(&stacked_bytes).unwrap();
    let unified_json: Value = serde_json::from_slice(&unified_bytes).unwrap();
    assert_eq!(
        stacked_json["version"],
        crate::LARGE_VST3_STATE_SCHEMA_VERSION
    );
    assert_eq!(unified_json["version"], CAPABILITY_MANIFEST_SCHEMA_VERSION);

    let from_stacked = crate::decode(&stacked_bytes).unwrap();
    let from_unified = crate::decode(&unified_bytes).unwrap();
    assert_eq!(from_stacked, project);
    assert_eq!(from_unified, project);
    assert_eq!(from_stacked, from_unified);
}

#[test]
fn single_post_112_feature_defaults_directly_to_schema126_and_matches_legacy() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    editor
        .execute(Command::AddArrangementLocator { frame: 48_000 })
        .unwrap();
    let project = editor.project().clone();
    assert_eq!(post_112_feature_count(&project), 1);
    assert_eq!(
        crate::schema_version_for(&project),
        CAPABILITY_MANIFEST_SCHEMA_VERSION
    );
    let unified = crate::encode(&project).unwrap();
    let json: Value = serde_json::from_slice(&unified).unwrap();
    assert_eq!(json["version"], CAPABILITY_MANIFEST_SCHEMA_VERSION);
    assert_eq!(
        json["capabilities"],
        serde_json::json!(["arrangement_locators"])
    );
    assert_eq!(crate::decode(&unified).unwrap(), project);

    let legacy = crate::encode_stacked_legacy(&project).unwrap();
    let legacy_json: Value = serde_json::from_slice(&legacy).unwrap();
    assert_eq!(
        legacy_json["version"],
        crate::ARRANGEMENT_LOCATOR_SCHEMA_VERSION
    );
    assert_eq!(crate::decode(&legacy).unwrap(), project);
}

#[test]
fn schema127_round_trips_clip_cc_pitch_bend_and_per_note_mpe_expression() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "mpe synth".into(),
        })
        .unwrap()
    else {
        panic!("expected midi track");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("expected midi clip");
    };
    let Created::Note(note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(2).unwrap(),
            pitch: 64,
            channel: 2,
            velocity: 110,
        })
        .unwrap()
    else {
        panic!("expected midi note");
    };
    editor
        .execute(Command::SetMidiExpressionLane {
            clip,
            kind: hex_project::MidiLaneKind::ControlChange(74),
            points: vec![hex_project::MidiLanePoint {
                beat: BeatTime::ONE,
                channel: 0,
                value: 96,
            }],
        })
        .unwrap();
    editor
        .execute(Command::SetMidiExpressionLane {
            clip,
            kind: hex_project::MidiLaneKind::PitchBend,
            points: vec![hex_project::MidiLanePoint {
                beat: BeatTime::ZERO,
                channel: 0,
                value: -2048,
            }],
        })
        .unwrap();
    editor
        .execute(Command::SetMidiNoteExpression {
            clip,
            note,
            expression: hex_project::NoteExpression {
                pitch_offset_cents: -150,
                timbre: 80,
                pressure: 64,
            },
            points: vec![hex_project::NoteExpressionPoint {
                offset: BeatTime::ONE,
                pitch_offset_cents: 120,
                timbre: 112,
                pressure: 90,
            }],
        })
        .unwrap();

    let project = editor.project();
    assert_eq!(
        crate::schema_version_for(project),
        crate::MIDI_EXPRESSION_SCHEMA_VERSION
    );
    let bytes = crate::encode(project).unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["version"], crate::MIDI_EXPRESSION_SCHEMA_VERSION);
    assert!(
        json["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "midi_expression")
    );

    let decoded = crate::decode(&bytes).unwrap();
    assert_eq!(decoded, *project);

    // Downgrading the version header to 126 must be refused cleanly.
    let mut downgraded = json;
    downgraded["version"] = serde_json::json!(CAPABILITY_MANIFEST_SCHEMA_VERSION);
    let downgraded_bytes = serde_json::to_vec(&downgraded).unwrap();
    assert!(crate::decode(&downgraded_bytes).is_err());
}

#[test]
#[ignore = "release benchmark comparing schema 126 vs stacked schemas 113..=125"]
fn benchmark_schema126_vs_stacked_legacy() {
    let project = build_rich_multi_feature_project();
    let feature_count = post_112_feature_count(&project);
    let iterations = 40u32;

    // Warm up
    let stacked_bytes = crate::encode_stacked_legacy(&project).unwrap();
    let unified_bytes = crate::encode(&project).unwrap();
    assert_eq!(crate::decode(&stacked_bytes).unwrap(), project);
    assert_eq!(crate::decode(&unified_bytes).unwrap(), project);

    let t0 = Instant::now();
    for _ in 0..iterations {
        let bytes = crate::encode_stacked_legacy(&project).unwrap();
        std::hint::black_box(bytes);
    }
    let stacked_encode = t0.elapsed();

    let t1 = Instant::now();
    for _ in 0..iterations {
        let bytes = crate::encode(&project).unwrap();
        std::hint::black_box(bytes);
    }
    let unified_encode = t1.elapsed();

    let t2 = Instant::now();
    for _ in 0..iterations {
        let decoded = crate::decode(&stacked_bytes).unwrap();
        std::hint::black_box(decoded);
    }
    let stacked_decode = t2.elapsed();

    let t3 = Instant::now();
    for _ in 0..iterations {
        let decoded = crate::decode(&unified_bytes).unwrap();
        std::hint::black_box(decoded);
    }
    let unified_decode = t3.elapsed();

    let enc_speedup = stacked_encode.as_secs_f64() / unified_encode.as_secs_f64();
    let dec_speedup = stacked_decode.as_secs_f64() / unified_decode.as_secs_f64();

    eprintln!(
        "=== Schema 126 vs Stacked 113..=125 ({feature_count} post-112 features, {iterations} iters) ==="
    );
    eprintln!(
        "bytes:   stacked={} B, unified={} B ({:.2}x smaller)",
        stacked_bytes.len(),
        unified_bytes.len(),
        stacked_bytes.len() as f64 / unified_bytes.len() as f64
    );
    eprintln!(
        "encode:  stacked={:?} ({:.2} ms/iter), unified={:?} ({:.2} ms/iter) => {:.2}x faster",
        stacked_encode,
        stacked_encode.as_secs_f64() * 1000.0 / f64::from(iterations),
        unified_encode,
        unified_encode.as_secs_f64() * 1000.0 / f64::from(iterations),
        enc_speedup
    );
    eprintln!(
        "decode:  stacked={:?} ({:.2} ms/iter), unified={:?} ({:.2} ms/iter) => {:.2}x faster",
        stacked_decode,
        stacked_decode.as_secs_f64() * 1000.0 / f64::from(iterations),
        unified_decode,
        unified_decode.as_secs_f64() * 1000.0 / f64::from(iterations),
        dec_speedup
    );
    assert!(
        enc_speedup > 3.0,
        "expected >3x encode speedup, got {enc_speedup:.2}x"
    );
    assert!(
        dec_speedup > 3.0,
        "expected >3x decode speedup, got {dec_speedup:.2}x"
    );
}

#[test]
#[ignore = "memory footprint probe for stacked legacy schemas 113..=125"]
fn bench_memory_stacked_legacy() {
    let project = build_rich_multi_feature_project();
    for _ in 0..40 {
        let bytes = crate::encode_stacked_legacy(&project).unwrap();
        let decoded = crate::decode(&bytes).unwrap();
        std::hint::black_box(decoded);
    }
}

#[test]
fn schema128_round_trip_and_older_schema_refusal() {
    use hex_project::{CurveTension, MixerAutomationPoint, MixerLaneTarget};

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack { name: "pad".into() })
        .unwrap()
    else {
        panic!("expected track");
    };
    let Created::ReturnTrack(ret) = editor
        .execute(Command::AddReturnTrack {
            name: "verb".into(),
        })
        .unwrap()
    else {
        panic!("expected return");
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: NativeDeviceState::new(DeviceTypeId::new("hex.utility").unwrap(), 1, vec![])
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("expected device");
    };
    let curve = AutomationInterpolation::Curve(CurveTension::from_millis(650).unwrap());
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::HostOutputGain,
            points: vec![
                AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.1).unwrap(),
                    outgoing: curve,
                },
                AutomationPoint {
                    beat: BeatTime::from_quarters(4).unwrap(),
                    value: AutomationValue::new(0.9).unwrap(),
                    outgoing: AutomationInterpolation::Linear,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::SetTrackMixerAutomation {
            track,
            target: MixerLaneTarget::Balance,
            points: vec![
                MixerAutomationPoint {
                    beat: BeatTime::ZERO,
                    value: -0.75,
                    outgoing: curve,
                },
                MixerAutomationPoint {
                    beat: BeatTime::from_quarters(4).unwrap(),
                    value: 0.75,
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::SetTrackMixerAutomation {
            track,
            target: MixerLaneTarget::Mute,
            points: vec![
                MixerAutomationPoint {
                    beat: BeatTime::ZERO,
                    value: 0.0,
                    outgoing: AutomationInterpolation::Step,
                },
                MixerAutomationPoint {
                    beat: BeatTime::from_quarters(2).unwrap(),
                    value: 1.0,
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::SetTrackMixerAutomation {
            track,
            target: MixerLaneTarget::Send(ret),
            points: vec![
                MixerAutomationPoint {
                    beat: BeatTime::ZERO,
                    value: 0.0,
                    outgoing: AutomationInterpolation::Linear,
                },
                MixerAutomationPoint {
                    beat: BeatTime::from_quarters(4).unwrap(),
                    value: 0.8,
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::FreezeTrack {
            track,
            path: "frozen/pad.wav".into(),
            start: 0,
            duration: 96_000,
            frames: 96_000,
        })
        .unwrap();

    let project = editor.project();
    assert_eq!(
        crate::schema_version_for(project),
        crate::AUTOMATION_CURVES_AND_FREEZE_SCHEMA_VERSION
    );
    let encoded = crate::encode(project).unwrap();
    let mut json: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        json["version"],
        crate::AUTOMATION_CURVES_AND_FREEZE_SCHEMA_VERSION
    );
    let caps = json["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c == "automation_curves_and_freeze"));

    let decoded = crate::decode(&encoded).unwrap();
    assert_eq!(&decoded, project);

    // Schema 127 must explicitly refuse schema 128 capability payloads.
    json["version"] = serde_json::json!(crate::MIDI_EXPRESSION_SCHEMA_VERSION);
    let downgraded = serde_json::to_vec(&json).unwrap();
    assert!(crate::decode(&downgraded).is_err());
}

#[test]
fn schema129_track_io_routing_round_trips_and_rejects_downgrade() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(source) = editor
        .execute(Command::AddTrack { name: "src".into() })
        .unwrap()
    else {
        panic!("source track");
    };
    let Created::Track(bus) = editor
        .execute(Command::AddTrack { name: "bus".into() })
        .unwrap()
    else {
        panic!("bus track");
    };
    editor
        .execute(Command::SetTrackOutput {
            track: source,
            destination: hex_project::TrackOutputDestination::Track(bus),
        })
        .unwrap();
    editor
        .execute(Command::SetTrackInputMonitoring {
            track: bus,
            mode: hex_project::InputMonitoring::In,
        })
        .unwrap();

    let project = editor.project();
    assert_eq!(
        crate::schema_version_for(project),
        crate::TRACK_IO_ROUTING_SCHEMA_VERSION
    );
    let encoded = crate::encode(project).unwrap();
    let mut json: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(json["version"], crate::TRACK_IO_ROUTING_SCHEMA_VERSION);
    let caps = json["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c == "track_io_routing"));

    let decoded = crate::decode(&encoded).unwrap();
    assert_eq!(&decoded, project);
    assert_eq!(
        decoded.track(source).unwrap().output_destination(),
        hex_project::TrackOutputDestination::Track(bus)
    );
    assert_eq!(
        decoded.track(bus).unwrap().input_monitoring(),
        hex_project::InputMonitoring::In
    );

    // Schema 128 must refuse schema 129 capability / track_io payloads cleanly.
    json["version"] = serde_json::json!(crate::AUTOMATION_CURVES_AND_FREEZE_SCHEMA_VERSION);
    let downgraded = serde_json::to_vec(&json).unwrap();
    assert!(crate::decode(&downgraded).is_err());
}

#[test]
fn unified_schema_feature_table_is_sorted_complete_and_blocks_legacy_encoders() {
    // 1. SCHEMA_FEATURES is strictly sorted and matches SUPPORTED_CAPABILITIES 1:1.
    let table_caps: Vec<&str> = super::SCHEMA_FEATURES
        .iter()
        .map(|feature| feature.capability)
        .collect();
    assert_eq!(table_caps.as_slice(), super::SUPPORTED_CAPABILITIES);
    assert!(table_caps.windows(2).all(|pair| pair[0] < pair[1]));

    // 2. M9 MIDI expression refuses encode_schema22 and encode_schema20.
    let mut midi_editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = midi_editor
        .execute(Command::AddTrack { name: "mpe".into() })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Clip(clip) = midi_editor
        .execute(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    midi_editor
        .execute(Command::SetMidiExpressionLane {
            clip,
            kind: hex_project::MidiLaneKind::ControlChange(1),
            points: vec![hex_project::MidiLanePoint {
                beat: hex_project::BeatTime::ZERO,
                channel: 0,
                value: 96,
            }],
        })
        .unwrap();
    assert!(crate::encode_schema22(midi_editor.project()).is_err());
    assert!(crate::encode_schema20(midi_editor.project()).is_err());

    // 3. M9 automation curves / track freeze refuse encode_schema22 and encode_schema20.
    let mut freeze_editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(ftrack) = freeze_editor
        .execute(Command::AddTrack {
            name: "frozen".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    freeze_editor
        .execute(Command::FreezeTrack {
            track: ftrack,
            path: "frozen.wav".into(),
            start: 0,
            duration: 128,
            frames: 128,
        })
        .unwrap();
    assert!(crate::encode_schema22(freeze_editor.project()).is_err());
    assert!(crate::encode_schema20(freeze_editor.project()).is_err());

    // 4. M9 track I/O routing refuses encode_schema22 and encode_schema20.
    let mut io_editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(iotrack) = io_editor
        .execute(Command::AddTrack { name: "io".into() })
        .unwrap()
    else {
        panic!("track");
    };
    io_editor
        .execute(Command::SetTrackInputMonitoring {
            track: iotrack,
            mode: hex_project::InputMonitoring::In,
        })
        .unwrap();
    assert!(crate::encode_schema22(io_editor.project()).is_err());
    assert!(crate::encode_schema20(io_editor.project()).is_err());

    // 5. Post-112 multi-feature projects refuse encode_schema22 and encode_schema20.
    let rich = build_rich_multi_feature_project();
    assert!(crate::encode_schema22(&rich).is_err());
    assert!(crate::encode_schema20(&rich).is_err());

    // 6. Top-level decode_impl size guard applies across legacy (schema 20/22/63) AND manifest schemas.
    let plain = Project::new(48_000).unwrap();
    for bytes in [
        crate::encode_schema20(&plain).unwrap(),
        crate::encode_schema22(&plain).unwrap(),
        crate::encode(&plain).unwrap(),
        crate::encode_capability_manifest(&plain).unwrap(),
    ] {
        let too_small = bytes.len() - 1;
        assert!(matches!(
            crate::decode_impl_within(&bytes, false, too_small),
            Err(StorageError::Invalid(
                "project exceeds the document size limit"
            ))
        ));
    }
}

#[test]
#[ignore = "memory footprint probe for unified schema 126"]
fn bench_memory_unified_schema126() {
    let project = build_rich_multi_feature_project();
    for _ in 0..40 {
        let bytes = crate::encode(&project).unwrap();
        let decoded = crate::decode(&bytes).unwrap();
        std::hint::black_box(decoded);
    }
}

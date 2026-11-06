use hex_lattice_contract::{NativeDeviceState, VST3_DEVICE_TYPE_ID};
use hex_project::{
    Command, Created, Editor, GroupOutputMuteRange, Project, TrackId, TrackSelectionIds, TrackTag,
    TrimEdge,
};
use serde_json::{Value, json};

fn fixture() -> (Project, TrackId, TrackId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported {
        track: child, clip, ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("source");
    };
    editor
        .execute(Command::MoveClip {
            clip,
            track: child,
            start: 48_000,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 24_000,
        })
        .unwrap();
    let Created::Track(group) = editor
        .execute(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[child]).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group");
    };
    editor
        .execute(Command::SetTrackTags {
            track: group,
            tags: vec![TrackTag::new("bounced").unwrap()],
        })
        .unwrap();
    (editor.project().clone(), group, child)
}

#[test]
fn group_output_mutes_round_trip_with_tags_and_a_leading_musical_clip() {
    let (mut project, group, _) = fixture();
    let previous = hex_storage::encode(&project).unwrap();
    assert_eq!(
        hex_storage::schema_version_for(&project),
        hex_storage::LEADING_AUDIO_ONCE_SCHEMA_VERSION
    );
    assert_eq!(hex_storage::decode(&previous).unwrap(), project);
    let fixture = include_bytes!("fixtures/warp-resampler/schema64.json");
    let historic = hex_storage::decode(fixture).unwrap();
    assert!(
        historic
            .tracks()
            .iter()
            .all(|track| track.group_output_mutes().is_empty())
    );
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(&historic).unwrap()).unwrap(),
        historic
    );

    for (start, end) in [(12_000, 24_000), (36_000, 42_000)] {
        project
            .add_group_output_mute(group, GroupOutputMuteRange::new(start, end).unwrap())
            .unwrap();
    }
    assert_eq!(
        hex_storage::schema_version_for(&project),
        hex_storage::GROUP_OUTPUT_MUTE_SCHEMA_VERSION
    );
    let directory = std::env::temp_dir().join(format!("hex-group-mute-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("group.hex");
    hex_storage::save_new(&path, &project).unwrap();
    let reopened = hex_storage::load(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
    assert_eq!(reopened, project);
}

#[test]
fn group_output_mute_codec_rejects_bad_inventory_and_version() {
    let (mut project, group, child) = fixture();
    project
        .add_group_output_mute(group, GroupOutputMuteRange::new(12_000, 24_000).unwrap())
        .unwrap();
    let encoded = hex_storage::encode(&project).unwrap();
    let original: Value = serde_json::from_slice(&encoded).unwrap();
    let row = original["group_output_mutes"][0].clone();
    for bad in [
        json!([]),
        json!([row.clone(), row.clone()]),
        json!([{ "group": child.value(), "ranges": [{"start": 1, "end": 2}] }]),
        json!([{ "group": u64::MAX, "ranges": [{"start": 1, "end": 2}] }]),
        json!([{ "group": group.value(), "ranges": [{"start": 2, "end": 1}] }]),
        json!([{ "group": group.value(), "ranges": [{"start": 1, "end": 2}, {"start": 2, "end": 3}] }]),
        json!([{ "group": group.value(), "ranges": [{"start": -1, "end": 2}] }]),
        json!([{ "group": group.value(), "ranges": [{"start": 1, "end": 2, "extra": true}] }]),
    ] {
        let mut document = original.clone();
        document["group_output_mutes"] = bad;
        assert!(hex_storage::decode(&serde_json::to_vec(&document).unwrap()).is_err());
    }
    let mut document = original.clone();
    document["version"] = hex_storage::LEADING_AUDIO_ONCE_SCHEMA_VERSION.into();
    assert!(hex_storage::decode(&serde_json::to_vec(&document).unwrap()).is_err());
    document["version"] = hex_storage::GROUP_OUTPUT_MUTE_SCHEMA_VERSION.into();
    document
        .as_object_mut()
        .unwrap()
        .remove("group_output_mutes");
    assert!(hex_storage::decode(&serde_json::to_vec(&document).unwrap()).is_err());
}

#[test]
fn larger_inactive_vst3_state_selects_schema75_and_preserves_group_mutes() {
    let (mut project, group, _) = fixture();
    project
        .add_group_output_mute(group, GroupOutputMuteRange::new(12_000, 24_000).unwrap())
        .unwrap();
    let schema74 = hex_storage::encode(&project).unwrap();
    assert_eq!(hex_storage::decode(&schema74).unwrap(), project);
    let mut older: Value = serde_json::from_slice(&schema74).unwrap();
    assert_eq!(older["version"], 74);
    older["version"] = 75.into();
    assert!(hex_storage::decode(&serde_json::to_vec(&older).unwrap()).is_err());

    let directory = std::env::temp_dir().join(format!("hex-large-vst3-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let previous_path = directory.join("schema74.hex");
    hex_storage::save_new(&previous_path, &project).unwrap();
    let (_, token) = hex_storage::load_with_token(&previous_path).unwrap();
    let small = NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![3; 4096]).unwrap();
    let large = NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![255; 150_545]).unwrap();
    let mut editor = Editor::new(project, 8).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: small.clone(),
        })
        .unwrap()
    else {
        panic!("main effect");
    };
    editor
        .execute(Command::ReplaceDeviceComparison {
            device,
            state: small,
            comparison: Some(hex_project::DeviceComparison::new(
                hex_project::DeviceVariant::A,
                large,
            )),
        })
        .unwrap();
    let project = editor.project();
    assert_eq!(hex_storage::schema_version_for(project), 75);
    assert!(token.requires_save_confirmation(project).unwrap());
    let current_path = directory.join("schema75.hex");
    hex_storage::save_new(&current_path, project).unwrap();
    let saved = std::fs::read(current_path).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(
        hex_storage::decode(&saved).unwrap().snapshot(),
        project.snapshot()
    );
    let mut document: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(document["version"], 75);
    let follows = project
        .clone()
        .with_sample_rate_mode(hex_project::SampleRateMode::FollowInterface);
    let encoded = hex_storage::encode(&follows).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&encoded).unwrap()["version"],
        hex_storage::SAMPLE_RATE_MODE_SCHEMA_VERSION,
    );
    assert_eq!(hex_storage::decode(&encoded).unwrap(), follows);

    document["version"] = 74.into();
    assert!(hex_storage::decode(&serde_json::to_vec(&document).unwrap()).is_err());
    assert!(hex_storage::encode_schema20(project).is_err());
    assert!(hex_storage::encode_schema22(project).is_err());
}

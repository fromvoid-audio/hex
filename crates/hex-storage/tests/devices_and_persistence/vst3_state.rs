//! Real generic current-format/files/history consumers; no foreign plug-in or fake DSP.
use hex_lattice_contract::NativeDeviceState;
use hex_plugin_state::{
    BinaryIdentity, ClassId, Error, Identity, State, TYPE_ID, available, decode, encode,
};
use hex_project::{Command, Created, DeviceId, Editor, Project, TrackId};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-vst3-state-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create unique owned test directory");
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn identity() -> Identity {
    Identity {
        class: ClassId::new([0x37; 16]).unwrap(),
        binary: BinaryIdentity {
            executable_sha256: [0x41; 32],
            info_sha256: [0x82; 32],
        },
    }
}
fn editor() -> (Editor, TrackId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "vst3 fixture".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    (editor, track)
}
fn insert(editor: &mut Editor, track: TrackId, state: NativeDeviceState) -> DeviceId {
    let Created::Device(id) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    id
}

// Schema75 admits a 512 KiB envelope including the 96-byte HXVS header.
const LEGACY_OPAQUE_BYTES: usize = hex_lattice_contract::MAX_VST3_DEVICE_STATE_BYTES - 96;

#[test]
fn current_file_and_history_preserve_exact_state_and_independent_project_instance_ids() {
    let directory = Directory::new();
    let path = directory.0.join("opaque.hex");
    let component: Vec<_> = (0..LEGACY_OPAQUE_BYTES - 17).map(|n| n as u8).collect();
    let controller = [0, 255, 1, 128, 2, 0, 7, 8, 9, 33, 42, 21, 18, 16, 15, 3, 0];
    let original = encode(State {
        identity: identity(),
        component: Some(&component),
        controller: Some(&controller),
    })
    .unwrap()
    .with_input_gain_millidb(-3750)
    .unwrap()
    .with_output_gain_millidb(1250)
    .unwrap();
    let (mut editor, track) = editor();
    let first = insert(&mut editor, track, original.clone());
    let second = insert(&mut editor, track, original.clone());
    assert_ne!(first, second); // same class and identical blobs never collapse instances
    let changed = encode(State {
        identity: identity(),
        component: Some(b"new component"),
        controller: Some(&[]),
    })
    .unwrap()
    .with_host_metadata_from(&original)
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: first,
            state: changed.clone(),
        })
        .unwrap();
    assert_eq!(editor.project().device(first).unwrap().state(), &changed);
    assert_eq!(editor.project().device(second).unwrap().state(), &original);
    assert!(editor.undo());
    assert_eq!(editor.project().device(first).unwrap().state(), &original);
    assert!(editor.redo());
    assert_eq!(editor.project().device(first).unwrap().state(), &changed);
    editor
        .execute(Command::MoveTrackDevice {
            device: second,
            track,
            before: Some(first),
        })
        .unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device: second,
            bypassed: true,
        })
        .unwrap();
    hex_storage::save_new(&path, editor.project()).unwrap();
    let (loaded, token) = hex_storage::load_with_token(&path).unwrap();
    assert_eq!(&loaded, editor.project());
    token.check_unchanged(&path).unwrap();
    for (id, expected) in [(first, &changed), (second, &original)] {
        let device = loaded.device(id).unwrap();
        assert_eq!(device.id(), id);
        assert_eq!(device.state(), expected);
        let view = available(device.state(), Some(identity())).unwrap();
        assert_eq!(view.identity, identity());
        assert_eq!(view, decode(expected).unwrap());
    }
    assert!(loaded.device(second).unwrap().bypassed());
    assert_eq!(loaded.tracks()[0].devices()[0].id(), second);
    // Ordinary insert after undo/branch never reuses a previously issued identity.
    assert!(editor.undo());
    assert!(editor.undo());
    assert!(editor.undo());
    let third = insert(&mut editor, track, original);
    assert!(third.value() > second.value());
    assert!(!editor.can_redo());
}

#[test]
fn unsupported_missing_and_malformed_payloads_roundtrip_but_report_unavailability() {
    let directory = Directory::new();
    let (mut editor, track) = editor();
    let valid = encode(State {
        identity: identity(),
        component: Some(b"opaque"),
        controller: None,
    })
    .unwrap();
    let missing = encode(State {
        identity: identity(),
        component: None,
        controller: Some(b"controller"),
    })
    .unwrap();
    let future = NativeDeviceState::new(TYPE_ID, 99, vec![0, 128, 255, 17]).unwrap();
    let malformed =
        NativeDeviceState::new(TYPE_ID, 1, b"retain damaged opaque payload".to_vec()).unwrap();
    for (index, (state, error)) in [
        (valid, Error::MissingBinary),
        (missing, Error::MissingComponent),
        (future, Error::UnsupportedVersion(99)),
        (malformed, Error::Malformed),
    ]
    .into_iter()
    .enumerate()
    {
        let id = insert(&mut editor, track, state.clone());
        let path = directory.0.join(format!("unavailable-{index}.hex"));
        hex_storage::save_new(&path, editor.project()).unwrap();
        let bytes = fs::read(&path).unwrap();
        let loaded = hex_storage::load(&path).unwrap();
        let retained = loaded.device(id).unwrap().state();
        assert_eq!(retained, &state);
        assert_eq!(available(retained, None), Err(error));
        assert_eq!(retained.payload(), state.payload());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(
            hex_storage::decode(&hex_storage::encode(&loaded).unwrap()).unwrap(),
            loaded
        );
    }
}

#[test]
fn oversized_generic_document_refuses_without_touching_original_file() {
    let directory = Directory::new();
    let (mut editor, track) = editor();
    let state = encode(State {
        identity: identity(),
        component: Some(&vec![255; LEGACY_OPAQUE_BYTES]),
        controller: None,
    })
    .unwrap();
    insert(&mut editor, track, state);
    let document = String::from_utf8(hex_storage::encode(editor.project()).unwrap()).unwrap();
    assert_eq!(document.matches("\"payload\":[").count(), 1);
    let oversized = document.replacen("\"payload\":[", "\"payload\":[0,", 1);
    let path = directory.0.join("oversized.hex");
    fs::write(&path, &oversized).unwrap();
    assert!(matches!(
        hex_storage::load(&path),
        Err(hex_storage::StorageError::Invalid(
            "invalid native device state envelope"
        ))
    ));
    assert_eq!(fs::read(&path).unwrap(), oversized.as_bytes());
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
}

#[test]
fn large_project_state_reopens_complete_without_rewriting_the_predecessor() {
    use hex_lattice_contract::{MAX_STORED_VST3_DEVICE_STATE_BYTES, Vst3StateAuthority};
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime,
        DeviceAutomationTarget, DeviceComparison, DeviceVariant,
    };
    let dir = Directory::new();
    let path = dir.0.join("before.hex");
    let (mut editor, track) = editor();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "retained reference.wav".into(),
            sample_rate: 48_000,
            frames: 48,
        })
        .unwrap()
    else {
        panic!("expected imported asset")
    };
    let reference = hex_lattice_contract::DeviceAssetReference {
        slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
        asset: hex_lattice_contract::DeviceAssetKey::new(asset.value()).unwrap(),
    };
    let small = encode(State {
        identity: identity(),
        component: Some(&[]),
        controller: Some(&[]),
    })
    .unwrap()
    .with_asset_references(vec![reference])
    .unwrap()
    .with_output_gain_millidb(-1250)
    .unwrap();
    let effect = insert(&mut editor, track, small.clone());
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device: effect,
            target: DeviceAutomationTarget::Vst3 { parameter: 0 },
            points: vec![AutomationPoint {
                beat: BeatTime::ONE,
                value: AutomationValue::new(0.25).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
        .unwrap();
    hex_storage::save_new(&path, editor.project()).unwrap();
    let before = fs::read(&path).unwrap();
    let (previous, token) = hex_storage::load_with_token(&path).unwrap();
    assert_eq!(hex_storage::encode(&previous).unwrap(), before);
    assert!(!token.requires_save_confirmation(&previous).unwrap());

    // Original header plus both SDK streams at the exact new project bound.
    let mut bytes = small.payload().to_vec();
    let component = MAX_STORED_VST3_DEVICE_STATE_BYTES - bytes.len() - 1;
    bytes[88..92].copy_from_slice(&(component as u32).to_le_bytes());
    bytes[92..96].copy_from_slice(&1u32.to_le_bytes());
    bytes.resize(MAX_STORED_VST3_DEVICE_STATE_BYTES, 0xd3);
    *bytes.last_mut().unwrap() = 0xfe;
    let state = NativeDeviceState::new(TYPE_ID, 1, bytes)
        .unwrap()
        .with_host_metadata_from(&small)
        .unwrap();
    assert_eq!(
        hex_plugin_state::decode(&state).unwrap().controller,
        Some(&[0xfe][..])
    );
    assert_eq!(
        available(&state, Some(identity())).unwrap().identity,
        identity()
    );
    let opaque = NativeDeviceState::new(TYPE_ID, 777, vec![0xa3; 550_001]).unwrap();
    editor
        .execute(Command::ReplaceDeviceComparison {
            device: effect,
            state,
            comparison: Some(DeviceComparison::new(
                DeviceVariant::B,
                opaque.clone().with_host_metadata_from(&small).unwrap(),
            )),
        })
        .unwrap();
    let authority = Vst3StateAuthority::new_on_control().unwrap();
    editor
        .execute(Command::CreateNativeSource {
            name: "unbound source".into(),
            devices: vec![authority.approve_processing_on_control(opaque).unwrap()],
            instrument_index: 0,
        })
        .unwrap();

    let project = editor.project();
    assert_eq!(
        hex_storage::schema_version_for(project),
        hex_storage::LARGE_VST3_STATE_SCHEMA_VERSION
    );
    assert!(token.requires_save_confirmation(project).unwrap());
    let upgraded = dir.0.join("upgraded.hex");
    hex_storage::save_new(&upgraded, project).unwrap();
    let (reopened, current) = hex_storage::load_with_token(&upgraded).unwrap();
    assert_eq!(reopened.snapshot(), project.snapshot());
    assert!(fs::metadata(&upgraded).unwrap().len() < 25 * 1024 * 1024);
    assert!(!current.requires_save_confirmation(&reopened).unwrap());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn compact_state_rejects_corruption_missing_slots_and_invalid_base_versions() {
    let (mut editor, track) = editor();
    let state = NativeDeviceState::new(TYPE_ID, 9, vec![7; 524_289]).unwrap();
    insert(&mut editor, track, state);
    let bytes = hex_storage::encode(editor.project()).unwrap();
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let refuses = |value: &serde_json::Value| {
        assert!(hex_storage::decode(&serde_json::to_vec(value).unwrap()).is_err())
    };
    let rows = document["vst3_states"].take();
    document["vst3_states"] = serde_json::json!([]);
    refuses(&document);
    document["vst3_states"] = serde_json::json!([rows[0], rows[0]]);
    refuses(&document);
    document["vst3_states"] = rows;
    document["vst3_states"][0]["inactive"] = true.into();
    refuses(&document);
    document["vst3_states"][0]["inactive"] = false.into();
    document["base"]["version"] = hex_storage::LARGE_VST3_STATE_SCHEMA_VERSION.into();
    refuses(&document);
    document = serde_json::from_slice(&bytes).unwrap();
    document["vst3_states"][0]["sha256"][0] =
        (document["vst3_states"][0]["sha256"][0].as_u64().unwrap() ^ 1).into();
    refuses(&document);
    document["vst3_states"][0]["payload"] = "!".into();
    refuses(&document);
    // One byte over the stored limit can have the same padded length as an
    // admissible payload. The decoded byte count still has to fit.
    use base64::{Engine, engine::general_purpose::STANDARD};
    let over = vec![0u8; hex_lattice_contract::MAX_STORED_VST3_DEVICE_STATE_BYTES + 1];
    document["vst3_states"][0]["payload"] = STANDARD.encode(&over).into();
    let error = hex_storage::decode(&serde_json::to_vec(&document).unwrap()).unwrap_err();
    assert!(matches!(
        error,
        hex_storage::StorageError::Invalid("invalid vst3 project state payload")
    ));
}

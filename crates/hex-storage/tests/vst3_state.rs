//! Real generic current-format/files/history consumers; no foreign plug-in or fake DSP.
use hex_lattice_contract::NativeDeviceState;
use hex_plugin_state::{
    BinaryIdentity, ClassId, Error, Identity, MAX_OPAQUE_BYTES, State, TYPE_ID, available, decode,
    encode,
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

#[test]
fn current_file_and_history_preserve_exact_state_and_independent_project_instance_ids() {
    let directory = Directory::new();
    let path = directory.0.join("opaque.hex");
    let component: Vec<_> = (0..MAX_OPAQUE_BYTES - 17).map(|n| n as u8).collect();
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
        component: Some(&vec![255; MAX_OPAQUE_BYTES]),
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

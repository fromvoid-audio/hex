//! The unsupported-host path refuses admission through the real Session/Root
//! adapter while preserving the ordinary graph and canonical history.
use super::{resident_preview::ResidentHarness, *};
use hex_session::service::{
    PluginEffectControl, PluginEffectDestination, PluginEffectInsertion, PluginEffectSelection,
};

#[test]
fn unavailable_plugin_insertion_preserves_project_history_and_ordinary_output() {
    let _native = native_fixture();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    assert!(initial.error.is_none());
    let (applied, _) = h.applied(0);
    let before = h.run(Request::Edit(hex_project::Command::AddTrack {
        name: "ordinary".into(),
    }));
    let (applied, _) = h.applied(applied);
    let track = before.state.project.tracks()[0].id();
    let source = before.state.canonical_audio_source.as_ref().unwrap();
    h.service
        .submit(Request::InsertPluginEffect(PluginEffectInsertion {
            context: source.context(),
            selection: PluginEffectSelection {
                // Valid copied metadata. No host is available to inspect or load it.
                module: PathBuf::from("/unavailable/fixture.vst3"),
                executable: "Contents/MacOS/fixture".into(),
                cid: [1; 16],
                executable_sha256: "01".repeat(32),
                info_sha256: "02".repeat(32),
            },
            destination: PluginEffectDestination::Track(track),
            before: None,
            track_device_name: None,
            control: Arc::new(PluginEffectControl::default()),
        }))
        .unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    let refused = loop {
        let event = h.next();
        if !event.plugin_effect_complete.is_empty() {
            break event;
        }
        assert!(Instant::now() < end, "insertion refusal did not complete");
    };
    assert_eq!(refused.plugin_effect_complete.len(), 1);
    assert!(!refused.plugin_effect_complete[0].accepted);
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("vst3 hosting requires macos arm64"),
        "unexpected refusal: {:?}",
        refused.error
    );
    assert_eq!(
        refused.state.project.snapshot(),
        before.state.project.snapshot()
    );
    assert_eq!(refused.state.revision, before.state.revision);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    let undone = h.run(Request::Undo);
    assert!(undone.error.is_none());
    assert!(undone.state.project.tracks().is_empty());
    h.applied(applied);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
}

#[test]
fn unavailable_foreign_graph_refuses_open_without_rewriting_opaque_project() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    assert!(initial.error.is_none());
    h.applied(0);
    let state = hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [2; 32],
                info_sha256: [3; 32],
            },
        },
        component: Some(b"original opaque component"),
        controller: Some(b"original opaque controller"),
    })
    .unwrap();
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(hex_project::Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let path = files.0.join("opaque.hex");
    hex_storage::save_new(&path, editor.project()).unwrap();
    let bytes = fs::read(&path).unwrap();
    h.service.submit(Request::Open(path.clone())).unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    let refused = loop {
        let event = h.next();
        if event.error.is_some() || event.open_complete {
            break event;
        }
        assert!(Instant::now() < end, "foreign graph open did not complete");
    };
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("vst3 hosting requires macos arm64"),
        "unexpected refusal: {:?}",
        refused.error
    );
    assert!(!refused.project_replaced);
    assert_eq!(
        refused.state.project.snapshot(),
        initial.state.project.snapshot()
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
}

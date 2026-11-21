//! Signed stopped history through the catalog path selected at Desktop startup.
use super::*;
use crate::native_audio::plugins::catalog_path_from_home;

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn relative_catalog_undo_prepares_a_fresh_worker_after_stopped_delete() {
    let _native = native_fixture();
    let files = Files::new();
    let fixture_home = files.0.canonicalize().unwrap();
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let mut relative_home = PathBuf::new();
    for _ in cwd.ancestors().skip(1) {
        relative_home.push("..");
    }
    relative_home.push(fixture_home.strip_prefix("/").unwrap());
    let browser_catalog =
        hex_preferences::browser_places_path(&relative_home).with_file_name("plugin-catalog.json");
    let catalog = catalog_path_from_home(&relative_home).expect("resolvable relative catalog");
    assert!(catalog.is_absolute());
    let directory = browser_catalog.parent().unwrap();
    fs::create_dir_all(directory).unwrap();
    hex_ui::write_plugin_effect_test_catalog(directory, &selection(), "Fx", 0).unwrap();
    assert_eq!(
        catalog.canonicalize().unwrap(),
        browser_catalog.canonicalize().unwrap()
    );

    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        catalog,
    );
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let document = h.run(Request::Import(files.wav(48_000, 4096))).state;
    let (imported, _) = h.applied(initial);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &document,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    assert!(inserted.plugin_effect_complete[0].accepted);
    let Some(Created::Device(device)) = inserted.created else {
        panic!("signed Main effect");
    };
    let (applied, _) = h.applied(imported);
    assert_stopped_hosted_removal_history(&mut h, &inserted.state, device, applied);
}

#[test]
#[ignore = "explicit original SDK source fixture required"]
fn missing_catalog_configuration_refuses_ordinary_insertion_before_a_worker() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::without_plugin_catalog_configuration();
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let document = h.run(Request::Import(files.wav(48_000, 4096))).state;
    let (applied, _) = h.applied(initial);
    let recording_source = |h: &ResidentHarness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source()
            .unwrap()
    };
    let predecessor = recording_source(&h);
    assert_eq!(predecessor.audio_revision, applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &document,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    let refused = terminal(&mut h);
    assert_eq!(
        refused.error.as_deref(),
        Some("vst3 catalog path unavailable · configure a valid home folder")
    );
    assert!(!refused.plugin_effect_complete[0].accepted);
    assert_eq!(refused.state.revision, document.revision);
    assert_eq!(refused.state.project, document.project);
    assert_eq!(refused.state.can_undo, document.can_undo);
    assert_eq!(refused.state.can_redo, document.can_redo);
    assert_eq!(recording_source(&h), predecessor);
    assert_eq!(h.plugin_records.load(Acquire), 0);
    assert!(h.plugin_progress.lock().unwrap().newest_source.is_none());
}

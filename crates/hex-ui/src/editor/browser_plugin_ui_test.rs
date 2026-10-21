//! Source-level rendered browser metadata/focus/history contract, no app launch.
use super::*;

fn original_plugin_catalog(path: std::path::PathBuf) -> hex_plugin_catalog::Catalog {
    let report=serde_json::from_value(serde_json::json!({
        "profile":"explicit-user-trusted-macos-factory-only","request":1,"generation":1,
        "receipt_sha256":"a".repeat(64),"helper_sha256":"b".repeat(64),
        "bundle":{"supplied_path":path,"canonical_path":path,"info_sha256":"c".repeat(64)},
        "executable":{"relative_path":"Contents/MacOS/Original","sha256":"d".repeat(64),"image":"thin_arm64"},
        "bundle_identifier":"org.hex.original","interruption":null,"error":null,"stdout":"","stderr":"",
        "cleanup":{"direct_reaped":true,"direct_exit_code":0,"direct_exit_signal":null,"custody_lost":false,"cleanup_unconfirmed":false,
          "exit_observed_before_signals":true,"original_group_term":{"status":"absent"},"original_group_kill":{"status":"absent"},"direct_kill":null,"descendants":"unknown","pending":false},
        "discovery":{"request":1,"sdk":"3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96","public_sdk":"586dc5e6c8012c3e4b01c79389375cbe96bdb1da","platform":"darwin-arm64",
          "factory_flags":0,"vendor":"Original Vendor","url":"","email":"","classes":[{"cid":vec![1u8;16],"cardinality":1,"category":"Audio Module Class","name":"Original Processor","class_flags":null,"class_version":null,"class_sdk_version":null}]}
    })).expect("original copied report fixture");
    let mut catalog = hex_plugin_catalog::Catalog::default();
    catalog
        .record(path, Ok((report, "e".repeat(40))))
        .expect("validated original catalog");
    catalog
}

#[gpui::test]
fn browser_vst3_restores_searchable_typed_rows_and_enter_never_queues_native_insertion(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor, _track, directory, _wave) = browser_drag_fixture(cx);
    let physical = directory.0.canonicalize().unwrap();
    let module = physical.join("Original.vst3");
    let catalog_path = physical.join("plugin-catalog.json");
    let catalog = original_plugin_catalog(module.clone());
    hex_plugin_catalog::storage::update(&catalog_path, |target| {
        *target = catalog;
        Ok(())
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.browser.preferences_path = physical.join("browser.json");
        view.select_browser_source(BrowserSection::Plugins, cx);
        view.start_plugin_catalog(cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        visual.run_until_parked();
        if editor.read_with(&visual, |view, _| !view.browser.plugins.loading) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "catalog load completion"
        );
        std::thread::yield_now();
    }
    let bounds = visual
        .debug_bounds("browser-vst3-row-0")
        .expect("painted actual VST3 row");
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    editor.read_with(&visual,|view,cx|{
        assert!(matches!(view.browser_selected_item(cx),Some(crate::editor::browser::BrowserItemKey::Plugin(ref path,Some(id))) if path==&module && id==[1;16]));
        assert!(view.browser_visible_devices(cx).is_empty());assert_eq!(view.browser_visible_plugins(cx)[0].name,"Original Processor");
    });
    visual.simulate_keystrokes("enter");
    editor.read_with(&visual, |view, _| {
        assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
        assert!(view.last_enqueued_edit.is_none());
        assert!(view.status.contains("hosting and insertion unavailable"));
    });
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.restore_query("vendor", cx))
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_plugins(cx).len(), 1)
    });
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.restore_query("absent", cx))
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert!(view.browser_visible_plugins(cx).is_empty())
    });
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.restore_query("", cx));
        view.select_browser_source(BrowserSection::All, cx);
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.restore_browser_selection(
            Some(crate::editor::browser::BrowserItemKey::Plugin(
                module.clone(),
                Some([1; 16]),
            )),
            cx,
        );
    });
    visual.run_until_parked();
    editor.read_with(&visual,|view,cx|{
        assert!(view.browser.selected_index>=view.browser_visible_entries(cx).len()+view.browser_visible_devices(cx).len());
        assert!(matches!(view.browser_selected_item(cx),Some(crate::editor::browser::BrowserItemKey::Plugin(ref path,Some(id))) if path==&module&&id==[1;16]));
    });
    editor.update(&mut visual,|view,cx|{
        let mut updated=original_plugin_catalog(module.clone());
        updated.entries.extend(original_plugin_catalog(physical.join("AAA.vst3")).entries);
        updated.entries.sort_by(|a,b|a.path.cmp(&b.path));updated.validate().unwrap();
        view.apply_plugin_catalog(updated,cx);
        assert!(matches!(view.browser_selected_item(cx),Some(crate::editor::browser::BrowserItemKey::Plugin(ref path,Some(id))) if path==&module&&id==[1;16]));
        view.browser.results_focused=false;view.browser.selected_index=1;
        view.apply_plugin_catalog(original_plugin_catalog(module.clone()),cx);
        assert_eq!(view.browser.selected_index,1,"background metadata must not reset unfocused browser position");
    });
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[gpui::test]
#[ignore = "explicit original signed package and native fixtures required"]
fn browser_vst3_selected_native_worker_persists_real_row_and_reload_without_foreign_rescan(
    cx: &mut TestAppContext,
) {
    let app = std::path::PathBuf::from(
        std::env::var_os("HEX_VST3_CATALOG_TEST_APP")
            .expect("explicit original signed package fixture"),
    );
    let module = std::path::PathBuf::from(
        std::env::var_os("HEX_VST3_CATALOG_FIXTURES").expect("explicit original native fixtures"),
    )
    .join("hex-vst3-host.app/Contents/Resources/factory-fixtures/normal.vst3");
    let (handle, editor, _track, directory, _wave) = browser_drag_fixture(cx);
    let physical = directory.0.canonicalize().unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    let wait = |visual: &mut VisualTestContext, editor: &gpui::Entity<EditorView>| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            visual.run_until_parked();
            if editor.read_with(visual, |view, _| {
                !view.browser.plugins.loading && !view.browser.plugins.busy
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "selected scan/retirement completion"
            );
            std::thread::yield_now();
        }
    };
    wait(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        view.set_plugin_test_app(app, physical.join("browser.json"));
        view.select_browser_source(BrowserSection::Plugins, cx);
        view.scan_trusted_plugin(module.clone(), cx);
    });
    wait(&mut visual, &editor);
    editor.read_with(&visual, |view, cx| {
        let rows = view.browser_visible_plugins(cx);
        assert_eq!(rows.len(), 1, "{}", view.browser.plugins.status);
        assert_eq!(rows[0].name, "hex original factory processor");
        assert_eq!(rows[0].module, module);
        assert!(
            rows[0]
                .cid
                .is_some_and(|id| id[..4] == *b"HEXF" && id[15] == 1)
        );
        assert!(view.browser.plugins.status.contains("discovered"));
    });
    assert!(visual.debug_bounds("browser-vst3-row-0").is_some());
    let stored = std::fs::read(physical.join("plugin-catalog.json")).unwrap();
    editor.update(&mut visual, |view, cx| view.start_plugin_catalog(cx));
    wait(&mut visual, &editor);
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_plugins(cx).len(), 1)
    });
    assert_eq!(
        std::fs::read(physical.join("plugin-catalog.json")).unwrap(),
        stored,
        "reload must not rewrite or rescan foreign code"
    );
}

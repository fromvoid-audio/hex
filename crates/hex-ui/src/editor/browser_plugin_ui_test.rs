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
    .join("hex-vst3-host.app/Contents/Resources/factory-fixtures/factory2-instrument.vst3");
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
        assert_eq!(rows[0].status, "vst3 instrument · hosting unavailable");
        assert_eq!(rows[0].vendor, "original extended vendor");
        assert!(
            rows[0]
                .cid
                .is_some_and(|id| id[..4] == *b"HEXF" && id[15] == 1)
        );
        assert!(view.browser.plugins.status.contains("discovered"));
    });
    assert!(visual.debug_bounds("browser-vst3-row-0").is_some());
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.restore_query("instrument", cx))
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_plugins(cx).len(), 1)
    });
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

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn wait_plugin_browser(visual: &mut VisualTestContext, editor: &gpui::Entity<EditorView>) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        visual.run_until_parked();
        if editor.read_with(visual, |view, _| {
            !view.browser.plugins.loading && !view.browser.plugins.busy
        }) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::yield_now();
    }
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn copy_original_plugin(source: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        assert!(!entry.file_type().unwrap().is_symlink());
        if entry.file_type().unwrap().is_dir() {
            copy_original_plugin(&entry.path(), &destination);
        } else {
            std::fs::copy(entry.path(), destination).unwrap();
        }
    }
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[gpui::test]
#[ignore = "explicit original signed package and owned native fixture roots required"]
fn browser_vst3_standard_folder_button_scans_owned_roots_and_restores_type_search(
    cx: &mut TestAppContext,
) {
    let app = std::path::PathBuf::from(
        std::env::var_os("HEX_VST3_CATALOG_TEST_APP").expect("explicit signed original package"),
    );
    let fixtures = std::path::PathBuf::from(
        std::env::var_os("HEX_VST3_CATALOG_FIXTURES").expect("explicit original fixtures"),
    )
    .join("hex-vst3-host.app/Contents/Resources/factory-fixtures");
    let (handle, editor, _track, directory, _wave) = browser_drag_fixture(cx);
    let physical = directory.0.canonicalize().unwrap();
    let roots = physical.join("owned-standard-root");
    std::fs::create_dir(&roots).unwrap();
    copy_original_plugin(
        &fixtures.join("factory2-instrument.vst3"),
        &roots.join("A.vst3"),
    );
    copy_original_plugin(
        &fixtures.join("factory2-effect.vst3"),
        &roots.join("B.vst3"),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    if !wait_plugin_browser(&mut visual, &editor) {
        std::mem::forget(directory);
        panic!("owned roots retained at {physical:?}; startup unresolved");
    }
    editor.update(&mut visual, |view, cx| {
        view.set_plugin_test_app(app, physical.join("browser.json"));
        view.set_plugin_test_roots(vec![roots, physical.join("missing-standard-root")]);
        view.select_browser_source(BrowserSection::Plugins, cx);
        cx.notify();
    });
    visual.run_until_parked();
    let button = visual
        .debug_bounds("browser-vst3-scan-standard")
        .expect("rendered standard-folder action");
    visual.simulate_mouse_down(button.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(button.center(), MouseButton::Left, Modifiers::default());
    if !wait_plugin_browser(&mut visual, &editor) {
        std::mem::forget(directory);
        panic!(
            "owned roots retained at {physical:?}; scanner retirement unresolved; no forced termination"
        );
    }
    editor.read_with(&visual, |view, cx| {
        let rows = view.browser_visible_plugins(cx);
        assert_eq!(rows.len(), 2, "{}", view.browser.plugins.status);
        assert!(view.browser.plugins.status.contains("2 discovered"));
        assert_ne!(
            rows[0].module, rows[1].module,
            "same reported UID in different modules remains visible"
        );
        assert_eq!(rows[0].cid, rows[1].cid);
    });
    assert!(visual.debug_bounds("browser-vst3-row-1").is_some());
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.restore_query("instrument", cx))
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let rows = view.browser_visible_plugins(cx);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "vst3 instrument · hosting unavailable");
    });
    let stored = std::fs::read(physical.join("plugin-catalog.json")).unwrap();
    editor.update(&mut visual, |view, cx| view.start_plugin_catalog(cx));
    if !wait_plugin_browser(&mut visual, &editor) {
        std::mem::forget(directory);
        panic!("owned roots retained at {physical:?}; reload unresolved");
    }
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_plugins(cx).len(), 1)
    });
    assert_eq!(
        std::fs::read(physical.join("plugin-catalog.json")).unwrap(),
        stored
    );
}

#[gpui::test]
fn browser_vst3_sidebar_visibility_selects_real_source_and_preserves_search_history(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor, _track, directory, _wave) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        visual.run_until_parked();
        if editor.read_with(&visual, |view, _| !view.browser.plugins.loading) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "metadata restoration completes"
        );
        std::thread::yield_now();
    }
    // No discovery is requested: the existing validated original metadata
    // fixture is enough to verify sidebar navigation and typed row selection.
    let module = directory.0.canonicalize().unwrap().join("Original.vst3");
    editor.update(&mut visual, |view, cx| {
        view.apply_plugin_catalog(original_plugin_catalog(module.clone()), cx);
        cx.notify();
    });
    visual.run_until_parked();
    let click = |visual: &mut VisualTestContext, selector: &'static str| {
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"));
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.run_until_parked();
    };
    for source in [
        "browser-section-all",
        "browser-section-samples",
        "browser-section-devices",
        "browser-section-projects",
        "browser-section-vst3",
    ] {
        assert!(
            visual.debug_bounds(source).is_some(),
            "enabled source {source} must paint"
        );
    }
    click(&mut visual, "browser-control-⚙");
    click(&mut visual, "browser-section-visibility-4");
    assert!(visual.debug_bounds("browser-section-vst3").is_none());
    click(&mut visual, "browser-section-visibility-4");
    click(&mut visual, "browser-section-vst3");
    click(&mut visual, "browser-vst3-row-0");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::Plugins);
        assert!(matches!(view.browser_selected_item(cx), Some(crate::editor::browser::BrowserItemKey::Plugin(ref path, Some(id))) if path == &module && id == [1; 16]));
        assert!(view.browser_visible_devices(cx).is_empty());
        assert!(view.browser.plugins.status.contains("unavailable"), "catalog metadata must retain truthful discovery/hosting status");
    });
    click(&mut visual, "browser-search");
    visual.simulate_input("vendor");
    visual.run_until_parked();
    // A query edit deliberately clears result focus. Select the filtered row
    // before navigating so history has a real selected identity to restore.
    click(&mut visual, "browser-vst3-row-0");
    click(&mut visual, "browser-section-devices");
    click(&mut visual, "browser-history-back");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::Plugins);
        assert_eq!(view.browser.search.read(cx).content, "vendor");
        assert_eq!(view.browser_visible_plugins(cx).len(), 1);
        assert!(matches!(view.browser_selected_item(cx), Some(crate::editor::browser::BrowserItemKey::Plugin(ref path, Some(id))) if path == &module && id == [1; 16]));
    });
    click(&mut visual, "browser-section-visibility-4");
    assert!(visual.debug_bounds("browser-section-vst3").is_none());
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser.section,
            BrowserSection::Samples,
            "hiding active vst3 selects the existing visible fallback"
        );
        assert_eq!(view.browser.search.read(cx).content, "vendor");
    });
    click(&mut visual, "browser-history-back");
    editor.read_with(&visual, |view, _| {
        assert_ne!(
            view.browser.section,
            BrowserSection::Plugins,
            "history cannot restore a hidden source"
        )
    });
    click(&mut visual, "browser-section-visibility-4");
    click(&mut visual, "browser-section-vst3");
    editor.update(&mut visual, |view, cx| {
        view.apply_plugin_catalog(hex_plugin_catalog::Catalog::default(), cx);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-section-vst3").is_some(),
        "empty metadata must not remove the sidebar entry"
    );
    assert!(visual.debug_bounds("browser-vst3-row-0").is_none());
    visual.simulate_keystrokes("enter");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::Plugins);
        assert!(view.browser_visible_plugins(cx).is_empty());
        assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
        assert!(view.last_enqueued_edit.is_none());
        assert!(
            !view.browser.plugins.busy,
            "source navigation leaves the scan idle"
        );
    });
}

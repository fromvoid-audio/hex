//! Source-level rendered browser metadata/focus/history contract, no app launch.
use super::*;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn wait_for_plugin_scan(visual: &mut VisualTestContext, editor: &gpui::Entity<EditorView>) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        visual.run_until_parked();
        if editor.read_with(visual, |view, _| {
            !view.browser.plugins.loading && !view.browser.plugins.busy
        }) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let evidence = editor.read_with(visual, |view, _| {
                format!("{:?}", view.browser.plugins.scan_evidence())
            });
            panic!("selected scan/retirement completion: {evidence}");
        }
        std::thread::yield_now();
    }
}

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

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[gpui::test]
fn browser_result_rows_use_compact_native_text_and_keep_selected_hover(cx: &mut TestAppContext) {
    use crate::theme;
    cx.use_native_text_system().expect("native text shaping");
    cx.update(|cx| {
        theme::load_fonts(cx).expect("bundled ioskeley mono");
        crate::editor::bind_editor_keys(cx);
    });
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    wait_for_plugin_scan(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        view.browser.auto_audition = false;
        view.apply_plugin_catalog(
            original_plugin_catalog(directory.0.join("Original.vst3")),
            cx,
        );
    });
    let fill = theme::color(theme::PaletteRole::Secondary);
    let ink = theme::readable_text(fill, theme::color(theme::PaletteRole::Background));
    for (source, query, row, label, icon) in [
        (
            BrowserSection::All,
            "loop",
            "browser-entry-0",
            "browser-entry-label-0",
            Some("browser-file-icon-0"),
        ),
        (
            BrowserSection::All,
            "utility",
            "browser-device-effect-hex.utility",
            "browser-device-label-effect-hex.utility",
            None,
        ),
        (
            BrowserSection::Plugins,
            "",
            "browser-vst3-row-0",
            "browser-vst3-name-0",
            None,
        ),
    ] {
        editor.update(&mut visual, |view, cx| {
            view.select_browser_source(source.clone(), cx);
            view.browser
                .search
                .update(cx, |input, cx| input.set_text(query, cx));
            view.sync_browser_query(cx);
            cx.notify();
        });
        visual.run_until_parked();
        let bounds = visual.debug_bounds(row).expect("actual projected result");
        assert_eq!(bounds.size.height, px(18.0));
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_move(bounds.center(), None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_background(row),
            Some(gpui::Background::from(gpui::rgb(fill)))
        );
        assert!(
            visual
                .rendered_monochrome_colors(label)
                .contains(&gpui::rgb(ink).into()),
            "selected text: {label}"
        );
        let label_bounds = visual.debug_bounds(label).unwrap();
        assert!(label_bounds.top() >= bounds.top() && label_bounds.bottom() <= bounds.bottom());
        if let Some(icon) = icon {
            assert!(
                visual
                    .rendered_monochrome_colors(icon)
                    .contains(&gpui::rgb(ink).into())
            );
        }
        editor.read_with(&visual, |view, _| {
            assert!(view.pending_audio_commands.is_empty())
        });
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[gpui::test]
fn vst3_groups_are_visible_without_becoming_keyboard_items(cx: &mut TestAppContext) {
    use crate::editor::browser::BrowserItemKey;
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let physical = directory.0.canonicalize().unwrap();
    let mut catalog = hex_plugin_catalog::Catalog::default();
    for (module, name, role) in [
        ("A-delay.vst3", "Space Delay", Some("Fx")),
        ("Z-synth.vst3", "Zebra Synth", Some("Instrument")),
        ("other.vst3", "Unknown Processor", None),
    ] {
        let mut entry = original_plugin_catalog(physical.join(module))
            .entries
            .remove(0);
        let class = &mut entry
            .last_good
            .as_mut()
            .unwrap()
            .discovery
            .as_mut()
            .unwrap()
            .classes[0];
        class.name = name.into();
        if let Some(role) = role {
            class.class_flags = Some(0);
            class.subcategories = Some(role.into());
            class.class_vendor = Some("Original Vendor".into());
            class.class_version = Some("1".into());
            class.class_sdk_version = Some("3.8".into());
        }
        catalog.entries.push(entry);
    }
    catalog.validate().unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    wait_for_plugin_scan(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        view.select_browser_source(BrowserSection::Plugins, cx);
        view.apply_plugin_catalog(catalog, cx);
        cx.notify();
    });
    visual.run_until_parked();
    let instruments = visual
        .debug_bounds("browser-vst3-group-instruments")
        .unwrap();
    let effects = visual.debug_bounds("browser-vst3-group-effects").unwrap();
    let first = visual.debug_bounds("browser-vst3-row-0").unwrap();
    let second = visual.debug_bounds("browser-vst3-row-1").unwrap();
    assert!(instruments.origin.y < first.origin.y && first.origin.y < effects.origin.y);
    assert!(effects.origin.y < second.origin.y);
    assert!(
        visual
            .debug_bounds("browser-vst3-group-other vst3")
            .is_some()
    );
    visual.simulate_click(first.center(), Modifiers::default());
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser_selected_item(cx),
            Some(BrowserItemKey::Plugin(
                physical.join("Z-synth.vst3"),
                Some([1; 16])
            ))
        );
    });
    visual.simulate_keystrokes("down");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.browser_selected_item(cx),
            Some(BrowserItemKey::Plugin(
                physical.join("A-delay.vst3"),
                Some([1; 16])
            ))
        );
    });
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("space", cx));
        view.sync_browser_query(cx);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-vst3-group-effects").is_some());
    assert!(
        visual
            .debug_bounds("browser-vst3-group-instruments")
            .is_none()
    );
    assert!(
        visual
            .debug_bounds("browser-vst3-group-other vst3")
            .is_none()
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[gpui::test]
fn browser_vst3_reload_marks_missing_source_without_rewriting_last_good_or_scanning(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _track, directory, _wave) = browser_drag_fixture(cx);
    let physical = directory.0.canonicalize().unwrap();
    let path = physical.join("plugin-catalog.json");
    let module = physical.join("missing.vst3");
    hex_plugin_catalog::storage::update(&path, |catalog| {
        *catalog = original_plugin_catalog(module.clone());
        catalog
            .entries
            .extend(original_plugin_catalog(physical.join("AAA.vst3")).entries);
        catalog.validate()?;
        Ok(())
    })
    .unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    wait_for_plugin_scan(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        view.browser.preferences_path = physical.join("browser.json");
        view.select_browser_source(BrowserSection::Plugins, cx);
        view.start_plugin_catalog(cx);
    });
    wait_for_plugin_scan(&mut visual, &editor);
    let row = visual
        .debug_bounds("browser-vst3-row-1")
        .expect("painted explicit duplicate module");
    visual.simulate_click(row.center(), Modifiers::default());
    editor.update(&mut visual, |view, cx| view.start_plugin_catalog(cx));
    wait_for_plugin_scan(&mut visual, &editor);
    editor.read_with(&visual, |view, cx| {
        let rows = view.browser_visible_plugins(cx);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].module, module);
        assert_eq!(rows[0].cid, rows[1].cid);
        assert_eq!(rows[1].cid, Some([1; 16]));
        assert!(matches!(view.browser_selected_item(cx), Some(crate::editor::browser::BrowserItemKey::Plugin(ref path, Some(id))) if path == &module && id == [1;16]), "reload must preserve selected path, never substitute another duplicate CID");
        assert!(
            rows[1]
                .status
                .contains("source unavailable · rescan required"),
            "missing source must not look current: {:?}",
            rows[1]
        );
        assert!(
            view.browser.plugins.diagnostics.is_none(),
            "reload must not start scanner"
        );
    });
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[gpui::test]
fn browser_vst3_effect_enter_reaches_worker_and_unavailable_terminal_retires_pending(
    cx: &mut TestAppContext,
) {
    unavailable_effect_activation(cx, false);
}

#[gpui::test]
fn browser_vst3_effect_double_click_reaches_worker_and_unavailable_terminal_retires_pending(
    cx: &mut TestAppContext,
) {
    unavailable_effect_activation(cx, true);
}

fn unavailable_effect_activation(cx: &mut TestAppContext, pointer: bool) {
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor, track, directory, _) = browser_drag_fixture(cx);
    let mut log = pointer.then(|| {
        let log = hex_diagnostics::SessionLog::start(directory.0.join("vst3-diagnostics")).unwrap();
        cx.update(|cx| {
            crate::diagnostics::install_session_log(cx, Ok(log.handle()));
            crate::diagnostics::install_action_diagnostics(cx, true);
        });
        log
    });
    let module = directory.0.canonicalize().unwrap().join("Original.vst3");
    let mut catalog = original_plugin_catalog(module);
    let class = &mut catalog.entries[0]
        .last_good
        .as_mut()
        .unwrap()
        .discovery
        .as_mut()
        .unwrap()
        .classes[0];
    class.class_flags = Some(0);
    class.subcategories = Some("Fx".into());
    class.class_vendor = Some("Original Vendor".into());
    class.class_version = Some("1".into());
    class.class_sdk_version = Some("3.8".into());
    catalog
        .select_effect(&catalog.entries[0].path, [1; 16])
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.select_browser_source(BrowserSection::Plugins, cx);
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        visual.run_until_parked();
        if editor.read_with(&visual, |view, _| !view.browser.plugins.loading) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "startup catalog load");
        std::thread::yield_now();
    }
    editor.update(&mut visual, |view, cx| {
        view.apply_plugin_catalog(catalog, cx);
        cx.notify();
    });
    visual.run_until_parked();
    let row = visual
        .debug_bounds("browser-vst3-row-0")
        .expect("painted effect");
    if pointer {
        visual.simulate_event(gpui::MouseDownEvent {
            position: row.center(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
            modifiers: Modifiers::default(),
        });
        visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    } else {
        visual.simulate_click(row.center(), Modifiers::default());
        visual.simulate_keystrokes("enter");
    }
    editor.read_with(&visual, |view, _| {
        assert!(
            view.pending_plugin_effect.is_some(),
            "effect activation must reach the canonical worker: error={:?} status={} selected={:?} state={:?}",
            view.error, view.status, view.selected_track, view.state.as_ref().map(|s| (s.revision, s.project.tracks().len()))
        );
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(16));
        visual.run_until_parked();
        if editor.read_with(&visual, |view, _| view.pending_plugin_effect.is_none()) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual unavailable-output terminal"
        );
        std::thread::yield_now();
    }
    editor.read_with(&visual, |view, _| {
        assert!(
            view.error
                .as_ref()
                .is_some_and(|e| e.contains("vst3 hosting unavailable for this output")),
            "{:?}",
            view.error
        );
        let state = view.state.as_ref().unwrap();
        assert!(!state.can_undo && !state.can_redo && !state.unsaved);
        assert!(
            state.project.tracks().is_empty(),
            "refusal restores the actual worker snapshot"
        );
        assert_eq!(view.worker_requests_pending, 0);
    });
    if let Some(log) = log.as_mut() {
        log.shutdown().unwrap();
        let trace = std::fs::read_to_string(log.path()).unwrap();
        let records: Vec<_> = trace
            .lines()
            .filter(|line| line.contains("vst3_load"))
            .collect();
        assert_eq!(records.len(), 2);
        assert!(records[0].contains("stage=submitted"));
        assert!(records[1].contains("stage=refused"));
        for record in records {
            assert!(record.contains("Original Processor") && record.contains("rate_hz=48000"));
            assert!(!record.contains("Original.vst3"));
        }
    }
}

#[gpui::test]
fn browser_vst3_restores_searchable_typed_rows_and_enter_refuses_unavailable_inventory(
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
    let deadline = crate::test_wait::hang_deadline();
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
        assert!(
            view.error.is_some(),
            "unavailable inventory must refuse explicitly"
        );
    });
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("vendor", cx))
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_plugins(cx).len(), 1)
    });
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("absent", cx))
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert!(view.browser_visible_plugins(cx).is_empty())
    });
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("", cx));
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
fn browser_vst3_native_refusal_keeps_actual_report_and_unavailable_scene(cx: &mut TestAppContext) {
    let app = std::path::PathBuf::from(std::env::var_os("HEX_VST3_CATALOG_TEST_APP").unwrap());
    let module = std::path::PathBuf::from(std::env::var_os("HEX_VST3_CATALOG_FIXTURES").unwrap())
        .join("hex-vst3-host.app/Contents/Resources/factory-fixtures/factory2-mismatch.vst3");
    let (handle, editor, _track, directory, _wave) = browser_drag_fixture(cx);
    let physical = directory.0.canonicalize().unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    wait_for_plugin_scan(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        view.set_plugin_test_app(app, physical.join("browser.json"));
        view.select_browser_source(BrowserSection::Plugins, cx);
        view.scan_trusted_plugin(module.clone(), cx);
    });
    wait_for_plugin_scan(&mut visual, &editor);
    editor.read_with(&visual, |view, cx| {
        let evidence = || format!("{:?}", view.browser.plugins.scan_evidence());
        let rows = view.browser_visible_plugins(cx);
        assert_eq!(rows.len(), 1, "{}", evidence());
        assert_eq!(rows[0].module, module, "{}", evidence());
        assert!(
            rows[0].cid.is_none(),
            "refusal has no accepted class: {}",
            evidence()
        );
        assert!(
            view.browser.plugins.status.contains("failed"),
            "{}",
            evidence()
        );
        let observed = view
            .browser
            .plugins
            .diagnostics
            .as_ref()
            .expect("actual native observations");
        assert!(
            observed.scanner_started && observed.scanner_exit.is_some(),
            "{}",
            evidence()
        );
        let report: serde_json::Value = serde_json::from_slice(&observed.report).unwrap();
        assert!(report["error"].is_string(), "{}", evidence());
        let cleanup = observed.native_cleanup.as_ref().expect("reported cleanup");
        assert!(cleanup.direct_reaped && !cleanup.pending, "{}", evidence());
        assert!(
            evidence().contains("direct_reaped"),
            "scene assertion evidence retains cleanup"
        );
    });
    assert!(visual.debug_bounds("browser-vst3-row-0").is_some());
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
    wait_for_plugin_scan(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        view.set_plugin_test_app(app, physical.join("browser.json"));
        view.select_browser_source(BrowserSection::Plugins, cx);
        view.scan_trusted_plugin(module.clone(), cx);
    });
    wait_for_plugin_scan(&mut visual, &editor);
    editor.read_with(&visual, |view, cx| {
        let rows = view.browser_visible_plugins(cx);
        let evidence = || format!("{:?}", view.browser.plugins.scan_evidence());
        assert_eq!(rows.len(), 1, "{}", evidence());
        assert_eq!(
            rows[0].name,
            "hex original factory processor",
            "{}",
            evidence()
        );
        assert_eq!(rows[0].module, module, "{}", evidence());
        assert_eq!(
            rows[0].status,
            "vst3 instrument · hosting unavailable",
            "{}",
            evidence()
        );
        assert_eq!(rows[0].vendor, "original extended vendor", "{}", evidence());
        assert!(
            rows[0]
                .cid
                .is_some_and(|id| id[..4] == *b"HEXF" && id[15] == 1),
            "{}",
            evidence()
        );
        assert!(
            view.browser.plugins.status.contains("discovered"),
            "{}",
            evidence()
        );
    });
    assert!(visual.debug_bounds("browser-vst3-row-0").is_some());
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("instrument", cx))
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_plugins(cx).len(), 1)
    });
    let stored = std::fs::read(physical.join("plugin-catalog.json")).unwrap();
    editor.update(&mut visual, |view, cx| view.start_plugin_catalog(cx));
    wait_for_plugin_scan(&mut visual, &editor);
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
    let deadline = crate::test_wait::hang_deadline();
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
    browser_vst3_folder_scan(cx, false);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[gpui::test]
#[ignore = "explicit original signed package and owned native fixture roots required"]
fn browser_vst3_custom_folder_picker_scans_owned_root_and_restores_type_search(
    cx: &mut TestAppContext,
) {
    browser_vst3_folder_scan(cx, true);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn browser_vst3_folder_scan(cx: &mut TestAppContext, custom: bool) {
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
        if !custom {
            view.set_plugin_test_roots(vec![roots.clone(), physical.join("missing-standard-root")]);
        }
        view.select_browser_source(BrowserSection::Plugins, cx);
        cx.notify();
    });
    visual.run_until_parked();
    if custom {
        let gear = visual.debug_bounds("browser-control-⚙").unwrap();
        visual.simulate_click(gear.center(), Modifiers::default());
        visual.run_until_parked();
        for source in ["browser-vst3-source-system", "browser-vst3-source-user"] {
            let flag = visual.debug_bounds(source).unwrap();
            visual.simulate_click(flag.center(), Modifiers::default());
            visual.run_until_parked();
        }
        editor.read_with(&visual, |view, _| {
            let sources = &view.browser.source_preferences.vst3_scan_sources;
            assert!(!sources.system && !sources.user);
        });
        let choose = visual.debug_bounds("browser-vst3-choose-folder").unwrap();
        visual.simulate_click(choose.center(), Modifiers::default());
        visual.run_until_parked();
        assert!(visual.did_prompt_for_paths());
        visual.simulate_paths_selection(Some(vec![roots]));
        visual.run_until_parked();
        assert!(!editor.read_with(&visual, |view, _| view.browser.plugins.busy));
    }
    let scan = visual.debug_bounds("browser-vst3-scan").unwrap();
    visual.simulate_click(scan.center(), Modifiers::default());
    if !wait_plugin_browser(&mut visual, &editor) {
        std::mem::forget(directory);
        panic!(
            "owned roots retained at {physical:?}; scanner retirement unresolved; no forced termination"
        );
    }
    editor.read_with(&visual, |view, cx| {
        let rows = view.browser_visible_plugins(cx);
        assert_eq!(rows.len(), 2, "{}", view.browser.plugins.status);
        assert!(
            view.browser.plugins.status.contains("2 discovered"),
            "{}; rows: {rows:?}",
            view.browser.plugins.status
        );
        assert_ne!(
            rows[0].module, rows[1].module,
            "same reported UID in different modules remains visible"
        );
        assert_eq!(rows[0].cid, rows[1].cid);
    });
    assert!(visual.debug_bounds("browser-vst3-row-1").is_some());
    assert!(visual.debug_bounds("browser-vst3-scan-activity").is_none());
    editor.update(&mut visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("instrument", cx))
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let rows = view.browser_visible_plugins(cx);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].selection.is_some());
        assert_eq!(rows[0].status, "vst3 instrument · drag or enter to insert");
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

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[gpui::test]
fn browser_vst3_custom_picker_cancel_and_error_preserve_catalog_without_scanner(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _track, directory, _wave) = browser_drag_fixture(cx);
    let physical = directory.0.canonicalize().unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    wait_for_plugin_scan(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        // No package exists: cancellation/error must never try to inspect or run it.
        view.set_plugin_test_app(physical.join("absent.app"), physical.join("browser.json"));
        view.apply_plugin_catalog(original_plugin_catalog(physical.join("Original.vst3")), cx);
        view.select_browser_source(BrowserSection::Plugins, cx);
        cx.notify();
    });
    visual.run_until_parked();
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    for error in [false, true] {
        visual.run_until_parked();
        let bounds = visual.debug_bounds("browser-vst3-choose-folder").unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.run_until_parked();
        assert!(visual.did_prompt_for_paths());
        if error {
            visual.simulate_paths_error("original picker failure");
        } else {
            visual.simulate_paths_selection(None);
        }
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert!(!view.browser.plugins.busy);
            assert!(view.browser.plugins.diagnostics.is_none());
            assert_eq!(view.browser_visible_plugins(cx).len(), 1);
            if error {
                assert_eq!(view.browser.plugins.status, "plugin selection failed");
            }
        });
        assert!(!physical.join("plugin-catalog.json").exists());
    }
}

#[gpui::test]
fn browser_vst3_sidebar_visibility_selects_real_source_and_preserves_search_history(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor, _track, directory, _wave) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let deadline = crate::test_wait::hang_deadline();
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
        assert_eq!(view.browser.search.read(cx).text(), "vendor");
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
        assert_eq!(view.browser.search.read(cx).text(), "vendor");
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

#[gpui::test]
fn all_search_devices_precede_samples_and_refresh_keeps_activation_identity(
    cx: &mut TestAppContext,
) {
    use crate::editor::browser::BrowserItemKey;
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor, track, directory, wave) = browser_drag_fixture(cx);
    let first = directory.0.join("a utility.wav");
    let last = directory.0.join("z utility.wav");
    for path in [&first, &last] {
        std::fs::copy(&wave, path).unwrap();
    }
    let module = directory.0.join("utility.vst3");
    let mut catalog = original_plugin_catalog(module.clone());
    catalog.entries[0]
        .last_good
        .as_mut()
        .unwrap()
        .discovery
        .as_mut()
        .unwrap()
        .classes[0]
        .name = "utility plugin".into();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.apply_plugin_catalog(catalog.clone(), cx);
        view.apply_sample_catalog(
            hex_library::catalog::scan_samples(std::slice::from_ref(&directory.0)).catalog,
            cx,
        );
    });
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("UtIlItY");
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_devices(cx).len(), 1);
        assert_eq!(view.browser_visible_plugins(cx).len(), 1);
        let files = view.browser_visible_entries(cx);
        assert_eq!(files.iter().map(|f| &f.path).collect::<Vec<_>>(), [&first, &last]);
        assert!(matches!(view.browser_item_at(0, cx), Some(BrowserItemKey::Device(..))));
        assert!(matches!(view.browser_item_at(1, cx), Some(BrowserItemKey::Plugin(ref p, _)) if p == &module));
        assert!(matches!(view.browser_item_at(2, cx), Some(BrowserItemKey::File(ref p)) if p == &first));
        assert!(matches!(view.browser_item_at(3, cx), Some(BrowserItemKey::File(ref p)) if p == &last));
        assert!(view.browser_item_at(4, cx).is_none());
    });
    let device = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .unwrap();
    let plugin = visual.debug_bounds("browser-vst3-row-1").unwrap();
    let sample = visual.debug_bounds("browser-entry-2").unwrap();
    assert!(device.origin.y < plugin.origin.y && plugin.origin.y < sample.origin.y);
    visual.simulate_keystrokes("down enter");
    editor.read_with(&visual, |view, cx| {
        assert!(matches!(
            view.browser_selected_item(cx),
            Some(BrowserItemKey::Device(..))
        ));
        assert!(view.browser_selected_sample(cx).is_none());
        assert!(view.pending_requests.is_empty());
        assert_eq!(
            view.status,
            "drag a browser device onto a track or device rail"
        );
    });
    visual.simulate_keystrokes("down down");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_selected_sample(cx), Some(first.clone()));
    });
    // These are the same receive methods used by asynchronous catalog workers.
    // New device/sample matches shift global indices without changing selection.
    let extra = directory.0.join("0 utility.vst3");
    let earlier = directory.0.join("0 utility.wav");
    std::fs::copy(&wave, &earlier).unwrap();
    editor.update(&mut visual, |view, cx| {
        let mut refreshed = catalog;
        let mut added = original_plugin_catalog(extra);
        added.entries[0]
            .last_good
            .as_mut()
            .unwrap()
            .discovery
            .as_mut()
            .unwrap()
            .classes[0]
            .name = "another utility plugin".into();
        refreshed.entries.extend(added.entries);
        view.apply_plugin_catalog(refreshed, cx);
        assert_eq!(view.browser_selected_sample(cx), Some(first.clone()));
        view.apply_sample_catalog(
            hex_library::catalog::scan_samples(std::slice::from_ref(&directory.0)).catalog,
            cx,
        );
        assert_eq!(view.browser.selected_index, 4);
        assert_eq!(view.browser_selected_sample(cx), Some(first.clone()));
        assert!(
            view.browser_history_index(&BrowserItemKey::File(last.clone()), cx)
                .is_some()
        );
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "sample import: {:?}", view.error);
        assert!(view.audio_batch.is_some());
        assert!(matches!(view.last_enqueued_import_to_track.as_ref(), Some((path, target, _, _)) if path == &first && *target == track));
    });
}

#[gpui::test]
fn all_search_keeps_within_kind_order_and_single_kind_queries(cx: &mut TestAppContext) {
    use crate::editor::browser::BrowserItemKey;
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for (section, query) in [
        (BrowserSection::All, "e"),
        (BrowserSection::All, "VeCtOr"),
        (BrowserSection::All, "loop"),
        (BrowserSection::All, "no-such-result-3270"),
        (BrowserSection::All, ""),
        (BrowserSection::Samples, "loop"),
        (BrowserSection::Instruments, "i"),
        (BrowserSection::Plugins, "processor"),
        (BrowserSection::Place(directory.0.clone()), "loop"),
    ] {
        editor.update(&mut visual, |view, cx| {
            view.select_browser_source(section.clone(), cx);
            view.browser
                .search
                .update(cx, |input, cx| input.set_text(query, cx));
            view.sync_browser_query(cx);
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            let files: Vec<_> = view
                .browser_visible_entries(cx)
                .iter()
                .map(|row| BrowserItemKey::File(row.path.clone()))
                .collect();
            let devices: Vec<_> = view
                .browser_visible_devices(cx)
                .iter()
                .map(|(class, row)| BrowserItemKey::Device(*class, row.type_id))
                .collect();
            let plugins: Vec<_> = view
                .browser_visible_plugins(cx)
                .iter()
                .map(|row| BrowserItemKey::Plugin(row.module.clone(), row.cid))
                .collect();
            if section == BrowserSection::All {
                match query {
                    "VeCtOr" => assert!(!devices.is_empty() && files.is_empty()),
                    "loop" => assert!(!files.is_empty() && devices.is_empty()),
                    "no-such-result-3270" => {
                        assert!(files.is_empty() && devices.is_empty() && plugins.is_empty())
                    }
                    _ => {}
                }
            }
            let expected: Vec<_> = if section == BrowserSection::All && !query.is_empty() {
                devices.into_iter().chain(plugins).chain(files).collect()
            } else {
                files.into_iter().chain(devices).chain(plugins).collect()
            };
            let actual: Vec<_> = (0..expected.len())
                .map(|index| view.browser_item_at(index, cx).unwrap())
                .collect();
            assert_eq!(actual, expected, "section={section:?}, query={query}");
            for (index, key) in expected.iter().enumerate() {
                assert_eq!(view.browser_history_index(key, cx), Some(index));
            }
            assert!(view.browser_item_at(expected.len(), cx).is_none());
        });
    }
}

#[gpui::test]
fn vst_effect_drag_to_unselected_track_reaches_plugin_worker(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    for (mode, lane) in [
        (crate::editor::EditorMode::Arrangement, false),
        (crate::editor::EditorMode::Session, false),
        (crate::editor::EditorMode::Arrangement, true),
    ] {
        let (handle, editor, tracks, directory, _) =
            browser_drag_fixture_with_tracks(cx, &["selected", "target"]);
        let module = directory.0.canonicalize().unwrap().join("Original.vst3");
        let mut catalog = original_plugin_catalog(module);
        let class = &mut catalog.entries[0]
            .last_good
            .as_mut()
            .unwrap()
            .discovery
            .as_mut()
            .unwrap()
            .classes[0];
        class.class_flags = Some(0);
        class.subcategories = Some("Fx".into());
        class.class_vendor = Some("Original Vendor".into());
        class.class_version = Some("1".into());
        class.class_sdk_version = Some("3.8".into());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.update(&mut visual, |view, cx| {
            view.selected_track = Some(tracks[0]);
            view.mode = mode;
            view.select_browser_source(BrowserSection::Plugins, cx);
        });
        let deadline = crate::test_wait::hang_deadline();
        loop {
            visual.run_until_parked();
            if editor.read_with(&visual, |view, _| !view.browser.plugins.loading) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        editor.update(&mut visual, |view, cx| {
            view.apply_plugin_catalog(catalog, cx);
            cx.notify();
        });
        visual.run_until_parked();
        let source = visual.debug_bounds("browser-vst3-row-0").unwrap().center();
        let selector = Box::leak(
            format!(
                "{}-{}",
                if lane {
                    "arrangement-lane"
                } else if mode == crate::editor::EditorMode::Arrangement {
                    "arrangement-track-header"
                } else {
                    "session-track"
                },
                tracks[1].value()
            )
            .into_boxed_str(),
        );
        let target = visual.debug_bounds(selector).unwrap().center();
        for refusal in 0..2 {
            visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(
                point(source.x + px(12.0), source.y),
                MouseButton::Left,
                Modifiers::default(),
            );
            visual.run_until_parked();
            editor.update(&mut visual, |view, _| {
                if refusal == 0 {
                    view.worker_requests_pending = 1;
                } else {
                    view.project_generation += 1;
                }
            });
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            editor.update(&mut visual, |view, cx| {
                assert!(view.pending_plugin_effect.is_none());
                assert!(view.active_native_device_drag.is_none());
                if refusal == 0 {
                    view.worker_requests_pending = 0;
                } else {
                    view.project_generation -= 1;
                }
                cx.notify();
            });
            visual.run_until_parked();
        }
        if lane {
            let lane_bounds = visual.debug_bounds(selector).unwrap();
            let insertion = point(lane_bounds.center().x, lane_bounds.origin.y);
            visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(
                point(source.x + px(12.0), source.y),
                MouseButton::Left,
                Modifiers::default(),
            );
            visual.simulate_mouse_move(insertion, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("arrangement-device-drop-preview")
                    .is_none()
            );
            visual.simulate_mouse_up(insertion, MouseButton::Left, Modifiers::default());
            editor.read_with(&visual, |view, _| {
                assert!(view.pending_plugin_effect.is_none())
            });
            visual.run_until_parked();
        }
        let original_feedback = visual.rendered_background(selector);
        visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(source.x + px(12.0), source.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        if lane {
            assert!(
                visual
                    .debug_bounds("arrangement-device-drop-preview")
                    .is_some()
            );
        } else {
            assert_ne!(visual.rendered_background(selector), original_feedback);
        }
        editor.read_with(&visual, |view, _| {
            assert!(
                view.pending_plugin_effect.is_none(),
                "hover must not instantiate the plugin"
            )
        });
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            let pending = view.pending_plugin_effect.as_ref().unwrap_or_else(|| {
                panic!("header drop must reach the plugin worker: {:?}", view.error)
            });
            assert_eq!(
                pending.destination,
                hex_session::service::PluginEffectDestination::Track(tracks[1])
            );
            assert_eq!(
                pending.before, None,
                "the plugin appends to the captured track"
            );
            assert_eq!(view.selected_track, Some(tracks[0]));
        });
    }
}

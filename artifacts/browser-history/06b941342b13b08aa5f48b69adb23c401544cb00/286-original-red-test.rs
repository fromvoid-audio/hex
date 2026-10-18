//! History through real toolbar input over bounded private worker roots.
use super::*;

fn fixture(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>, PreferencesTestDirectory) {
    cx.update(crate::editor::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    std::fs::create_dir_all(directory.0.join("samples")).unwrap();
    std::fs::write(directory.0.join("samples/loop.wav"), mono_pcm_wav(24)).unwrap();
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |window, cx| {
            EditorView::new_for_test_with_browser_directory(window, cx, Service::new(48_000,32,8_000_000).unwrap(), &directory.0)
        })
    }).unwrap();
    (handle, editor, directory)
}

#[gpui::test]
fn browser_history_toolbar_restores_source_without_preview_or_import(cx: &mut TestAppContext) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle,cx);
    visual.run_until_parked();
    for selector in ["browser-section-devices", "browser-section-projects"] {
        let hit = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(hit, Modifiers::default());
        visual.run_until_parked();
    }
    let hit = visual.debug_bounds("browser-history-back").unwrap().center();
    visual.simulate_click(hit, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.section, BrowserSection::Instruments);
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
        assert!(view.browser.playback_path.is_none());
    });
}

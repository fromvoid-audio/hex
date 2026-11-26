//! Rendered GPUI tests for the top tag selector and combined filtering (#2666).

use super::*;
use hex_library::catalog::scan_samples;

fn minimal_wav() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&44u32.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);
    out
}

#[gpui::test]
fn top_tag_selector_combines_within_group_or_across_group_and_descendants_query_and_hide_show(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let dir = PreferencesTestDirectory::new();
    let samples = dir.0.join("samples");
    std::fs::create_dir_all(&samples).unwrap();
    let kick_loop = samples.join("kick-loop.wav");
    let snare_oneshot = samples.join("snare-shot.wav");
    let pad_loop = samples.join("pad-loop.wav");
    for path in [&kick_loop, &snare_oneshot, &pad_loop] {
        std::fs::write(path, minimal_wav()).unwrap();
    }
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(
                    window,
                    cx,
                    Service::new(48_000, 16, 8_000_000).unwrap(),
                    &dir.0,
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    // Seed catalog with hierarchical taxonomy + assigned tags.
    let mut catalog = scan_samples(std::slice::from_ref(&samples)).catalog;
    catalog.define_tag("sample", None).unwrap();
    catalog.define_tag("kick", Some("sample")).unwrap();
    catalog.add_tag(&kick_loop, "kick").unwrap();
    catalog.add_tag(&kick_loop, "loop").unwrap();
    catalog.add_tag(&snare_oneshot, "sample").unwrap();
    catalog.add_tag(&snare_oneshot, "one shot").unwrap();
    catalog.add_tag(&pad_loop, "loop").unwrap();

    editor.update(&mut visual, |view, cx| {
        view.browser.section = BrowserSection::Samples;
        view.browser.samples.replace(catalog);
        cx.notify();
    });
    visual.run_until_parked();

    // 1. Click "sample" chip in group 0 (content): matches kick_loop (descendant "kick"!) AND snare_oneshot.
    let sample_chip = visual
        .debug_bounds("browser-tag-chip-0-sample")
        .expect("sample tag chip");
    visual.simulate_mouse_down(
        sample_chip.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.path == kick_loop));
        assert!(entries.iter().any(|e| e.path == snare_oneshot));
    });

    // 2. Click "loop" chip in group 1 (type): AND across groups narrows to kick_loop only!
    let loop_chip = visual
        .debug_bounds("browser-tag-chip-1-loop")
        .expect("loop tag chip");
    visual.simulate_mouse_down(loop_chip.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, kick_loop);
    });

    // 3. Also click "one shot" in group 1 (type): OR within group 1 widens back to kick_loop + snare_oneshot!
    let oneshot_chip = visual
        .debug_bounds("browser-tag-chip-1-one shot")
        .expect("one shot tag chip");
    visual.simulate_mouse_down(
        oneshot_chip.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser_visible_entries(cx).len(), 2);
        assert_eq!(view.browser.samples.active_tag_count(), 3);
    });

    // 4. Hide and re-show browser retains active tag filters.
    editor.update_in(&mut visual, |view, window, cx| {
        view.toggle_browser(window, cx);
        view.toggle_browser(window, cx);
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser.samples.active_tag_count(), 3);
        assert_eq!(view.browser_visible_entries(cx).len(), 2);
    });

    // 5. Clicking "clear" clears all active tag filters cleanly.
    let clear_btn = visual
        .debug_bounds("browser-tag-clear")
        .expect("clear tag filters button");
    visual.simulate_mouse_down(clear_btn.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.browser.samples.active_tag_count(), 0);
        assert!(!view.browser.samples.has_active_tags());
    });
}

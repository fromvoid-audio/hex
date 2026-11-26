//! Rendered GPUI tests for the inline mini tag strip, vertical resize, and full tag editor (#2667).

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
fn inline_and_full_tag_editors_share_canonical_state_support_bulk_and_resize_without_moving_audition(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let dir = PreferencesTestDirectory::new();
    let samples = dir.0.join("samples");
    std::fs::create_dir_all(&samples).unwrap();
    let kick = samples.join("kick.wav");
    let snare = samples.join("snare.wav");
    std::fs::write(&kick, minimal_wav()).unwrap();
    std::fs::write(&snare, minimal_wav()).unwrap();

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

    let catalog = scan_samples(std::slice::from_ref(&samples)).catalog;
    let listing = hex_library::scan_directory(&samples).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.browser.directory = samples.clone();
        view.browser.listing = Some(listing);
        view.browser.section = BrowserSection::Place(samples.clone());
        view.browser.samples.replace(catalog);
        view.browser.samples.invalidate_listing();
        view.browser.selected_index = 0;
        cx.notify();
    });
    visual.run_until_parked();

    // 1. Open full right-side tag editor and click checkbox for "loop" (group 1).
    let toggle_btn = visual
        .debug_bounds("browser-control-⋯")
        .expect("tag editor toggle");
    visual.simulate_click(toggle_btn.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-tag-editor").is_some());

    let loop_checkbox = visual
        .debug_bounds("browser-tag-editor-toggle-1-loop")
        .expect("loop checkbox in full tag editor");
    visual.simulate_mouse_down(
        loop_checkbox.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();

    // Mini tag strip immediately reflects the assigned "loop" tag chip!
    let mini_chip = visual
        .debug_bounds("browser-mini-tag-remove-loop")
        .expect("mini tag chip for loop");
    editor.read_with(&visual, |view, _| {
        assert!(
            view.browser
                .samples
                .catalog
                .tags_for(&kick)
                .unwrap()
                .contains("loop")
        );
    });

    // 2. Clicking the mini tag chip removes it from canonical state and full editor.
    visual.simulate_mouse_down(mini_chip.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("browser-mini-tag-remove-loop")
            .is_none()
    );
    editor.read_with(&visual, |view, _| {
        assert!(view.browser.samples.catalog.tags_for(&kick).is_none());
    });

    // 3. Atomic bulk mixed-state toggle across kick & snare assigns to both, then clears both,
    //    and preserves existing tags when an invalid label fails validation.
    editor.update(&mut visual, |view, cx| {
        view.add_sample_tag(&kick, "punchy", cx).unwrap();
        let pair = vec![kick.clone(), snare.clone()];
        assert!(view.bulk_toggle_sample_tag(&pair, "punchy", cx).unwrap());
        assert!(
            view.browser
                .samples
                .catalog
                .tags_for(&snare)
                .unwrap()
                .contains("punchy")
        );
        assert!(view.bulk_toggle_sample_tag(&pair, "   ", cx).is_err());
        assert!(
            view.browser
                .samples
                .catalog
                .tags_for(&kick)
                .unwrap()
                .contains("punchy")
        );
        assert!(view.bulk_toggle_sample_tag(&pair, "punchy", cx).unwrap());
        assert!(view.browser.samples.catalog.tags_for(&kick).is_none());
        assert!(view.browser.samples.catalog.tags_for(&snare).is_none());
        cx.notify();
    });
    visual.run_until_parked();

    // 4. Dragging the mini tag resize divider upward expands ONLY the tag area;
    //    the bottom waveform/audition row stays pinned at identical height and bounds!
    let audition_before = visual
        .debug_bounds("browser-audition-row")
        .expect("audition row before resize");
    let preview_before = visual
        .debug_bounds("browser-selection-preview")
        .expect("preview before resize");
    let resize_handle = visual
        .debug_bounds("browser-mini-tags-resize")
        .expect("mini tags resize handle");
    let drag_target = point(
        resize_handle.center().x,
        resize_handle.center().y - px(48.0),
    );
    visual.simulate_mouse_down(
        resize_handle.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(drag_target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(drag_target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    let preview_after = visual
        .debug_bounds("browser-selection-preview")
        .expect("preview after resize");
    let audition_after = visual
        .debug_bounds("browser-audition-row")
        .expect("audition row after resize");
    assert!(
        preview_after.size.height > preview_before.size.height + px(20.0),
        "mini tag area must grow upward when divider is dragged up"
    );
    assert_eq!(
        audition_after, audition_before,
        "audition/waveform row must stay completely stationary and unresized"
    );
}

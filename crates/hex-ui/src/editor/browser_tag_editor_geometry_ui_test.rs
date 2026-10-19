//! Real GPUI bounds and stationary-pointer gestures for the outward tag column.
use super::*;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    PreferencesTestDirectory,
) {
    let directory = PreferencesTestDirectory::new();
    let (handle, editor) = place_icon_preferences_window(
        cx,
        &directory,
        &directory.0.join("browser.json"),
        &directory.0.join("recent.json"),
    );
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let music = visual.debug_bounds("browser-place-0").unwrap();
    visual.simulate_click(music.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-entry-0").is_some(),
        "positive WAV fixture must be rendered"
    );
    let automatic = visual.debug_bounds("browser-audition-auto").unwrap();
    visual.simulate_click(automatic.center(), Modifiers::default());
    let sample = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_click(sample.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        let key = view
            .browser
            .preview_selected
            .as_ref()
            .expect("actual sample selection prepared metadata");
        assert!(view.browser.preview_cache[key].metadata.borrow().is_some());
        assert!(!view.browser.auto_audition);
    });
    (handle, editor, directory)
}

fn picker_width(visual: &mut VisualTestContext, width: f32) {
    let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
    let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
    let target = divider.center() + point(px(width) - sidebar.size.width, px(0.0));
    visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("browser-sidebar").unwrap().size.width,
        px(width)
    );
}

const STATIONARY: [&str; 16] = [
    "browser-toolbar",
    "browser-search",
    "browser-history-back",
    "browser-control-≡",
    "browser-control-⋮",
    "browser-control-×",
    "browser-sidebar",
    "browser-sidebar-resize",
    "browser-entry-0",
    "browser-tag-selector",
    "browser-selection-preview",
    "browser-control-⋯",
    "browser-audition-row",
    "browser-audition-auto",
    "browser-audition-transport",
    "browser-audition-waveform",
];

#[gpui::test]
fn browser_tag_editor_stationary_rail_and_full_height_outward_column(cx: &mut TestAppContext) {
    let (handle, editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    for width in [124.0, 23.0] {
        picker_width(visual, width);
        for affordance in ["browser-control-⋮", "browser-control-⋯"] {
            let before = STATIONARY.map(|selector| visual.debug_bounds(selector).unwrap());
            let outer = visual.debug_bounds("browser-resize-handle").unwrap();
            let generation = editor.read_with(visual, |view, _| view.browser.playback_generation);
            let (key, metadata, preparations) = editor.read_with(visual, |view, _| {
                let key = view.browser.preview_selected.as_ref().unwrap().clone();
                (
                    key.clone(),
                    Rc::clone(&view.browser.preview_cache[&key].metadata),
                    view.browser.preview_preparations,
                )
            });
            let pointer = visual.debug_bounds(affordance).unwrap().center();
            for _ in 0..3 {
                visual.simulate_click(pointer, Modifiers::default());
                visual.run_until_parked();
                let column = visual
                    .debug_bounds("browser-tag-editor")
                    .expect("actual toggle opened tag column");
                for (selector, expected) in STATIONARY.into_iter().zip(before) {
                    assert_eq!(
                        visual.debug_bounds(selector).unwrap(),
                        expected,
                        "opening from {affordance} moved {selector} at picker width {width}"
                    );
                }
                let toolbar = visual.debug_bounds("browser-toolbar").unwrap();
                let preview = visual.debug_bounds("browser-selection-preview").unwrap();
                assert_eq!(
                    column.top(),
                    toolbar.top(),
                    "tag column starts alongside top rail"
                );
                assert_eq!(
                    column.left(),
                    toolbar.right(),
                    "column is outward of the WHOLE base browser"
                );
                assert_eq!(
                    column.bottom(),
                    preview.bottom(),
                    "column spans browser height"
                );
                assert!(column.right() > outer.left());
                assert_eq!(
                    visual.debug_bounds("browser-resize-handle").unwrap().left(),
                    column.right()
                );
                // No pointer movement: the original affordance must still be hit and close.
                visual.simulate_click(pointer, Modifiers::default());
                visual.run_until_parked();
                assert!(
                    visual.debug_bounds("browser-tag-editor").is_none(),
                    "second stationary click must close from {affordance}"
                );
                for (selector, expected) in STATIONARY.into_iter().zip(before) {
                    assert_eq!(
                        visual.debug_bounds(selector).unwrap(),
                        expected,
                        "closing moved {selector}"
                    );
                }
                assert_eq!(visual.debug_bounds("browser-resize-handle").unwrap(), outer);
            }
            editor.read_with(visual, |view, _| {
                assert_eq!(view.browser.playback_generation, generation);
                assert_eq!(
                    view.browser.preview_preparations, preparations,
                    "tag toggles must not reprepare sample metadata"
                );
                assert!(Rc::ptr_eq(
                    &metadata,
                    &view.browser.preview_cache[&key].metadata
                ));
                assert!(metadata.borrow().is_some());
                assert!(view.pending_requests.is_empty() && view.pending_audio_commands.is_empty());
            });
        }
    }
}

#[gpui::test]
fn browser_tag_editor_mini_strip_is_a_stationary_two_click_toggle(cx: &mut TestAppContext) {
    let (handle, _editor, _directory) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    let pointer = visual.debug_bounds("browser-control-⋯").unwrap().center();
    visual.simulate_click(pointer, Modifiers::default());
    assert!(visual.debug_bounds("browser-tag-editor").is_some());
    visual.simulate_click(pointer, Modifiers::default());
    assert!(
        visual.debug_bounds("browser-tag-editor").is_none(),
        "mini-strip toggle must close on second actual click at identical coordinates"
    );
}

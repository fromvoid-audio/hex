//! Rendered GPUI tests for quick collections, membership/filtering/missing-media,
//! accessible section icon picker, ordering, last-section guard, and persistence (#2672).

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
fn collections_membership_filtering_crud_and_missing_media_round_trip(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let dir = PreferencesTestDirectory::new();
    let config = dir.0.join("browser.json");
    let recent = dir.0.join("recent.json");
    let samples = dir.0.join("icon-home").join("Music");
    std::fs::create_dir_all(&samples).unwrap();
    let kick = samples.join("kick.wav");
    let snare = samples.join("snare.wav");
    std::fs::write(&kick, minimal_wav()).unwrap();
    std::fs::write(&snare, minimal_wav()).unwrap();

    let (handle, editor) = place_icon_preferences_window(cx, &dir, &config, &recent);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    // 1. Empty collection displays clear summary and empty guidance.
    let fav_row = visual
        .debug_bounds("browser-collection-0")
        .expect("favorites collection row");
    visual.simulate_mouse_down(fav_row.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-collection-summary").is_some());
    assert!(visual.debug_bounds("browser-collection-empty").is_some());

    // 2. Select kick in Samples and assign it to "favorites" via the tag editor checkbox.
    let catalog = scan_samples(std::slice::from_ref(&samples)).catalog;
    let listing = hex_library::scan_directory(&samples).unwrap();
    let kick_index = listing
        .entries
        .iter()
        .position(|entry| entry.path == kick)
        .expect("kick in listing");
    editor.update(&mut visual, |view, cx| {
        view.browser.directory = samples.clone();
        view.browser.listing = Some(listing);
        view.browser.section = BrowserSection::Place(samples.clone());
        view.browser.samples.replace(catalog);
        view.browser.samples.invalidate_listing();
        view.browser.selected_index = kick_index;
        cx.notify();
    });
    visual.run_until_parked();

    let toggle_btn = visual
        .debug_bounds("browser-control-⋯")
        .expect("tag editor toggle");
    visual.simulate_click(toggle_btn.center(), Modifiers::default());
    visual.run_until_parked();

    let assign_fav = visual
        .debug_bounds("browser-collection-assign-0")
        .expect("favorites assignment checkbox in tag editor");
    visual.simulate_mouse_down(assign_fav.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    // Switch back to favorites: kick appears as a live entry and empty placeholder disappears.
    let fav_row = visual.debug_bounds("browser-collection-0").unwrap();
    visual.simulate_mouse_down(fav_row.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-collection-empty").is_none());
    editor.read_with(&visual, |view, cx| {
        let entries = view.browser_visible_entries(cx);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, kick);
        assert_eq!(view.browser.samples.collection_counts("favorites"), (1, 0));
    });

    // 3. Missing-media item in a collection is counted truthfully without hiding available items.
    let ghost = samples.join("missing-loop.wav");
    editor.update(&mut visual, |view, cx| {
        view.toggle_sample_collection(&ghost, "favorites", cx)
            .unwrap();
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser.samples.collection_counts("favorites"), (1, 1));
        assert_eq!(view.browser_visible_entries(cx).len(), 1);
    });

    // 4. Create, rename (preserving members), recolor, reorder, and delete collections.
    editor.update(&mut visual, |view, cx| {
        let idx = view
            .create_browser_collection("drums", hex_project::ElementColor::Amber, cx)
            .unwrap();
        assert_eq!(idx, 3);
        view.toggle_sample_collection(&snare, "drums", cx).unwrap();
        view.rename_browser_collection(idx, "percussion", cx)
            .unwrap();
        assert_eq!(view.browser.samples.collection_counts("percussion"), (1, 0));
        assert!(view.recolor_browser_collection(idx, hex_project::ElementColor::Green, cx));
        // Active is Collection(0); moving 0 -> 2 tracks active section to Collection(2).
        assert!(view.reorder_browser_collection(0, 2, cx));
        assert_eq!(view.browser.section, BrowserSection::Collection(2));
        // Deleting the active collection falls back cleanly to a visible section.
        assert!(view.delete_browser_collection(2, cx));
        assert!(!matches!(
            view.browser.section,
            BrowserSection::Collection(_)
        ));
        cx.notify();
    });
    visual.run_until_parked();

    // 5. Restart restores custom collections and their colors from browser.json.
    let (reopened_handle, reopened) = place_icon_preferences_window(cx, &dir, &config, &recent);
    let reopened_visual = &mut VisualTestContext::from_window(reopened_handle, cx);
    reopened_visual.run_until_parked();
    reopened.read_with(reopened_visual, |view, _| {
        let names: Vec<&str> = view
            .browser
            .layout
            .collections
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["inspiration", "recent finds", "percussion"]);
        assert_eq!(
            view.browser.layout.collections[2].color,
            hex_project::ElementColor::Green
        );
        assert_eq!(view.browser.samples.collection_counts("percussion"), (1, 0));
    });
}

#[gpui::test]
fn sidebar_section_visibility_accessible_icon_picker_order_and_last_section_persist(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let dir = PreferencesTestDirectory::new();
    let config = dir.0.join("browser.json");
    let recent = dir.0.join("recent.json");
    let custom_place = dir.0.join("packs");
    std::fs::create_dir_all(&custom_place).unwrap();
    hex_preferences::save(
        &config,
        &hex_preferences::BrowserPlaces::new([custom_place.clone()]).unwrap(),
    )
    .unwrap();

    let (handle, editor) = place_icon_preferences_window(cx, &dir, &config, &recent);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    // Open gear settings.
    let gear = visual.debug_bounds("browser-control-⚙").unwrap();
    visual.simulate_click(gear.center(), Modifiers::default());
    visual.run_until_parked();

    // Hide Projects (index 2).
    let hide_projects = visual
        .debug_bounds("browser-section-visibility-2")
        .expect("projects visibility toggle");
    visual.simulate_click(hide_projects.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-section-projects").is_none());

    // Open accessible icon grid popup for Devices (index 1) and choose waveform (choice 6).
    let devices_icon = visual
        .debug_bounds("browser-section-icon-1")
        .expect("devices icon picker button");
    visual.simulate_mouse_down(
        devices_icon.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        devices_icon.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("context-menu-icon-grid").is_some(),
        "section icon button must open accessible bundled icon grid"
    );
    let waveform_choice = visual.debug_bounds("context-menu-choice-6").unwrap();
    visual.simulate_click(waveform_choice.center(), Modifiers::default());
    visual.run_until_parked();

    // Move Devices (index 1) up one slot in section order.
    let move_up = visual
        .debug_bounds("browser-section-move-up-1")
        .expect("move devices up");
    visual.simulate_mouse_down(move_up.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    // Verify saved preferences on disk keep existing custom place alongside sidebar config.
    let saved = hex_preferences::load(&config).unwrap();
    assert_eq!(saved.additional(), std::slice::from_ref(&custom_place));
    assert!(saved.sidebar_section_hidden(hex_preferences::BrowserSidebarSection::Projects));
    assert_eq!(
        saved.sidebar_section_icon(hex_preferences::BrowserSidebarSection::Devices),
        Some("waveform")
    );
    assert_eq!(
        saved.sidebar_order(),
        &[
            hex_preferences::BrowserSidebarSection::All,
            hex_preferences::BrowserSidebarSection::Devices,
            hex_preferences::BrowserSidebarSection::Samples,
            hex_preferences::BrowserSidebarSection::Projects,
            hex_preferences::BrowserSidebarSection::Plugins,
        ]
    );

    // Restart from disk: Projects stays hidden, Devices keeps waveform icon & moved above Samples.
    let (reopened_handle, reopened) = place_icon_preferences_window(cx, &dir, &config, &recent);
    let reopened_visual = &mut VisualTestContext::from_window(reopened_handle, cx);
    reopened_visual.run_until_parked();
    assert!(
        reopened_visual
            .debug_bounds("browser-section-projects")
            .is_none()
    );
    let devices_bounds = reopened_visual
        .debug_bounds("browser-section-devices")
        .unwrap();
    let samples_bounds = reopened_visual
        .debug_bounds("browser-section-samples")
        .unwrap();
    assert!(
        devices_bounds.top() < samples_bounds.top(),
        "reordered section must stay above samples after restart"
    );
    reopened.read_with(reopened_visual, |view, _| {
        assert_eq!(
            view.browser.layout.section_icon_path(1),
            "icons/browser-place/waveform.svg"
        );
    });

    // Guard last visible section: hiding 3 more sections leaves 1 visible; attempting to hide
    // the final visible section is refused cleanly.
    let gear = reopened_visual.debug_bounds("browser-control-⚙").unwrap();
    reopened_visual.simulate_click(gear.center(), Modifiers::default());
    reopened_visual.run_until_parked();
    for idx in [0, 1, 4] {
        let sel = format!("browser-section-visibility-{idx}");
        let bounds = reopened_visual
            .debug_bounds(Box::leak(sel.into_boxed_str()))
            .unwrap();
        reopened_visual.simulate_click(bounds.center(), Modifiers::default());
        reopened_visual.run_until_parked();
    }
    // Only All (index 3) remains visible; clicking its toggle must be refused.
    let last = reopened_visual
        .debug_bounds("browser-section-visibility-3")
        .unwrap();
    reopened_visual.simulate_click(last.center(), Modifiers::default());
    reopened_visual.run_until_parked();
    assert!(
        reopened_visual
            .debug_bounds("browser-section-all")
            .is_some(),
        "last visible section cannot be hidden"
    );
    // Keep compiler happy about editor binding used earlier.
    let _ = editor;
}

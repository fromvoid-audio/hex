use super::*;

#[gpui::test]
fn browser_search_owns_typing_and_blocks_editor_shortcuts(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            });
            editor.update(cx, |view, cx| {
                let project = Editor::new(Project::new(48_000).expect("test project"), 4)
                    .expect("test editor")
                    .project()
                    .clone();
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 0,
                    can_undo: false,
                    can_redo: false,
                    revision: 0,
                    unsaved: false,
                    document_path: None,
                }));
                view.browser
                    .search
                    .update(cx, |input, _| input.focus(window));
                cx.notify();
            });
            editor
        })
        .expect("editor window");

    cx.simulate_keystrokes(handle, "shift-a space cmd-shift-t");

    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "a ");
        assert!(
            view.pending_requests.is_empty(),
            "search focus must suppress add-track"
        );
        assert!(
            view.audio_pub.pending_audio_commands.is_empty(),
            "search focus must suppress transport actions"
        );
        assert_eq!(
            view.state
                .as_ref()
                .map(|state| state.project.tracks().len()),
            Some(0)
        );
    });
}

#[gpui::test]
fn device_browser_lists_registered_devices_and_drag_starts_without_track_focus(
    cx: &mut TestAppContext,
) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            });
            editor.update(cx, |view, cx| {
                let project = Project::new(48_000).expect("project");
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 0,
                    can_undo: false,
                    can_redo: false,
                    revision: 0,
                    unsaved: false,
                    document_path: None,
                }));
                view.browser.section = BrowserSection::Instruments;
                view.audio_detail.clip_view_open = false;
                view.device_panel.active = false;
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let instrument = visual
        .debug_bounds("browser-device-instrument-hex.instrument.sine")
        .expect("registered sine instrument row");
    assert!(
        visual
            .debug_bounds("browser-device-effect-hex.utility")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("browser-device-instrument-hex.instrument.operator")
            .is_some()
    );

    let down = point(
        instrument.origin.x + instrument.size.width / 2.0,
        instrument.origin.y + instrument.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    assert!(visual.debug_bounds("device-panel-empty").is_some());
    editor.read_with(cx, |view, _| {
        assert!(view.selected_track.is_none());
        assert!(
            view.audio_detail.clip_view_open,
            "browser drag reveals the closed pane"
        );
        assert!(
            view.device_panel.active,
            "browser drag selects device details"
        );
        let drag = view
            .device_panel
            .active_native_drag
            .as_ref()
            .expect("browser device drag begins without a focused track");
        assert!(matches!(
            drag.source,
            super::super::drag::NativeDeviceDragSource::Browser {
                class: hex_engine::NativeDeviceClass::Instrument,
                ..
            }
        ));
        assert!(drag.destination.get().is_none());
    });
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.active_native_drag.is_none());
        assert!(
            view.audio_detail.clip_view_open,
            "a no-target drop leaves the pane open"
        );
        assert!(view.device_panel.active);
        assert!(
            view.selected_track.is_none(),
            "drag does not invent a track"
        );
    });
}

#[gpui::test]
fn browser_device_drag_reveals_devices_and_preserves_clip_selection_on_cancel(
    cx: &mut TestAppContext,
) {
    let (project, track, _, clip) = super::super::super::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        super::super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.device_panel.active = false;
        view.audio_detail.clip_view_open = true;
        view.browser.section = BrowserSection::Instruments;
        cx.notify();
    });

    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("arrangement-audio-clip-view").is_some());
    let instrument = visual
        .debug_bounds("browser-device-instrument-hex.instrument.sine")
        .expect("registered sine instrument row");
    let down = point(instrument.center().x, instrument.center().y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();

    assert!(visual.debug_bounds("device-panel").is_some());
    editor.read_with(cx, |view, _| {
        assert!(view.audio_detail.clip_view_open);
        assert!(view.device_panel.active);
        assert_eq!(view.selected_track, Some(track));
        assert_eq!(view.selected_clip, Some(clip));
        assert!(view.device_panel.active_native_drag.is_some());
    });

    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.active_native_drag.is_none());
        assert!(
            view.audio_detail.clip_view_open,
            "cancel keeps the revealed pane open"
        );
        assert!(view.device_panel.active);
        assert_eq!(view.selected_track, Some(track));
        assert_eq!(view.selected_clip, Some(clip));
    });
}

#[gpui::test]
fn browser_project_open_queues_selected_path_without_second_click_confirmation(
    cx: &mut TestAppContext,
) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            });
            editor.update(cx, |view, cx| {
                let project = Editor::new(Project::new(48_000).expect("test project"), 4)
                    .expect("test editor")
                    .project()
                    .clone();
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: 0,
                    can_undo: true,
                    can_redo: false,
                    revision: 1,
                    unsaved: true,
                    document_path: None,
                }));
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    let first = std::path::PathBuf::from("/tmp/first.hex");
    let second = std::path::PathBuf::from("/tmp/second.hex");

    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| {
            view.open_browser_project(first.clone(), cx);
            assert_eq!(view.persistence.pending_open_path.as_ref(), Some(&first));
            assert!(view.pending_requests.is_empty());
        });
    })
    .expect("editor window");
    assert!(
        cx.has_pending_prompt(),
        "the first selection reaches confirmation"
    );
    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| {
            view.open_browser_project(second.clone(), cx);
            assert!(!view.persistence.opening_project);
            assert!(view.pending_requests.is_empty());
            assert!(
                view.error.is_some(),
                "another path cannot replace an active intent"
            );
        });
    })
    .expect("editor window");
    cx.simulate_prompt_answer("cancel");
    cx.run_until_parked();
    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.persistence.project_open.busy());
            assert!(view.persistence.pending_open_path.is_none());
            view.open_browser_project(second.clone(), cx);
            assert_eq!(view.persistence.pending_open_path.as_ref(), Some(&second));
            assert!(view.pending_requests.is_empty());
        });
    })
    .expect("editor window");
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("cancel");
    cx.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(!view.persistence.opening_project);
        assert!(!view.persistence.project_open.busy());
        assert!(view.persistence.pending_open_path.is_none());
        assert!(view.state.as_ref().unwrap().unsaved);
        assert!(view.pending_requests.is_empty());
    });
}

#[gpui::test]
fn dropping_browser_audio_on_pinned_main_cancels_import_and_track_creation(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _track, _directory, _path) = browser_drag_fixture(cx);
    let track_count = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.tracks().len()
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("sample browser entry");
    let main = visual
        .debug_bounds("pinned-main-track")
        .expect("pinned main row");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));

    let target = point(main.origin.x + px(20.0), main.center().y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
            .as_ref()
            .expect("browser drag remains captured");
        assert!(drag.destination.get().is_none());
        assert!(drag.preview.get().is_none());
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.tracks().len(),
            track_count
        );
        assert!(view.browser.active_audio_drag.is_none());
        assert!(view.test_probes.last_enqueued_import_to_track.is_none());
        assert!(view.test_probes.last_enqueued_import_to_new_track.is_none());
    });
}

#[gpui::test]
fn browser_audio_drag_uses_command_override_live_and_at_mouse_up(cx: &mut TestAppContext) {
    let (handle, editor, track, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual.debug_bounds("browser-entry-0").expect("wav row");
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual.debug_bounds(lane_selector).expect("target lane");
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, command);
    visual.simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, command);
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    let target = point(
        lane.origin.x + lane.size.width * 0.573,
        lane.origin.y + lane.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, command);
    visual.run_until_parked();
    for _ in 0..8 {
        if editor.read_with(cx, |view, _| {
            view.browser
                .active_audio_drag
                .as_ref()
                .is_some_and(|drag| drag.preview.get().is_some())
        }) {
            break;
        }
        visual.run_until_parked();
    }
    let pointer_start = editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
            .as_ref()
            .expect("active browser drag");
        assert!(drag.snap_modifier.get());
        let painted_lane = view
            .timeline_caches
            .drag_lanes
            .get(&track)
            .expect("painted drop lane");
        let fraction = ((f64::from(target.x) - f64::from(painted_lane.left))
            / f64::from(painted_lane.width))
        .clamp(0.0, 1.0);
        let expected_pointer_start = (fraction * drag.viewport_length as f64).floor() as u64;
        assert_eq!(
            drag.destination.get(),
            Some(BrowserDropDestination::ExistingTrack {
                track,
                start: expected_pointer_start,
            }),
            "held command bypasses adaptive snapping from initial drag through destination"
        );
        expected_pointer_start
    });
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    let snapped_start = editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
            .as_ref()
            .expect("active browser drag");
        assert!(!drag.snap_modifier.get());
        match drag.destination.get() {
            Some(BrowserDropDestination::ExistingTrack { start, .. }) => start,
            _ => panic!("browser drag remains over target lane"),
        }
    });
    assert_ne!(snapped_start, pointer_start);
    visual.simulate_mouse_up(target, MouseButton::Left, command);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.test_probes.last_enqueued_import_to_track,
            Some((path.clone(), track, pointer_start, true)),
            "mouse-up modifiers are authoritative even without a final pointer move"
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_drop_between_tracks_shows_only_insertion_line_and_keeps_snapped_start(
    cx: &mut TestAppContext,
) {
    let (handle, editor, tracks, _directory, path) =
        browser_drag_fixture_with_tracks(cx, &["first", "second"]);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let first_selector =
        Box::leak(format!("arrangement-lane-{}", tracks[0].value()).into_boxed_str());
    let second_selector =
        Box::leak(format!("arrangement-lane-{}", tracks[1].value()).into_boxed_str());
    let first = visual
        .debug_bounds(first_selector)
        .expect("first track lane");
    let second = visual
        .debug_bounds(second_selector)
        .expect("second track lane");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("arrangement body");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = point(
        first.origin.x + first.size.width * 0.73,
        first.origin.y + first.size.height,
    );
    assert_eq!(first.origin.x, body.origin.x);
    assert_eq!(
        first.size.width,
        body.size.width - editor.read_with(cx, |view, _| px(view.track_header_width()))
    );
    let pointer_start = (((f32::from(target.x) - f32::from(first.origin.x))
        / f32::from(first.size.width))
        * 384_000.0)
        .floor() as u64;
    let expected_pointer_start = editor.read_with(cx, |view, _| {
        view.snap_grid
            .snap_frame(
                view.state.as_ref().expect("project").project.as_ref(),
                pointer_start,
                view.visible_viewport(384_000),
                f32::from(first.size.width),
            )
            .expect("adaptive insertion start")
    });
    assert!((f32::from(target.y) - f32::from(second.origin.y)).abs() <= 1.0);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let expected = editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(drag.preview.get().is_none());
        let Some(BrowserDropDestination::NewTrack {
            before_track,
            start,
        }) = drag.destination.get()
        else {
            panic!("gap should target a newly inserted track");
        };
        assert!(drag.lane_hovered.get());
        assert_eq!(before_track, Some(tracks[1]));
        assert_eq!(
            start, expected_pointer_start,
            "drop start follows the painted lane's pointer position"
        );
        assert!(drag.insertion_line_y.get().is_some());
        start
    });
    let line = visual
        .debug_bounds("track-insert-preview")
        .expect("full-width insertion line");
    assert_eq!(line.size.width, body.size.width);
    assert!(visual.debug_bounds("clip-drop-preview").is_none());
    assert!(
        visual.debug_bounds("append-track-drop-highlight").is_none(),
        "between-track insertion must not highlight the rows below"
    );
    let ghost = visual
        .debug_bounds("browser-audio-floating-preview-hidden")
        .expect("drag ghost node remains mounted but collapsed");
    assert_eq!(ghost.size.width, px(0.0));
    assert_eq!(ghost.size.height, px(0.0));

    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.test_probes.last_enqueued_import_to_new_track,
            Some((path.clone(), Some(tracks[1]), expected)),
            "release uses the same stable anchor and snapped start as the preview"
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_drop_below_last_track_appends_with_line_and_locked_ghost(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual.debug_bounds(lane_selector).expect("last track lane");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("arrangement body");
    assert!(
        body.size.height > lane.size.height,
        "Arrangement has blank space below last row: body={:?}, lane={:?}",
        body,
        lane
    );
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let arrangement = editor.read_with(cx, |view, _| {
        view.timeline_caches
            .arrangement_bounds
            .expect("arrangement bounds")
    });
    let target = point(
        lane.origin.x + lane.size.width * 0.62,
        px(arrangement.top + arrangement.height - 8.0),
    );
    assert!(
        editor.read_with(cx, |view, _| view
            .painted_track_insertion_target(f32::from(target.x), f32::from(target.y))
            .is_some()),
        "last painted track should expose the blank area below it: target={target:?}, body={body:?}, lane={lane:?}, bounds={:?}/{}, lane_epoch={:?}/{}, lanes={:?}",
        editor.read_with(cx, |view, _| view.timeline_caches.arrangement_bounds),
        editor.read_with(cx, |view, _| view.timeline_caches.arrangement_bounds_epoch),
        editor.read_with(cx, |view, _| view
            .timeline_caches
            .drag_lane_epochs
            .get(&track)
            .copied()),
        editor.read_with(cx, |view, _| view.timeline_caches.drag_layout_generation),
        editor.read_with(cx, |view, _| view.timeline_caches.drag_lanes.clone()),
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let start = editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
        let Some(BrowserDropDestination::NewTrack {
            before_track: None,
            start,
        }) = drag.destination.get()
        else {
            panic!("blank area under the final track should append");
        };
        assert!(view.browser_audio_drag_is_current(drag));
        assert!(drag.lane_hovered.get());
        start
    });
    let line = visual
        .debug_bounds("track-insert-preview")
        .expect("append insertion line");
    assert_eq!(line.size.width, body.size.width);
    editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(matches!(
            drag.destination.get(),
            Some(BrowserDropDestination::NewTrack {
                before_track: None,
                ..
            })
        ));
        assert!(view.browser_audio_drag_is_current(drag));
        assert!(
            view.browser
                .active_audio_drag
                .as_ref()
                .filter(|drag| view.browser_audio_drag_is_current(drag))
                .is_some_and(|drag| matches!(
                    drag.destination.get(),
                    Some(BrowserDropDestination::NewTrack {
                        before_track: None,
                        ..
                    })
                ))
        );
        assert!(
            view.state()
                .is_some_and(|state| !state.project.tracks().is_empty())
        );
        let last = view.state().unwrap().project.tracks().last().unwrap().id();
        assert_eq!(
            view.timeline_caches.drag_lane_epochs.get(&last),
            Some(&view.timeline_caches.drag_layout_generation)
        );
        let lane = view.timeline_caches.drag_lanes[&last];
        let bounds = view.timeline_caches.arrangement_bounds.unwrap();
        assert!(
            lane.top + lane.height < bounds.top + bounds.height,
            "lane={lane:?} body={bounds:?}"
        );
        assert_eq!(drag.insertion_line_y.get(), Some(lane.top + lane.height));
    });
    assert!(visual.debug_bounds("append-track-drop-highlight").is_none());
    let line = visual
        .debug_bounds("track-insert-preview")
        .expect("bottom insertion line");
    assert!((line.origin.y - (lane.origin.y + lane.size.height - px(1.0))).abs() <= px(1.0));
    let ghost = visual
        .debug_bounds("browser-audio-append-preview")
        .expect("blank-space append locks the sample ghost to the new row");
    assert!(ghost.size.width > px(3.0));
    let (expected_left, expected_width, expected_top) = editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
            .as_ref()
            .expect("active drag");
        let metadata = drag
            .metadata
            .borrow()
            .clone()
            .expect("prepared sample duration");
        let viewport = view.visible_viewport(384_000);
        let timeline_width = body.size.width - px(view.track_header_width());
        let left = body.origin.x
            + px(
                (start.saturating_sub(viewport.start) as f64 / viewport.length as f64
                    * f64::from(timeline_width)) as f32,
            );
        let width = px((metadata.playback_frames as f64 / viewport.length as f64
            * f64::from(timeline_width)) as f32);
        (left, width, lane.origin.y + lane.size.height)
    });
    assert!((ghost.origin.x - expected_left).abs() <= px(1.0));
    assert!((ghost.size.width - expected_width).abs() <= px(1.0));
    assert!((ghost.origin.y - expected_top).abs() <= px(1.0));
    let body_after = visual
        .debug_bounds("arrangement-content")
        .expect("Arrangement body remains mounted");
    assert_eq!(
        body_after.origin, body.origin,
        "drag must not shift the workspace"
    );
    assert_eq!(
        body_after.size, body.size,
        "drag must not resize the workspace"
    );
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.test_probes.last_enqueued_import_to_new_track,
            Some((path.clone(), None, start))
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_drop_in_empty_arrangement_creates_bottom_track(cx: &mut TestAppContext) {
    let (handle, editor, tracks, _directory, path) = browser_drag_fixture_with_tracks(cx, &[]);
    assert!(tracks.is_empty());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("empty arrangement body");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = point(
        body.origin.x + body.size.width * 0.4,
        body.origin.y + body.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let start = editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
        let Some(BrowserDropDestination::NewTrack {
            before_track: None,
            start,
        }) = drag.destination.get()
        else {
            panic!("empty Arrangement should target a bottom track");
        };
        start
    });
    assert!(
        visual.debug_bounds("track-insert-preview").is_none(),
        "empty Arrangement shows the clip without a cursor-following line"
    );
    assert!(visual.debug_bounds("empty-track-drop-highlight").is_none());
    let ghost = visual
        .debug_bounds("browser-audio-append-preview")
        .expect("empty Arrangement locks the sample ghost to the first row");
    assert!(ghost.size.width > px(3.0));
    assert!(
        (ghost.origin.x
            - body.origin.x
            - px((start as f64 / 384_000.0
                * f64::from(
                    body.size.width - editor.read_with(cx, |view, _| px(view.track_header_width()))
                )) as f32))
        .abs()
            <= px(1.0)
    );
    assert!((ghost.origin.y - body.origin.y).abs() <= px(1.0));
    // The pointer-down audition is still playing when the sample lands.
    editor.update(cx, |view, _| {
        view.browser.playback_phase = hex_audio::sample_preview::Phase::Playing;
        view.browser.playback_output = Some((1, 1));
        view.browser.playback_path = Some(path.clone());
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.test_probes.last_enqueued_import_to_new_track,
            Some((path.clone(), None, start))
        );
        assert!(view.browser.playback_output.is_none(), "drop stops preview");
        assert_eq!(
            view.browser.playback_phase,
            hex_audio::sample_preview::Phase::Idle
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_sample_gap_target_uses_only_currently_painted_rows_after_scroll(
    cx: &mut TestAppContext,
) {
    let names = (0..24)
        .map(|index| format!("track {index}"))
        .collect::<Vec<_>>();
    let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let (handle, editor, tracks, _directory, _) = browser_drag_fixture_with_tracks(cx, &name_refs);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let arrangement = visual
        .debug_bounds("arrangement-content")
        .expect("painted arrangement body");
    let pan_start = point(
        arrangement.origin.x + px(500.0),
        arrangement.origin.y + px(250.0),
    );
    visual.simulate_mouse_down(pan_start, MouseButton::Middle, Modifiers::default());
    visual.simulate_mouse_move(
        point(pan_start.x, pan_start.y - px(1_100.0)),
        MouseButton::Middle,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(pan_start.x, pan_start.y - px(1_100.0)),
        MouseButton::Middle,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let (first_track, second_track, target) = editor.update(cx, |view, _| {
        let generation = view.timeline_caches.drag_layout_generation;
        let tracks = &view.state.as_ref().expect("project").project.tracks();
        let pair = tracks.windows(2).find(|pair| {
            view.timeline_caches.drag_lane_epochs.get(&pair[0].id()) == Some(&generation)
                && view.timeline_caches.drag_lane_epochs.get(&pair[1].id()) == Some(&generation)
        });
        let pair = pair.expect("at least two adjacent rows are painted");
        let first_lane = view.timeline_caches.drag_lanes[&pair[0].id()];
        let second_lane = view.timeline_caches.drag_lanes[&pair[1].id()];
        let pointer_x = first_lane.left + first_lane.width * 0.62;
        let pointer_y = first_lane.top + first_lane.height;
        assert!(
            view.timeline_caches.drag_lane_epochs[&tracks[0].id()] < generation,
            "first row should be offscreen after scroll; epochs={:?}, generation={generation}, lanes={:?}, arrangement={:?}",
            view.timeline_caches.drag_lane_epochs,
            view.timeline_caches.drag_lanes,
            view.timeline_caches.arrangement_bounds
        );
        view.timeline_caches.drag_lanes.insert(tracks[0].id(), second_lane);
        let insertion = view
            .painted_track_insertion_target(pointer_x, pointer_y)
            .expect("visible gap remains a valid insertion target");
        assert_eq!(insertion.0, Some(pair[1].id()));
        (
            pair[0].id(),
            pair[1].id(),
            point(px(pointer_x), px(pointer_y)),
        )
    });
    assert_ne!(first_track, tracks[0]);

    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .browser
            .active_audio_drag
            .as_ref()
            .expect("browser drag active");
        assert!(matches!(
            drag.destination.get(),
            Some(BrowserDropDestination::NewTrack {
                before_track: Some(track),
                ..
            }) if track == second_track
        ));
        assert!(drag.insertion_line_y.get().is_some());
    });
    assert!(visual.debug_bounds("track-insert-preview").is_some());
}

#[gpui::test]
fn empty_arrangement_keeps_the_grid_aligned_with_the_ruler(cx: &mut TestAppContext) {
    let (handle, editor, tracks, _directory, _) = browser_drag_fixture_with_tracks(cx, &[]);
    assert!(tracks.is_empty());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("empty Arrangement body");
    let ruler = visual
        .debug_bounds("timeline-ruler")
        .expect("empty Arrangement ruler");
    let grid = visual
        .debug_bounds("arrangement-grid")
        .expect("empty Arrangement grid");
    let line = visual
        .debug_bounds("arrangement-grid-line-0")
        .expect("empty Arrangement first grid line");
    assert!(
        visual.debug_bounds("arrangement-grid-band-0").is_some(),
        "four-adaptive-unit shading spans the empty Arrangement timeline"
    );
    assert_eq!(grid.origin.x, body.origin.x);
    assert_eq!(
        grid.size.width + editor.read_with(cx, |view, _| px(view.track_header_width())),
        body.size.width
    );
    assert_eq!(line.origin.x, ruler.origin.x);
}

#[gpui::test]
fn middle_drag_pans_both_arrangement_axes_until_release(cx: &mut TestAppContext) {
    let names = (0..32)
        .map(|index| format!("track {index}"))
        .collect::<Vec<_>>();
    let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let (handle, editor, _, _directory, _) = browser_drag_fixture_with_tracks(cx, &name_refs);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        view.viewport = Some(super::super::super::viewport::Viewport {
            start: 96_000,
            length: 192_000,
        });
    });
    visual.run_until_parked();
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("painted arrangement body");
    let start = point(body.origin.x + px(400.0), body.origin.y + px(100.0));
    let initial = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        view.visible_viewport(state.timeline_frames)
    });
    visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
    visual.simulate_mouse_move(
        point(start.x - px(90.0), start.y - px(180.0)),
        MouseButton::Middle,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let (panned, active, scroll_offset, scroll_max) = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        (
            view.visible_viewport(state.timeline_frames),
            view.interaction.arrangement_pan.is_some(),
            view.track_scroll.offset(),
            view.track_scroll.max_offset(),
        )
    });
    let timeline_width =
        f32::from(body.size.width - editor.read_with(cx, |view, _| px(view.track_header_width())));
    let expected_delta = (90.0 * initial.length as f64 / f64::from(timeline_width)).round() as i128;
    assert_eq!(
        panned,
        initial.pan_by_frames(384_000, expected_delta),
        "the held move outside Arrangement pans by the measured timeline width"
    );
    assert!(
        scroll_offset.y < px(0.0),
        "vertical drag updates the list scroll handle; max={scroll_max:?}"
    );
    assert!(active);

    let outside = point(start.x - px(90.0), px(-18.0));
    visual.simulate_mouse_move(
        point(outside.x, outside.y - px(10_000.0)),
        MouseButton::Middle,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let bottom_offset = editor.read_with(cx, |view, _| {
        let handle = view.track_scroll.clone();
        (handle.offset().y, handle.max_offset().height)
    });
    assert_eq!(
        bottom_offset.0, -bottom_offset.1,
        "vertical pan clamps at the last row"
    );
    visual.simulate_mouse_move(outside, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    let top_offset = editor.read_with(cx, |view, _| view.track_scroll.offset().y);
    assert_eq!(top_offset, px(0.0), "vertical pan clamps at the first row");
    visual.simulate_mouse_up(outside, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    let after_release = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        (
            view.visible_viewport(state.timeline_frames),
            view.track_scroll.offset(),
            view.interaction.arrangement_pan.is_some(),
        )
    });
    assert_eq!(
        after_release,
        (panned, point(scroll_offset.x, top_offset), false)
    );
    visual.simulate_mouse_move(
        point(outside.x - px(30.0), outside.y - px(30.0)),
        None,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let after_unpressed_move = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        (
            view.visible_viewport(state.timeline_frames),
            view.track_scroll.offset(),
        )
    });
    assert_eq!(
        after_unpressed_move,
        (panned, point(scroll_offset.x, top_offset))
    );
}

#[gpui::test]
fn browser_audio_drag_release_outside_arrangement_cancels_import(cx: &mut TestAppContext) {
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-entry-0")
        .expect("rendered wav browser row");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let outside = point(source.origin.x + px(24.0), source.origin.y + px(8.0));
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.test_probes.last_enqueued_import_to_track.is_none());
        assert!(view.browser.active_audio_drag.is_none());
    });
    visual.run_until_parked();
}

#[gpui::test]
fn browser_audio_drag_rejects_changed_snap_or_viewport(cx: &mut TestAppContext) {
    let (_handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    editor.update(cx, |view, _| {
        let state = view.state.as_ref().expect("project state");
        let viewport = view.visible_viewport(state.timeline_frames);
        view.snap_grid = super::super::super::musical_time::SnapGrid::Quarter;
        let payload = BrowserAudioDrag {
            project_asset: None,
            project_generation: view.project_generation,
            revision: state.revision,
            browser_generation: Some(view.browser.scan_generation()),
            sample_rate: state.project.sample_rate(),
            path,
            label: "loop.wav".into(),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: view.snap_grid,
            snap_modifier: Rc::new(std::cell::Cell::new(false)),
            copy_modifier: Rc::new(std::cell::Cell::new(false)),
            metadata: Rc::new(RefCell::new(None)),
            metadata_error: Rc::new(RefCell::new(None)),
            preview: Rc::new(std::cell::Cell::new(None)),
            destination: Rc::new(std::cell::Cell::new(None)),
            insertion_line_y: Rc::new(std::cell::Cell::new(None)),
            lane_hovered: Rc::new(std::cell::Cell::new(false)),
            last_pointer: Rc::new(std::cell::Cell::new(None)),
        };
        view.begin_browser_audio_drag(&payload);
        assert!(view.browser_audio_drag_is_current(&payload));
        view.snap_grid = super::super::super::musical_time::SnapGrid::Off;
        assert!(!view.browser_audio_drag_is_current(&payload));
        view.snap_grid = payload.snap_grid;
        view.viewport = Some(super::super::super::viewport::Viewport {
            start: 0,
            length: viewport.length / 2,
        });
        assert!(!view.browser_audio_drag_is_current(&payload));
    });
}

#[gpui::test]
fn clip_and_sample_drags_clear_unavailable_targets_and_recover_from_invalid_pointer(
    cx: &mut TestAppContext,
) {
    use hex_session::service::AudioPreview;
    use hex_session::{CachedPeak as Peak, CachedWaveform as Waveform};
    use std::cell::Cell;

    let (handle, editor, track, _directory, path) = browser_drag_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (return_id, browser_drag, clip_drag) = editor.update(cx, |view, cx| {
        let current = Arc::clone(view.state.as_ref().expect("project state"));
        let mut project = Editor::new((*current.project).clone(), 8).expect("editor");
        let Created::ReturnTrack(return_id) = project
            .execute(Command::AddReturnTrack {
                name: "send bus".into(),
            })
            .expect("return track")
        else {
            panic!("return track expected")
        };
        let Created::Clip(clip_id) = project
            .execute(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("clip length"),
            })
            .expect("midi clip")
        else {
            panic!("midi clip expected")
        };
        let clip = project.project().midi_clip(clip_id).expect("clip");
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(project.project().clone()),
            waveforms: current.waveforms.clone(),
            source_overviews: current.source_overviews.clone(),
            audio_clip_waveforms: Default::default(),
            midi_preview: current.midi_preview.clone(),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: current.timeline_frames,
            can_undo: current.can_undo,
            can_redo: current.can_redo,
            revision: current.revision.saturating_add(1),
            unsaved: current.unsaved,
            document_path: current.document_path.clone(),
        }));
        let viewport = view.visible_viewport(current.timeline_frames);
        let browser_drag = BrowserAudioDrag {
            project_asset: None,
            project_generation: view.project_generation,
            revision: current.revision.saturating_add(1),
            browser_generation: Some(view.browser.scan_generation()),
            sample_rate: current.project.sample_rate(),
            path,
            label: "loop.wav".into(),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: view.snap_grid,
            snap_modifier: Rc::new(Cell::new(false)),
            copy_modifier: Rc::new(Cell::new(false)),
            metadata: Rc::new(RefCell::new(Some(Rc::new(AudioPreview {
                playback_frames: 480,
                sample_rate: current.project.sample_rate(),
                waveform: Arc::new(
                    Waveform::from_peaks(
                        480,
                        vec![
                            [Peak {
                                min: -0.2,
                                max: 0.2
                            }; 2];
                            48
                        ],
                    )
                    .expect("sample waveform"),
                ),
            })))),
            metadata_error: Rc::new(RefCell::new(None)),
            preview: Rc::new(Cell::new(None)),
            destination: Rc::new(Cell::new(None)),
            insertion_line_y: Rc::new(Cell::new(None)),
            lane_hovered: Rc::new(Cell::new(false)),
            last_pointer: Rc::new(Cell::new(None)),
        };
        let clip_drag = ClipDrag {
            group: Default::default(),
            project_generation: view.project_generation,
            revision: current.revision.saturating_add(1),
            source_track: track,
            clip: clip_id,
            position: ClipPosition::Midi {
                start: clip.start(),
                length: clip.length(),
            },
            grab_offset: Rc::new(Cell::new(Some(0))),
            snap_edge: Rc::new(Cell::new(Some(super::super::drag::ClipSnapEdge::Leading))),
            source_span: Default::default(),
            capture: Rc::new(Cell::new(None)),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: view.snap_grid,
            snap_modifier: Rc::new(Cell::new(false)),
            copy_modifier: Rc::new(Cell::new(false)),
            preview: Rc::new(Cell::new(None)),
            destination: Rc::new(Cell::new(None)),
            insertion_line_y: Rc::new(Cell::new(None)),
            ghost_label: "clip".into(),
            ghost_waveform: None,
            lane_hovered: Rc::new(Cell::new(false)),
            last_pointer: Rc::new(Cell::new(None)),
        };
        cx.notify();
        (return_id, browser_drag, clip_drag)
    });
    visual.run_until_parked();
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual.debug_bounds(lane_selector).expect("track lane");
    let return_selector =
        Box::leak(format!("return-track-row-{}", return_id.value()).into_boxed_str());
    let return_row = visual
        .debug_bounds(return_selector)
        .expect("pinned return row");
    let lane_point = point(lane.origin.x + px(120.0), lane.center().y);
    let return_point = point(lane_point.x, return_row.center().y);

    editor.update(cx, |view, _| {
        view.begin_browser_audio_drag(&browser_drag);
        let (preview, _) = view.browser_audio_drop_position(
            &browser_drag,
            f32::from(lane_point.x),
            f32::from(lane_point.y),
        );
        assert!(
            preview.is_some(),
            "sample should preview over a regular track"
        );
        assert!(matches!(
            browser_drag.destination.get(),
            Some(BrowserDropDestination::ExistingTrack { track: target, .. }) if target == track
        ));
        browser_drag.preview.set(preview);
        let (preview, lane_hovered) = view.browser_audio_drop_position(
            &browser_drag,
            f32::from(return_point.x),
            f32::from(return_point.y),
        );
        browser_drag.preview.set(preview);
        assert!(preview.is_none());
        assert!(!lane_hovered);
        assert!(browser_drag.destination.get().is_none());
        assert!(browser_drag.insertion_line_y.get().is_none());

        view.begin_clip_drag(&clip_drag);
        let (preview, _) = view.clip_drag_drop_position(
            &clip_drag,
            f32::from(lane_point.x),
            f32::from(lane_point.y),
        );
        assert!(
            preview.is_some(),
            "clip should preview over a regular track"
        );
        assert_eq!(
            clip_drag.destination.get(),
            Some(ClipDropDestination::ExistingTrack(track))
        );
        for (invalid, origin) in [
            (f32::NAN, None),
            (f32::INFINITY, None),
            (f32::NEG_INFINITY, None),
            (-f32::MAX, Some(f32::MAX)),
        ] {
            for target_y in [lane_point.y, lane.top()] {
                clip_drag.capture.set(None);
                let (valid, hovered) = view.clip_drag_drop_position(
                    &clip_drag,
                    f32::from(lane_point.x),
                    f32::from(target_y),
                );
                assert!(hovered);
                assert!(clip_drag.destination.get().is_some());
                if target_y == lane_point.y {
                    assert!(valid.is_some());
                } else {
                    assert!(matches!(
                        clip_drag.destination.get(),
                        Some(ClipDropDestination::NewTrack { .. })
                    ));
                    assert!(clip_drag.insertion_line_y.get().is_some());
                }
                clip_drag.capture.set(origin.and_then(|x| {
                    super::super::drag::PointerDown::begin(
                        super::super::drag::ClipPointerKey {
                            generation: clip_drag.project_generation,
                            clip: clip_drag.clip,
                        },
                        hex_widgets::RectanglePart::Body,
                        x,
                        true,
                    )
                }));
                let held = clip_drag.capture.get();
                let (rejected, hovered) =
                    view.clip_drag_drop_position(&clip_drag, invalid, f32::from(target_y));
                assert!(rejected.is_none() && !hovered);
                assert!(
                    clip_drag.destination.get().is_none(),
                    "invalid pointer must clear the previous target"
                );
                assert!(clip_drag.insertion_line_y.get().is_none());
                assert_eq!(clip_drag.capture.get(), held, "retain the finite press");
                let (recovered, hovered) = view.clip_drag_drop_position(
                    &clip_drag,
                    f32::from(lane_point.x),
                    f32::from(lane_point.y),
                );
                assert!(recovered.is_some() && hovered);
                assert_eq!(
                    clip_drag.destination.get(),
                    Some(ClipDropDestination::ExistingTrack(track))
                );
            }
        }
        clip_drag.preview.set(preview);
        let (preview, lane_hovered) = view.clip_drag_drop_position(
            &clip_drag,
            f32::from(return_point.x),
            f32::from(return_point.y),
        );
        clip_drag.preview.set(preview);
        assert!(preview.is_none());
        assert!(!lane_hovered);
        assert!(clip_drag.destination.get().is_none());
        assert!(clip_drag.insertion_line_y.get().is_none());
    });
}

#[gpui::test]
fn browser_restores_missing_user_folder_without_touching_real_preferences(cx: &mut TestAppContext) {
    use gpui::VisualTestContext;
    let directory = PreferencesTestDirectory::new();
    let preferences_path = directory.0.join("browser.json");
    let saved =
        hex_preferences::BrowserPlaces::new([directory.0.join("folder-that-is-currently-missing")])
            .expect("saved missing folder");
    hex_preferences::save(&preferences_path, &saved).expect("write isolated preferences");
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test(
                    window,
                    cx,
                    service,
                    Err("offline test".into()),
                    preferences_path.clone(),
                    directory.0.join("recent-projects.json"),
                )
            })
        })
        .expect("editor window");
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.browser.places.contains(&saved.additional()[0]));
        assert!(view.browser.preferences_writable);
    });
    assert_eq!(
        hex_preferences::load(&preferences_path).expect("preserved saved places"),
        saved
    );
}

#[gpui::test]
fn browser_preferences_coalesce_background_saves_and_preserve_corrupt_files(
    cx: &mut TestAppContext,
) {
    use gpui::VisualTestContext;
    let directory = PreferencesTestDirectory::new();
    let preferences_path = directory.0.join("browser.json");
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test(
                    window,
                    cx,
                    service,
                    Err("offline test".into()),
                    preferences_path.clone(),
                    directory.0.join("recent-projects.json"),
                )
            })
        })
        .expect("editor window");
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let first = directory.0.join("first-added-folder");
    let second = directory.0.join("second-added-folder");
    editor.update(cx, |view, cx| {
        view.browser.places.push(first.clone());
        view.queue_browser_places_save(cx);
        view.browser.places.push(second.clone());
        view.queue_browser_places_save(cx);
    });
    visual.run_until_parked();
    let saved = hex_preferences::load(&preferences_path).expect("coalesced save");
    assert_eq!(saved.additional(), &[first.clone(), second.clone()]);

    let corrupt = b"not valid browser preferences";
    std::fs::write(&preferences_path, corrupt).expect("replace with corrupt fixture");
    editor.update(cx, |view, cx| {
        view.browser.places.push(directory.0.join("third-folder"));
        view.queue_browser_places_save(cx);
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.browser
                .places_status
                .as_deref()
                .is_some_and(|status| status.contains("could not save"))
        );
    });
    assert_eq!(
        std::fs::read(&preferences_path).expect("corrupt file remains"),
        corrupt
    );
}

#[gpui::test]
fn browser_places_failed_save_survives_a_later_queued_success(cx: &mut TestAppContext) {
    use gpui::VisualTestContext;
    let directory = PreferencesTestDirectory::new();
    let corrupt_path = directory.0.join("corrupt.json");
    let good_path = directory.0.join("browser.json");
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test(
                    window,
                    cx,
                    service,
                    Err("offline test".into()),
                    corrupt_path.clone(),
                    directory.0.join("recent-projects.json"),
                )
            })
        })
        .expect("editor window");
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // Corrupt preferences are preserved, so the first save fails.
    std::fs::write(&corrupt_path, b"not valid browser preferences").expect("corrupt fixture");
    let first = directory.0.join("first-folder");
    let second = directory.0.join("second-folder");
    editor.update(cx, |view, cx| {
        view.browser.places.push(first.clone());
        view.queue_browser_places_save(cx);
        // The queued save targets a writable file and succeeds.
        view.browser.set_test_preferences_path(good_path.clone());
        view.browser.places.push(second.clone());
        view.queue_browser_places_save(cx);
    });
    visual.run_until_parked();
    assert_eq!(
        hex_preferences::load(&good_path)
            .expect("queued save")
            .additional(),
        &[first, second]
    );
    editor.read_with(cx, |view, _| {
        let status = view.browser.places_status.as_deref().unwrap_or_default();
        assert!(
            status.contains("could not save browser places")
                && status.contains("a later save succeeded"),
            "earlier failure must stay visible after a queued success: {status:?}"
        );
    });
}

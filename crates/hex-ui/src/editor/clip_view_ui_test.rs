use super::{EditorMode, clip_drag_ui_test};
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::Command;
use hex_session::{CachedPeak, CachedWaveform, service::ViewState};
use std::{collections::BTreeMap, sync::Arc};

#[gpui::test]
fn midi_clip_view_toggle_preserves_selection_and_moves_focus_out_of_hidden_roll(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    let (project, track, _, clip) = clip_drag_ui_test::midi_project();
    let note = project
        .midi_source(clip)
        .and_then(|source| source.notes().first())
        .expect("fixture note")
        .id();
    let (handle, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, window_cx| {
        editor.update(window_cx, |view, cx| {
            view.selected_track = Some(track);
            view.selected_clip = Some(clip);
            view.midi_selection.set_single(note);
            view.midi_marquee = Some(([1.0, 1.0], [2.0, 2.0], false));
            view.midi_focus.focus(window);
            view.clip_view_open = true;
            cx.notify();
        });
    })
    .expect("focus midi editor");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("midi-source-controls").is_some());
    assert!(visual.debug_bounds("midi-note-roll").is_some());
    let open_panel = visual.debug_bounds("clip-view").expect("open detail panel");
    assert!(f32::from(open_panel.size.height) >= 213.0);

    let toggle = visual
        .debug_bounds("clip-view-toggle")
        .expect("clip view toggle");
    let toggle_point = point(toggle.center().x, toggle.center().y);
    visual.simulate_mouse_down(toggle_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(!view.clip_view_open, "toggle click should hide detail")
    });
    let collapsed_panel = visual
        .debug_bounds("clip-view")
        .expect("collapsed detail panel");
    assert!(f32::from(collapsed_panel.size.height) < f32::from(open_panel.size.height));
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_clip, Some(clip));
        assert_eq!(view.midi_selection.primary(), Some(note));
        assert!(!view.clip_view_open);
        assert!(view.midi_marquee.is_none());
    });
    cx.update_window(handle, |_, window, cx| {
        editor.read_with(cx, |view, _| assert!(!view.midi_focus.is_focused(window)));
    })
    .expect("read editor focus");

    visual.simulate_keystrokes("right");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.midi_selection.primary(), Some(note));
        assert!(!matches!(
            view.last_enqueued_edit,
            Some(Command::UpdateMidiNote { .. } | Command::UpdateMidiNotes { .. })
        ));
    });

    let toggle = visual
        .debug_bounds("clip-view-toggle")
        .expect("collapsed toggle");
    let toggle_point = point(toggle.center().x, toggle.center().y);
    visual.simulate_mouse_down(toggle_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let reopened_panel = visual
        .debug_bounds("clip-view")
        .expect("reopened detail panel");
    assert_eq!(reopened_panel.size.height, open_panel.size.height);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_clip, Some(clip));
        assert_eq!(view.midi_selection.primary(), Some(note));
        assert!(view.clip_view_open);
    });
}

fn audio_detail_fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<super::EditorView>,
    hex_project::TrackId,
    hex_project::ClipId,
) {
    let (project, track, _, clip) = clip_drag_ui_test::audio_project();
    let asset = project.clip(clip).expect("clip").asset();
    let peaks = Arc::new(
        CachedWaveform::from_peaks(
            96_000,
            vec![
                [
                    CachedPeak {
                        min: -0.6,
                        max: 0.7
                    },
                    CachedPeak {
                        min: -0.4,
                        max: 0.5
                    },
                ];
                64
            ],
        )
        .expect("cached peaks"),
    );
    let (handle, editor) = clip_drag_ui_test::editor_window(cx, project.clone(), Arc::default());
    editor.update(cx, |view, window_cx| {
        let mut waveforms = BTreeMap::new();
        waveforms.insert(asset, peaks);
        view.state = Some(Arc::new(ViewState {
            project: Arc::new(project),
            waveforms,
            midi_preview: Arc::new(Default::default()),
            timeline_frames: 384_000,
            can_undo: false,
            can_redo: false,
            revision: 1,
            unsaved: false,
            document_path: None,
        }));
        view.mode = EditorMode::Arrangement;
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.clip_view_open = true;
        window_cx.notify();
    });
    (handle, editor, track, clip)
}

#[gpui::test]
fn selected_arrangement_audio_shows_left_controls_ruler_and_stereo_waveform(
    cx: &mut TestAppContext,
) {
    let (handle, _, _, _) = audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let controls = visual
        .debug_bounds("arrangement-audio-controls")
        .expect("audio controls column");
    let waveform = visual
        .debug_bounds("arrangement-audio-waveform")
        .expect("detailed waveform panel");
    let ruler = visual
        .debug_bounds("arrangement-audio-time-ruler")
        .expect("source time ruler");
    let play_start = visual
        .debug_bounds("audio-clip-playstart-value")
        .expect("actual play-start value");
    assert!(f32::from(controls.size.width) > 0.0);
    assert!(f32::from(waveform.size.width) > 0.0);
    assert!(f32::from(waveform.size.height) > 0.0);
    assert!(f32::from(ruler.size.width) > 0.0);
    assert!(f32::from(play_start.size.width) > 0.0);
    assert!(visual.debug_bounds("source-start-marker").is_some());
    assert!(visual.debug_bounds("play-start-marker").is_some());
}

#[gpui::test]
fn open_clip_detail_keeps_its_body_height_after_selection_is_removed(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let populated = visual
        .debug_bounds("clip-view")
        .expect("populated detail pane");

    editor.update(cx, |view, window_cx| {
        view.selected_clip = None;
        window_cx.notify();
    });
    visual.run_until_parked();

    let empty = visual.debug_bounds("clip-view").expect("empty detail pane");
    let empty_body = visual
        .debug_bounds("clip-detail-empty")
        .expect("truthful empty detail body");
    assert_eq!(empty.size.height, populated.size.height);
    assert!(f32::from(empty_body.size.height) >= 213.0);
    editor.read_with(cx, |view, _| {
        assert!(
            view.clip_view_open,
            "losing a selection does not close the pane"
        );
        assert!(
            !view.device_panel_active,
            "empty clip context stays truthful"
        );
    });
}

#[gpui::test]
fn explicit_clip_and_track_focus_switch_detail_without_changing_pane_visibility(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, clip) = audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let track_selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    let track_name = visual
        .debug_bounds(track_selector)
        .expect("focused track name");
    let track_point = point(track_name.center().x, track_name.center().y);
    visual.simulate_mouse_down(track_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(track_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel_active, "track focus selects devices");
        assert!(view.clip_view_open, "track focus keeps the pane open");
        assert_eq!(view.selected_track, Some(track));
        assert_eq!(
            view.selected_clip, None,
            "track focus clears stale clip selection"
        );
    });
    assert!(visual.debug_bounds("device-panel").is_some());

    let clip_selector = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    let clip_bounds = visual
        .debug_bounds(clip_selector)
        .expect("arrangement clip");
    let clip_point = point(clip_bounds.center().x, clip_bounds.center().y);
    visual.simulate_mouse_down(clip_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(clip_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            !view.device_panel_active,
            "clip focus returns to clip details"
        );
        assert!(view.clip_view_open, "clip focus keeps the pane open");
        assert_eq!(view.selected_clip, Some(clip));
    });
    assert!(visual.debug_bounds("arrangement-audio-waveform").is_some());

    let toggle = visual
        .debug_bounds("clip-view-toggle")
        .expect("detail toggle");
    let toggle_point = point(toggle.center().x, toggle.center().y);
    visual.simulate_mouse_down(toggle_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(!view.clip_view_open));

    let track_name = visual
        .debug_bounds(track_selector)
        .expect("track name while closed");
    let track_point = point(track_name.center().x, track_name.center().y);
    visual.simulate_mouse_down(track_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(track_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel_active);
        assert!(
            !view.clip_view_open,
            "automatic mode change does not reopen a closed pane"
        );
    });
    let toggle = visual
        .debug_bounds("clip-view-toggle")
        .expect("closed detail toggle");
    let toggle_point = point(toggle.center().x, toggle.center().y);
    visual.simulate_mouse_down(toggle_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.clip_view_open, "explicit reopen remains available");
        assert!(
            view.device_panel_active,
            "reopen uses the most recent track focus"
        );
    });
    assert!(visual.debug_bounds("device-panel").is_some());
}

#[gpui::test]
fn native_device_panel_renders_registered_controls_and_routes_chain_actions(
    cx: &mut TestAppContext,
) {
    let (base, track, _, _) = clip_drag_ui_test::midi_project();
    let catalog = hex_engine::native_device_catalog().expect("built-in catalog");
    let entry = catalog.first().expect("registered utility");
    let device_state =
        hex_engine::default_native_device_state(entry.type_id).expect("valid default state");
    let mut editor = hex_project::Editor::new(base, 16).expect("project editor");
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: device_state.clone(),
        })
        .expect("insert utility");
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: device_state,
        })
        .expect("insert second utility");
    let project = editor.project().clone();
    let project_track = project.track(track).unwrap();
    let instrument_id = project_track
        .instrument()
        .expect("new track has canonical sine instrument")
        .id();
    let devices = project_track.devices();
    let device_id = devices[0].id();
    let second_device_id = devices[1].id();
    let parameter_id = entry.parameters[0].id;
    let mut committed_edit =
        hex_project::Editor::new(project.clone(), 16).expect("valid device panel project");
    committed_edit
        .execute(Command::RemoveDevice(device_id))
        .expect("remove utility from committed project");
    let project_after_remove = Arc::new(committed_edit.project().clone());
    let (handle, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel_active = false;
        view.clip_view_open = true;
        view.mode = EditorMode::Session;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("clip-detail-tab").is_some());
    assert!(visual.debug_bounds("device-detail-tab").is_some());
    editor.read_with(cx, |view, _| assert!(!view.device_panel_active));
    let tab = visual
        .debug_bounds("device-detail-tab")
        .expect("device tab");
    let tab_point = point(tab.center().x, tab.center().y);
    visual.simulate_mouse_down(tab_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(tab_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let panel = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel.size.height, px(213.0));
    assert!(visual.debug_bounds("device-panel-header").is_some());
    assert!(visual.debug_bounds("device-add-toggle").is_some());
    let chain = visual
        .debug_bounds("device-chain-scroll")
        .expect("device chain viewport");
    let instrument_card = visual
        .debug_bounds(Box::leak(
            format!("instrument-device-{}", instrument_id.value()).into_boxed_str(),
        ))
        .expect("instrument card");
    let effect_card = visual
        .debug_bounds(Box::leak(
            format!("effect-device-{}", device_id.value()).into_boxed_str(),
        ))
        .expect("effect card");
    assert_eq!(instrument_card.size.height, chain.size.height - px(6.0));
    assert_eq!(effect_card.size.height, instrument_card.size.height);
    assert_eq!(instrument_card.size.width, px(286.0 + 25.0));
    assert_eq!(effect_card.size.width, px(144.0 + 25.0));
    assert!(
        instrument_card.origin.x < effect_card.origin.x,
        "instrument is first"
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("instrument-replace-{}", instrument_id.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("instrument-remove-{}", instrument_id.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-up-{}", instrument_id.value()).into_boxed_str()
            ))
            .is_none(),
        "instruments do not show serial reorder controls"
    );
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel.has_only_host_control_for(instrument_id),
            "the analytic sine instrument exposes only its host output trim"
        );
    });
    let parameter_selector = Box::leak(
        format!(
            "device-{}-parameter-{}",
            device_id.value(),
            parameter_id.value()
        )
        .into_boxed_str(),
    );
    assert!(
        visual.debug_bounds(parameter_selector).is_some(),
        "generic parameter control should render"
    );

    let last_parameter = entry.parameters.last().expect("utility parameters");
    let last_parameter_selector = Box::leak(
        format!(
            "device-{}-parameter-{}",
            device_id.value(),
            last_parameter.id.value()
        )
        .into_boxed_str(),
    );
    let device_body = visual
        .debug_bounds(Box::leak(
            format!("device-body-{}", device_id.value()).into_boxed_str(),
        ))
        .expect("bounded device controls viewport");
    let last_control = visual
        .debug_bounds(last_parameter_selector)
        .expect("last utility control is rendered");
    assert!(
        last_control.origin.y >= device_body.origin.y
            && last_control.origin.y + last_control.size.height
                <= device_body.origin.y + device_body.size.height,
        "the compact Utility panel keeps its final control visible in the fixed body viewport"
    );

    let panel_before_menu = visual.debug_bounds("device-panel").expect("device panel");
    let chain_before_menu = visual
        .debug_bounds("device-chain-scroll")
        .expect("device chain viewport");
    let add = visual
        .debug_bounds("device-add-toggle")
        .expect("add device toggle");
    let click_point = point(add.center().x, add.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_some());
    let panel_with_menu = visual.debug_bounds("device-panel").expect("device panel");
    let chain_with_menu = visual
        .debug_bounds("device-chain-scroll")
        .expect("device chain viewport");
    let instrument_with_menu = visual
        .debug_bounds(Box::leak(
            format!("instrument-device-{}", instrument_id.value()).into_boxed_str(),
        ))
        .expect("instrument frame with menu");
    let effect_with_menu = visual
        .debug_bounds(Box::leak(
            format!("effect-device-{}", device_id.value()).into_boxed_str(),
        ))
        .expect("effect frame with menu");
    assert_eq!(panel_before_menu, panel_with_menu);
    assert_eq!(chain_before_menu, chain_with_menu);
    assert_eq!(instrument_card, instrument_with_menu);
    assert_eq!(effect_card, effect_with_menu);
    let add_utility_selector = Box::leak(
        format!("add-native-effect-{}", entry.type_id.as_str().unwrap()).into_boxed_str(),
    );
    let add_utility = visual
        .debug_bounds(add_utility_selector)
        .expect("registered utility add action");
    let click_point = point(add_utility.center().x, add_utility.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_native_device_add,
            Some((track, hex_engine::NativeDeviceClass::Effect, entry.type_id)),
            "add menu dispatches the registered class and type to the selected track"
        );
    });

    let sine =
        hex_engine::native_device_catalog_for_class(hex_engine::NativeDeviceClass::Instrument)
            .expect("instrument catalog")
            .into_iter()
            .next()
            .expect("registered sine instrument");
    let replace =
        Box::leak(format!("instrument-replace-{}", instrument_id.value()).into_boxed_str());
    let replace = visual
        .debug_bounds(replace)
        .expect("replace instrument action");
    let click_point = point(replace.center().x, replace.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let add_sine = Box::leak(
        format!("add-native-instrument-{}", sine.type_id.as_str().unwrap()).into_boxed_str(),
    );
    let add_sine = visual
        .debug_bounds(add_sine)
        .expect("registered sine replace action");
    let click_point = point(add_sine.center().x, add_sine.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_enqueued_native_device_replace,
            Some((
                instrument_id,
                hex_engine::NativeDeviceClass::Instrument,
                sine.type_id,
            )),
            "replace action targets the stable instrument ID and its explicit class"
        );
    });

    let bypass_selector =
        Box::leak(format!("device-bypass-{}", device_id.value()).into_boxed_str());
    let bypass = visual.debug_bounds(bypass_selector).expect("bypass action");
    let click_point = point(bypass.center().x, bypass.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit,
            Some(Command::SetDeviceBypass {
                device,
                bypassed: true
            }) if device == device_id
        ));
    });

    let second_up_selector =
        Box::leak(format!("device-up-{}", second_device_id.value()).into_boxed_str());
    let second_up = visual
        .debug_bounds(second_up_selector)
        .expect("move second device left");
    let click_point = point(second_up.center().x, second_up.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit,
            Some(Command::MoveDevice {
                device,
                track: moved_track,
                before: Some(before),
            }) if device == second_device_id && moved_track == track && before == device_id
        ));
    });

    let remove_selector =
        Box::leak(format!("device-remove-{}", device_id.value()).into_boxed_str());
    let remove = visual
        .debug_bounds(remove_selector)
        .expect("remove first device");
    let click_point = point(remove.center().x, remove.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    // The test editor only records commands. Apply the committed removal to its
    // view snapshot so the next render no longer contains this device.
    editor.update(cx, |view, cx| {
        let previous = view.state.as_ref().expect("initial view state");
        view.state = Some(Arc::new(ViewState {
            project: Arc::clone(&project_after_remove),
            waveforms: previous.waveforms.clone(),
            midi_preview: Arc::clone(&previous.midi_preview),
            timeline_frames: previous.timeline_frames,
            can_undo: true,
            can_redo: previous.can_redo,
            revision: previous.revision + 1,
            unsaved: true,
            document_path: previous.document_path.clone(),
        }));
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(remove_selector).is_none(),
        "removed device selector must not retain bounds from the previous frame"
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("effect-device-{}", device_id.value()).into_boxed_str()
            ))
            .is_none(),
        "removed device frame must not remain in rendered debug bounds"
    );
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit,
            Some(Command::RemoveDevice(device)) if device == device_id
        ));
    });

    let tab = visual.debug_bounds("clip-detail-tab").expect("clip tab");
    let tab_point = point(tab.center().x, tab.center().y);
    visual.simulate_mouse_down(tab_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(tab_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(!view.device_panel_active));
}

#[gpui::test]
fn status_slots_keep_layout_stable_while_edits_and_work_switch_pending(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = |visual: &mut VisualTestContext, selector| {
        visual.debug_bounds(selector).expect("status slot bounds")
    };
    let before = [
        bounds(&mut visual, "editor-status-message"),
        bounds(&mut visual, "editor-revision-status"),
        bounds(&mut visual, "editor-activity-status"),
        bounds(&mut visual, "editor-unsaved-status"),
        bounds(&mut visual, "editor-audio-diagnostics"),
    ];
    editor.update(cx, |view, cx| {
        view.worker_requests_pending = 1;
        view.export_pending = true;
        view.saving_project = true;
        view.error = Some("audio device is unavailable".into());
        view.applied_project_revision = Some(0);
        cx.notify();
    });
    visual.run_until_parked();
    let after = [
        bounds(&mut visual, "editor-status-message"),
        bounds(&mut visual, "editor-revision-status"),
        bounds(&mut visual, "editor-activity-status"),
        bounds(&mut visual, "editor-unsaved-status"),
        bounds(&mut visual, "editor-audio-diagnostics"),
    ];
    for (before, after) in before.iter().zip(after.iter()) {
        assert_eq!(
            before.size, after.size,
            "status updates keep slot widths fixed"
        );
        assert_eq!(
            before.origin, after.origin,
            "status updates do not shift neighbors"
        );
    }
    assert!(
        f32::from(after[0].size.width) >= 100.0,
        "device errors retain visible space at the compact window width"
    );
}

#[gpui::test]
fn audio_detail_dispatches_canonical_edits_and_rejects_stale_or_pending_clicks(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, clip) = audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual
        .debug_bounds("audio-clip-position-1")
        .expect("move clip control");
    let point = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit,
            Some(Command::MoveClip { clip: actual, track: actual_track, start })
                if actual == clip && actual_track == track && start == 48
        ));
    });

    // The callback still owns the rendered revision. A newer project revision
    // must invalidate its captured control instead of overwriting that edit.
    editor.update(cx, |view, _| {
        view.last_enqueued_edit = None;
        let old = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            project: Arc::clone(&old.project),
            waveforms: old.waveforms.clone(),
            midi_preview: Arc::clone(&old.midi_preview),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: old.revision + 1,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
    });
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));

    editor.update(cx, |view, _| {
        let old = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            project: Arc::clone(&old.project),
            waveforms: old.waveforms.clone(),
            midi_preview: Arc::clone(&old.midi_preview),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: 1,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
        view.worker_requests_pending = 1;
    });
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
}

#[gpui::test]
fn audio_gain_dispatches_real_clip_edit_and_rejects_stale_revision(cx: &mut TestAppContext) {
    let (handle, editor, _, clip) = audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual
        .debug_bounds("audio-clip-gain-1")
        .expect("gain control");
    let click = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.last_enqueued_edit, Some(Command::SetAudioClipGain { clip: id, millidb: 1_000 }) if id == clip));
    });
    editor.update(cx, |view, _| {
        view.last_enqueued_edit = None;
        let old = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            project: Arc::clone(&old.project),
            waveforms: old.waveforms.clone(),
            midi_preview: Arc::clone(&old.midi_preview),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: old.revision + 1,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
    });
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
}

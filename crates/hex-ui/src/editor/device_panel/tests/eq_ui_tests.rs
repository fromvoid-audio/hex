use super::*;
use hex_engine::eq::{EQ_TYPE_ID, EqState};
fn fixture(legacy: bool) -> (Service, Arc<ViewState>, TrackId, DeviceId) {
    let service = Service::new(48000, 16, 8000000).unwrap();
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "eq source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(track) = event.created.unwrap() else {
        panic!()
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(if legacy {
                hex_lattice_contract::DeviceTypeId::from_static("hex.eq8")
            } else {
                EQ_TYPE_ID
            }),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).unwrap().devices()[0].id();
    (service, state, track, device)
}
/// Option on a blank press keeps the held create-and-shape gesture; a plain
/// blank press creates and only a shift blank drag draws the marquee.
const OPTION_CREATE: Modifiers = Modifiers {
    control: false,
    alt: true,
    shift: false,
    platform: false,
    function: false,
};
fn read(view: &EditorView, device: DeviceId) -> EqState {
    EqState::decode(
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state(),
    )
    .unwrap()
}
fn settled(view: &EditorView) -> bool {
    view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.device_gesture_barriers.is_empty()
        && view.device_gesture.snapshot().is_none()
}
// Seed already-authored data through the canonical command, as in an opened project.
// This is fixture setup, not a reachable EQ-local editing route.
fn seed_existing_lane(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    device: DeviceId,
) {
    editor.update(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let entry = project.device(device).unwrap();
        let expected_state = entry.state().clone();
        let expected_variant = entry.active_variant();
        let mut state = EqState::decode(&expected_state).unwrap();
        state
            .set_lane(hex_engine::eq::AutomationLane {
                node: state.nodes[0].id,
                parameter: hex_engine::eq::NodeParameter::Gain,
                points: vec![hex_engine::eq::AutomationPoint {
                    beat: 0,
                    normalized: 0.75,
                }],
            })
            .unwrap();
        let request = Request::Edit(Command::ReplaceDeviceStateWithIdentityReservation {
            device,
            expected_state,
            expected_variant,
            state: state.encode().unwrap(),
            expected_next_id: project.next_identity(),
            reserved_ids: 0,
        });
        assert!(view.enqueue(request));
    });
    pump_until(editor, cx, |v| {
        settled(v) && read(v, device).lanes.len() == 1
    });
}
fn selector(prefix: &str, device: DeviceId) -> &'static str {
    Box::leak(format!("{prefix}-{}", device.value()).into_boxed_str())
}
fn plot(
    visual: &mut VisualTestContext,
    device: DeviceId,
    x: f32,
    y: f32,
) -> gpui::Point<gpui::Pixels> {
    let plot = visual.debug_bounds(Box::leak(
        format!("eq-response-{}-plot", device.value()).into_boxed_str(),
    ));
    let bounds = plot.unwrap_or_else(|| {
        visual
            .debug_bounds(selector("eq-response", device))
            .expect("rendered eq graph")
    });
    let inset = if plot.is_some() { px(0.0) } else { px(7.0) };
    let location = point(
        bounds.origin.x + inset + (bounds.size.width - inset * 2.0) * x,
        bounds.origin.y + inset + (bounds.size.height - inset * 2.0) * (1.0 - y),
    );
    let viewport = visual.update(|window, _| window.viewport_size());
    assert!(
        location.x >= px(0.0)
            && location.x < viewport.width
            && location.y >= px(0.0)
            && location.y < viewport.height,
        "test plot point {location:?} lies outside viewport {viewport:?}; graph bounds: {bounds:?}"
    );
    location
}

fn assert_action_label_fits(
    visual: &mut VisualTestContext,
    label: &str,
    bounds: gpui::Bounds<gpui::Pixels>,
) {
    let width = visual.update(|window, _| {
        window
            .text_system()
            .shape_line(
                label.to_owned().into(),
                px(8.0),
                &[gpui::TextRun {
                    len: label.len(),
                    font: gpui::font(theme::FONT_FAMILY),
                    color: gpui::rgb(0xffffff).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width
    });
    assert!(
        width + px(6.0) <= bounds.size.width,
        "full rendered {label:?} ({width:?}) plus padding must fit its action {bounds:?}"
    );
}

#[gpui::test]
fn eq_backspace_removes_only_the_focused_band_and_preserves_history(cx: &mut TestAppContext) {
    eq_remove_key_history(cx, "backspace");
}

#[gpui::test]
fn eq_x_removes_only_the_focused_band_and_preserves_history(cx: &mut TestAppContext) {
    eq_remove_key_history(cx, "x");
}

fn eq_remove_key_history(cx: &mut TestAppContext, key: &str) {
    cx.update(super::super::super::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let position = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_click(position, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    visual.run_until_parked();
    seed_existing_lane(&editor, cx, device);
    visual.run_until_parked();
    let original = editor.read_with(cx, |view, _| read(view, device));
    let position = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_click(position, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes(key);
    pump_until(&editor, cx, |view| {
        settled(view)
            && (view.state.as_ref().unwrap().project.track(track).is_none()
                || read(view, device).nodes.is_empty())
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.state.as_ref().unwrap().project.track(track).is_some(),
            "EQ Backspace must never delete its track"
        );
        assert!(read(view, device).nodes.is_empty());
        assert!(
            read(view, device).lanes.is_empty(),
            "node removal atomically removes its automation"
        );
    });
    visual.simulate_keystrokes(key);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.state.as_ref().unwrap().project.track(track).is_some(),
            "empty focused EQ consumes Backspace"
        )
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == original
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_some()
        )
    });
    let removed = editor.read_with(cx, |view, _| read(view, device));
    assert_eq!(removed.next_node_id, original.next_node_id);
    let path = std::env::temp_dir().join(format!(
        "hex-eq-backspace-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SaveNew(path.clone())))
    });
    pump_until(&editor, cx, |view| settled(view) && path.exists());
    let reload = Service::new(48000, 8, 8000000).unwrap();
    let _ = receive_service_event(&reload);
    reload.submit(Request::Open(path.clone())).unwrap();
    let reopened = receive_service_event(&reload);
    assert!(reopened.error.is_none());
    assert!(reopened.state.project.track(track).is_some());
    assert_eq!(
        EqState::decode(reopened.state.project.device(device).unwrap().state()).unwrap(),
        removed
    );
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn eq_backspace_refuses_pending_work_and_keeps_numeric_editing_local(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let position = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_click(position, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    visual.run_until_parked();
    let original = editor.read_with(cx, |view, _| read(view, device));
    editor.update(cx, |view, _| view.worker_requests_pending = 1);
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        view.worker_requests_pending = 0;
        assert_eq!(read(view, device), original);
        assert!(view.pending_requests.is_empty());
    });
    let id = original.nodes[0].id;
    enter_typed_value(
        &mut visual,
        device,
        id.parameter(hex_engine::eq::NodeParameter::Frequency)
            .unwrap(),
        "1234",
    );
    visual.simulate_keystrokes("backspace enter");
    pump_until(&editor, cx, |view| {
        settled(view) && (read(view, device).nodes[0].frequency_hz - 123.0).abs() < 0.01
    });
    editor.read_with(cx, |view, _| {
        let state = read(view, device);
        assert_eq!(state.nodes.len(), 1);
        assert_eq!(state.nodes[0].id, id);
        assert!(view.state.as_ref().unwrap().project.track(track).is_some());
    });
    // A held creation is a preview, not a removable canonical node. The key
    // must neither commit that preview nor escape into editor deletion.
    let blank = plot(&mut visual, device, 0.8, 0.6);
    visual.simulate_mouse_down(blank, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("backspace escape");
    visual.simulate_mouse_up(blank, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| assert_eq!(read(view, device).nodes.len(), 1));
}

#[gpui::test]
fn eq_layout_contains_full_controls_without_overlap_at_supported_widths(cx: &mut TestAppContext) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let position = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_click(position, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    let node = editor.read_with(cx, |view, _| read(view, device).nodes[0].id);
    for width in [880.0, 1262.0, 1500.0] {
        visual.simulate_resize(size(px(width), px(650.0)));
        visual.run_until_parked();
        editor.update(cx, |view, cx| {
            view.device_panel
                .chain_window
                .scroll
                .set_offset(point(px(-600.0), px(0.0)));
            cx.notify();
        });
        visual.run_until_parked();
        let sidebar = visual.debug_bounds(selector("eq-sidebar", device)).unwrap();
        let graph = visual
            .debug_bounds(selector("eq-response", device))
            .unwrap();
        assert!(graph.right() <= sidebar.left());
        assert!(graph.size.height >= px(150.0));
        let mut names = vec![
            format!(
                "eq-control-{}-{}",
                device.value(),
                hex_engine::eq::MODE.value()
            ),
            format!(
                "eq-control-{}-{}",
                device.value(),
                hex_engine::eq::OUTPUT_GAIN.value()
            ),
        ];
        for field in [
            hex_engine::eq::NodeParameter::Enabled,
            hex_engine::eq::NodeParameter::Kind,
            hex_engine::eq::NodeParameter::Frequency,
            hex_engine::eq::NodeParameter::Gain,
            hex_engine::eq::NodeParameter::Q,
            hex_engine::eq::NodeParameter::Slope,
        ] {
            names.push(format!(
                "eq-control-{}-{}",
                device.value(),
                node.parameter(field).unwrap().value()
            ));
        }
        for i in 0..5 {
            names.push(format!("eq-bank-{}-{i}", device.value()));
        }
        for prefix in [
            "eq8-spectrum",
            "eq-spectrum-input",
            "eq-audition",
            "eq-remove",
        ] {
            names.push(format!("{prefix}-{}", device.value()));
        }
        let mut placed: Vec<gpui::Bounds<gpui::Pixels>> = Vec::new();
        for name in names {
            let bounds = visual
                .debug_bounds(Box::leak(name.clone().into_boxed_str()))
                .expect("actual control");
            assert!(
                bounds.left() >= sidebar.left()
                    && bounds.right() <= sidebar.right()
                    && bounds.top() >= sidebar.top()
                    && bounds.bottom() <= sidebar.bottom(),
                "{width}px {name} {bounds:?} must fit {sidebar:?}"
            );
            for other in &placed {
                assert!(
                    bounds.left() >= other.right()
                        || bounds.right() <= other.left()
                        || bounds.top() >= other.bottom()
                        || bounds.bottom() <= other.top(),
                    "overlap {name} {bounds:?} with{other:?}"
                );
            }
            placed.push(bounds);
        }
        for (prefix, label) in [
            ("eq-remove", "remove"),
            ("eq-spectrum-input", "input"),
            ("eq-audition", "listen"),
        ] {
            let bounds = visual.debug_bounds(selector(prefix, device)).unwrap();
            assert_action_label_fits(&mut visual, label, bounds);
        }
        for field in [
            hex_engine::eq::NodeParameter::Frequency,
            hex_engine::eq::NodeParameter::Gain,
            hex_engine::eq::NodeParameter::Q,
        ] {
            let id = node.parameter(field).unwrap();
            let key = Box::leak(
                format!("device-{}-parameter-{}-value", device.value(), id.value())
                    .into_boxed_str(),
            );
            let field_bounds = visual.debug_bounds(key).unwrap();
            assert!(
                field_bounds.size.width >= px(100.0),
                "full formatted number and units"
            );
        }
        assert!(visual.debug_bounds(selector("eq-auto", device)).is_none());
        assert!(
            visual
                .debug_bounds(selector("eq-automation-toolbar", device))
                .is_none()
        );
    }
}

#[gpui::test]
fn eq_blank_press_drags_edge_cuts_and_bell_as_one_history_edit(cx: &mut TestAppContext) {
    creation_history_and_round_trip(cx, CreationOwner::Track);
}

#[gpui::test]
fn main_eq_catalog_creation_drag_history_and_round_trip(cx: &mut TestAppContext) {
    creation_history_and_round_trip(cx, CreationOwner::Main);
}

#[gpui::test]
fn return_eq_creation_drag_history_and_round_trip(cx: &mut TestAppContext) {
    creation_history_and_round_trip(cx, CreationOwner::Return);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CreationOwner {
    Track,
    Main,
    Return,
}

fn creation_history_and_round_trip(cx: &mut TestAppContext, owner: CreationOwner) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, mut state, track, mut device) = fixture(false);
    if owner != CreationOwner::Track {
        service
            .submit(Request::Edit(Command::AddReturnTrack {
                name: "return sentinel".into(),
            }))
            .unwrap();
        state = receive_service_event(&service).state;
        service
            .submit(Request::Edit(Command::InsertReturnDevice {
                return_track: state.project.return_tracks()[0].id(),
                before: None,
                state: EqState::default().encode().unwrap(),
            }))
            .unwrap();
        state = receive_service_event(&service).state;
    }
    let sentinel = Arc::clone(&state);
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    if owner != CreationOwner::Track {
        editor.update(&mut visual, |view, cx| {
            view.mode = crate::editor::EditorMode::Arrangement;
            cx.notify();
        });
    }
    visual.run_until_parked();
    if owner != CreationOwner::Track {
        let return_id = sentinel.project.return_tracks()[0].id();
        let focus = match owner {
            CreationOwner::Main => "main-track-header".to_owned(),
            CreationOwner::Return => format!("return-track-focus-{}", return_id.value()),
            CreationOwner::Track => unreachable!(),
        };
        let header = visual
            .debug_bounds(Box::leak(focus.into_boxed_str()))
            .unwrap();
        visual.simulate_click(
            if owner == CreationOwner::Main {
                point(header.left() + px(14.), header.center().y)
            } else {
                header.center()
            },
            Modifiers::default(),
        );
        visual.run_until_parked();
        if owner == CreationOwner::Main {
            let add = visual.debug_bounds("device-add-toggle").unwrap();
            visual.simulate_click(add.center(), Modifiers::default());
            visual.run_until_parked();
            let eq = visual.debug_bounds("add-main-effect-hex.eq").unwrap();
            visual.simulate_click(eq.center(), Modifiers::default());
            pump_until(&editor, cx, |view| {
                settled(view) && view.state.as_ref().unwrap().project.main_devices().len() == 1
            });
        }
        device = editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let entry = if owner == CreationOwner::Main {
                assert!(view.selected_track.is_none());
                assert!(view.focused_return.is_none());
                assert_eq!(
                    view.device_panel.main_selected,
                    Some(view.project_generation)
                );
                &project.main_devices()[0]
            } else {
                assert_eq!(view.focused_return, Some(return_id));
                assert_eq!(view.selected_track, Some(track));
                &project.return_tracks()[0].devices()[0]
            };
            assert_eq!(entry.state().type_id(), EQ_TYPE_ID);
            entry.id()
        });
        visual.run_until_parked();
    }
    for (index, (x, y, kind, hint)) in [
        (0.02, 0.4, hex_engine::eq::NodeKind::LowCut, "low-cut"),
        (0.98, 0.4, hex_engine::eq::NodeKind::HighCut, "high-cut"),
        (0.5, 0.4, hex_engine::eq::NodeKind::Bell, "bell"),
        (0.5, 0.02, hex_engine::eq::NodeKind::Notch, "notch"),
    ]
    .into_iter()
    .enumerate()
    {
        let start = plot(&mut visual, device, x, y);
        visual.simulate_mouse_move(start, None, Modifiers::default());
        visual.run_until_parked();
        let hint_selector = Box::leak(
            format!("eq-response-{}-creation-hint-{hint}", device.value()).into_boxed_str(),
        );
        assert!(
            visual.debug_bounds(hint_selector).is_some(),
            "actual {hint} creation indicator before press"
        );
        let moved = plot(&mut visual, device, 0.6, 0.7);
        visual.simulate_mouse_down(start, MouseButton::Left, OPTION_CREATE);
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(hint_selector).is_none(),
            "a captured creation owns the pointer, not its hover hint"
        );
        editor.read_with(cx, |v, _| {
            assert_eq!(
                read(v, device).nodes.len(),
                index,
                "held creation cannot commit canonical state"
            );
            assert!(
                v.device_gesture.snapshot().is_some(),
                "held creation owns its keyed preview"
            );
        });
        assert!(
            visual.debug_bounds(selector("eq-remove", device)).is_some(),
            "the provisional stable node is visibly selected"
        );
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        // A move after release, including one reporting a held button, must not
        // attach itself to the new node when asynchronous publication arrives.
        let late = plot(&mut visual, device, 0.8, 0.2);
        visual.simulate_mouse_move(late, Some(MouseButton::Left), Modifiers::default());
        pump_until(&editor, cx, |v| {
            settled(v) && read(v, device).nodes.len() == index + 1
        });
        visual.run_until_parked();
        let authored = editor.read_with(cx, |v, _| read(v, device));
        let node = authored.nodes[index];
        assert_eq!(
            node.kind, kind,
            "creation shape follows the press zone, not the release"
        );
        assert!((node.frequency_hz - 20.0 * 1000.0_f32.powf(0.6)).abs() < 0.1);
        if kind == hex_engine::eq::NodeKind::Bell {
            assert!((node.gain_db - 9.6).abs() < 0.001);
        } else {
            assert_eq!(node.gain_db, 0.0);
            assert!((node.q - 0.1 * 180.0_f32.powf(0.7)).abs() < 0.001);
        }
        visual.simulate_mouse_move(late, Some(MouseButton::Left), Modifiers::default());
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_until(&editor, cx, |v| {
            settled(v) && read(v, device).nodes.len() == index
        });
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Redo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == authored);
        visual.run_until_parked();
    }
    if owner != CreationOwner::Track {
        let before_parameter = editor.read_with(cx, |view, _| read(view, device));
        let id = before_parameter.nodes[0].id;
        // Undo may retire detail selection. Select the first authored node again,
        // just as a user must, before editing its numeric controls.
        let node = plot(&mut visual, device, 0.6, 0.7);
        visual.simulate_click(node, Modifiers::default());
        visual.run_until_parked();
        enter_typed_value(
            &mut visual,
            device,
            id.parameter(hex_engine::eq::NodeParameter::Frequency)
                .unwrap(),
            "1234",
        );
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, |view| {
            settled(view) && (read(view, device).nodes[0].frequency_hz - 1234.).abs() < 0.01
        });
        let changed = editor.read_with(cx, |view, _| read(view, device));
        assert_eq!(changed.nodes.len(), before_parameter.nodes.len());
        assert_eq!(changed.nodes[0].id, id);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == before_parameter
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == changed
        });
    }
    let authored = editor.read_with(cx, |v, _| read(v, device));
    assert!(
        authored
            .nodes
            .windows(2)
            .all(|pair| pair[0].id < pair[1].id)
    );
    let path = std::env::temp_dir().join(format!(
        "hex-eq-gesture-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump_until(&editor, cx, |v| settled(v) && path.exists());
    let reload = Service::new(48000, 8, 8000000).unwrap();
    let _ = receive_service_event(&reload);
    reload.submit(Request::Open(path.clone())).unwrap();
    let reopened = receive_service_event(&reload);
    assert!(reopened.error.is_none());
    assert_eq!(
        EqState::decode(reopened.state.project.device(device).unwrap().state()).unwrap(),
        authored
    );
    if owner != CreationOwner::Track {
        editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(project.tracks(), sentinel.project.tracks());
            if owner == CreationOwner::Main {
                assert_eq!(project.return_tracks(), sentinel.project.return_tracks());
                assert!(view.selected_track.is_none());
                assert!(view.focused_return.is_none());
                assert_eq!(
                    view.device_panel.main_selected,
                    Some(view.project_generation)
                );
            } else {
                assert_eq!(project.main_devices(), sentinel.project.main_devices());
                assert_eq!(
                    view.focused_return,
                    Some(sentinel.project.return_tracks()[0].id())
                );
                assert_eq!(view.selected_track, Some(track));
            }
            assert_eq!(reopened.state.project.tracks(), project.tracks());
            assert_eq!(
                reopened.state.project.return_tracks(),
                project.return_tracks()
            );
            assert_eq!(
                reopened.state.project.main_devices(),
                project.main_devices()
            );
        });
    }
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn eq_creation_escape_and_changed_source_cancel_without_reserving_identity(
    cx: &mut TestAppContext,
) {
    creation_cancellation(cx, [0.02, 0.5]);
}
#[gpui::test]
fn eq_notch_creation_escape_and_changed_source_cancel_without_reserving_identity(
    cx: &mut TestAppContext,
) {
    creation_cancellation(cx, [0.5, 0.025]);
}
fn creation_cancellation(cx: &mut TestAppContext, origin: [f32; 2]) {
    let (service, state, track, device) = fixture(false);
    let mut initial_next = state.project.next_identity();
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let start = plot(&mut visual, device, origin[0], origin[1]);
    let moved = plot(&mut visual, device, 0.4, 0.7);
    for cancellation in 0..3 {
        visual.simulate_mouse_down(start, MouseButton::Left, OPTION_CREATE);
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
        match cancellation {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.update(|window, _| window.blur()),
            _ => visual.simulate_mouse_move(moved, None, Modifiers::default()),
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, settled);
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(
                read(v, device).nodes.is_empty(),
                "cancellation {cancellation} must discard preview"
            );
            assert_eq!(
                v.state.as_ref().unwrap().project.next_identity(),
                initial_next
            );
            assert!(settled(v));
        });
    }
    // Other in-flight UI work cannot discard a creation's reserved terminal.
    visual.simulate_mouse_down(start, MouseButton::Left, OPTION_CREATE);
    editor.update(cx, |view, _| view.worker_requests_pending = 1);
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.device_gesture.terminal().is_some())
    });
    editor.update(cx, |view, _| view.worker_requests_pending = 0);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.is_empty()
    });
    initial_next = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.next_identity()
    });
    visual.simulate_mouse_down(start, MouseButton::Left, OPTION_CREATE);
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    // A canonical edit during the held preview invalidates its expected state.
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::SetDeviceParameter {
            device,
            parameter: hex_engine::eq::OUTPUT_GAIN,
            value: ParameterValue::Float(0.5),
        }))
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).output_gain == 0.5
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(read(v, device).nodes.is_empty());
        assert_eq!(
            v.state.as_ref().unwrap().project.next_identity(),
            initial_next
        );
        assert!(settled(v));
    });
    // The cancelled capture does not poison the next independent press.
    visual.simulate_click(start, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
}

#[gpui::test]
fn eq_creation_outside_release_clamps_and_allocator_or_project_change_cancels(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let start = plot(&mut visual, device, 0.02, 0.4);
    let bounds = visual
        .debug_bounds(selector("eq-response", device))
        .unwrap();
    let outside = point(bounds.right() + px(12.0), bounds.center().y);
    visual.simulate_mouse_down(start, MouseButton::Left, OPTION_CREATE);
    visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    editor.read_with(cx, |v, _| {
        let state = read(v, device);
        assert_eq!(state.nodes[0].kind, hex_engine::eq::NodeKind::LowCut);
        assert!((state.nodes[0].frequency_hz - 20000.0).abs() < 0.1);
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(start, MouseButton::Left, OPTION_CREATE);
    // The EQ source is unchanged, but another identity reservation invalidates
    // the provisional ID. It must never be rebased onto a replacement ID.
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "concurrent track".into(),
        })))
    });
    pump_until(&editor, cx, |v| {
        settled(v) && v.state.as_ref().unwrap().project.tracks().len() == 2
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(editor.read_with(cx, |v, _| read(v, device).nodes.len()), 1);
    visual.simulate_mouse_down(start, MouseButton::Left, OPTION_CREATE);
    editor.update(cx, |v, cx| {
        v.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_move(start, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device).nodes.len(), 1);
        assert!(settled(v));
    });
}
#[gpui::test]
fn eq_rendered_creation_drag_preserves_existing_lanes_and_history(cx: &mut TestAppContext) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    // Keep the complete EQ visible after the preceding instrument card.
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let location = plot(&mut visual, device, 0.4, 0.5);
    visual.simulate_click(location, Modifiers::default());
    // The second press can arrive before the worker publishes the first node.
    visual.simulate_event(gpui::MouseDownEvent {
        position: location,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    visual.simulate_event(gpui::MouseUpEvent {
        position: location,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(selector("eq-remove", device)).is_some(),
        "a newly created node remains selected after publication"
    );
    let initial = editor.read_with(cx, |v, _| read(v, device));
    let id = initial.nodes[0].id;
    let moved = plot(&mut visual, device, 0.7, 0.7);
    visual.simulate_mouse_down(location, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes[0].frequency_hz > initial.nodes[0].frequency_hz * 2.0
    });
    visual.run_until_parked();
    assert_eq!(editor.read_with(cx, |v, _| read(v, device).nodes[0].id), id);
    seed_existing_lane(&editor, cx, device);
    visual.run_until_parked();
    let with_lane = editor.read_with(cx, |v, _| read(v, device));
    assert_eq!(with_lane.nodes[0].id, id);
    let button = visual.debug_bounds(selector("eq-remove", device)).unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.is_empty()
    });
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Undo));
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    editor.read_with(cx, |v, _| {
        let state = read(v, device);
        assert_eq!(state.nodes[0].id, id);
        assert_eq!(state.lanes.len(), 1);
        assert_eq!(state.lanes[0].node, id);
    });
    visual.run_until_parked();

    assert_eq!(editor.read_with(cx, |v, _| read(v, device)), with_lane);
    // Select the restored stable node and remove it through the ordinary action.
    visual.simulate_click(moved, Modifiers::default());
    visual.run_until_parked();
    let button = visual.debug_bounds(selector("eq-remove", device)).unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.is_empty()
    });
    visual.run_until_parked();
    let replacement_position = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_click(replacement_position, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel.message.is_none(),
            "empty response creation refused: {:?}",
            view.device_panel.message
        )
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    visual.run_until_parked();
    let replacement = editor.read_with(cx, |v, _| {
        let state = read(v, device);
        assert!(state.lanes.is_empty());
        assert!(
            state.nodes[0].id > id,
            "deleting and undoing never reuses a node identity"
        );
        state.nodes[0].id
    });

    // Undoing creation must leave a usable empty response with no stale target.
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Undo));
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.is_empty()
    });
    visual.run_until_parked();
    visual.simulate_click(replacement_position, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel.message.is_none(),
            "empty response creation refused: {:?}",
            view.device_panel.message
        )
    });
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(selector("eq-remove", device)).is_some(),
        "the replacement node is selected after undo and recreation"
    );
    assert!(editor.read_with(cx, |v, _| read(v, device).nodes[0].id) > replacement);
}
#[gpui::test]
fn legacy_eq_upgrade_is_one_undoable_state_and_identity_transaction(cx: &mut TestAppContext) {
    let (service, state, track, device) = fixture(true);
    let original = state.project.device(device).unwrap().state().clone();
    let cursor = state.project.next_identity();
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1100.0), px(650.0)));
    visual.run_until_parked();
    let upgrade = visual.debug_bounds(selector("eq-upgrade", device)).unwrap();
    visual.simulate_click(upgrade.center(), Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v)
            && v.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state()
                .type_id()
                == EQ_TYPE_ID
    });
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device).nodes.len(), 40);
        assert_eq!(
            v.state.as_ref().unwrap().project.next_identity(),
            cursor + 40
        );
    });
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Undo));
    });
    pump_until(&editor, cx, |v| {
        settled(v)
            && v.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state()
                == &original
    });
    assert_eq!(
        editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.next_identity()),
        cursor + 40
    );
}

#[path = "eq_graph_layout_tests.rs"]
pub(in crate::editor) mod graph_layout_tests;

#[path = "eq_node_gesture_tests.rs"]
mod node_gesture_tests;

#[path = "eq_marquee_tests.rs"]
mod marquee_tests;

#[gpui::test]
fn eq_node_controls_stay_bound_without_a_local_automation_editor(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let location = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_click(location, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 1
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(selector("eq-auto", device)).is_none(),
        "selecting a node must expose ordinary parameters, not an EQ-local automation entry"
    );
    assert!(
        visual
            .debug_bounds(selector("eq-automation-toolbar", device))
            .is_none()
    );
    let id = editor.read_with(cx, |v, _| read(v, device).nodes[0].id);
    // A second node must not steal the first node's stable parameter bindings.
    let second_position = plot(&mut visual, device, 0.8, 0.7);
    visual.simulate_click(second_position, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 2
    });
    let other = editor.read_with(cx, |v, _| read(v, device).nodes[1]);
    visual.run_until_parked();
    visual.simulate_click(location, Modifiers::default());
    visual.run_until_parked();
    for (field, value) in [
        (hex_engine::eq::NodeParameter::Frequency, "1234"),
        (hex_engine::eq::NodeParameter::Gain, "6"),
        (hex_engine::eq::NodeParameter::Q, "2"),
    ] {
        let before = editor.read_with(cx, |v, _| read(v, device));
        enter_typed_value(&mut visual, device, id.parameter(field).unwrap(), value);
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, |v| settled(v) && read(v, device) != before);
        let after = editor.read_with(cx, |v, _| read(v, device));
        assert_eq!(
            after.node(id).unwrap().value(field),
            ParameterValue::Float(value.parse().unwrap())
        );
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == before);
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Redo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == after);
        visual.run_until_parked();
    }
    for (field, option, expected) in [
        (
            hex_engine::eq::NodeParameter::Kind,
            4,
            ParameterValue::Choice(4),
        ),
        (
            hex_engine::eq::NodeParameter::Slope,
            3,
            ParameterValue::Choice(3),
        ),
    ] {
        let parameter = id.parameter(field).unwrap();
        let before = editor.read_with(cx, |v, _| read(v, device));
        let control = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{}", device.value(), parameter.value())
                    .into_boxed_str(),
            ))
            .unwrap();
        visual.simulate_click(control.center(), Modifiers::default());
        visual.run_until_parked();
        let item = visual
            .debug_bounds(Box::leak(
                format!(
                    "device-{}-parameter-{}-option-{option}",
                    device.value(),
                    parameter.value()
                )
                .into_boxed_str(),
            ))
            .unwrap();
        visual.simulate_click(item.center(), Modifiers::default());
        pump_until(&editor, cx, |v| {
            settled(v) && read(v, device).node(id).unwrap().value(field) == expected
        });
        let after = editor.read_with(cx, |v, _| read(v, device));
        assert_eq!(*after.node(other.id).unwrap(), other);
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == before);
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Redo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == after);
        visual.run_until_parked();
    }
    let before = editor.read_with(cx, |v, _| read(v, device));
    click_toggle(
        &mut visual,
        device,
        id.parameter(hex_engine::eq::NodeParameter::Enabled)
            .unwrap(),
    );
    pump_until(&editor, cx, |v| {
        settled(v) && !read(v, device).node(id).unwrap().enabled
    });
    let after = editor.read_with(cx, |v, _| read(v, device));
    assert_eq!(*after.node(other.id).unwrap(), other);
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    pump_until(&editor, cx, |v| settled(v) && read(v, device) == before);
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Redo)));
    pump_until(&editor, cx, |v| settled(v) && read(v, device) == after);
    assert!(visual.debug_bounds(selector("eq-auto", device)).is_none());
}

#[path = "eq_held_pointer_tests.rs"]
pub(in crate::editor) mod held_pointer_tests;

#[gpui::test]
fn eq_notch_hover_matches_creation_and_node_priority_at_narrow_and_wide_sizes(
    cx: &mut TestAppContext,
) {
    fn reveal_in_chain(
        editor: &gpui::Entity<EditorView>,
        visual: &mut VisualTestContext,
        target: &'static str,
    ) -> gpui::Bounds<gpui::Pixels> {
        let chain = visual.debug_bounds("device-chain-scroll").unwrap();
        let viewport = visual.update(|window, _| window.viewport_size());
        let left = chain.left().max(px(0.0)) + px(1.0);
        let right = chain.right().min(viewport.width) - px(1.0);
        let before = visual.debug_bounds(target).unwrap();
        assert!(
            before.size.width <= right - left,
            "{target} cannot fit the actual chain viewport: target={before:?}, chain={chain:?}, viewport={viewport:?}"
        );
        let delta = if before.left() < left {
            left - before.left()
        } else if before.right() > right {
            right - before.right()
        } else {
            px(0.0)
        };
        if delta != px(0.0) {
            editor.update(&mut *visual, |view, cx| {
                let scroll = &view.device_panel.chain_window.scroll;
                scroll.set_offset(scroll.offset() + point(delta, px(0.0)));
                cx.notify();
            });
            visual.run_until_parked();
        }
        let revealed = visual.debug_bounds(target).unwrap();
        assert!(
            revealed.left() >= left
                && revealed.right() <= right
                && revealed.top() >= chain.top()
                && revealed.bottom() <= chain.bottom(),
            "{target} must be physically reachable through the chain, including its edges: target={revealed:?}, chain={chain:?}, viewport={viewport:?}"
        );
        revealed
    }

    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (width, bank) in [(880.0, 1), (1500.0, 4)] {
        visual.simulate_resize(size(px(width), px(650.0)));
        visual.run_until_parked();
        for (x, y, kind, hint) in [
            (0.5, 0.025, hex_engine::eq::NodeKind::Notch, "notch"),
            (0.5, 0.5, hex_engine::eq::NodeKind::Bell, "bell"),
            (0.02, 0.025, hex_engine::eq::NodeKind::LowCut, "low-cut"),
            (0.98, 0.025, hex_engine::eq::NodeKind::HighCut, "high-cut"),
        ] {
            // Undo rebuilds detail selection under the existing owner. Select
            // the intended bank again before each independent creation.
            // The existing narrow chain viewport scrolls the EQ sidebar
            // horizontally. Bring its real control onscreen, select, then
            // reveal the whole graph before testing its pointer targets.
            let bank_selector =
                Box::leak(format!("eq-bank-{}-{bank}", device.value()).into_boxed_str());
            let bank_control = reveal_in_chain(&editor, &mut visual, bank_selector).center();
            visual.simulate_click(bank_control, Modifiers::default());
            visual.run_until_parked();
            let graph = reveal_in_chain(&editor, &mut visual, selector("eq-response", device));
            let start = plot(&mut visual, device, x, y);
            let chain = visual.debug_bounds("device-chain-scroll").unwrap();
            assert!(
                chain.contains(&start),
                "{hint} pointer must belong to the revealed chain: point={start:?}, graph={graph:?}, chain={chain:?}"
            );
            visual.simulate_mouse_move(start, None, Modifiers::default());
            visual.run_until_parked();
            let hint_selector = Box::leak(
                format!("eq-response-{}-creation-hint-{hint}", device.value()).into_boxed_str(),
            );
            let shown = visual
                .debug_bounds(hint_selector)
                .unwrap_or_else(|| panic!("missing actual {hint} hint at width={width}, bank={bank}, normalized=({x},{y}), pointer={start:?}, graph={graph:?}, chain={chain:?}"));
            assert!(
                shown.left() >= graph.left()
                    && shown.right() <= graph.right()
                    && shown.top() >= graph.top()
                    && shown.bottom() <= graph.bottom(),
                "{hint} hint must stay within compact graph: {shown:?} {graph:?}"
            );
            assert_eq!(
                visual.rendered_cursor_style(),
                Some(gpui::CursorStyle::Crosshair)
            );
            visual.simulate_click(start, Modifiers::default());
            pump_until(&editor, cx, |v| {
                settled(v) && read(v, device).nodes.len() == 1
            });
            visual.run_until_parked();
            let authored = editor.read_with(cx, |v, _| read(v, device));
            assert_eq!(
                authored.nodes[0].kind, kind,
                "hint action must equal actual canonical shape"
            );
            assert_eq!(
                authored.nodes[0].bank, bank,
                "{hint} at {width}px must retain its selected bank"
            );
            if kind != hex_engine::eq::NodeKind::Bell {
                assert_eq!(authored.nodes[0].gain_db, 0.0);
                assert!((authored.nodes[0].q - 0.1 * 180.0_f32.powf(y)).abs() < 0.001);
            }
            visual.simulate_mouse_move(start, None, Modifiers::default());
            visual.run_until_parked();
            assert!(
                visual.debug_bounds(hint_selector).is_none(),
                "existing node consumes the same bottom/edge zone"
            );
            assert_ne!(
                visual.rendered_cursor_style(),
                Some(gpui::CursorStyle::Crosshair)
            );
            visual.simulate_click(start, Modifiers::default());
            pump_until(&editor, cx, settled);
            editor.read_with(cx, |v, _| {
                assert_eq!(
                    read(v, device),
                    authored,
                    "existing node click must select without creating or changing it"
                )
            });
            editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
            pump_until(&editor, cx, |v| {
                settled(v) && read(v, device).nodes.is_empty()
            });
            visual.run_until_parked();
        }
    }
}

#[gpui::test]
fn eq_notch_hint_retires_for_real_pending_work_and_invalid_numeric_draft(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let start = plot(&mut visual, device, 0.5, 0.025);
    let hint_selector =
        Box::leak(format!("eq-response-{}-creation-hint-notch", device.value()).into_boxed_str());
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_move(start, None, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(hint_selector).is_some());
    // Queue a real no-op command. Before consuming its exact worker event,
    // the owner must hide the offer without creating any EQ state/history.
    editor.update(cx, |v, cx| {
        let name = v
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .name()
            .to_owned();
        assert!(v.enqueue(Request::Edit(Command::RenameTrack { track, name })));
        assert_eq!(v.worker_requests_pending, 1);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(hint_selector).is_none());
    visual.simulate_mouse_move(start, None, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(hint_selector).is_none());
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    pump_until(&editor, cx, settled);
    visual.simulate_mouse_move(start, None, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(hint_selector).is_some());
    enter_typed_value(&mut visual, device, hex_engine::eq::OUTPUT_GAIN, "invalid");
    editor.read_with(cx, |v, cx| assert!(v.device_panel.has_pending_draft(cx)));
    visual.simulate_mouse_move(start, None, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(hint_selector).is_none());
    editor.read_with(cx, |v, _| {
        assert!(settled(v));
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(read(v, device).nodes.is_empty());
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.simulate_mouse_move(start, None, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(hint_selector).is_some());
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}

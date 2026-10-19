//! Actual pointer, keyboard and canonical worker diagnostics; no native-output claim.
use super::*;
use crate::editor::test_environment::Environment;

type Fixture = (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    Arc<Environment>,
    DeviceId,
);

fn held_node_window(cx: &mut TestAppContext) -> Fixture {
    cx.update(crate::editor::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    pump_until(&editor, cx, |view| view.state.is_some() && settled(view));
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "held eq".into()
        })))
    });
    pump_until(&editor, cx, |view| {
        settled(view) && view.state.as_ref().unwrap().project.tracks().len() == 1
    });
    let track = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(EQ_TYPE_ID),
            before: None,
        }))
    });
    pump_until(&editor, cx, |view| {
        settled(view)
            && !view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()
                .is_empty()
    });
    let device = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()[0]
            .id()
    });
    editor.update(cx, |view, cx| {
        assert!(Arc::ptr_eq(&environment, &view.test_environment));
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = false;
        view.mode = crate::editor::EditorMode::Arrangement;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(800.0)));
    visual.run_until_parked();
    let toggle = visual
        .debug_bounds("device-panel-bottom-toggle")
        .expect("actual detail-pane toggle");
    visual.simulate_click(toggle.center(), Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.clip_view_open && view.device_panel_active)
    });
    let start = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_click(start, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    (handle, editor, environment, device)
}

fn graph(
    editor: &gpui::Entity<EditorView>,
    cx: &TestAppContext,
    device: DeviceId,
) -> gpui::Entity<hex_widgets::ResponseGraph> {
    editor.read_with(cx, |view, _| eq_panel::graph_for_test(view, device))
}

#[gpui::test]
fn eq_held_node_position_survives_real_preview_ack_and_repeated_completed_frames(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let node = before.nodes[0].id;
    let graph = graph(&editor, cx, device);
    let start = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    for (x, y) in [(0.62, 0.72), (0.7, 0.6), (0.58, 0.8), (0.65, 0.67)] {
        let pointer = plot(&mut visual, device, x, y);
        visual.simulate_mouse_move(pointer, Some(MouseButton::Left), Modifiers::default());
        for frame in 0..12 {
            editor.update(cx, |view, cx| {
                view.tick(cx);
                cx.notify();
            });
            visual.run_until_parked();
            graph.read_with(cx, |graph, _| {
                assert!(graph.is_dragging(), "owner lost at held frame {frame}");
                let actual = graph.node_position(node.value()).unwrap();
                assert!(
                    (actual[0] - x).abs() < 0.0001 && (actual[1] - y).abs() < 0.0001,
                    "held rendered model reverted at frame {frame}: {actual:?}, expected {x},{y}"
                );
            });
            editor.read_with(cx, |view, _| {
                assert_eq!(
                    read(view, device),
                    before,
                    "preview cannot commit canonical state"
                )
            });
        }
    }
    let end = plot(&mut visual, device, 0.65, 0.67);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) != before
    });
    let after = editor.read_with(cx, |view, _| read(view, device));
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == after
    });
}

#[gpui::test]
fn eq_held_node_space_key_routes_transport_without_retiring_pointer_or_edit(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let graph = graph(&editor, cx, device);
    for moving in [false, true] {
        let start = plot(&mut visual, device, 0.5, 0.5);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        let end = if moving {
            plot(&mut visual, device, 0.62, 0.7)
        } else {
            start
        };
        if moving {
            visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        }
        let key = editor.read_with(cx, |view, _| {
            assert!(view.last_transport_command.is_none());
            view.device_gesture.snapshot().map(|snapshot| snapshot.key)
        });
        visual.simulate_keystrokes("space");
        editor.read_with(cx, |view, _| {
            assert!(
                view.last_transport_command.is_some(),
                "actual Space binding did not reach transport while moving={moving}"
            );
            assert_eq!(
                view.device_gesture.snapshot().map(|snapshot| snapshot.key),
                key
            );
            assert!(
                view.audio.is_none(),
                "routing diagnostic only, no audible/native result"
            );
        });
        graph.read_with(cx, |graph, _| assert_eq!(graph.is_dragging(), moving));
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, settled);
        editor.update(cx, |view, _| view.last_transport_command = None);
    }
}

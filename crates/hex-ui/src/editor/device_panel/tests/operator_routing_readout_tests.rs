use super::*;
use gpui::{Focusable, TextRun};

fn selector(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn cells(visual: &mut VisualTestContext, device: DeviceId) -> Vec<(u32, Bounds<Pixels>)> {
    let mut cells = Vec::new();
    for source in 0..4 {
        for target in 0..4 {
            if source != target {
                let id = 13 + source * 4 + target;
                let bounds = visual
                    .debug_bounds(selector(format!(
                        "device-{}-parameter-{id}-value",
                        device.value()
                    )))
                    .expect("modulation value cell");
                cells.push((id, bounds));
            }
        }
        let id = 29 + source;
        cells.push((
            id,
            visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{id}-value",
                    device.value()
                )))
                .expect("carrier value cell"),
        ));
    }
    cells
}

fn assert_readouts_fit(
    visual: &mut VisualTestContext,
    editor: &gpui::Entity<EditorView>,
    device: DeviceId,
    cells: &[(u32, Bounds<Pixels>)],
) {
    for (id, bounds) in cells {
        let text = visual.update(|_, cx| {
            editor.read_with(cx, |view, cx| {
                let ParameterControl::Float { entity, .. } = &view.device_panel.controls
                    [&ParameterKey {
                        device,
                        parameter: hex_lattice_contract::ParameterId::from_static(*id),
                    }]
                else {
                    panic!("routing float")
                };
                entity.read(cx).display_text().to_owned()
            })
        });
        let width = visual.update(|window, _| {
            let run = TextRun {
                len: text.len(),
                font: gpui::font(theme::FONT_FAMILY),
                color: gpui::rgb(0xffffff).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(
                    text.clone().into(),
                    px(control_style().font_size),
                    &[run],
                    None,
                )
                .width
        });
        assert!(
            width <= bounds.size.width - px(8.0),
            "{text:?} must fit with inset/caret space: {width:?} in {bounds:?}"
        );
        assert!(
            text.ends_with(if *id < 29 { 't' } else { 'x' }),
            "units stay distinguishable"
        );
    }
    for row in cells.chunks_exact(4) {
        for pair in row.windows(2) {
            assert!(
                pair[0].1.right() < pair[1].1.left(),
                "columns never overlap"
            );
        }
    }
}

#[gpui::test]
fn operator_routing_readouts_fit_signed_bounds_and_keep_real_edits_in_stable_cells(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, mut state, track, device) = operator_navigation_tests::operator_fixture();
    for (id, value) in [(25, -8.0), (29, -1.0)] {
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(id),
                value: ParameterValue::Float(value),
            })
            .unwrap();
        state = receive_service_event(&service).state;
    }
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let routing = visual
        .debug_bounds(selector(format!(
            "operator-preview-{}-route-link",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(routing.center(), Modifiers::default());
    visual.run_until_parked();
    for width in [880.0, 1280.0] {
        visual.simulate_resize(size(px(width), px(540.0)));
        visual.run_until_parked();
        let baseline = cells(&mut visual, device);
        assert_readouts_fit(&mut visual, &editor, device, &baseline);
        for (id, text, value) in [
            (25, "8t", 8.0),
            (25, "-7.99 turns", -7.99),
            (25, "0", 0.0),
            (25, "-8t", -8.0),
            (29, "1x", 1.0),
            (29, "-0.99 linear", -0.99),
            (29, "0", 0.0),
            (29, "-1x", -1.0),
        ] {
            let parameter = hex_lattice_contract::ParameterId::from_static(id);
            let (revision, expected_state) = editor.read_with(cx, |view, _| {
                let state = view.state.as_ref().unwrap();
                let instrument = state.project.track(track).unwrap().instrument().unwrap();
                (
                    state.revision,
                    hex_engine::patch_native_device_parameter_for_class(
                        NativeDeviceClass::Instrument,
                        instrument.state(),
                        parameter,
                        ParameterValue::Float(value as f32),
                    )
                    .unwrap(),
                )
            });
            if text.ends_with("turns") || text.ends_with("linear") {
                // Exercise full-unit input through the real clipboard action.
                // Single-letter editor shortcuts are a separate routing concern.
                enter_typed_value(&mut visual, device, parameter, "");
                visual.update(|_, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.into()))
                });
                visual.simulate_keystrokes("cmd-v");
                visual.run_until_parked();
            } else {
                enter_typed_value(&mut visual, device, parameter, text);
            }
            editor.read_with(cx, |view, cx| {
                let ParameterControl::Float { entity, .. } =
                    &view.device_panel.controls[&ParameterKey { device, parameter }]
                else {
                    panic!("numeric")
                };
                assert_eq!(entity.read(cx).display_text(), text, "actual typed draft");
                assert!(entity.read(cx).is_editing());
            });
            assert_eq!(
                cells(&mut visual, device),
                baseline,
                "typing cannot move columns"
            );
            visual.simulate_keystrokes("enter");
            visual.run_until_parked();
            pump_until(&editor, cx, |view| {
                view.state.as_ref().unwrap().revision > revision
                    && view.worker_requests_pending == 0
                    && view.pending_requests.is_empty()
                    && view.device_gesture_barriers.is_empty()
                    && view.device_gesture.snapshot().is_none()
            });
            visual.run_until_parked();
            editor.read_with(cx, |view, cx| {
                let state = view.state.as_ref().unwrap();
                assert_eq!(
                    state.revision,
                    revision + 1,
                    "one canonical edit, no shortcut side effects"
                );
                assert_eq!(
                    state
                        .project
                        .track(track)
                        .unwrap()
                        .instrument()
                        .unwrap()
                        .state(),
                    &expected_state,
                    "the display codec preserves every unrelated sound parameter"
                );
                let ParameterControl::Float {
                    entity, descriptor, ..
                } = &view.device_panel.controls[&ParameterKey { device, parameter }]
                else {
                    panic!("numeric control")
                };
                assert!(
                    (entity.read(cx).value() - value).abs() < 1e-6,
                    "parameter {id}, typed {text:?}: expected {value}, actual {}",
                    entity.read(cx).value()
                );
                assert!(!entity.read(cx).is_editing());
                assert_eq!(
                    descriptor.kind,
                    if id == 25 {
                        ParameterKind::Float {
                            min: -8.0,
                            max: 8.0,
                            unit: Some("turns"),
                        }
                    } else {
                        ParameterKind::Float {
                            min: -1.0,
                            max: 1.0,
                            unit: Some("linear"),
                        }
                    }
                );
            });
            assert_eq!(cells(&mut visual, device), baseline);
            assert_readouts_fit(&mut visual, &editor, device, &baseline);
        }
        let (_, bounds) = baseline.iter().find(|(id, _)| *id == 25).unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            cells(&mut visual, device),
            baseline,
            "active press preserves columns"
        );
        assert_readouts_fit(&mut visual, &editor, device, &baseline);
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        let focus = editor.read_with(cx, |view, cx| {
            let ParameterControl::Float { entity, .. } = &view.device_panel.controls
                [&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(25),
                }]
            else {
                panic!("numeric")
            };
            entity.focus_handle(cx)
        });
        assert!(visual.update(|window, _| focus.is_focused(window)));
    }
}

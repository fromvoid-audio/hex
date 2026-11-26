use super::*;

pub(super) fn operator_fixture() -> (Service, Arc<ViewState>, TrackId, DeviceId) {
    let (service, _, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .unwrap();
    let _ = receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let device = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    (service, state, track, device)
}

fn selector(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

#[gpui::test]
fn lfo_pitch_assignment_clicks_commit_history_and_project_reopen(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _, track, device) = operator_fixture();
    let path = std::env::temp_dir().join(format!("hex-lfo-mapping-{}.hex", std::process::id()));
    let _ = std::fs::remove_file(&path);
    service.submit(Request::SaveNew(path.clone())).unwrap();
    let saved = receive_service_event(&service);
    assert!(saved.error.is_none());
    let (handle, editor) = actor_editor_window(cx, service, saved.state.clone(), track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1440.0), px(540.0)));
    visual.run_until_parked();
    let tab = lfo_link_bounds(&mut visual, device);
    visual.simulate_click(tab.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector(format!(
                "operator-lfo-destinations-{}",
                device.value()
            )))
            .is_some()
    );
    for parameter in 89..=92 {
        let bounds = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}",
                device.value()
            )))
            .unwrap();
        let rail = tab_bounds(&mut visual, device, "envelope", "amp env");
        let detail = visual
            .debug_bounds(selector(format!("operator-detail-{}", device.value())))
            .unwrap();
        assert!(
            bounds.bottom() <= rail.top() || bounds.top() >= rail.bottom(),
            "destination control overlaps shared navigation: {bounds:?} rail={rail:?}"
        );
        assert!(
            bounds.bottom() <= detail.bottom() && bounds.right() <= detail.right(),
            "destination control is clipped: {bounds:?} detail={detail:?}"
        );
        visual.simulate_click(bounds.center(), Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, parameter)
                        == ParameterValue::Bool(false)
                })
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, parameter)
                        == ParameterValue::Bool(true)
                })
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, parameter)
                        == ParameterValue::Bool(false)
                })
        });
        visual.run_until_parked();
        let selected = ["a", "b", "c", "d"][(parameter - 88) as usize..].join(" ");
        assert!(
            visual
                .debug_bounds(selector(format!(
                    "operator-lfo-selected-{}-{selected}",
                    device.value()
                )))
                .is_some(),
            "selected readout must follow each canonical destination edit"
        );
    }
    assert!(
        visual
            .debug_bounds(selector(format!(
                "operator-lfo-selected-{}-",
                device.value()
            )))
            .is_some(),
        "all-off selected readout follows canonical destination values"
    );
    let detail = visual
        .debug_bounds(selector(format!("operator-detail-{}", device.value())))
        .unwrap();
    let depth = visual
        .debug_bounds(selector(format!("operator-lfo-depth-{}", device.value())))
        .unwrap();
    let rail = tab_bounds(&mut visual, device, "envelope", "amp env");
    assert!(
        depth.bottom() <= rail.top(),
        "pitch-depth explanation overlaps navigation: {depth:?} rail={rail:?}"
    );
    assert!(
        depth.bottom() <= detail.bottom() && depth.right() <= detail.right(),
        "pitch-depth explanation is clipped: {depth:?} detail={detail:?}"
    );
    for width in [880.0, 1440.0] {
        visual.simulate_resize(size(px(width), px(540.0)));
        for sync in [true, false] {
            editor.update(cx, |view, _| {
                assert!(view.enqueue(Request::SetDeviceParameter {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(84),
                    value: ParameterValue::Bool(sync),
                }));
            });
            pump_until(&editor, cx, |view| {
                view.worker_requests_pending == 0
                    && view.state.as_ref().is_some_and(|state| {
                        operator_parameter_value(state, track, device, 84)
                            == ParameterValue::Bool(sync)
                    })
            });
            visual.run_until_parked();
            let depth = visual
                .debug_bounds(selector(format!("operator-lfo-depth-{}", device.value())))
                .unwrap();
            let rail = tab_bounds(&mut visual, device, "envelope", "amp env");
            assert!(
                depth.bottom() <= rail.top(),
                "depth overlaps navigation at width={width}, sync={sync}: {depth:?} rail={rail:?}"
            );
            let detail = visual
                .debug_bounds(selector(format!("operator-detail-{}", device.value())))
                .unwrap();
            for caption in [
                format!("operator-lfo-depth-{}", device.value()),
                format!("operator-lfo-destinations-{}", device.value()),
                format!("operator-lfo-selected-{}-", device.value()),
            ] {
                let bounds = visual.debug_bounds(selector(caption)).unwrap();
                assert!(
                    bounds.right() <= detail.right() && bounds.bottom() <= rail.top(),
                    "lfo caption is clipped or overlaps navigation at width={width}, sync={sync}: {bounds:?}"
                );
            }
        }
    }
    enter_typed_value(
        &mut visual,
        device,
        hex_lattice_contract::ParameterId::from_static(87),
        "50",
    );
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 87) == ParameterValue::Float(50.0)
            })
    });
    let mapped = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state()
            .clone()
    });
    editor.update(cx, |view, cx| view.save_project(cx));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && !view.persistence.saving_project
            && view.state.as_ref().is_some_and(|state| !state.unsaved)
    });
    let reload = Service::new(48_000, 8, 8_000_000).unwrap();
    let _ = receive_service_event(&reload);
    reload.submit(Request::Open(path.clone())).unwrap();
    let reopened = receive_service_event(&reload);
    assert!(reopened.error.is_none());
    assert_eq!(
        reopened.state.project.device(device).unwrap().state(),
        &mapped
    );
    std::fs::remove_file(path).unwrap();
}

fn lfo_link_bounds(visual: &mut VisualTestContext, device: DeviceId) -> Bounds<Pixels> {
    visual
        .debug_bounds(selector(format!("operator-lfo-link-{}", device.value())))
        .unwrap()
}

// The legacy fallback lets this exact regression execute on the unfixed source:
// it detects the vertical jump, rather than merely failing a renamed selector.
fn tab_bounds(
    visual: &mut VisualTestContext,
    device: DeviceId,
    page: &str,
    legacy: &str,
) -> Bounds<Pixels> {
    visual
        .debug_bounds(selector(format!("operator-page-{}-{page}", device.value())))
        .or_else(|| {
            visual.debug_bounds(selector(format!(
                "operator-page-{}-{legacy}",
                device.value()
            )))
        })
        .unwrap()
}

fn tab_to_routing(
    visual: &mut VisualTestContext,
    editor: &gpui::Entity<EditorView>,
    device: DeviceId,
) {
    for _ in 0..96 {
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        if visual.update(|window, app| {
            editor.read_with(app, |view, _| {
                view.device_panel.operator_previews[&device]
                    .routing_focus
                    .as_ref()
                    .unwrap()
                    .is_focused(window)
            })
        }) {
            return;
        }
    }
    panic!("routing remains reachable in the complete tab order");
}

#[gpui::test]
fn operator_section_navigation_keeps_rendered_bounds_and_owner_fields(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_fixture();
    let original = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .state()
        .clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1440.0), px(540.0)));
    visual.run_until_parked();
    open_operator_options(&mut visual, device);
    let envelope = tab_bounds(&mut visual, device, "envelope", "amp env");
    let oscillator = tab_bounds(&mut visual, device, "oscillator", "osc");
    let rail = visual
        .debug_bounds(selector(format!(
            "operator-preview-row-{}-global",
            device.value()
        )))
        .unwrap();
    let lfo = lfo_link_bounds(&mut visual, device);
    visual.simulate_click(lfo.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Lfo
        );
    });
    assert_eq!(
        tab_bounds(&mut visual, device, "envelope", "amp env"),
        envelope
    );
    assert_eq!(
        tab_bounds(&mut visual, device, "oscillator", "osc"),
        oscillator
    );
    assert_eq!(
        lfo_link_bounds(&mut visual, device),
        Bounds {
            origin: lfo.origin + point(px(120.0), px(0.0)),
            ..lfo
        }
    );
    for parameter in 82..=92 {
        assert!(
            visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{parameter}",
                    device.value()
                )))
                .is_some(),
            "LFO page retains real canonical controls"
        );
    }
    visual.simulate_click(oscillator.center(), Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        tab_bounds(&mut visual, device, "envelope", "amp env"),
        envelope,
        "envelope navigation must not move when oscillator content replaces it"
    );
    assert_eq!(
        tab_bounds(&mut visual, device, "oscillator", "osc"),
        oscillator,
        "oscillator navigation must not move when selected"
    );
    assert!(
        envelope.origin.x < oscillator.origin.x,
        "reference order is envelope left, oscillator right"
    );
    for operator in 0..4 {
        let badge = visual
            .debug_bounds(selector(format!(
                "operator-select-{}-{operator}",
                device.value()
            )))
            .unwrap();
        visual.simulate_click(badge.center(), Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_panel.gesture_barriers.is_empty()
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, 513 + operator)
                        == ParameterValue::Bool(false)
                })
        });
        let background = super::operator_context_selection_tests::row_background(
            &mut visual,
            device,
            operator as usize,
        );
        visual.simulate_click(background, Modifiers::default());
        visual.run_until_parked();
        // Power edits and explicit row selection are independent.
        // Undo that edit before testing that subsequent navigation preserves sound.
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_panel.gesture_barriers.is_empty()
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, 513 + operator)
                        == ParameterValue::Bool(true)
                })
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{}",
                    device.value(),
                    57 + operator
                )))
                .is_some(),
            "wave field belongs to selected oscillator"
        );
        visual.simulate_click(envelope.center(), Modifiers::default());
        visual.run_until_parked();
        for parameter in 33 + operator * 6..39 + operator * 6 {
            assert!(
                visual
                    .debug_bounds(selector(format!(
                        "device-{}-parameter-{parameter}-value",
                        device.value()
                    )))
                    .is_some()
            );
        }
        visual.simulate_click(oscillator.center(), Modifiers::default());
        visual.run_until_parked();
        let fixed = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{}",
                device.value(),
                62 + operator * 3
            )))
            .unwrap();
        visual.simulate_click(fixed.center(), Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_panel.gesture_barriers.is_empty()
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{}-value",
                    device.value(),
                    63 + operator * 3
                )))
                .is_some(),
            "fixed frequency retains its canonical binding"
        );
        assert_eq!(
            tab_bounds(&mut visual, device, "envelope", "amp env"),
            envelope
        );
        assert_eq!(
            tab_bounds(&mut visual, device, "oscillator", "osc"),
            oscillator
        );
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
    }
    assert!(
        visual
            .debug_bounds(selector(format!(
                "operator-page-{}-routing",
                device.value()
            )))
            .is_none()
    );
    // Select the optional global context explicitly through its background.
    let global = visual
        .debug_bounds(selector(format!(
            "operator-preview-row-{}-global",
            device.value()
        )))
        .unwrap();
    let routing = visual
        .debug_bounds(Box::leak(
            format!("operator-preview-{}-route-link", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(
        point(routing.right() + px(2.0), global.center().y),
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Routing
        )
    });
    assert_eq!(
        visual
            .debug_bounds(selector(format!(
                "operator-preview-row-{}-global",
                device.value()
            )))
            .unwrap(),
        Bounds {
            origin: rail.origin + point(px(120.0), px(0.0)),
            ..rail
        }
    );
    assert!(
        visual
            .debug_bounds(selector(format!(
                "operator-page-{}-envelope",
                device.value()
            )))
            .is_none()
    );
    assert!(
        visual
            .debug_bounds(selector(format!(
                "operator-page-{}-oscillator",
                device.value()
            )))
            .is_none()
    );
    let shell = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-1-slider",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(shell.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    let background =
        super::operator_context_selection_tests::row_background(&mut visual, device, 0);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        tab_bounds(&mut visual, device, "envelope", "amp env"),
        envelope
    );
    assert_eq!(
        tab_bounds(&mut visual, device, "oscillator", "osc"),
        oscillator
    );
    visual.simulate_click(envelope.center(), Modifiers::default());
    let fine = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-610-slider",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(fine.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    tab_to_routing(&mut visual, &editor, device);
    assert!(
        visual.update(|window, app| editor.read_with(app, |view, _| view
            .device_panel
            .operator_previews[&device]
            .routing_focus
            .as_ref()
            .unwrap()
            .is_focused(window))),
        "tab from a device widget discovers the routing rail"
    );
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Envelope,
            "focused routing picker preserves envelope context"
        );
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_keystrokes("alt-enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Routing
        );
        assert!(
            view.device_panel.operator_previews[&device]
                .routing_popup
                .is_none()
        );
    });
    visual.simulate_click(shell.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_click(envelope.center(), Modifiers::default());
    visual.simulate_click(fine.center(), Modifiers::default());
    tab_to_routing(&mut visual, &editor, device);
    let page_before_space = editor.update(&mut visual, |view, _| {
        view.test_probes.last_transport_command = None;
        view.device_panel.operator_pages[&device].page
    });
    visual.simulate_keystrokes("space");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            page_before_space
        );
        assert!(view.test_probes.last_transport_command.is_none());
        assert!(
            view.device_panel.operator_previews[&device]
                .routing_popup
                .is_some()
        );
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            page_before_space
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &original,
            "navigation and selection preserve canonical sound state"
        );
    });
}

#[gpui::test]
fn retained_operator_preview_survives_unassigned_repaints_and_retires_on_replacement(
    cx: &mut TestAppContext,
) {
    let (service, _, track, bound_device) = operator_fixture();
    let operator = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "unassigned operator".into(),
            devices: vec![operator.clone()],
            instrument_index: 0,
        }))
        .unwrap();
    let created = receive_service_event(&service);
    let Some(Created::NativeSource(source)) = created.created else {
        panic!("source")
    };
    let device = created
        .state
        .project
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let mut retained = Vec::new();
    for name in ["delete operator", "replacement project"] {
        service
            .submit(Request::Edit(Command::CreateNativeSource {
                name: name.into(),
                devices: vec![operator.clone()],
                instrument_index: 0,
            }))
            .unwrap();
        let event = receive_service_event(&service);
        let Some(Created::NativeSource(id)) = event.created else {
            panic!("source")
        };
        retained.push((
            id,
            event
                .state
                .project
                .native_source(id)
                .unwrap()
                .instrument()
                .id(),
        ));
    }
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "contributor".into(),
        }))
        .unwrap();
    let added = receive_service_event(&service);
    let Some(Created::Track(contributor)) = added.created else {
        panic!("contributor")
    };
    let (handle, editor) = actor_editor_window(cx, service, added.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1440.0), px(700.0)));
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        assert!(
            view.device_panel
                .operator_previews
                .contains_key(&bound_device)
        );
        view.device_panel
            .operator_previews
            .entry(device)
            .or_default()
            .routing_focus = Some(cx.focus_handle());
        for &(_, retained_device) in &retained {
            view.device_panel
                .operator_previews
                .entry(retained_device)
                .or_default();
        }
        cx.notify();
    });
    for _ in 0..3 {
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(
                view.device_panel.operator_previews.contains_key(&device),
                "unassigned preview was retired"
            );
            assert!(
                view.device_panel.operator_previews[&device]
                    .routing_focus
                    .is_some()
            );
            assert!(
                view.device_panel
                    .operator_previews
                    .contains_key(&bound_device)
            );
            for &(_, retained_device) in &retained {
                assert!(
                    view.device_panel
                        .operator_previews
                        .contains_key(&retained_device)
                );
            }
        });
        editor.update(&mut visual, |_, cx| cx.notify());
    }
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source)
        })))
    });
    pump_until(&editor, &mut visual, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(contributor)
            .unwrap()
            .source_id()
            == Some(source)
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.device_panel.operator_previews.contains_key(&device))
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::ReplaceTrackInstrument {
            track: contributor,
            replacing: device,
            state: operator
        })))
    });
    pump_until(&editor, &mut visual, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .native_source(source)
            .unwrap()
            .instrument()
            .id()
            != device
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(!view.device_panel.operator_previews.contains_key(&device));
        assert!(
            view.device_panel
                .operator_previews
                .contains_key(&bound_device)
        );
        for &(_, retained_device) in &retained {
            assert!(
                view.device_panel
                    .operator_previews
                    .contains_key(&retained_device)
            );
        }
    });
    let (deleted_source, deleted_device) = retained[0];
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveNativeSource(deleted_source))))
    });
    pump_until(&editor, &mut visual, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .native_source(deleted_source)
            .is_none()
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            !view
                .device_panel
                .operator_previews
                .contains_key(&deleted_device)
        );
        assert!(
            view.device_panel
                .operator_previews
                .contains_key(&retained[1].1)
        );
        assert!(
            view.device_panel
                .operator_previews
                .contains_key(&bound_device)
        );
    });
    let generation = editor.read_with(&visual, |view, _| view.project_generation);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None,
        }))
    });
    pump_until(&editor, &mut visual, |view| {
        view.project_generation != generation
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            !view
                .device_panel
                .operator_previews
                .contains_key(&retained[1].1)
        );
        assert!(
            !view
                .device_panel
                .operator_previews
                .contains_key(&bound_device)
        );
    });
}

#[gpui::test]
fn lfo_slot_editor_exposes_waveform_extended_params_and_routes_with_undo(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(2000.0), px(540.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    let tab = lfo_link_bounds(&mut visual, device);
    visual.simulate_click(tab.center(), Modifiers::default());
    visual.run_until_parked();

    let detail = visual
        .debug_bounds(selector(format!("operator-detail-{}", device.value())))
        .unwrap();
    let rail = tab_bounds(&mut visual, device, "envelope", "amp env");
    let waveform = visual
        .debug_bounds(selector(format!(
            "operator-lfo-waveform-{}",
            device.value()
        )))
        .expect("two-cycle lfo waveform preview");
    let routes = visual
        .debug_bounds(selector(format!("operator-lfo-routes-{}", device.value())))
        .expect("lfo 1 routes summary");
    assert!(waveform.bottom() <= rail.top());
    assert!(waveform.right() <= detail.right());
    assert!(routes.top() >= rail.bottom());
    assert!(routes.bottom() <= detail.bottom());

    // Every extended LFO parameter sits inside detail and never overlaps the tab rail.
    for parameter in [
        711_u32, 712, 713, 714, 715, 716, 717, 718, 719, 720, 721, 722, 726,
    ] {
        let bounds = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}",
                device.value()
            )))
            .expect("extended lfo control mounted");
        assert!(
            bounds.bottom() <= rail.top() || bounds.top() >= rail.bottom(),
            "lfo param {parameter} overlaps tab rail: {bounds:?} rail={rail:?}"
        );
        assert!(
            bounds.right() <= detail.right() && bounds.bottom() <= detail.bottom(),
            "lfo param {parameter} clipped: {bounds:?} detail={detail:?}"
        );
    }

    // Toggle filter destination (712) and verify canonical edit + single-step Undo/Redo.
    let flt = visual
        .debug_bounds(selector(format!("device-{}-parameter-712", device.value())))
        .unwrap();
    visual.simulate_click(flt.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 712) == ParameterValue::Bool(true)
            })
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 712) == ParameterValue::Bool(false)
            })
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 712) == ParameterValue::Bool(true)
            })
    });

    // Edit r<key (713) and confirm canonical update + Undo.
    let rate_key = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-713-value",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(rate_key.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("50");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 713) == ParameterValue::Float(50.0)
            })
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 713) == ParameterValue::Float(0.0)
            })
    });
}

#[gpui::test]
fn pitch_page_binds_bipolar_envelope_transpose_spread_and_destinations_with_undo(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(2000.0), px(540.0)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    let pitch_tab = visual
        .debug_bounds(selector(format!("operator-page-{}-pitch", device.value())))
        .expect("pitch page tab");
    visual.simulate_click(pitch_tab.center(), Modifiers::default());
    visual.run_until_parked();

    let detail = visual
        .debug_bounds(selector(format!("operator-detail-{}", device.value())))
        .unwrap();
    let rail = tab_bounds(&mut visual, device, "envelope", "amp env");
    let graph = visual
        .debug_bounds("operator-pitch-envelope-graph")
        .expect("bipolar pitch envelope graph");
    let routes = visual
        .debug_bounds(selector(format!(
            "operator-pitch-routes-{}",
            device.value()
        )))
        .expect("pitch routes and glide summary");
    assert!(graph.bottom() <= rail.top());
    assert!(routes.top() >= rail.bottom());
    assert!(routes.bottom() <= detail.bottom());

    for parameter in [
        640_u32, 641, 650, 651, 653, 654, 655, 656, 657, 658, 659, 666, 667, 668, 669, 670,
    ] {
        let bounds = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}",
                device.value()
            )))
            .expect("pitch page control mounted");
        assert!(
            bounds.top() >= rail.bottom() && bounds.bottom() <= detail.bottom(),
            "pitch param {parameter} out of lower region: {bounds:?} rail={rail:?} detail={detail:?}"
        );
        assert!(
            bounds.right() <= detail.right(),
            "pitch param {parameter} clipped horizontally: {bounds:?} detail={detail:?}"
        );
    }

    // Toggle pitch env on (650), edit transpose (640) and spread (641), verify Undo/Redo.
    let on_btn = visual
        .debug_bounds(selector(format!("device-{}-parameter-650", device.value())))
        .unwrap();
    visual.simulate_click(on_btn.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 650) == ParameterValue::Bool(true)
            })
    });

    for (parameter, typed, expected) in [(640_u32, "12", 12.0_f32), (641, "45", 45.0)] {
        let field = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}-value",
                device.value()
            )))
            .unwrap();
        visual.simulate_click(field.center(), Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(typed);
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, parameter)
                        == ParameterValue::Float(expected)
                })
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, parameter)
                        == ParameterValue::Float(0.0)
                })
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, parameter)
                        == ParameterValue::Float(expected)
                })
        });
    }
}

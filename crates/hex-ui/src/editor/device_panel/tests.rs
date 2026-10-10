use super::*;
use gpui::{
    AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size,
};
use hex_project::{Command, Created, Editor, Project};
use hex_session::service::{Event, Request, Service, ViewState};
use std::sync::Arc;

fn receive_service_event(service: &Service) -> Event {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => panic!("service response unavailable: {error}"),
        }
    }
}

fn actor_utility_fixture() -> (
    Service,
    Arc<ViewState>,
    TrackId,
    DeviceId,
    hex_lattice_contract::ParameterId,
    bool,
) {
    let entry = hex_engine::native_device_catalog()
        .expect("catalog")
        .remove(0);
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _initial = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "device track".into(),
        }))
        .expect("add track request");
    let track_event = receive_service_event(&service);
    let Created::Track(track) = track_event.created.expect("created track") else {
        panic!("created track result")
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(entry.type_id),
            before: None,
        })
        .expect("add native device request");
    let device_event = receive_service_event(&service);
    let state = device_event.state;
    let device = state.project.track(track).expect("track").devices()[0].id();
    let device_view = hex_engine::native_device_view(
        state.project.track(track).expect("track").devices()[0].state(),
    )
    .expect("device view");
    let bool_parameter = device_view
        .parameters
        .iter()
        .find(|parameter| matches!(parameter.descriptor.kind, ParameterKind::Bool))
        .expect("utility boolean parameter");
    let ParameterValue::Bool(original) = bool_parameter.value else {
        panic!("boolean parameter value")
    };
    (
        service,
        state,
        track,
        device,
        bool_parameter.descriptor.id,
        original,
    )
}

fn actor_return_utility_fixture() -> (
    Service,
    Arc<ViewState>,
    TrackId,
    hex_project::ReturnId,
    DeviceId,
    hex_lattice_contract::ParameterId,
) {
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _initial = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "regular source".into(),
        }))
        .expect("regular track request");
    let track_event = receive_service_event(&service);
    let Created::Track(track) = track_event.created.expect("regular track") else {
        panic!("regular track result")
    };
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "send bus".into(),
        }))
        .expect("return track request");
    let return_event = receive_service_event(&service);
    let Created::ReturnTrack(return_track) = return_event.created.expect("return track") else {
        panic!("return track result")
    };
    service
        .submit(Request::Edit(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .expect("Utility state"),
        }))
        .expect("Utility request");
    let _device_event = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::SetReturnSend {
            track,
            return_track,
            gain: 0.25,
        }))
        .expect("initial send request");
    let state = receive_service_event(&service).state;
    let device = state
        .project
        .return_tracks()
        .iter()
        .find(|candidate| candidate.id() == return_track)
        .expect("return owner")
        .devices()[0]
        .id();
    (
        service,
        state,
        track,
        return_track,
        device,
        hex_lattice_contract::ParameterId::from_static(1),
    )
}

fn actor_editor_window(
    cx: &mut TestAppContext,
    service: Service,
    state: Arc<ViewState>,
    track: TrackId,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let bootstrap = cx.add_window(|_, _| gpui::Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(state);
                view.selected_track = Some(track);
                view.device_panel_active = true;
                view.clip_view_open = true;
                view.mode = super::super::EditorMode::Session;
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    (handle, editor)
}

fn pump_until(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    mut done: impl FnMut(&EditorView) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        if editor.read_with(cx, |view, _| done(view)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker-backed device action did not complete: {}",
            editor.read_with(cx, |view, _| format!(
                "worker={} queued={} gesture={:?} barriers={} error={:?} revision={:?}",
                view.worker_requests_pending,
                view.pending_requests.len(),
                view.device_gesture.snapshot(),
                view.device_gesture_barriers.len(),
                view.error,
                view.state.as_ref().map(|state| state.revision)
            ))
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[gpui::test]
fn settled_device_removal_has_no_stale_warning_or_empty_instrument_block(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("default instrument")
        .id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let panel_before = visual.debug_bounds("device-panel").expect("device panel");
    let remove_effect = Box::leak(format!("device-remove-{}", device.value()).into_boxed_str());
    let bounds = visual
        .debug_bounds(remove_effect)
        .expect("effect remove action");
    let click_point = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.devices().is_empty())
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.message, None);
    });
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.has_control_for(device));
        assert!(!view.device_panel.views.contains_key(&device));
    });
    let panel_after_effect = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel_after_effect.size.height, panel_before.size.height);

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.devices().iter().any(|item| item.id() == device))
        })
    });
    visual.run_until_parked();

    let remove_instrument =
        Box::leak(format!("instrument-remove-{}", instrument.value()).into_boxed_str());
    let bounds = visual
        .debug_bounds(remove_instrument)
        .expect("instrument remove action");
    let click_point = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.instrument().is_none())
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.message, None);
    });
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.views.contains_key(&instrument));
    });
    assert!(
        visual.debug_bounds("instrument-empty").is_none(),
        "removing the instrument leaves the chain without a placeholder block"
    );
    let panel_after_instrument = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel_after_instrument.size.height, px(188.0));
    assert_eq!(panel_after_instrument.size.height, panel_before.size.height);

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state.project.track(track).is_some_and(|track| {
                track
                    .instrument()
                    .is_some_and(|item| item.id() == instrument)
            })
        })
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("instrument-empty").is_none());
}

#[gpui::test]
fn browser_device_drop_uses_hovered_card_half_and_commits_before_target(cx: &mut TestAppContext) {
    let (service, state, track, existing, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        view.clip_view_open = false;
        view.device_panel_active = false;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let source = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .expect("registered utility in device browser");
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
    assert!(visual.debug_bounds("device-panel").is_some());
    editor.read_with(cx, |view, _| {
        assert!(view.clip_view_open, "browser drag reveals the closed pane");
        assert!(view.device_panel_active);
        assert_eq!(view.selected_track, Some(track));
    });
    let card_selector =
        Box::leak(format!("device-drop-card-{}", existing.value()).into_boxed_str());
    let card = visual
        .debug_bounds(card_selector)
        .expect("existing utility card drop receiver");
    let destination = point(
        card.origin.x + px(24.0),
        card.origin.y + card.size.height / 2.0,
    );
    visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_native_device_drag
            .as_ref()
            .expect("native device drag remains active over rail");
        assert_eq!(
            drag.destination.get(),
            Some(drag::NativeDeviceDropTarget {
                track,
                before: Some(existing),
            })
        );
    });

    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .is_some_and(|track| track.devices().len() == 2)
    });
    editor.read_with(cx, |view, _| {
        let devices = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices();
        assert_eq!(devices[1].id(), existing);
        assert_ne!(devices[0].id(), existing);
        assert!(view.active_native_device_drag.is_none());
        assert!(view.clip_view_open, "successful drop keeps the pane open");
        assert!(view.device_panel_active);
    });
}

#[gpui::test]
fn fm4_browser_drop_admits_builtin_instrument_in_fixed_panel(cx: &mut TestAppContext) {
    let (service, _state, track, effect, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("clear default sine slot");
    let empty = receive_service_event(&service);
    assert!(
        empty
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_none()
    );

    let (handle, editor) = actor_editor_window(cx, service, empty.state, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let panel_before = visual
        .debug_bounds("device-panel")
        .expect("fixed device panel");
    assert_eq!(panel_before.size.height, px(188.0));
    let source = visual
        .debug_bounds("browser-device-instrument-hex.instrument.fm4")
        .expect("registered FM4 browser entry after sine");
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

    let card_selector = Box::leak(format!("device-drop-card-{}", effect.value()).into_boxed_str());
    let card = visual
        .debug_bounds(card_selector)
        .expect("effect card receives instrument drop");
    let destination = point(
        card.origin.x + px(24.0),
        card.origin.y + card.size.height / 2.0,
    );
    visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.active_native_device_drag
                .as_ref()
                .and_then(|drag| drag.destination.get())
                .map(|target| target.before),
            Some(Some(effect)),
            "instrument drops target the canonical front position"
        );
    });
    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .and_then(|track| track.instrument())
            .is_some_and(|instrument| {
                instrument.state().type_id().as_str() == Ok("hex.instrument.fm4")
            })
    });
    visual.run_until_parked();
    let instrument_id = editor.read_with(cx, |view, _| {
        let instrument = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap();
        assert_eq!(
            instrument.state().type_id().as_str(),
            Ok("hex.instrument.fm4")
        );
        instrument.id()
    });
    let body = Box::leak(format!("instrument-device-{}", instrument_id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(body).is_some(),
        "FM4 has a rendered card"
    );
    let carrier =
        Box::leak(format!("device-{}-parameter-29-slider", instrument_id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(carrier).is_some(),
        "FM4's real carrier descriptor is rendered"
    );
    let panel_after = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel_after.size.height, px(188.0));
    assert_eq!(panel_after.size.height, panel_before.size.height);
}

#[gpui::test]
fn utility_left_mode_disables_width_and_mid_side_controls_without_losing_values(
    cx: &mut TestAppContext,
) {
    let (service, mut state, track, device, _, _) = actor_utility_fixture();
    let descriptors = hex_engine::native_device_descriptors_for_class(
        hex_engine::NativeDeviceClass::Effect,
        state.project.track(track).expect("track").devices()[0]
            .state()
            .type_id(),
    )
    .expect("Utility descriptors");
    let parameter = |label: &str| {
        descriptors
            .iter()
            .find(|descriptor| descriptor.label == label)
            .expect("Utility parameter")
            .id
    };
    let width = parameter("width");
    let channel = parameter("channel");
    let mid_side = parameter("mid-side");
    let mid_side_balance = parameter("mid-side balance");
    for (parameter, value) in [
        (width, ParameterValue::Float(2.5)),
        (mid_side, ParameterValue::Bool(true)),
        (mid_side_balance, ParameterValue::Float(-0.4)),
        (channel, ParameterValue::Choice(1)),
    ] {
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter,
                value,
            })
            .expect("Utility setup request");
        state = receive_service_event(&service).state;
    }

    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let width_slider = Box::leak(
        format!(
            "device-{}-parameter-{}-slider",
            device.value(),
            width.value()
        )
        .into_boxed_str(),
    );
    let width_bounds = visual
        .debug_bounds(width_slider)
        .expect("disabled width slider");
    let width_click = point(width_bounds.center().x, width_bounds.center().y);
    let balance_slider = Box::leak(
        format!(
            "device-{}-parameter-{}-slider",
            device.value(),
            mid_side_balance.value()
        )
        .into_boxed_str(),
    );
    let balance_bounds = visual
        .debug_bounds(balance_slider)
        .expect("disabled mid-side balance slider");
    let balance_click = point(balance_bounds.center().x, balance_bounds.center().y);
    let mid_side_toggle = Box::leak(
        format!("device-{}-parameter-{}", device.value(), mid_side.value()).into_boxed_str(),
    );
    let toggle_bounds = visual
        .debug_bounds(mid_side_toggle)
        .expect("disabled mid-side toggle");
    let toggle_click = point(toggle_bounds.center().x, toggle_bounds.center().y);
    let channel_selector = Box::leak(
        format!("device-{}-parameter-{}", device.value(), channel.value()).into_boxed_str(),
    );
    let channel_bounds = visual
        .debug_bounds(channel_selector)
        .expect("channel selector");
    editor.read_with(cx, |view, _| {
        let current = view.state.as_ref().expect("state");
        let utility_state = current.project.track(track).expect("track").devices()[0].state();
        assert_eq!(current.revision, initial_revision);
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility_state,
                width,
            )
            .unwrap(),
            ParameterValue::Float(2.5)
        );
        assert!(current.can_undo);
    });

    visual.simulate_mouse_down(width_click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(width_click.x + px(24.0), width_click.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(width_click.x + px(24.0), width_click.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_down(balance_click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(balance_click.x + px(24.0), balance_click.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(balance_click.x + px(24.0), balance_click.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_down(toggle_click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle_click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let current = view.state.as_ref().expect("state");
        assert_eq!(current.revision, initial_revision);
        assert!(view.pending_requests.is_empty());
        assert!(view.device_gesture.snapshot().is_none());
        let utility_state = current.project.track(track).expect("track").devices()[0].state();
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility_state,
                width,
            )
            .unwrap(),
            ParameterValue::Float(2.5)
        );
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility_state,
                mid_side,
            )
            .unwrap(),
            ParameterValue::Bool(true)
        );
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility_state,
                mid_side_balance,
            )
            .unwrap(),
            ParameterValue::Float(-0.4)
        );
    });

    let channel_click = point(channel_bounds.center().x, channel_bounds.center().y);
    visual.simulate_mouse_down(channel_click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let swap_selector = Box::leak(
        format!(
            "device-{}-parameter-{}-option-3",
            device.value(),
            channel.value()
        )
        .into_boxed_str(),
    );
    let swap = visual.debug_bounds(swap_selector).expect("swap option");
    let swap_click = point(swap.center().x, swap.center().y);
    visual.simulate_mouse_down(swap_click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(swap_click, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state.project.track(track).is_some_and(|track| {
                hex_engine::native_device_parameter_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    track.devices()[0].state(),
                    channel,
                ) == Ok(ParameterValue::Choice(3))
            })
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let current = view.state.as_ref().expect("state");
        let utility_state = current.project.track(track).expect("track").devices()[0].state();
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility_state,
                width,
            )
            .unwrap(),
            ParameterValue::Float(2.5),
            "re-enabling width preserves its stored value"
        );
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility_state,
                mid_side,
            )
            .unwrap(),
            ParameterValue::Bool(true),
            "re-enabling mid-side preserves its stored value"
        );
        let view = hex_engine::native_device_view_for_class(
            hex_engine::NativeDeviceClass::Effect,
            utility_state,
        )
        .unwrap();
        assert!(
            view.parameters
                .iter()
                .filter(|parameter| matches!(
                    parameter.descriptor.label,
                    "width" | "mid-side" | "mid-side balance"
                ))
                .all(|parameter| parameter.enabled)
        );
    });

    visual.simulate_mouse_down(width_click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(width_click.x + px(20.0), width_click.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(width_click.x + px(20.0), width_click.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state.project.track(track).is_some_and(|track| {
                hex_engine::native_device_parameter_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    track.devices()[0].state(),
                    width,
                ) != Ok(ParameterValue::Float(2.5))
            })
        })
    });
    visual.simulate_mouse_down(toggle_click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle_click, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state.project.track(track).is_some_and(|track| {
                hex_engine::native_device_parameter_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    track.devices()[0].state(),
                    mid_side,
                ) == Ok(ParameterValue::Bool(false))
            })
        })
    });
}

#[gpui::test]
fn clipped_overflow_card_cannot_restore_a_drop_preview_outside_the_rail(cx: &mut TestAppContext) {
    let (service, initial_state, track, _, _, _) = actor_utility_fixture();
    let utility = hex_engine::native_device_catalog()
        .expect("registered utility")
        .remove(0);
    let mut state = initial_state;
    for _ in 0..8 {
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(utility.type_id),
                before: None,
            })
            .expect("append utility");
        state = receive_service_event(&service).state;
    }
    let device_ids: Vec<_> = state
        .project
        .track(track)
        .expect("track")
        .devices()
        .iter()
        .map(hex_project::Device::id)
        .collect();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let rail = visual
        .debug_bounds("device-chain-scroll")
        .expect("visible rail bounds");
    let outside = point(
        rail.origin.x + rail.size.width + px(2.0),
        rail.origin.y + rail.size.height / 2.0,
    );
    let clipped_card_contains_pointer = device_ids.iter().any(|device| {
        let selector = Box::leak(format!("device-drop-card-{}", device.value()).into_boxed_str());
        visual
            .debug_bounds(selector)
            .is_some_and(|bounds| bounds.contains(&outside))
    });
    assert!(
        clipped_card_contains_pointer,
        "fixture reaches overflow card bounds"
    );

    let source = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .expect("registered utility row");
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
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.active_native_device_drag
                .as_ref()
                .is_some_and(|drag| { drag.destination.get().is_none() })
        );
    });
}

#[gpui::test]
fn existing_device_header_grip_reorders_cards_by_stable_identity(cx: &mut TestAppContext) {
    let (service, _state, track, first, _, _) = actor_utility_fixture();
    let utility = hex_engine::native_device_catalog()
        .expect("registered utility")
        .remove(0);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(utility.type_id),
            before: None,
        })
        .expect("second utility");
    let second_state = receive_service_event(&service).state;
    let second = second_state
        .project
        .track(track)
        .expect("track")
        .devices()
        .iter()
        .find(|device| device.id() != first)
        .expect("second utility")
        .id();
    let (handle, editor) = actor_editor_window(cx, service, second_state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let grip_selector = Box::leak(format!("device-drag-grip-{}", first.value()).into_boxed_str());
    let target_selector =
        Box::leak(format!("device-drop-card-{}", second.value()).into_boxed_str());
    let grip = visual
        .debug_bounds(grip_selector)
        .expect("first device grip");
    let target = visual
        .debug_bounds(target_selector)
        .expect("second device receiver");
    let down = point(
        grip.origin.x + grip.size.width / 2.0,
        grip.origin.y + grip.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let destination = point(
        target.origin.x + target.size.width - px(8.0),
        target.origin.y + target.size.height / 2.0,
    );
    visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_native_device_drag
            .as_ref()
            .expect("existing device drag");
        assert_eq!(
            drag.destination.get(),
            Some(drag::NativeDeviceDropTarget {
                track,
                before: None
            })
        );
    });
    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state.project.track(track).is_some_and(|track| {
                track
                    .devices()
                    .iter()
                    .map(|device| device.id())
                    .collect::<Vec<_>>()
                    == vec![second, first]
            })
        })
    });
}

#[gpui::test]
fn device_drag_clears_preview_on_outside_release_and_escape(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.active_native_device_drag.is_none());
        assert!(view.last_enqueued_native_device_drop.is_none());
    });

    let source = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .expect("registered utility row");
    let card_selector = Box::leak(format!("device-drop-card-{}", device.value()).into_boxed_str());
    let card = visual.debug_bounds(card_selector).expect("device card");
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
    let card_point = point(
        card.origin.x + card.size.width / 2.0,
        card.origin.y + card.size.height / 2.0,
    );
    visual.simulate_mouse_move(card_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.active_native_device_drag
                .as_ref()
                .and_then(|drag| drag.destination.get())
                .is_some()
        );
    });

    let outside_rail = point(px(500.0), px(360.0));
    visual.simulate_mouse_move(outside_rail, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.active_native_device_drag
                .as_ref()
                .is_some_and(|drag| drag.destination.get().is_none())
        );
    });
    visual.simulate_mouse_up(outside_rail, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.active_native_device_drag.is_none());
        assert!(view.last_enqueued_native_device_drop.is_none());
    });

    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(card_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.active_native_device_drag.is_some())
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.active_native_device_drag.is_none());
        assert!(view.last_enqueued_native_device_drop.is_none());
    });
}

fn utility_fixture() -> (
    hex_project::Project,
    TrackId,
    DeviceId,
    hex_lattice_contract::ParameterId,
) {
    let entry = hex_engine::native_device_catalog()
        .expect("catalog")
        .remove(0);
    let mut project_editor =
        Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Track(track) = project_editor
        .execute(Command::AddTrack {
            name: "device track".into(),
        })
        .expect("track")
    else {
        panic!("track result")
    };
    project_editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(entry.type_id).expect("default"),
        })
        .expect("device");
    let device = project_editor
        .project()
        .track(track)
        .expect("track")
        .devices()[0]
        .id();
    (
        project_editor.project().clone(),
        track,
        device,
        entry
            .parameters
            .iter()
            .find(|parameter| parameter.label == "width")
            .expect("linear Utility float parameter")
            .id,
    )
}

fn enter_typed_value(
    visual: &mut VisualTestContext,
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
    text: &str,
) {
    let selector = Box::leak(
        format!(
            "device-{}-parameter-{}-value",
            device.value(),
            parameter.value()
        )
        .into_boxed_str(),
    );
    let value = visual.debug_bounds(selector).expect("device numeric field");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input(text);
    visual.run_until_parked();
}

#[gpui::test]
fn utility_gain_is_presented_in_db_and_typed_values_remain_linear_canonical(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let gain = hex_engine::native_device_view(
        state.project.track(track).expect("track").devices()[0].state(),
    )
    .expect("device view")
    .parameters
    .into_iter()
    .find(|parameter| parameter.descriptor.label == "gain")
    .expect("Utility gain")
    .descriptor
    .id;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let control = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device,
                parameter: gain,
            })
        else {
            panic!("Utility gain control should be mounted")
        };
        entity.clone()
    });
    control.read_with(cx, |control, _| assert_eq!(control.value(), 1.0));

    enter_typed_value(&mut visual, device, gain, "6 db");
    editor.update(cx, |view, cx| assert!(super::finalize_controls(view, cx)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            matches!(
                parameter_value(state, track, device, gain),
                ParameterValue::Float(value) if (value - 10.0_f32.powf(6.0 / 20.0)).abs() < 0.000_01
            )
        })
    });
    editor.read_with(cx, |view, _| {
        let value = parameter_value(view.state.as_ref().expect("state"), track, device, gain);
        let ParameterValue::Float(value) = value else {
            panic!("gain remains a linear float")
        };
        assert!(
            (value - 10.0_f32.powf(6.0 / 20.0)).abs() < 0.000_01,
            "the project stores linear amplitude, not the displayed dB value"
        );
    });

    let slider = Box::leak(
        format!(
            "device-{}-parameter-{}-slider",
            device.value(),
            gain.value()
        )
        .into_boxed_str(),
    );
    let slider_bounds = visual.debug_bounds(slider).expect("gain slider");
    visual.simulate_click(
        point(slider_bounds.center().x, slider_bounds.center().y),
        Modifiers::default(),
    );
    visual.simulate_keystrokes("up");
    let seven_db = 10.0_f32.powf(7.0 / 20.0);
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            matches!(
                parameter_value(state, track, device, gain),
                ParameterValue::Float(value) if (value - seven_db).abs() < 0.000_01
            )
        })
    });
    visual.simulate_keystrokes("shift-up");
    let seven_point_one_db = 10.0_f32.powf(7.1 / 20.0);
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            matches!(
                parameter_value(state, track, device, gain),
                ParameterValue::Float(value) if (value - seven_point_one_db).abs() < 0.000_01
            )
        })
    });

    enter_typed_value(&mut visual, device, gain, "9 db");
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let ParameterValue::Float(value) =
            parameter_value(view.state.as_ref().expect("state"), track, device, gain)
        else {
            panic!("gain remains a linear float")
        };
        assert!((value - seven_point_one_db).abs() < 0.000_01);
    });
}

#[gpui::test]
fn same_update_typed_value_finishes_before_save_barrier(cx: &mut TestAppContext) {
    let (project, track, device, parameter) = utility_fixture();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        let current = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            project: current.project.clone(),
            waveforms: current.waveforms.clone(),
            midi_preview: current.midi_preview.clone(),
            timeline_frames: current.timeline_frames,
            can_undo: current.can_undo,
            can_redo: current.can_redo,
            revision: current.revision,
            unsaved: current.unsaved,
            document_path: Some(std::path::PathBuf::from("/tmp/hex-device-save.hex")),
        }));
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, parameter, "2.5");
    editor.update(cx, |view, cx| view.save_project(cx));
    editor.read_with(cx, |view, _| {
        let snapshot = view.device_gesture.snapshot().expect("synchronous begin");
        assert_eq!(
            snapshot.target,
            GestureTarget::DeviceParameter { device, parameter }
        );
        assert_eq!(snapshot.value, ParameterValue::Float(2.5));
        assert!(view.device_gesture.has_pending_terminal());
        assert!(matches!(
            view.device_gesture_barriers.front(),
            Some(Request::SaveCurrent)
        ));
        assert!(view.saving_project);
    });
}

#[gpui::test]
fn typed_numeric_owner_finalization_saves_actor_state_and_undo_restores_it(
    cx: &mut TestAppContext,
) {
    let (service, initial_state, track, device, _, _) = actor_utility_fixture();
    let numeric_parameter = hex_engine::native_device_view(
        initial_state.project.track(track).expect("track").devices()[0].state(),
    )
    .expect("device view")
    .parameters
    .into_iter()
    .find(|item| item.descriptor.label == "width")
    .expect("utility numeric parameter")
    .descriptor
    .id;
    let initial_value = parameter_value(&initial_state, track, device, numeric_parameter);
    let path = std::env::temp_dir().join(format!(
        "hex-device-owner-save-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos(),
    ));
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save initial fixture");
    let saved = receive_service_event(&service);
    assert!(saved.save_complete);
    assert!(saved.error.is_none());
    let (handle, editor) = actor_editor_window(cx, service, saved.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, numeric_parameter, "2.5");
    editor.update(cx, |view, cx| view.save_project(cx));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && !view.saving_project
            && view.state.as_ref().is_some_and(|state| !state.unsaved)
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            parameter_value(
                view.state.as_ref().expect("saved state"),
                track,
                device,
                numeric_parameter,
            ),
            ParameterValue::Float(2.5)
        );
        assert!(view.device_panel.pending_values.is_empty());
    });
    let reload = Service::new(48_000, 8, 8_000_000).expect("reload service");
    let _initial = receive_service_event(&reload);
    reload
        .submit(Request::Open(path.clone()))
        .expect("open saved project");
    let disk_state = receive_service_event(&reload).state;
    assert_eq!(
        parameter_value(&disk_state, track, device, numeric_parameter),
        ParameterValue::Float(2.5),
        "owner-finalized value is persisted before SaveCurrent completes"
    );

    let revision = editor.read_with(cx, |view, _| view.state.as_ref().expect("state").revision);
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.revision > revision)
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            parameter_value(
                view.state.as_ref().expect("undo state"),
                track,
                device,
                numeric_parameter,
            ),
            initial_value,
            "one Undo restores the pre-draft device value"
        );
    });
    let _ = std::fs::remove_file(path);
}

#[gpui::test]
fn hiding_a_returned_pointer_gesture_finishes_the_overlay_without_history(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let device_view = hex_engine::native_device_view(
        state.project.track(track).expect("track").devices()[0].state(),
    )
    .expect("device view");
    let balance = device_view
        .parameters
        .iter()
        .find(|item| item.descriptor.label == "balance")
        .expect("balance parameter");
    let ParameterValue::Float(initial) = balance.value else {
        panic!("balance parameter is numeric")
    };
    let parameter = balance.descriptor.id;
    let initial_revision = state.revision;
    let initial_undo = state.can_undo;
    let initial_project = state.project.clone();
    let initial_device_state = state
        .project
        .device(device)
        .expect("canonical device")
        .state()
        .clone();
    assert_eq!(
        hex_engine::patch_native_device_parameter(
            &initial_device_state,
            parameter,
            ParameterValue::Float(initial),
        )
        .expect("patch to original value"),
        initial_device_state,
        "setting the authored parameter to its existing value is canonical no-op"
    );
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let slider_selector = Box::leak(
        format!(
            "device-{}-parameter-{}-slider",
            device.value(),
            parameter.value()
        )
        .into_boxed_str(),
    );
    let slider = visual
        .debug_bounds(slider_selector)
        .expect("rendered balance slider");
    let start = point(slider.center().x, slider.center().y);
    let moved = point(start.x + gpui::px(60.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let snapshot = view
            .device_gesture
            .snapshot()
            .expect("active pointer gesture");
        assert_eq!(
            snapshot.target,
            GestureTarget::DeviceParameter { device, parameter }
        );
        assert_ne!(snapshot.value, ParameterValue::Float(initial));
    });

    visual.simulate_mouse_move(start, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_gesture
                .snapshot()
                .expect("returned gesture")
                .value,
            ParameterValue::Float(initial)
        );
        assert!(!view.device_gesture.has_pending_terminal());
    });

    editor.update(cx, |view, cx| {
        assert!(finalize_controls(view, cx));
        view.device_panel_active = false;
        cx.notify();
        assert!(view.device_gesture.has_pending_terminal());
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let current = view.state.as_ref().expect("canonical state");
        assert_eq!(current.revision, initial_revision);
        assert_eq!(current.can_undo, initial_undo);
        assert_eq!(&*current.project, &*initial_project);
        assert_eq!(
            parameter_value(current, track, device, parameter),
            ParameterValue::Float(initial)
        );
        assert!(view.device_gesture.snapshot().is_none());
    });
}

#[gpui::test]
fn same_update_typed_value_on_clean_close_opens_prompt_and_cancel_keeps_it(
    cx: &mut TestAppContext,
) {
    let (project, track, device, parameter) = utility_fixture();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, parameter, "3.0");
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.request_window_close(window, cx));
            assert!(view.close_prompt_open);
            let snapshot = view.device_gesture.snapshot().expect("synchronous begin");
            assert_eq!(snapshot.value, ParameterValue::Float(3.0));
            assert!(view.device_gesture.has_pending_terminal());
        });
    })
    .expect("request close");
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("cancel");
    cx.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.state.as_ref().is_some_and(|state| !state.unsaved));
        assert!(!view.close_prompt_open);
        let Some(ParameterControl::Float { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("numeric control remains mounted after close cancel")
        };
        entity.read_with(cx, |control, _| {
            assert!(!control.is_editing());
            assert_eq!(control.value(), 3.0);
        });
    });
}

#[gpui::test]
fn full_device_barrier_refusal_keeps_numeric_draft_and_retry_commits_once(cx: &mut TestAppContext) {
    let (project, track, first_device, parameter) = utility_fixture();
    let catalog_entry = hex_engine::native_device_catalog()
        .expect("catalog")
        .remove(0);
    let mut project_editor = Editor::new(project, 8).expect("editor");
    project_editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(catalog_entry.type_id).expect("default"),
        })
        .expect("second device");
    let second_device = project_editor
        .project()
        .track(track)
        .expect("track")
        .devices()[1]
        .id();
    let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        project_editor.project().clone(),
        Arc::default(),
    );
    editor.update(cx, |view, cx| {
        let current = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            project: current.project.clone(),
            waveforms: current.waveforms.clone(),
            midi_preview: current.midi_preview.clone(),
            timeline_frames: current.timeline_frames,
            can_undo: current.can_undo,
            can_redo: current.can_redo,
            revision: current.revision,
            unsaved: current.unsaved,
            document_path: Some(std::path::PathBuf::from("/tmp/device-retry.hex")),
        }));
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        view.device_gesture
            .begin(
                view.project_generation,
                GestureTarget::DeviceParameter {
                    device: first_device,
                    parameter,
                },
                ParameterValue::Float(0.0),
            )
            .expect("hold another control's edit");
        for _ in 0..super::super::UI_REQUEST_CAPACITY {
            view.device_gesture_barriers.push_back(Request::Undo);
        }
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, second_device, parameter, "2.5");

    editor.update(cx, |view, cx| view.save_project(cx));
    editor.read_with(cx, |view, _| {
        assert!(!view.saving_project, "full barrier queue refuses the save");
        assert_eq!(
            view.device_gesture.snapshot().expect("other edit").target,
            GestureTarget::DeviceParameter {
                device: first_device,
                parameter,
            }
        );
        assert!(!view.device_gesture.has_pending_terminal());
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: second_device,
                parameter,
            })
        else {
            panic!("second numeric control remains mounted")
        };
        entity.read_with(cx, |control, _| {
            assert!(control.is_editing(), "refused draft remains editable")
        });
    });

    editor.update(cx, |view, cx| {
        view.device_gesture_barriers.clear();
        view.device_gesture.reset();
        view.save_project(cx);
    });
    editor.read_with(cx, |view, _| {
        let snapshot = view.device_gesture.snapshot().expect("retried gesture");
        assert_eq!(
            snapshot.target,
            GestureTarget::DeviceParameter {
                device: second_device,
                parameter,
            }
        );
        assert_eq!(snapshot.value, ParameterValue::Float(2.5));
        assert!(view.device_gesture.has_pending_terminal());
        assert_eq!(
            view.device_panel.pending_values.get(&ParameterKey {
                device: second_device,
                parameter,
            }),
            Some(&ParameterValue::Float(2.5)),
        );
        assert!(matches!(
            view.device_gesture_barriers.front(),
            Some(Request::SaveCurrent)
        ));
        assert!(view.saving_project);
    });
}

#[gpui::test]
fn invalid_numeric_draft_is_cancelled_on_project_replacement_and_target_removal(
    cx: &mut TestAppContext,
) {
    let entry = hex_engine::native_device_catalog()
        .expect("catalog")
        .remove(0);
    let mut project_editor =
        Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Track(track) = project_editor
        .execute(Command::AddTrack {
            name: "device track".into(),
        })
        .expect("track")
    else {
        panic!("track result")
    };
    project_editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(entry.type_id).expect("default"),
        })
        .expect("device");
    let initial_project = project_editor.project().clone();
    let device = initial_project.track(track).expect("track").devices()[0].id();
    let parameter = entry.parameters[0].id;

    let bootstrap = cx.add_window(|_, _| gpui::Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 8, 8_000_000).expect("service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(Arc::new(ViewState {
                    project: Arc::new(initial_project.clone()),
                    waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    timeline_frames: 384_000,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                view.selected_track = Some(track);
                view.device_panel_active = true;
                view.clip_view_open = true;
                view.mode = super::super::EditorMode::Session;
                cx.notify();
            });
            editor
        })
        .expect("window");

    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(
        format!(
            "device-{}-parameter-{}-value",
            device.value(),
            parameter.value()
        )
        .into_boxed_str(),
    );
    let value = visual.debug_bounds(selector).expect("numeric field");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.run_until_parked();

    let old_control = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Float { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("numeric field entity should be cached")
        };
        entity.clone()
    });
    old_control.read_with(cx, |control, _| assert!(control.is_editing()));
    editor.update(cx, |view, cx| view.save_project(cx));
    editor.read_with(cx, |view, _| {
        assert!(!view.saving_project);
        assert!(
            !view.save_dialog_open,
            "invalid draft blocks save admission"
        );
    });
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.request_window_close(window, cx))
        });
    })
    .expect("close request");
    assert!(
        cx.has_pending_prompt(),
        "invalid draft still permits explicit discard"
    );
    cx.simulate_prompt_answer("cancel");
    cx.run_until_parked();
    old_control.read_with(cx, |control, _| {
        assert!(
            control.is_editing(),
            "cancel keeps the invalid draft visible"
        );
    });
    editor.update(cx, |view, cx| {
        view.project_generation = view.project_generation.saturating_add(1);
        cx.notify();
    });
    visual.run_until_parked();
    old_control.read_with(cx, |control, _| {
        assert!(!control.is_editing(), "replacement cancels the old draft");
    });

    let value = visual
        .debug_bounds(selector)
        .expect("rebuilt numeric field");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.run_until_parked();
    let stale_control = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Float { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("rebuilt numeric field entity should be cached")
        };
        entity.clone()
    });

    let mut without_device = Editor::new(initial_project, 8).expect("replacement editor");
    without_device
        .execute(Command::RemoveDevice(device))
        .expect("remove target");
    let next_project = without_device.project().clone();
    editor.update(cx, |view, cx| {
        let current = view.state.as_ref().expect("view state");
        view.state = Some(Arc::new(ViewState {
            project: Arc::new(next_project.clone()),
            waveforms: current.waveforms.clone(),
            midi_preview: current.midi_preview.clone(),
            timeline_frames: current.timeline_frames,
            can_undo: current.can_undo,
            can_redo: current.can_redo,
            revision: current.revision,
            unsaved: current.unsaved,
            document_path: current.document_path.clone(),
        }));
        cx.notify();
    });
    visual.run_until_parked();
    stale_control.read_with(cx, |control, _| {
        assert!(!control.is_editing(), "removed target cancels its draft");
    });
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.controls.is_empty());
        assert_eq!(
            view.device_panel.message.as_deref(),
            Some("device target is no longer available")
        );
    });
}

#[gpui::test]
fn invalid_return_numeric_draft_is_cancelled_when_its_owner_is_removed(cx: &mut TestAppContext) {
    let (service, state, track, return_track, device, parameter) = actor_return_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let selector = Box::leak(
        format!(
            "device-{}-parameter-{}-value",
            device.value(),
            parameter.value()
        )
        .into_boxed_str(),
    );
    let value = visual
        .debug_bounds(selector)
        .expect("return Utility gain field");
    let click = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.run_until_parked();
    let old_control = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Float { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("return numeric field entity should be cached")
        };
        entity.clone()
    });
    old_control.read_with(cx, |control, _| assert!(control.is_editing()));

    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Edit(Command::RemoveReturnTrack(return_track))));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.return_tracks().is_empty())
    });
    visual.run_until_parked();
    old_control.read_with(cx, |control, _| {
        assert!(
            !control.is_editing(),
            "removed return owner cancels the draft"
        )
    });
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.controls.is_empty());
        assert!(view.state.as_ref().unwrap().project.track(track).is_some());
        assert_eq!(view.device_panel.message, None);
    });
}

#[gpui::test]
fn return_send_numeric_entry_uses_one_undoable_canonical_edit(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, return_track, _, _) = actor_return_utility_fixture();
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::EditorMode::Arrangement;
        view.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let value = visual
        .debug_bounds(selector)
        .expect("return send value field");
    let click = point(value.center().x, value.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.375");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control.entity.read_with(cx, |control, _| {
            assert!(
                control.is_editing(),
                "send value field entered text editing"
            )
        });
    });
    editor.update(cx, |view, cx| assert!(super::finalize_controls(view, cx)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        assert_eq!(
            state.revision,
            initial_revision + 1,
            "one send edit committed"
        );
        let send = state
            .project
            .track(track)
            .expect("source track")
            .sends()
            .iter()
            .find(|send| send.return_track() == return_track)
            .expect("canonical send");
        assert!(
            (send.gain() - 0.375).abs() < 0.000_01,
            "status: {:?}",
            view.status
        );
    });
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.state.as_ref().is_some_and(|state| {
                state.project.track(track).is_some_and(|track| {
                    track.sends().iter().any(|send| {
                        send.return_track() == return_track && (send.gain() - 0.25).abs() < 0.000_01
                    })
                })
            })
    });
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Redo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.state.as_ref().is_some_and(|state| {
                state.project.track(track).is_some_and(|track| {
                    track.sends().iter().any(|send| {
                        send.return_track() == return_track
                            && (send.gain() - 0.375).abs() < 0.000_01
                    })
                })
            })
    });
}

#[gpui::test]
fn return_send_popup_fits_minimum_window_and_retires_offscreen_or_removed_owner(
    cx: &mut TestAppContext,
) {
    let (service, mut state, track, return_track, _, _) = actor_return_utility_fixture();
    for index in 0..24 {
        service
            .submit(Request::Edit(Command::AddTrack {
                name: format!("extra {index}"),
            }))
            .expect("add scroll fixture track");
        state = receive_service_event(&service).state;
    }
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::EditorMode::Arrangement;
        view.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let menu_selector = Box::leak(format!("return-send-menu-{}", track.value()).into_boxed_str());
    let value_selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let menu = visual
        .debug_bounds(menu_selector)
        .expect("send popup at minimum size");
    let value = visual
        .debug_bounds(value_selector)
        .expect("send numeric field");
    let window = visual.update(|window, _| window.bounds());
    assert!(menu.origin.x >= window.origin.x && menu.origin.y >= window.origin.y);
    assert!(menu.origin.x + menu.size.width <= window.origin.x + window.size.width);
    assert!(menu.origin.y + menu.size.height <= window.origin.y + window.size.height);
    assert_eq!(
        value.size.width,
        px(46.0),
        "numeric value hit target is not clipped"
    );

    editor.update(cx, |view, cx| {
        view.track_scroll
            .scroll_to_item(24, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.update(|window, app| {
        editor.update(app, |view, app| {
            let _ = view.return_send_menu_overlay(window, app);
        });
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_ne!(
            view.track_scroll.0.borrow().base_handle.offset().y,
            px(0.0),
            "the source row must actually be outside the current viewport"
        );
        assert_eq!(
            view.send_menu_track, None,
            "popup closes when its source row scrolls out of view"
        );
    });
    assert!(
        visual.update(|window, app| editor.update(app, |view, _| view.focus.is_focused(window)))
    );

    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Edit(Command::RemoveTrack(track))));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.track(track).is_none())
    });
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        let _ = super::view(view, cx);
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.send_menu_track, None,
            "removed source closes its send popup"
        );
        assert!(
            !view
                .device_panel
                .send_controls
                .contains_key(&SendControlKey {
                    track,
                    return_track,
                }),
            "removed source retires its cached send widget"
        );
    });
}

#[gpui::test]
fn valid_return_send_draft_commits_once_when_owner_scrolls_offscreen(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, mut state, track, return_track, _, _) = actor_return_utility_fixture();
    for index in 0..24 {
        service
            .submit(Request::Edit(Command::AddTrack {
                name: format!("extra {index}"),
            }))
            .expect("add scroll fixture track");
        state = receive_service_event(&service).state;
    }
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::EditorMode::Arrangement;
        view.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let value = visual
        .debug_bounds(selector)
        .expect("return send value field");
    let click = point(value.center().x, value.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.625");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control
            .entity
            .read_with(cx, |control, _| assert!(control.is_editing()));
    });

    editor.update(cx, |view, cx| {
        view.track_scroll
            .scroll_to_item(24, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.update(|window, app| {
        editor.update(app, |view, app| {
            let _ = view.return_send_menu_overlay(window, app);
        });
    });
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.send_menu_track.is_none()
            && view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|state| {
                state.revision == initial_revision + 1
                    && state.project.track(track).is_some_and(|track| {
                        track.sends().iter().any(|send| {
                            send.return_track() == return_track
                                && (send.gain() - 0.625).abs() < 0.000_01
                        })
                    })
            })
    });
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control
            .entity
            .read_with(cx, |control, _| assert!(!control.is_editing()));
    });
    assert!(
        visual.update(|window, app| editor.update(app, |view, _| view.focus.is_focused(window)))
    );
}

#[gpui::test]
fn invalid_return_send_draft_cancels_and_restores_when_owner_scrolls_offscreen(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, mut state, track, return_track, _, _) = actor_return_utility_fixture();
    for index in 0..24 {
        service
            .submit(Request::Edit(Command::AddTrack {
                name: format!("extra {index}"),
            }))
            .expect("add scroll fixture track");
        state = receive_service_event(&service).state;
    }
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::EditorMode::Arrangement;
        view.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let value = visual
        .debug_bounds(selector)
        .expect("return send value field");
    let click = point(value.center().x, value.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("2.5");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control
            .entity
            .read_with(cx, |control, _| assert!(control.is_editing()));
    });

    editor.update(cx, |view, cx| {
        view.track_scroll
            .scroll_to_item(24, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.update(|window, app| {
        editor.update(app, |view, app| {
            let _ = view.return_send_menu_overlay(window, app);
        });
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.send_menu_track, None);
        assert!(view.device_gesture.snapshot().is_none());
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.pending_requests.is_empty());
        assert!(view.error.as_deref().is_some_and(|error| {
            error.contains("invalid send value") && error == error.to_lowercase()
        }));
        assert_eq!(
            view.state.as_ref().expect("state").revision,
            initial_revision
        );
        let send = view
            .state
            .as_ref()
            .expect("state")
            .project
            .track(track)
            .expect("source track")
            .sends()
            .iter()
            .find(|send| send.return_track() == return_track)
            .expect("canonical send");
        assert!((send.gain() - 0.25).abs() < 0.000_01);
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control
            .entity
            .read_with(cx, |control, _| assert!(!control.is_editing()));
    });
    assert!(
        visual.update(|window, app| editor.update(app, |view, _| view.focus.is_focused(window)))
    );
}

#[gpui::test]
fn invalid_return_send_draft_blocks_finalize_and_escape_restores_canonical(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, return_track, _, _) = actor_return_utility_fixture();
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::EditorMode::Arrangement;
        view.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let value = visual
        .debug_bounds(selector)
        .expect("return send value field");
    let click = point(value.center().x, value.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("2.5");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control
            .entity
            .read_with(cx, |control, _| assert!(control.is_editing()));
    });
    editor.update(cx, |view, cx| {
        assert!(!super::finalize_controls(view, cx));
        cx.notify();
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().expect("state").revision,
            initial_revision
        );
        assert!(view.error.is_some());
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        assert!(super::finalize_controls(view, cx));
        cx.notify();
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        assert_eq!(state.revision, initial_revision);
        let send = state
            .project
            .track(track)
            .expect("source track")
            .sends()
            .iter()
            .find(|send| send.return_track() == return_track)
            .expect("canonical send remains");
        assert!((send.gain() - 0.25).abs() < 0.000_01);
    });
}

#[gpui::test]
fn return_send_pointer_drag_previews_then_commits_once(cx: &mut TestAppContext) {
    let (service, state, track, return_track, _, _) = actor_return_utility_fixture();
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::EditorMode::Arrangement;
        view.send_menu_track = Some(track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let selector = Box::leak(
        format!(
            "return-send-{}-{}-slider",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let slider = visual.debug_bounds(selector).expect("return send slider");
    let start = point(slider.center().x, slider.center().y);
    let moved = point(start.x + px(36.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let control = &view.device_panel.send_controls[&SendControlKey {
            track,
            return_track,
        }];
        control.entity.read_with(cx, |control, _| {
            assert!(
                control.is_interacting(),
                "send slider is in a pointer gesture"
            )
        });
        let snapshot = view.device_gesture.snapshot().expect("active send gesture");
        assert_eq!(
            snapshot.target,
            GestureTarget::ReturnSend {
                track,
                return_track
            }
        );
        assert!(matches!(snapshot.value, ParameterValue::Float(value) if value > 0.25));
        let canonical = view
            .state
            .as_ref()
            .expect("state")
            .project
            .track(track)
            .unwrap();
        assert!(canonical.sends().iter().any(|send| {
            send.return_track() == return_track && (send.gain() - 0.25).abs() < 0.000_01
        }));
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|state| {
                state.revision == initial_revision + 1
                    && state.project.track(track).is_some_and(|track| {
                        track
                            .sends()
                            .iter()
                            .any(|send| send.return_track() == return_track && send.gain() > 0.25)
                    })
            })
    });
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.state.as_ref().is_some_and(|state| {
                state.project.track(track).is_some_and(|track| {
                    track.sends().iter().any(|send| {
                        send.return_track() == return_track && (send.gain() - 0.25).abs() < 0.000_01
                    })
                })
            })
    });
}

fn click_toggle(
    visual: &mut gpui::VisualTestContext,
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
) {
    let selector = Box::leak(
        format!("device-{}-parameter-{}", device.value(), parameter.value()).into_boxed_str(),
    );
    let bounds = visual.debug_bounds(selector).expect("boolean control");
    let point = gpui::point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();
}

fn bool_value(
    state: &ViewState,
    track: TrackId,
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
) -> bool {
    let device = state
        .project
        .track(track)
        .and_then(|track| track.devices().iter().find(|item| item.id() == device))
        .expect("canonical device");
    let view = hex_engine::native_device_view(device.state()).expect("device view");
    match view
        .parameters
        .iter()
        .find(|item| item.descriptor.id == parameter)
        .expect("boolean parameter")
        .value
    {
        ParameterValue::Bool(value) => value,
        _ => panic!("boolean value expected"),
    }
}

fn parameter_value(
    state: &ViewState,
    track: TrackId,
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
) -> ParameterValue {
    let device = state
        .project
        .track(track)
        .and_then(|track| track.devices().iter().find(|item| item.id() == device))
        .expect("canonical device");
    hex_engine::native_device_view(device.state())
        .expect("device view")
        .parameters
        .into_iter()
        .find(|item| item.descriptor.id == parameter)
        .expect("parameter")
        .value
}

#[gpui::test]
fn discrete_device_edit_acknowledges_then_undo_restores_canonical_value(cx: &mut TestAppContext) {
    let (service, state, track, device, parameter, original) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click_toggle(&mut visual, device, parameter);
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            bool_value(
                view.state.as_ref().expect("state"),
                track,
                device,
                parameter
            ),
            !original
        );
    });
    visual.run_until_parked();

    let prior_revision =
        editor.read_with(cx, |view, _| view.state.as_ref().expect("state").revision);
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.state.as_ref().expect("state").revision > prior_revision
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.pending_values.is_empty())
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        assert_eq!(bool_value(state, track, device, parameter), original);
        assert!(view.device_panel.pending_values.is_empty());
        let Some(ParameterControl::Bool { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("boolean control remains mounted")
        };
        entity.read_with(cx, |toggle, _| assert_eq!(toggle.checked(), original));
    });
}

#[gpui::test]
fn rejected_discrete_device_edit_retires_optimistic_value(cx: &mut TestAppContext) {
    let (service, state, track, device, parameter, original) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        // Occupy the UI FIFO so the click stays queued and can be refused
        // by the real actor after this test substitutes an invalid type.
        view.pending_requests
            .push_back(Request::SetDeviceParameter {
                device,
                parameter,
                value: ParameterValue::Float(1.0),
            });
        cx.notify();
    });
    click_toggle(&mut visual, device, parameter);
    editor.update(cx, |view, cx| {
        let request = view
            .pending_requests
            .back_mut()
            .expect("queued toggle request");
        *request = Request::SetDeviceParameter {
            device,
            parameter,
            value: ParameterValue::Float(1.0),
        };
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.pending_values.is_empty())
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            bool_value(
                view.state.as_ref().expect("state"),
                track,
                device,
                parameter
            ),
            original
        );
        assert!(view.error.is_some(), "actor refusal remains visible");
        let Some(ParameterControl::Bool { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("boolean control remains mounted")
        };
        entity.read_with(cx, |toggle, _| assert_eq!(toggle.checked(), original));
    });
}

#[gpui::test]
fn matching_first_ack_does_not_retire_latest_value_while_same_parameter_edits_are_queued(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device, parameter, original) = actor_utility_fixture();
    let first_ack_value = !original;
    let original_device_state = state
        .project
        .device(device)
        .expect("device")
        .state()
        .clone();
    let acknowledged_state = hex_engine::patch_native_device_parameter(
        &original_device_state,
        parameter,
        ParameterValue::Bool(first_ack_value),
    )
    .expect("apply first acknowledged value");
    let mut project_editor = Editor::new((*state.project).clone(), 8).expect("project editor");
    project_editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: acknowledged_state,
        })
        .expect("first acknowledged edit");
    let changed_state = Arc::new(ViewState {
        project: Arc::new(project_editor.project().clone()),
        waveforms: state.waveforms.clone(),
        midi_preview: state.midi_preview.clone(),
        timeline_frames: state.timeline_frames,
        can_undo: project_editor.can_undo(),
        can_redo: project_editor.can_redo(),
        revision: state.revision + 1,
        unsaved: true,
        document_path: None,
    });
    let (handle, editor) = actor_editor_window(cx, service, changed_state, track);
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        let key = ParameterKey { device, parameter };
        view.device_panel
            .pending_values
            .insert(key, ParameterValue::Bool(first_ack_value));
        // The first `true` has been acknowledged; `false` then `true` are still
        // queued, so the newest optimistic value must survive that matching ack.
        view.pending_requests
            .push_back(Request::SetDeviceParameter {
                device,
                parameter,
                value: ParameterValue::Bool(original),
            });
        view.pending_requests
            .push_back(Request::SetDeviceParameter {
                device,
                parameter,
                value: ParameterValue::Bool(first_ack_value),
            });
        let _panel = super::view(view, cx);
        assert_eq!(
            view.device_panel.pending_values.get(&key),
            Some(&ParameterValue::Bool(first_ack_value))
        );
        assert_eq!(view.pending_requests.len(), 2);
        let Some(ParameterControl::Bool { entity, .. }) = view.device_panel.controls.get(&key)
        else {
            panic!("toggle control should render")
        };
        entity.read_with(cx, |toggle, _| {
            assert_eq!(toggle.checked(), first_ack_value)
        });
    });
    let _ = handle;
}

#[test]
fn clipped_device_card_bounds_cannot_claim_offscreen_pointer() {
    let viewport = gpui::Bounds::new(
        gpui::point(px(200.0), px(300.0)),
        gpui::size(px(400.0), px(100.0)),
    );
    let clipped_card = gpui::Bounds::new(
        gpui::point(px(100.0), px(300.0)),
        gpui::size(px(286.0), px(100.0)),
    );
    let inside_both = gpui::point(px(250.0), px(330.0));
    let offscreen_card_area = gpui::point(px(150.0), px(330.0));
    let outside_rail = gpui::point(px(250.0), px(420.0));

    assert!(super::pointer_within_visible_device_drop_bounds(
        &inside_both,
        &clipped_card,
        Some(&viewport),
    ));
    assert!(!super::pointer_within_visible_device_drop_bounds(
        &offscreen_card_area,
        &clipped_card,
        Some(&viewport),
    ));
    assert!(!super::pointer_within_visible_device_drop_bounds(
        &outside_rail,
        &clipped_card,
        Some(&viewport),
    ));
}

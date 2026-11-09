use super::*;
use gpui::{
    AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size,
};
use hex_project::{Command, Created, Editor, Project};
use hex_session::service::{Event, Request, Service, ViewState};
use std::sync::Arc;

mod accessibility_tests;
mod auto_pan_ui_tests;
mod bindings_tests;
mod chain_window_tests;
mod compressor_ui_tests;
mod delay_ui_tests;
mod device_add_ui_tests;
mod device_comparison_tests;
mod device_picker_modal_ui_tests;
mod device_picker_ui_tests;
mod device_preset_ui_tests;
mod device_reorder_tests;
mod device_theme_tests;
mod eq8_marquee_tests;
pub(in crate::editor) mod eq_ui_tests;
mod gate_ui_tests;
mod header_actions_tests;
mod header_drop_tests;
mod header_style_tests;
mod keyboard_delete_tests;
mod keyboard_navigation_tests;
mod limiter_ui_tests;
mod navigation_ui_tests;
mod operator_algorithm_ui_tests;
mod operator_context_selection_tests;
mod operator_enable_ui_tests;
mod operator_envelope_reference_ui_tests;
mod operator_filter_response_tests;
mod operator_fine_ui_tests;
#[path = "operator_harmonic_ui_tests.rs"]
mod operator_harmonic_ui_tests;
mod operator_keytrack_ui_tests;
mod operator_navigation_tests;
mod operator_options_tests;
mod operator_preview_navigation_tests;
mod operator_routing_picker_tests;
mod operator_selector_keys_ui_tests;
mod operator_tab_tests;
mod operator_tuning_ui_tests;
mod operator_voice_ui_tests;
mod operator_waveform_editor_tests;
mod overdrive_ui_tests;
mod parameter_rows_tests;
mod rattle_ui_tests;
mod reverb_ui_tests;
mod saturator_ui_tests;
mod shortcut_text_tests;
mod vst3_degraded_tests;

fn receive_service_event(service: &Service) -> Event {
    let deadline = crate::test_wait::hang_deadline();
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

fn actor_native_effect_fixture(
    type_id: hex_lattice_contract::DeviceTypeId,
) -> (Service, Arc<ViewState>, TrackId, DeviceId) {
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _initial = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "device track".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
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
            type_id: Box::new(type_id),
            before: None,
        })
        .expect("add native device request");
    let device_event = receive_service_event(&service);
    let state = device_event.state;
    let device = state.project.track(track).expect("track").devices()[0].id();
    (service, state, track, device)
}

fn actor_utility_fixture() -> (
    Service,
    Arc<ViewState>,
    TrackId,
    DeviceId,
    hex_lattice_contract::ParameterId,
    bool,
) {
    let (service, state, track, device) =
        actor_native_effect_fixture(hex_devices::utility::UTILITY_TYPE_ID);
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
    let deadline = crate::test_wait::hang_deadline();
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
fn device_card_selection_follows_clicks_and_retires_removed_devices(cx: &mut TestAppContext) {
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("instrument")
        .id();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bodies: Vec<_> = [instrument, effect]
        .into_iter()
        .map(|device| {
            let selector: &'static str =
                Box::leak(format!("device-body-{}", device.value()).into_boxed_str());
            (
                selector,
                visual.debug_bounds(selector).expect("device body"),
                visual
                    .rendered_background(selector)
                    .expect("device body paint"),
            )
        })
        .collect();
    for device in [instrument, effect] {
        let title = visual
            .debug_bounds(Box::leak(
                format!("device-drag-title-{}", device.value()).into_boxed_str(),
            ))
            .expect("device title")
            .center();
        visual.simulate_click(title, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.device_panel.selected_device, Some(device));
            assert_eq!(view.state.as_ref().expect("state").revision, revision);
            assert_eq!(view.worker_requests_pending, 0);
        });
        for (selector, bounds, paint) in &bodies {
            assert_eq!(visual.debug_bounds(selector), Some(*bounds));
            assert_eq!(visual.rendered_background(selector), Some(*paint));
        }
    }
    // Right-clicking a different card must not change the left-click selection.
    let bounds = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", instrument.value()).into_boxed_str(),
        ))
        .expect("instrument");
    let title = bounds.center();
    visual.simulate_mouse_down(title, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(title, MouseButton::Right, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(effect))
    });
    let remove = visual
        .debug_bounds(Box::leak(
            format!("device-remove-{}", effect.value()).into_boxed_str(),
        ))
        .expect("remove action");
    visual.simulate_click(remove.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        !view
            .state
            .as_ref()
            .expect("state")
            .project
            .track(track)
            .expect("track")
            .devices()
            .iter()
            .any(|device| device.id() == effect)
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, None)
    });
}

#[gpui::test]
fn device_chain_clicks_support_ranges_and_command_toggles(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let type_id = state.project.device(effect).unwrap().state().type_id();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let last = state.project.track(track).unwrap().devices()[1].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();
    let title = |visual: &mut VisualTestContext, device: DeviceId| {
        visual
            .debug_bounds(Box::leak(
                format!("device-drag-title-{}", device.value()).into_boxed_str(),
            ))
            .unwrap()
            .center()
    };
    let instrument_title = title(&mut visual, instrument);
    let effect_title = title(&mut visual, effect);
    let last_title = title(&mut visual, last);
    visual.simulate_click(instrument_title, Modifiers::default());
    visual.simulate_click(
        last_title,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            [instrument, effect, last]
                .into_iter()
                .all(|id| view.device_panel.is_selected(id))
        );
        assert_eq!(view.device_panel.selected_device, Some(last));
    });
    visual.simulate_click(
        effect_title,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.is_selected(instrument));
        assert!(view.device_panel.is_selected(effect));
        assert!(!view.device_panel.is_selected(last));
    });
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()
                .len(),
            2
        );
        assert!(view.device_panel.is_selected(instrument));
        assert!(view.device_panel.is_selected(effect));
    });
    visual.simulate_click(
        last_title,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.simulate_click(
        effect_title,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.is_selected(instrument));
        assert!(!view.device_panel.is_selected(effect));
        assert!(view.device_panel.is_selected(last));
    });
    visual.simulate_click(effect_title, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(effect));
        assert!(view.device_panel.selected_others.is_empty());
    });
    visual.simulate_click(
        last_title,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.selected_track = None;
        view.device_panel.main_selected = Some(view.project_generation);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.owner, Some(DevicePanelOwner::Main));
        assert_eq!(view.device_panel.selected_device, None);
        assert!(view.device_panel.selected_others.is_empty());
    });
}

#[gpui::test]
fn main_and_return_device_chains_support_multi_selection(cx: &mut TestAppContext) {
    let (service, state, track, return_id, first_return, _) = actor_return_utility_fixture();
    let utility = state.project.device(first_return).unwrap().state().clone();
    service
        .submit(Request::Edit(Command::InsertReturnDevice {
            return_track: return_id,
            before: None,
            state: utility.clone(),
        }))
        .unwrap();
    let second_return_state = receive_service_event(&service).state;
    let second_return = second_return_state
        .project
        .return_tracks()
        .iter()
        .find(|track| track.id() == return_id)
        .unwrap()
        .devices()[1]
        .id();
    for _ in 0..2 {
        service
            .submit(Request::Edit(Command::InsertMainDevice {
                before: None,
                state: utility.clone(),
            }))
            .unwrap();
        let state = receive_service_event(&service).state;
        if state.project.main_devices().len() == 2 {
            let main = [
                state.project.main_devices()[0].id(),
                state.project.main_devices()[1].id(),
            ];
            let (handle, editor) = actor_editor_window(cx, service, state, track);
            editor.update(cx, |view, cx| {
                view.focused_return = Some(return_id);
                cx.notify();
            });
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.simulate_resize(size(px(1400.0), px(800.0)));
            visual.run_until_parked();
            let title = |visual: &mut VisualTestContext, id: DeviceId| {
                visual
                    .debug_bounds(Box::leak(
                        format!("device-drag-title-{}", id.value()).into_boxed_str(),
                    ))
                    .unwrap()
                    .center()
            };
            let first = title(&mut visual, first_return);
            let second = title(&mut visual, second_return);
            visual.simulate_click(first, Modifiers::default());
            visual.simulate_click(
                second,
                Modifiers {
                    shift: true,
                    ..Default::default()
                },
            );
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert!(view.device_panel.is_selected(first_return));
                assert!(view.device_panel.is_selected(second_return));
            });
            editor.update(cx, |view, cx| {
                view.focused_return = None;
                view.selected_track = None;
                view.device_panel.main_selected = Some(view.project_generation);
                cx.notify();
            });
            visual.run_until_parked();
            let first = title(&mut visual, main[0]);
            let second = title(&mut visual, main[1]);
            visual.simulate_click(first, Modifiers::default());
            visual.simulate_click(
                second,
                Modifiers {
                    platform: true,
                    ..Default::default()
                },
            );
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert_eq!(view.device_panel.owner, Some(DevicePanelOwner::Main));
                assert!(view.device_panel.is_selected(main[0]));
                assert!(view.device_panel.is_selected(main[1]));
                assert!(!view.device_panel.is_selected(first_return));
            });
            return;
        }
    }
    panic!("main fixture did not create two devices");
}

#[gpui::test]
fn device_keyboard_copy_cut_paste_operates_on_selected_group(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, first, _, _) = actor_utility_fixture();
    let type_id = state.project.device(first).unwrap().state().type_id();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let second = state.project.track(track).unwrap().devices()[1].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();
    let pos = |visual: &mut VisualTestContext, id: DeviceId| {
        visual
            .debug_bounds(Box::leak(
                format!("device-drag-title-{}", id.value()).into_boxed_str(),
            ))
            .unwrap()
            .center()
    };
    let one = pos(&mut visual, first);
    let two = pos(&mut visual, second);
    visual.simulate_click(one, Modifiers::default());
    visual.simulate_click(
        two,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-c");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-v");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()
            .len()
            == 4
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-x");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()
            .len()
            == 2
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-v");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()
            .len()
            == 4
    });
}

#[gpui::test]
fn device_paste_during_playback_applies_to_actual_callback(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::Acquire;
    let _guard = eq_ui_tests::graph_layout_tests::status_fixture_guard();
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(800.0)));
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RenameTrack {
            track,
            name: "device paste ready".into()
        })))
    });
    let (mut output, mut backend) =
        eq_ui_tests::held_pointer_tests::attach_actual_output(&editor, cx, &mut visual);
    let title = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", device.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_click(title, Modifiers::default());
    visual.simulate_keystrokes("space");
    let deadline = crate::test_wait::hang_deadline();
    while !backend.status().unwrap().playing.load(Acquire) {
        eq_ui_tests::held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        assert!(
            std::time::Instant::now() < deadline,
            "playback did not start"
        );
    }
    visual.simulate_keystrokes("cmd-c");
    loop {
        eq_ui_tests::held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        if editor.read_with(cx, |view, _| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "copy did not complete"
        );
    }
    visual.simulate_keystrokes("cmd-v");
    loop {
        eq_ui_tests::held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        let applied = editor.read_with(cx, |view, _| {
            view.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .track(track)
                    .is_some_and(|track| track.devices().len() == 2)
                    && view.applied_project_revision() == Some(state.revision)
            })
        });
        if applied && backend.status().unwrap().playing.load(Acquire) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pasted device was not applied during playback: {:?}",
            editor.read_with(cx, |view, _| view.error.clone())
        );
    }
}

#[gpui::test]
fn device_copy_pastes_into_an_empty_track_chain(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _state, source, device, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "destination".into(),
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let destination = state
        .project
        .tracks()
        .iter()
        .find(|track| track.id() != source)
        .unwrap()
        .id();
    let (handle, editor) = actor_editor_window(cx, service, state, source);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let title = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", device.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_click(title, Modifiers::default());
    visual.simulate_keystrokes("cmd-c");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.update(cx, |view, cx| {
        view.selected_track = Some(destination);
        cx.notify();
    });
    visual.run_until_parked();
    let empty = visual
        .debug_bounds(Box::leak(
            format!("device-drop-end-{}", destination.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_click(empty, Modifiers::default());
    visual.simulate_keystrokes("cmd-v");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(destination)
            .unwrap()
            .devices()
            .len()
            == 1
    });
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(
            project.track(source).unwrap().devices()[0].state(),
            project.track(destination).unwrap().devices()[0].state()
        );
        assert_ne!(
            project.track(source).unwrap().devices()[0].id(),
            project.track(destination).unwrap().devices()[0].id()
        );
    });
}

#[gpui::test]
fn return_device_control_click_selects_without_stealing_its_gesture(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, return_track, device, _) = actor_return_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body: &'static str = Box::leak(format!("device-body-{}", device.value()).into_boxed_str());
    let body_bounds = visual.debug_bounds(body).expect("return device body");
    let body_paint = visual
        .rendered_background(body)
        .expect("return device body paint");
    let knob = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-knob", device.value()).into_boxed_str(),
        ))
        .expect("gain knob");
    visual.simulate_mouse_down(knob.center(), MouseButton::Left, Modifiers::default());
    let moved = point(knob.center().x, knob.center().y - px(12.0));
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(device));
        assert!(view.device_gesture.snapshot().is_some());
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(body), Some(body_bounds));
    assert_eq!(visual.rendered_background(body), Some(body_paint));
    assert_eq!(
        visual.rendered_background("device-frame-header"),
        Some(gpui::Background::from(gpui::rgb(
            device_layout_style().accent
        )))
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(device))
    });
    editor.update(cx, |view, cx| {
        view.focused_return = None;
        cx.notify();
    });
    pump_until(&editor, cx, |view| view.device_gesture.snapshot().is_none());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, None)
    });
    editor.update(cx, |view, cx| {
        view.device_panel.selected_device = Some(device);
        view.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, None)
    });
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
    assert_eq!(panel_after_instrument.size.height, px(229.0));
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
    cx.update(crate::editor::bind_editor_keys);
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
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.browser.search.read(cx).focus_handle().focus(window)
        });
    });

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
                replace: None,
            })
        );
    });

    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("space").unwrap(),
        is_held: false,
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::PlaybackCommand::PlayFrom(0)),
            "Space immediately after the device drop must reach transport"
        );
    });
    editor.update(cx, |view, _| view.last_transport_command = None);
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("space").unwrap(),
        is_held: true,
    });
    editor.read_with(cx, |view, _| {
        assert!(
            view.last_transport_command.is_none(),
            "held Space must not toggle playback repeatedly"
        )
    });
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("space").unwrap(),
    });
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
fn device_drop_then_space_starts_actual_callback_and_applies_device(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::Acquire;
    let _guard = eq_ui_tests::graph_layout_tests::status_fixture_guard();
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, existing, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(800.0)));
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RenameTrack {
            track,
            name: "device track ready".into(),
        })));
    });
    let (mut output, mut backend) =
        eq_ui_tests::held_pointer_tests::attach_actual_output(&editor, cx, &mut visual);
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.browser.search.read(cx).focus_handle().focus(window)
        });
    });
    let source = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .unwrap()
        .center();
    visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(source.x + px(12.0), source.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let selector = Box::leak(format!("device-drop-card-{}", existing.value()).into_boxed_str());
    let card = visual.debug_bounds(selector).unwrap();
    let destination = point(card.origin.x + px(24.0), card.center().y);
    visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("space");
    let deadline = crate::test_wait::hang_deadline();
    loop {
        eq_ui_tests::held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        let settled = editor.read_with(cx, |view, _| {
            view.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .track(track)
                    .is_some_and(|track| track.devices().len() == 2)
                    && view.applied_project_revision() == Some(state.revision)
            })
        });
        if settled && backend.status().unwrap().playing.load(Acquire) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "drop followed by Space did not both start and apply: {:?}",
            editor.read_with(cx, |view, _| view.error.clone())
        );
    }
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
    assert_eq!(panel_before.size.height, px(229.0));
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
    assert!(
        editor.read_with(cx, |view, _| view.device_panel.views[&instrument_id]
            .2
            .as_ref()
            .unwrap()
            .parameters
            .iter()
            .any(|p| p.descriptor.id.value() == 29)),
        "full carrier metadata remains available before scrolling"
    );
    editor.update(cx, |view, cx| {
        view.device_panel.parameter_rows[&instrument_id]
            .scroll
            .set_offset(point(px(0.0), px(-1_000_000.0)));
        cx.notify();
    });
    visual.run_until_parked();
    let carrier =
        Box::leak(format!("device-{}-parameter-29-slider", instrument_id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(carrier).is_some(),
        "FM4's real carrier descriptor is rendered"
    );
    let panel_after = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel_after.size.height, px(229.0));
    assert_eq!(panel_after.size.height, panel_before.size.height);
}

#[gpui::test]
fn browser_instrument_dropped_on_instrument_card_replaces_in_place_and_retires_old_ui(
    cx: &mut TestAppContext,
) {
    let (service, _state, track, effect, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ONE,
            length: hex_project::BeatTime::from_quarters(4).expect("clip length"),
        }))
        .expect("place clip before replacement");
    let initial = receive_service_event(&service).state;
    let initial_revision = initial.revision;
    let original_track = initial.project.track(track).expect("track").clone();
    let instrument = original_track.instrument().expect("default sine").id();
    let (handle, editor) = actor_editor_window(cx, service, initial, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let operator_source = visual
        .debug_bounds("browser-device-instrument-hex.instrument.operator")
        .expect("operator browser row");
    let utility_card = visual
        .debug_bounds(Box::leak(
            format!("device-drop-card-{}", effect.value()).into_boxed_str(),
        ))
        .expect("effect card");
    let operator_down = operator_source.center();
    visual.simulate_mouse_down(operator_down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(operator_down.x + px(12.0), operator_down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        point(utility_card.origin.x + px(24.0), utility_card.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let target = view
            .active_native_device_drag
            .as_ref()
            .and_then(|drag| drag.destination.get())
            .expect("drop target over effect card");
        assert_eq!(target.replace, None, "effects are not replacement targets");
    });
    let effect_drop = point(utility_card.origin.x + px(24.0), utility_card.center().y);
    visual.simulate_mouse_up(effect_drop, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_panel.message.as_deref() == Some("instrument slot is already occupied")
    });
    editor.read_with(cx, |view, _| {
        let current = view.state.as_ref().expect("state");
        assert_eq!(current.revision, initial_revision);
        assert_eq!(
            current
                .project
                .track(track)
                .expect("track")
                .instrument()
                .map(hex_project::Device::id),
            Some(instrument)
        );
    });

    let source = visual
        .debug_bounds("browser-device-instrument-hex.instrument.operator")
        .expect("operator browser row after rejected effect-card drop");
    let card_selector =
        Box::leak(format!("device-drop-card-{}", instrument.value()).into_boxed_str());
    let card = visual.debug_bounds(card_selector).expect("instrument card");
    assert_eq!(
        card.size.width,
        px(DEVICE_FRAME_WIDTH
            + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
            + device_meter::WIDTH
            + DEVICE_CHAIN_PADDING)
    );
    let painted_card = visual
        .debug_bounds(Box::leak(
            format!("instrument-device-{}", instrument.value()).into_boxed_str(),
        ))
        .expect("painted instrument card");
    assert_eq!(
        painted_card.size.width,
        px(DEVICE_FRAME_WIDTH + hex_widgets::DEVICE_FRAME_HEADER_WIDTH + device_meter::WIDTH)
    );
    let down = source.center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let sine_destination = card.center();
    visual.simulate_mouse_move(sine_destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let target = view
            .active_native_device_drag
            .as_ref()
            .and_then(|drag| drag.destination.get())
            .expect("replacement target on right side of wide card");
        assert_eq!(target.track, track);
        assert_eq!(target.before, None);
        assert_eq!(target.replace, Some(instrument));
    });
    visual.simulate_mouse_up(sine_destination, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .and_then(|track| track.instrument())
            .is_some_and(|device| {
                device.state().type_id().as_str() == Ok("hex.instrument.operator")
            })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drop = view
            .last_enqueued_native_device_drop
            .expect("replacement drop request");
        assert_eq!(drop.project_generation, view.project_generation);
        assert_eq!(drop.revision, initial_revision);
        assert_eq!(drop.track, track);
        assert_eq!(drop.before, None);
        assert_eq!(drop.replace, Some(instrument));
        assert!(matches!(
            drop.source,
            hex_session::service::NativeDeviceDropSource::Browser {
                class: NativeDeviceClass::Instrument,
                type_id,
            } if type_id.as_str() == Ok("hex.instrument.operator")
        ));
        let current = view.state.as_ref().expect("state");
        let current_track = current.project.track(track).expect("track");
        assert_eq!(
            current_track.instrument().map(hex_project::Device::id),
            Some(instrument)
        );
        assert_eq!(current_track.devices(), original_track.devices());
        assert_eq!(current_track.clips(), original_track.clips());
        assert_eq!(current_track.midi_clips(), original_track.midi_clips());
        assert!(
            view.device_panel
                .controls
                .keys()
                .any(|key| key.device == instrument)
        );
    });

    let sine_source = visual
        .debug_bounds("browser-device-instrument-hex.instrument.sine")
        .expect("sine browser row");
    let operator_card = visual
        .debug_bounds(card_selector)
        .expect("wide Operator card");
    assert_eq!(
        operator_card.size.width,
        px(operator_panel::OPERATOR_FRAME_WIDTH
            + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
            + device_meter::WIDTH
            + DEVICE_CHAIN_PADDING)
    );
    let right_side = point(
        operator_card.origin.x + operator_card.size.width - px(16.0),
        operator_card.center().y,
    );
    let operator_view = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        hex_engine::native_device_view_for_class(
            NativeDeviceClass::Instrument,
            state
                .project
                .track(track)
                .expect("track")
                .instrument()
                .expect("operator")
                .state(),
        )
        .expect("operator parameter view")
    });
    editor.update(cx, |view, cx| {
        view.device_panel.operator_pages.insert(
            instrument,
            operator_panel::OperatorPanelState {
                selected_operator: 3,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
        );
        let parameter = operator_view.parameters[0];
        view.device_panel.pending_values.insert(
            ParameterKey {
                device: instrument,
                parameter: parameter.descriptor.id,
            },
            parameter.value,
        );
        cx.notify();
    });
    visual.run_until_parked();
    let sine_down = sine_source.center();
    visual.simulate_mouse_down(sine_down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(sine_down.x + px(12.0), sine_down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(right_side, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(right_side, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .and_then(|track| track.instrument())
            .is_some_and(|device| device.state().type_id().as_str() == Ok("hex.instrument.sine"))
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.has_control_for(instrument));
        assert!(
            !view
                .device_panel
                .pending_values
                .keys()
                .any(|key| key.device == instrument)
        );
        assert!(!view.device_panel.operator_pages.contains_key(&instrument));
        assert_ne!(
            view.device_panel.message.as_deref(),
            Some("device target is no longer available")
        );
    });

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .and_then(|track| track.instrument())
            .is_some_and(|device| {
                device.state().type_id().as_str() == Ok("hex.instrument.operator")
            })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel
                .controls
                .keys()
                .any(|key| key.device == instrument)
        );
        assert_ne!(
            view.device_panel.message.as_deref(),
            Some("device target is no longer available")
        );
    });
}

#[gpui::test]
fn eq8_panel_renders_on_regular_and_return_tracks_and_retires_on_owner_switch(
    cx: &mut TestAppContext,
) {
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _initial = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "eq8 source".into(),
        }))
        .expect("add track");
    let track = match receive_service_event(&service)
        .created
        .expect("track created")
    {
        Created::Track(track) => track,
        _ => panic!("track result"),
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static("hex.eq8")),
            before: None,
        })
        .expect("add regular EQ8");
    let regular_event = receive_service_event(&service);
    let regular_device = regular_event
        .state
        .project
        .track(track)
        .expect("regular track")
        .devices()[0]
        .id();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "eq8 return".into(),
        }))
        .expect("add return");
    let return_track = match receive_service_event(&service)
        .created
        .expect("return created")
    {
        Created::ReturnTrack(id) => id,
        _ => panic!("return result"),
    };
    service
        .submit(Request::Edit(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.eq8"),
            )
            .expect("EQ8 default state"),
        }))
        .expect("insert return EQ8");
    let state = receive_service_event(&service).state;
    let return_device = state
        .project
        .return_tracks()
        .iter()
        .find(|candidate| candidate.id() == return_track)
        .expect("return owner")
        .devices()[0]
        .id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let regular_body = Box::leak(format!("eq8-body-{}", regular_device.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(regular_body).is_some(),
        "regular EQ8 body"
    );
    let scale = visual.update(|window, _| window.scale_factor());
    assert_eq!(
        visual
            .rendered_surface_geometry("device-plot-section")
            .expect("shared rounded EQ8 graph")
            .0
            .top_left,
        px(hex_widgets::DEVICE_SECTION_RADIUS).scale(scale)
    );
    let eq_body = visual.debug_bounds(regular_body).unwrap();
    for id in [3, 4, 5, 202] {
        let bounds = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{id}", regular_device.value()).into_boxed_str(),
            ))
            .expect("selected-band control");
        assert_eq!(
            bounds.size,
            size(
                px(hex_widgets::MICRO_DIAL_WIDTH),
                px(hex_widgets::MICRO_DIAL_HEIGHT)
            )
        );
        assert!(bounds.origin.x >= eq_body.origin.x && bounds.right() <= eq_body.right());
        assert!(
            bounds.origin.y >= eq_body.origin.y
                && bounds.bottom_left().y <= eq_body.bottom_left().y
        );
    }
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    visual.run_until_parked();
    let return_body = Box::leak(format!("eq8-body-{}", return_device.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(return_body).is_some(),
        "Return EQ8 body"
    );
    editor.read_with(cx, |view, _| {
        assert!(
            !view.device_panel.eq8_panels.contains_key(&regular_device),
            "changing owner retires cached regular panel state"
        );
        assert!(view.device_panel.eq8_panels.contains_key(&return_device));
    });
    editor.update(cx, |view, cx| {
        view.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(regular_body).is_some(),
        "regular EQ8 returns"
    );
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.eq8_panels.contains_key(&regular_device));
        assert!(!view.device_panel.eq8_panels.contains_key(&return_device));
    });
}

#[gpui::test]
fn operator_panel_exposes_shared_controls_across_operator_envelope_and_routing_pages(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _state, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove default instrument");
    let _empty = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "preview owner".into(),
        }))
        .expect("add a second device-panel owner");
    let second_state = receive_service_event(&service);
    let second_track = match second_state.created.expect("created track") {
        Created::Track(track) => track,
        _ => panic!("created track result"),
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .expect("add native Operator");
    let added = receive_service_event(&service);
    let state = added.state;
    let instrument = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("Operator instrument");
    assert_eq!(
        instrument.state().type_id().as_str(),
        Ok("hex.instrument.operator")
    );
    let instrument_id = instrument.id();
    // Custom authored routing retains all new global controls without exposing a matrix.
    service
        .submit(Request::SetDeviceParameter {
            device: instrument_id,
            parameter: hex_lattice_contract::ParameterId::from_static(17),
            value: ParameterValue::Float(0.31),
        })
        .unwrap();
    let custom = receive_service_event(&service);
    assert!(custom.error.is_none());
    let state = custom.state;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let frame_selector =
        Box::leak(format!("instrument-device-{}", instrument_id.value()).into_boxed_str());
    let frame = visual.debug_bounds(frame_selector).expect("Operator frame");
    assert_eq!(
        frame.size.width,
        px(operator_panel::OPERATOR_FRAME_WIDTH
            + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
            + device_meter::WIDTH)
    );
    assert_eq!(frame.size.height, px(200.0));
    // One passive stereo meter follows the device: left and right bars, no gain handles.
    let meter_selector =
        Box::leak(format!("device-{}-output-meter", instrument_id.value()).into_boxed_str());
    assert!(visual.debug_bounds(meter_selector).is_some());
    for side in ["left", "right"] {
        let selector = Box::leak(format!("{meter_selector}-{side}").into_boxed_str());
        let bar = visual.debug_bounds(selector).expect("channel bar");
        assert_eq!(bar.size.width, px(hex_widgets::STEREO_PEAK_BAR_WIDTH));
        assert!(
            bar.origin.x
                >= frame.origin.x
                    + px(operator_panel::OPERATOR_FRAME_WIDTH
                        + hex_widgets::DEVICE_FRAME_HEADER_WIDTH)
        );
        assert!(bar.right() <= frame.right());
    }
    for side in ["input", "output"] {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("{meter_selector}-{side}-meter-gain-handle").into_boxed_str()
                ))
                .is_none()
        );
    }
    let body_selector =
        Box::leak(format!("operator-body-{}", instrument_id.value()).into_boxed_str());
    let body = visual.debug_bounds(body_selector).expect("Operator body");
    assert_eq!(body.size.height, px(198.0));
    let shell_selector =
        Box::leak(format!("operator-shell-{}", instrument_id.value()).into_boxed_str());
    let shell = visual.debug_bounds(shell_selector).expect("operator shell");
    assert_eq!(
        shell.size.width,
        px(crate::device_composition::VECTOR_SHELL_WIDTH)
    );
    let envelope_graph = visual
        .debug_bounds("operator-envelope-graph")
        .expect("amp envelope is the default contextual view");
    assert!(
        envelope_graph.size.width >= px(180.0)
            && envelope_graph.size.width
                <= px(crate::device_composition::VECTOR_DETAIL_WIDTH + 30.0)
    );
    assert!(
        envelope_graph.size.height >= px(80.0),
        "amp graph bounds: {:?}",
        envelope_graph.size
    );
    let tab = visual
        .debug_bounds(Box::leak(
            format!("operator-page-{}-envelope", instrument_id.value()).into_boxed_str(),
        ))
        .expect("flat envelope tab");
    assert!(
        tab.origin.y >= envelope_graph.origin.y + envelope_graph.size.height,
        "page tabs belong beneath the envelope plot"
    );
    let detail = visual
        .debug_bounds(Box::leak(
            format!("operator-detail-{}", instrument_id.value()).into_boxed_str(),
        ))
        .expect("central contextual detail");
    let preview = visual
        .debug_bounds(Box::leak(
            format!("operator-preview-{}", instrument_id.value()).into_boxed_str(),
        ))
        .expect("right side control column");
    assert!(shell.origin.x + shell.size.width <= detail.origin.x);
    assert!(detail.origin.x + detail.size.width <= preview.origin.x);
    for section in ["lfo", "filter", "feedback", "global"] {
        let row = visual
            .debug_bounds(Box::leak(
                format!("operator-preview-row-{}-{section}", instrument_id.value())
                    .into_boxed_str(),
            ))
            .expect("reference-ordered right-side section");
        assert_eq!(row.size.height, px(40.0));
        assert!(row.origin.x >= preview.origin.x);
        assert!(row.origin.x + row.size.width <= preview.origin.x + preview.size.width);
    }
    for field in 0..6 {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("operator-preview-{}-{field}", instrument_id.value()).into_boxed_str()
                ))
                .is_none(),
            "native Vector omits unsupported preview dials"
        );
    }
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("operator-preview-{}-toggle-0", instrument_id.value()).into_boxed_str()
            ))
            .is_none()
    );
    // Keep the narrow-window geometry checks above, then expose the right rail
    // before testing a mouse click on the wider companion inspector.
    visual.simulate_resize(size(px(1440.0), px(540.0)));
    visual.run_until_parked();
    let route_link = Box::leak(
        format!("operator-preview-{}-route-link", instrument_id.value()).into_boxed_str(),
    );
    let route_bounds = visual
        .debug_bounds(route_link)
        .expect("global routing link");
    assert!(route_bounds.origin.x + route_bounds.size.width <= px(1440.0));
    visual.simulate_click(route_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&instrument_id].page,
            operator_panel::OperatorPage::Envelope,
            "routing picker preserves the selected editor context"
        );
    });
    let envelope_tab =
        Box::leak(format!("operator-page-{}-envelope", instrument_id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(envelope_tab).is_some(),
        "routing popup leaves the envelope working surface mounted"
    );
    let shell = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-slider", instrument_id.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(shell.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    let background =
        operator_context_selection_tests::row_background(&mut visual, instrument_id, 0);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    let envelope_bounds = visual.debug_bounds(envelope_tab).expect("amp envelope tab");
    visual.simulate_click(envelope_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_resize(size(px(1440.0), px(540.0)));
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.selected_track = Some(second_track);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    for operator in 0..4 {
        let row = Box::leak(
            format!("operator-row-{}-{operator}", instrument_id.value()).into_boxed_str(),
        );
        let bounds = visual.debug_bounds(row).expect("operator row");
        assert_eq!(bounds.size.height, px(44.0));
        let level_id = operator * 3 + 2;
        let slider = Box::leak(
            format!(
                "device-{}-parameter-{level_id}-slider",
                instrument_id.value()
            )
            .into_boxed_str(),
        );
        let bounds = visual.debug_bounds(slider).expect("filled level slider");
        assert_eq!(bounds.size, size(px(58.0), px(14.0)));
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{level_id}-knob", instrument_id.value())
                        .into_boxed_str(),
                ))
                .is_none(),
            "level no longer reserves a rotary control"
        );
    }
    for parameter in [1, 2, 4, 5, 7, 8, 10, 11] {
        let readout = Box::leak(
            format!(
                "device-{}-parameter-{parameter}-value",
                instrument_id.value()
            )
            .into_boxed_str(),
        );
        assert!(
            visual.debug_bounds(readout).is_some(),
            "shell parameter {parameter}"
        );
    }

    let envelope_tab =
        Box::leak(format!("operator-page-{}-envelope", instrument_id.value()).into_boxed_str());
    let oscillator_tab =
        Box::leak(format!("operator-page-{}-oscillator", instrument_id.value()).into_boxed_str());
    let oscillator_tab_bounds = visual
        .debug_bounds(oscillator_tab)
        .expect("oscillator detail remains available");
    visual.simulate_click(oscillator_tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let waveform_preview = visual
        .debug_bounds("operator-waveform-cycle")
        .expect("selected oscillator cycle preview");
    assert!(
        (px(200.0)..=px(crate::device_composition::VECTOR_DETAIL_WIDTH + 30.0))
            .contains(&waveform_preview.size.width)
    );
    open_operator_options(&mut visual, instrument_id);
    assert_eq!(waveform_preview.size.height, px(48.0));
    let ratio = hex_lattice_contract::ParameterId::from_static(1);
    enter_typed_value(&mut visual, instrument_id, ratio, "invalid");
    let fix_selector =
        Box::leak(format!("device-{}-parameter-62", instrument_id.value()).into_boxed_str());
    let fix_bounds = visual
        .debug_bounds(fix_selector)
        .expect("operator fixed toggle");
    visual.simulate_click(fix_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let Some(ParameterControl::Bool { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: instrument_id,
                parameter: hex_lattice_contract::ParameterId::from_static(62),
            })
        else {
            panic!("operator fixed toggle remains cached")
        };
        entity.read_with(cx, |toggle, _| {
            assert!(
                !toggle.checked(),
                "hidden invalid ratio blocks fixed-mode switch"
            )
        });
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: instrument_id,
                parameter: ratio,
            })
        else {
            panic!("ratio field remains cached")
        };
        entity.read_with(cx, |control, _| assert!(control.is_editing()));
    });
    enter_typed_value(&mut visual, instrument_id, ratio, "1");

    let phase = hex_lattice_contract::ParameterId::from_static(3);
    enter_typed_value(&mut visual, instrument_id, phase, "invalid");
    let tab_bounds = visual.debug_bounds(envelope_tab).expect("envelope tab");
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.device_panel.operator_pages[&instrument_id].page,
            operator_panel::OperatorPage::Oscillator,
            "invalid numeric drafts block page navigation"
        );
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: instrument_id,
                parameter: phase,
            })
        else {
            panic!("phase field remains cached")
        };
        entity.read_with(cx, |control, _| assert!(control.is_editing()));
    });
    enter_typed_value(&mut visual, instrument_id, phase, "0");

    let osc_bounds = visual
        .debug_bounds(oscillator_tab)
        .expect("oscillator tab remains available");
    visual.simulate_click(osc_bounds.center(), Modifiers::default());
    visual.run_until_parked();

    let waveform_parameter = ParameterKey {
        device: instrument_id,
        parameter: hex_lattice_contract::ParameterId::from_static(57),
    };
    let waveform_selector = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Choice { entity, .. }) =
            view.device_panel.controls.get(&waveform_parameter)
        else {
            panic!("operator waveform selector should be shared and cached")
        };
        entity.clone()
    });
    let waveform_bounds = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-57", instrument_id.value()).into_boxed_str(),
        ))
        .expect("waveform selector bounds");
    visual.simulate_click(waveform_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    waveform_selector.read_with(cx, |selector, _| {
        assert!(
            selector.is_open(),
            "waveform selector opens before navigation"
        )
    });

    let tab_bounds = visual.debug_bounds(envelope_tab).expect("envelope tab");
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    waveform_selector.read_with(cx, |selector, _| {
        assert!(
            !selector.is_open(),
            "page navigation dismisses selector popup"
        )
    });
    assert!(visual.debug_bounds("operator-envelope-graph").is_some());
    for parameter in 33..=38 {
        let selector = Box::leak(
            format!(
                "device-{}-parameter-{parameter}-value",
                instrument_id.value()
            )
            .into_boxed_str(),
        );
        assert!(
            visual.debug_bounds(selector).is_some(),
            "envelope {parameter}"
        );
    }

    let routing_tab =
        Box::leak(format!("operator-page-{}-routing", instrument_id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(routing_tab).is_none(),
        "routing has no standalone tab"
    );
    let routing_rail = Box::leak(
        format!("operator-preview-row-{}-global", instrument_id.value()).into_boxed_str(),
    );
    let tab_bounds = visual.debug_bounds(routing_rail).expect("routing rail");
    let routing = visual.debug_bounds(route_link).expect("routing picker");
    visual.simulate_click(
        point(routing.right() + px(2.0), tab_bounds.center().y),
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("operator-routing-{}", instrument_id.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(visual.debug_bounds("operator-routing-header").is_none());
    for parameter in [
        14, 15, 16, 17, 19, 20, 21, 22, 24, 25, 26, 27, 29, 30, 31, 32, 61,
    ] {
        let selector = Box::leak(
            format!("device-{}-parameter-{parameter}", instrument_id.value()).into_boxed_str(),
        );
        assert!(
            visual.debug_bounds(selector).is_none(),
            "legacy routing is preserved without a matrix editor"
        );
    }
    for parameter in
        (601..=608).chain((620..=639).filter(|parameter| !matches!(*parameter, 626 | 634)))
    {
        let selector = Box::leak(
            format!("device-{}-parameter-{parameter}", instrument_id.value()).into_boxed_str(),
        );
        assert!(
            visual.debug_bounds(selector).is_some(),
            "global parameter {parameter}"
        );
    }
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&instrument_id].page,
            operator_panel::OperatorPage::Routing
        );
    });

    let background =
        operator_context_selection_tests::row_background(&mut visual, instrument_id, 1);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    let waveform_b = ParameterKey {
        device: instrument_id,
        parameter: hex_lattice_contract::ParameterId::from_static(58),
    };
    let waveform_selector = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Choice { entity, .. }) =
            view.device_panel.controls.get(&waveform_b)
        else {
            panic!("operator b waveform selector should be shared and cached")
        };
        entity.clone()
    });
    let waveform_bounds = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-58", instrument_id.value()).into_boxed_str(),
        ))
        .expect("operator b waveform selector bounds");
    visual.simulate_click(waveform_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    waveform_selector.read_with(cx, |selector, _| assert!(selector.is_open()));
    let tab_bounds = visual.debug_bounds(envelope_tab).expect("envelope tab");
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    waveform_selector.read_with(cx, |selector, _| {
        assert!(
            !selector.is_open(),
            "page navigation dismisses selector popup"
        )
    });
    let background =
        operator_context_selection_tests::row_background(&mut visual, instrument_id, 1);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&instrument_id].page,
            operator_panel::OperatorPage::Envelope,
            "choosing another oscillator keeps the amp envelope visible"
        );
    });
    let osc_bounds = visual
        .debug_bounds(oscillator_tab)
        .expect("oscillator tab remains available");
    visual.simulate_click(osc_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-58", instrument_id.value()).into_boxed_str()
            ))
            .is_some(),
        "selecting oscillator b reveals its own waveform control"
    );
    editor.read_with(cx, |view, _| {
        let page = view.device_panel.operator_pages[&instrument_id];
        assert_eq!(page.selected_operator, 1);
        assert_eq!(page.page, operator_panel::OperatorPage::Oscillator);
    });
}

#[gpui::test]
fn operator_envelope_graph_keyboard_edit_is_canonical_undoable_and_persistent(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _state, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove default instrument");
    let _empty = receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .expect("add native Operator");
    let initial = receive_service_event(&service).state;
    let device = initial
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("Operator")
        .id();
    let get = |state: &ViewState, parameter: u32| {
        operator_parameter_value(state, track, device, parameter)
    };
    let original_peak = get(&initial, 34);
    let original_attack = get(&initial, 36);
    let envelope_values: Vec<f32> = (33..=38)
        .map(|parameter| match get(&initial, parameter) {
            ParameterValue::Float(value) => value,
            _ => panic!("Operator envelope value is not numeric"),
        })
        .collect();
    let attack_x = |seconds: f32| {
        ((f64::from(seconds) / 0.001).ln_1p() / (20.0_f64 / 0.001).ln_1p()) as f32 / 3.0
    };
    let stage_x =
        |seconds: f32| ((f64::from(seconds) / 0.001).ln() / (60.0_f64 / 0.001).ln()) as f32 / 3.0;
    let path = std::env::temp_dir().join(format!(
        "hex-operator-envelope-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos(),
    ));
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save initial Operator fixture");
    let saved = receive_service_event(&service);
    assert!(saved.save_complete);
    assert!(saved.error.is_none());
    let (handle, editor) = actor_editor_window(cx, service, saved.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let env_tab = Box::leak(format!("operator-page-{}-envelope", device.value()).into_boxed_str());
    let tab_bounds = visual.debug_bounds(env_tab).expect("envelope tab");
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let graph = visual
        .debug_bounds("operator-envelope-graph")
        .expect("editable Operator envelope graph");
    let width = graph.size.width - px(14.0);
    let peak = point(
        graph.origin.x + px(7.0) + width * attack_x(envelope_values[3]),
        graph.origin.y + px(7.0),
    );
    visual.simulate_click(peak, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("right");
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view
                .state
                .as_ref()
                .is_some_and(|state| get(state, 36) != original_attack)
    });
    let edited_attack = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("edited state");
        assert_eq!(
            get(state, 34),
            original_peak,
            "horizontal peak edit preserves level"
        );
        let ParameterValue::Float(attack) = get(state, 36) else {
            panic!("attack remains numeric")
        };
        assert!(
            attack
                > match original_attack {
                    ParameterValue::Float(value) => value,
                    _ => -1.0,
                }
        );
        attack
    });

    // A terminal commit must not fit the changed total duration. Cumulative
    // decay follows attack while release stays in its fixed final third.
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let peak = operator_panel::envelope::displayed_node(view, device, 0, 1, cx).unwrap();
        let decay = operator_panel::envelope::displayed_node(view, device, 0, 2, cx).unwrap();
        let release = operator_panel::envelope::displayed_node(view, device, 0, 3, cx).unwrap();
        assert!(
            (peak[0] - attack_x(edited_attack)).abs() < 1e-6,
            "commit preserves zoom"
        );
        assert!(
            (decay[0] - (attack_x(edited_attack) + stage_x(envelope_values[4]))).abs() < 1e-6,
            "decay follows cumulative attack"
        );
        assert!(
            (release[0] - (2.0 / 3.0 + stage_x(envelope_values[5]))).abs() < 1e-6,
            "release stays in fixed final section without fitting"
        );
    });

    editor.update(cx, |view, cx| view.save_project(cx));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
            && !view.saving_project
            && view.state.as_ref().is_some_and(|state| !state.unsaved)
    });
    let reload = Service::new(48_000, 8, 8_000_000).expect("reload service");
    let _initial = receive_service_event(&reload);
    reload
        .submit(Request::Open(path.clone()))
        .expect("reopen saved Operator project");
    let reopened = receive_service_event(&reload).state;
    assert_eq!(get(&reopened, 36), ParameterValue::Float(edited_attack));

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| get(state, 36) == original_attack)
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            get(view.state.as_ref().expect("undo state"), 34),
            original_peak
        );
    });
    let initial_parameter = hex_lattice_contract::ParameterId::from_static(33);
    enter_typed_value(&mut visual, device, initial_parameter, "invalid");
    let graph = visual
        .debug_bounds("operator-envelope-graph")
        .expect("envelope graph remains visible during numeric draft");
    let peak = point(
        graph.origin.x + px(7.0) + (graph.size.width - px(14.0)) * attack_x(envelope_values[3]),
        graph.origin.y + px(7.0),
    );
    let moved = point(peak.x + px(12.0), peak.y);
    visual.simulate_mouse_down(peak, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_gesture.snapshot().is_none());
        assert!(view.device_panel.pending_values.is_empty());
        assert_eq!(
            get(view.state.as_ref().expect("draft state"), 33),
            ParameterValue::Float(0.0)
        );
        let key = ParameterKey {
            device,
            parameter: initial_parameter,
        };
        let Some(ParameterControl::Float { entity, .. }) = view.device_panel.controls.get(&key)
        else {
            panic!("Operator initial value field remains cached")
        };
        entity.read_with(cx, |control, _| {
            assert!(
                control.is_editing(),
                "invalid numeric draft stays available for repair"
            )
        });
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    let _ = std::fs::remove_file(path);
}

#[gpui::test]
fn operator_envelope_release_drag_cancels_and_commits_after_outside_release(
    cx: &mut TestAppContext,
) {
    let (service, _state, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove default instrument");
    let _empty = receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .expect("add native Operator");
    let _operator_added = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "second device owner".into(),
        }))
        .expect("add second owner");
    let second_added = receive_service_event(&service);
    let second_track = match second_added.created.expect("created track") {
        Created::Track(track) => track,
        _ => panic!("created track result"),
    };
    let state = second_added.state;
    let device = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("Operator")
        .id();
    let float_value = |state: &ViewState, parameter| match operator_parameter_value(
        state, track, device, parameter,
    ) {
        ParameterValue::Float(value) => value,
        _ => panic!("Operator envelope parameter is numeric"),
    };
    let original_release = float_value(&state, 38);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let env_tab = Box::leak(format!("operator-page-{}-envelope", device.value()).into_boxed_str());
    let tab = visual.debug_bounds(env_tab).expect("envelope tab");
    visual.simulate_click(tab.center(), Modifiers::default());
    visual.run_until_parked();

    let release_endpoint = |visual: &mut VisualTestContext, cx: &mut TestAppContext| {
        let graph = visual
            .debug_bounds("operator-envelope-graph")
            .expect("envelope graph");
        let x = editor.read_with(cx, |view, cx| {
            operator_panel::envelope::displayed_node(view, device, 0, 3, cx).unwrap()[0]
        });
        point(
            graph.origin.x + px(7.0) + (graph.size.width - px(14.0)) * x,
            graph.origin.y + graph.size.height - px(7.0),
        )
    };

    let start = release_endpoint(&mut visual, cx);
    let moved = point(start.x + px(16.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    // Pointer feedback is already in the retained graph and numeric entity
    // before another whole-editor layout or worker acknowledgement is pumped.
    let bounds = visual
        .debug_bounds("operator-envelope-graph")
        .expect("graph bounds");
    let feedback_x = f32::from(moved.x - bounds.origin.x - px(7.0))
        / f32::from(bounds.size.width - px(14.0))
        + 0.02;
    editor.update(cx, |view, cx| {
        operator_panel::envelope::dispatch_change(view, device, 0, 3, [feedback_x, 0.0], cx);
        let node = operator_panel::envelope::displayed_node(view, device, 0, 3, cx)
            .expect("immediate dragged node");
        assert!(
            (node[0] - feedback_x).abs() < 1e-5,
            "node follows this input turn"
        );
        let key = ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(38),
        };
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&key] else {
            panic!("release readout");
        };
        let ParameterValue::Float(pending) = view.device_panel.pending_values[&key] else {
            panic!("pending release");
        };
        assert_eq!(entity.read(cx).value(), f64::from(pending));
        assert_eq!(
            float_value(view.state.as_ref().expect("canonical state"), 38),
            original_release
        );
    });

    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let snapshot = view.device_gesture.snapshot().expect("release drag begins");
        assert_eq!(
            snapshot.target,
            GestureTarget::DeviceParameter {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(38),
            }
        );
        assert!(
            view.device_panel
                .pending_values
                .contains_key(&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(38),
                })
        );
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view
                .state
                .as_ref()
                .is_some_and(|state| float_value(state, 38) == original_release)
    });
    editor.read_with(cx, |view, _| {
        assert!(
            !view
                .device_panel
                .pending_values
                .contains_key(&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(38),
                })
        );
    });

    let start = release_endpoint(&mut visual, cx);
    let graph = visual
        .debug_bounds("operator-envelope-graph")
        .expect("graph after cancel");
    let outside = point(graph.origin.x + graph.size.width + px(16.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_gesture.snapshot().is_some(),
            "pointer capture continues past the plot edge"
        );
    });
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view
                .state
                .as_ref()
                .is_some_and(|state| float_value(state, 38) > original_release)
    });
    let committed_release = editor.read_with(cx, |view, _| {
        float_value(view.state.as_ref().expect("committed state"), 38)
    });
    assert_eq!(
        committed_release, 60.0,
        "outside release clamps to hard maximum"
    );

    let start = release_endpoint(&mut visual, cx);
    let moved = point(start.x - px(12.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_some()));
    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .get_mut(&device)
            .expect("Operator page")
            .page = operator_panel::OperatorPage::Oscillator;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| view.device_gesture.snapshot().is_none());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            float_value(view.state.as_ref().expect("page state"), 38),
            committed_release
        );
        assert!(
            !view
                .device_panel
                .pending_values
                .contains_key(&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(38),
                })
        );
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel
                .operator_pages
                .get(&device)
                .expect("page")
                .page,
            operator_panel::OperatorPage::Oscillator
        );
        assert!(view.device_panel.operator_envelope_graphs.is_empty());
    });

    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .get_mut(&device)
            .expect("Operator page")
            .page = operator_panel::OperatorPage::Envelope;
        cx.notify();
    });
    visual.run_until_parked();
    let start = release_endpoint(&mut visual, cx);
    let moved = point(start.x - px(12.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.selected_track = Some(second_track);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.owner,
            Some(DevicePanelOwner::Track(track))
        );
        assert!(
            view.device_gesture.snapshot().is_some(),
            "owner switching keeps the active graph visible until its gesture ends"
        );
    });
    assert!(visual.debug_bounds("operator-envelope-graph").is_some());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view.device_panel.owner == Some(DevicePanelOwner::Track(second_track))
    });
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.pending_values.is_empty());
        assert!(view.device_panel.operator_envelope_graphs.is_empty());
    });
}

#[gpui::test]
fn operator_shell_controls_preserve_edits_across_explicit_context_selection(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _state, track, utility, utility_parameter, utility_enabled) =
        actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove default instrument");
    let _empty = receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .expect("add native Operator");
    let state = receive_service_event(&service).state;
    let operator = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("Operator instrument")
        .id();
    let operator_level = |state: &ViewState, parameter: u32| {
        let instrument = state
            .project
            .track(track)
            .and_then(hex_project::Track::instrument)
            .expect("Operator instrument");
        hex_engine::native_device_view_for_class(NativeDeviceClass::Instrument, instrument.state())
            .expect("Operator view")
            .parameters
            .into_iter()
            .find(|item| item.descriptor.id.value() == parameter)
            .expect("Operator parameter")
            .value
    };
    let original_d_level = operator_level(&state, 11);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    editor.update(cx, |view, cx| {
        view.device_panel.operator_pages.insert(
            operator,
            operator_panel::OperatorPanelState {
                selected_operator: 0,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
        );
        cx.notify();
    });
    visual.run_until_parked();
    let oscillator_tab =
        Box::leak(format!("operator-page-{}-oscillator", operator.value()).into_boxed_str());
    assert!(visual.debug_bounds(oscillator_tab).is_none());
    let shell = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-slider", operator.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(shell.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    let background = operator_context_selection_tests::row_background(&mut visual, operator, 0);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    let tab = visual.debug_bounds(oscillator_tab).unwrap();
    visual.simulate_click(tab.center(), Modifiers::default());
    visual.run_until_parked();

    let waveform_a = ParameterKey {
        device: operator,
        parameter: hex_lattice_contract::ParameterId::from_static(57),
    };
    let waveform_selector = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Choice { entity, .. }) =
            view.device_panel.controls.get(&waveform_a)
        else {
            panic!("operator a waveform selector is cached")
        };
        entity.clone()
    });
    let waveform = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-57", operator.value()).into_boxed_str(),
        ))
        .expect("operator a waveform selector");
    visual.simulate_click(waveform.center(), Modifiers::default());
    visual.run_until_parked();
    waveform_selector.read_with(cx, |selector, _| assert!(selector.is_open()));

    for selected in [1_usize, 2, 3, 0] {
        let parameter = selected as u32 * 3 + 2;
        let knob = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{parameter}-slider", operator.value())
                    .into_boxed_str(),
            ))
            .expect("Operator shell level slider");
        let background =
            operator_context_selection_tests::row_background(&mut visual, operator, selected);
        visual.simulate_click(background, Modifiers::default());
        visual.simulate_click(knob.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_panel.operator_pages[&operator],
                operator_panel::OperatorPanelState {
                    selected_operator: selected,
                    page: operator_panel::OperatorPage::Oscillator,
                    ..Default::default()
                }
            );
        });
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{}", operator.value(), 57 + selected)
                        .into_boxed_str(),
                ))
                .is_some(),
            "the selected oscillator's center detail is shown"
        );
    }
    waveform_selector.read_with(cx, |selector, _| {
        assert!(!selector.is_open(), "context changes dismiss the old popup")
    });

    open_operator_options(&mut visual, operator);
    let phase_a = hex_lattice_contract::ParameterId::from_static(3);
    enter_typed_value(&mut visual, operator, phase_a, "invalid");
    let d_level = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-11-slider", operator.value()).into_boxed_str(),
        ))
        .expect("operator d level slider");
    visual.simulate_click(d_level.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.device_panel.operator_pages[&operator].selected_operator,
            0
        );
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: operator,
                parameter: phase_a,
            })
        else {
            panic!("operator a phase field remains cached")
        };
        entity.read_with(cx, |control, _| assert!(control.is_editing()));
    });
    let phase_a_value = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-3-value", operator.value()).into_boxed_str(),
        ))
        .expect("operator a phase value");
    visual.simulate_click(phase_a_value.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();

    let d_level = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-11-slider", operator.value()).into_boxed_str(),
        ))
        .expect("operator d level slider");
    let start = point(d_level.center().x, d_level.center().y);
    let moved = point(start.x, start.y - px(30.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&operator].selected_operator,
            0
        );
    });
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&operator].selected_operator,
            0
        );
        let gesture = view
            .device_gesture
            .snapshot()
            .expect("shell drag starts its parameter gesture");
        assert_eq!(
            gesture.target,
            GestureTarget::DeviceParameter {
                device: operator,
                parameter: hex_lattice_contract::ParameterId::from_static(11),
            }
        );
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_ne!(
            operator_level(view.state.as_ref().expect("state"), 11),
            original_d_level,
            "dragging the shell control changes its own parameter"
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .is_some_and(|state| operator_level(state, 11) == original_d_level)
    });

    let original_b_level = editor.read_with(cx, |view, _| {
        operator_level(view.state.as_ref().expect("state"), 5)
    });
    let b_level = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-5-slider", operator.value()).into_boxed_str(),
        ))
        .expect("operator b level slider");
    visual.simulate_click(b_level.center(), Modifiers::default());
    // A filled field opens text entry on click; return to numeric stepping.
    visual.simulate_keystrokes("escape");
    editor.update(cx, |view, cx| {
        view.device_panel.operator_pages.insert(
            operator,
            operator_panel::OperatorPanelState {
                selected_operator: 0,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
        );
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("up");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .is_some_and(|state| operator_level(state, 5) != original_b_level)
            && view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&operator],
            operator_panel::OperatorPanelState {
                selected_operator: 0,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
            "numeric adjustment preserves the explicitly selected editor"
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .is_some_and(|state| operator_level(state, 5) == original_b_level)
    });

    let c_level_value = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-8-value", operator.value()).into_boxed_str(),
        ))
        .expect("operator c level numeric field");
    visual.simulate_click(c_level_value.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.device_panel.operator_pages[&operator].selected_operator,
            0
        );
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: operator,
                parameter: hex_lattice_contract::ParameterId::from_static(8),
            })
        else {
            panic!("operator c level field remains cached")
        };
        entity.read_with(cx, |control, _| assert!(control.is_editing()));
    });
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-6 db");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        assert_eq!(
            operator_level(state, 8),
            ParameterValue::Float(10.0_f32.powf(-6.0 / 20.0)),
            "the newly focused numeric field accepts and commits typed input"
        );
        assert_eq!(
            parameter_value(state, track, utility, utility_parameter,),
            ParameterValue::Bool(utility_enabled),
            "Operator context changes leave the neighboring Utility unchanged"
        );
        assert!(
            view.device_panel
                .controls
                .keys()
                .any(|key| key.device == utility)
        );
    });
}

#[gpui::test]
fn operator_shell_fixed_rows_fit_without_shrinking_controls(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _state, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove default instrument");
    let _empty = receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .expect("add native Operator");
    let mut state = receive_service_event(&service).state;
    let instrument = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("Operator instrument");
    let instrument_id = instrument.id();
    for operator in 0..4_u32 {
        service
            .submit(Request::SetDeviceParameter {
                device: instrument_id,
                parameter: hex_lattice_contract::ParameterId::from_static(62 + operator * 3),
                value: ParameterValue::Bool(true),
            })
            .expect("enable fixed frequency");
        state = receive_service_event(&service).state;
    }
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for width in [880.0, 1280.0] {
        visual.simulate_resize(size(px(width), px(540.0)));
        visual.run_until_parked();
        let shell = visual
            .debug_bounds(Box::leak(
                format!("operator-shell-{}", instrument_id.value()).into_boxed_str(),
            ))
            .expect("Operator shell");
        assert_eq!(
            shell.size.width,
            px(crate::device_composition::VECTOR_SHELL_WIDTH)
        );
        for operator in 0..4 {
            let row = Box::leak(
                format!("operator-row-{}-{operator}", instrument_id.value()).into_boxed_str(),
            );
            let row_bounds = visual.debug_bounds(row).expect("fixed-mode row");
            assert_eq!(row_bounds.size.height, px(44.0));
            assert!(row_bounds.origin.y >= shell.origin.y);
            assert!(
                row_bounds.origin.y + row_bounds.size.height <= shell.origin.y + shell.size.height,
                "wider gutters must not push the bottom oscillator outside its shell"
            );
            if operator < 3 {
                let previous = visual
                    .debug_bounds(Box::leak(
                        format!("operator-row-{}-{}", instrument_id.value(), operator + 1)
                            .into_boxed_str(),
                    ))
                    .expect("previous oscillator row");
                assert_eq!(
                    row_bounds.origin.y - previous.origin.y - previous.size.height,
                    px(2.0),
                    "dark oscillator rows have a visible two-pixel gutter"
                );
            }
            let badge = visual
                .debug_bounds(Box::leak(
                    format!("operator-select-{}-{operator}", instrument_id.value())
                        .into_boxed_str(),
                ))
                .expect("compact oscillator badge");
            assert_eq!(badge.size, gpui::size(px(6.0), px(36.0)));
            let ratio_id = 1 + operator * 3;
            let level_id = operator * 3 + 2;
            for parameter in [ratio_id, level_id, 609 + operator] {
                let field = Box::leak(
                    format!("device-{}-parameter-{parameter}", instrument_id.value())
                        .into_boxed_str(),
                );
                let bounds = visual.debug_bounds(field).expect("fixed-mode control");
                assert_eq!(bounds.size.width, px(60.0));
                let slider = visual
                    .debug_bounds(Box::leak(
                        format!(
                            "device-{}-parameter-{parameter}-slider",
                            instrument_id.value()
                        )
                        .into_boxed_str(),
                    ))
                    .unwrap();
                assert_eq!(slider.size, size(px(58.0), px(14.0)));
                assert!(
                    visual
                        .debug_bounds(Box::leak(
                            format!(
                                "device-{}-parameter-{parameter}-knob",
                                instrument_id.value()
                            )
                            .into_boxed_str()
                        ))
                        .is_none()
                );
                assert!(bounds.origin.x >= row_bounds.origin.x + px(4.0));
                assert!(
                    bounds.origin.x + bounds.size.width
                        <= row_bounds.origin.x + row_bounds.size.width - px(4.0),
                    "oscillator controls need an inset from the dark row edges"
                );
            }
            let field_bounds = |visual: &mut VisualTestContext, parameter| {
                visual
                    .debug_bounds(Box::leak(
                        format!("device-{}-parameter-{parameter}", instrument_id.value())
                            .into_boxed_str(),
                    ))
                    .unwrap()
            };
            let coarse = field_bounds(&mut visual, ratio_id);
            let fine = field_bounds(&mut visual, 609 + operator);
            let level = field_bounds(&mut visual, level_id);
            let wave = field_bounds(&mut visual, 57 + operator);
            assert!(badge.right() < coarse.left());
            assert!(coarse.bottom() <= fine.top());
            assert!(coarse.right() <= level.left());
            assert!(level.bottom() <= wave.top());
            for field in [coarse, fine, level, wave] {
                assert!(field.top() >= row_bounds.top() && field.bottom() <= row_bounds.bottom());
                assert!(field.right() <= row_bounds.right());
            }
            let keytrack = field_bounds(&mut visual, 613 + operator);
            assert!(keytrack.left() >= level.right());
            assert!(keytrack.right() <= row_bounds.right() - px(4.0));
        }
        let lfo = visual
            .debug_bounds(Box::leak(
                format!("operator-preview-row-{}-lfo", instrument_id.value()).into_boxed_str(),
            ))
            .expect("native LFO row");
        for parameter in [82, 83, 85, 87] {
            let control = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}", instrument_id.value())
                        .into_boxed_str(),
                ))
                .expect("native LFO control");
            assert!(control.origin.x >= lfo.origin.x + px(4.0));
            assert!(
                control.origin.x + control.size.width <= lfo.origin.x + lfo.size.width - px(4.0)
            );
        }
        let filter = visual
            .debug_bounds(Box::leak(
                format!("operator-preview-row-{}-filter", instrument_id.value()).into_boxed_str(),
            ))
            .unwrap();
        for parameter in [20001, 20002, 20005, 20006] {
            let control = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}", instrument_id.value())
                        .into_boxed_str(),
                ))
                .unwrap();
            assert!(control.origin.x >= filter.origin.x + px(4.0));
            assert!(
                control.origin.x + control.size.width
                    <= filter.origin.x + filter.size.width - px(4.0)
            );
        }
        for field in 0..6 {
            assert!(
                visual
                    .debug_bounds(Box::leak(
                        format!("operator-preview-{}-{field}", instrument_id.value())
                            .into_boxed_str(),
                    ))
                    .is_none(),
                "unsupported preview controls are not mounted"
            );
        }
        let feedback = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-81", instrument_id.value()).into_boxed_str(),
            ))
            .expect("real feedback slider");
        let row = visual
            .debug_bounds(Box::leak(
                format!("operator-preview-row-{}-feedback", instrument_id.value()).into_boxed_str(),
            ))
            .unwrap();
        assert!(feedback.left() >= row.left() && feedback.right() <= row.right());
    }
    // LFO uses canonical controls after PR154, with the same accepted row inset.
    let lfo_row = visual
        .debug_bounds(Box::leak(
            format!("operator-preview-row-{}-lfo", instrument_id.value()).into_boxed_str(),
        ))
        .expect("canonical LFO row");
    for parameter in [82, 85, 87] {
        let control = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{parameter}", instrument_id.value()).into_boxed_str(),
            ))
            .expect("canonical LFO control");
        assert!(control.origin.x >= lfo_row.origin.x + px(4.0));
        assert!(
            control.origin.x + control.size.width
                <= lfo_row.origin.x + lfo_row.size.width - px(4.0)
        );
    }
    open_operator_options(&mut visual, instrument_id);
    // Fixed frequency and multiplier have distinct IDs from ratio controls.
    // A keyboard edit must follow the focused fixed-frequency field too.
    let at = operator_context_selection_tests::row_background(&mut visual, instrument_id, 1);
    visual.simulate_click(at, Modifiers::default());
    visual.run_until_parked();
    let frequency = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-66-value", instrument_id.value()).into_boxed_str(),
        ))
        .expect("operator b fixed frequency");
    visual.simulate_click(frequency.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    editor.update(cx, |view, cx| {
        view.device_panel.operator_pages.insert(
            instrument_id,
            operator_panel::OperatorPanelState {
                selected_operator: 0,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
        );
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&instrument_id],
            operator_panel::OperatorPanelState {
                selected_operator: 0,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            }
        );
    });
}

#[gpui::test]
fn utility_panel_matches_compact_geometry_and_uses_shared_mode_popup(cx: &mut TestAppContext) {
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
    assert_eq!(descriptors.len(), 14);
    for (parameter, value) in [
        (width, ParameterValue::Float(2.5)),
        (mid_side, ParameterValue::Bool(false)),
        (mid_side_balance, ParameterValue::Float(-0.4)),
        (channel, ParameterValue::Choice(0)),
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

    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.worker_requests_pending = 1;
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: mid_side,
            },
            ParameterValue::Bool(true),
        );
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let panel = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel.size.height, px(229.0));
    let frame_selector = Box::leak(format!("effect-device-{}", device.value()).into_boxed_str());
    let frame = visual.debug_bounds(frame_selector).expect("utility frame");
    assert_eq!(
        frame.size.width,
        px(144.0 + hex_widgets::DEVICE_FRAME_HEADER_WIDTH + device_meter::WIDTH)
    );
    assert_eq!(frame.size.height, px(200.0));
    let input_selector = Box::leak(format!("utility-input-{}", device.value()).into_boxed_str());
    let output_selector = Box::leak(format!("utility-output-{}", device.value()).into_boxed_str());
    let input = visual.debug_bounds(input_selector).expect("input column");
    let output = visual.debug_bounds(output_selector).expect("output column");
    assert_eq!(input.size.width, px(64.0));
    assert_eq!(output.size.width, px(64.0));
    let scale = visual.update(|window, _| window.scale_factor());
    for section in ["device-input-section", "device-output-section"] {
        let bounds = visual
            .debug_bounds(section)
            .expect("shared rounded Utility section");
        assert_eq!(
            bounds.size.width,
            px(64.0),
            "rounded sections preserve control width"
        );
        let corners = visual
            .rendered_surface_geometry(section)
            .expect("painted rounded section")
            .0;
        assert_eq!(
            corners.top_left,
            px(hex_widgets::DEVICE_SECTION_RADIUS).scale(scale)
        );
        assert_eq!(
            visual.rendered_background(section),
            Some(gpui::Background::from(rgb(device_layout_style().rail)))
        );
    }
    let body_selector = Box::leak(format!("device-body-{}", device.value()).into_boxed_str());
    let body = visual.debug_bounds(body_selector).expect("Utility body");
    assert_eq!(body.size.width, px(140.0));
    assert_eq!(body.size.height, px(198.0));
    let mut parameter_bounds = |id: u32, suffix: &str| {
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{id}{suffix}", device.value()).into_boxed_str(),
            ))
            .unwrap_or_else(|| panic!("Utility parameter {id}{suffix} is rendered"))
    };
    for id in [1, 2, 10] {
        let dial = parameter_bounds(id, "-knob");
        assert!(dial.size.width >= px(24.0) && dial.size.width <= px(26.0));
        assert!(dial.size.height >= px(24.0) && dial.size.height <= px(26.0));
        let readout = parameter_bounds(id, "-value");
        assert_eq!(readout.size.width, px(64.0));
        assert_eq!(readout.size.height, px(14.0));
    }
    let cutoff = parameter_bounds(13, "");
    assert_eq!(cutoff.size.width, px(46.0));
    assert_eq!(cutoff.size.height, px(14.0));
    assert_eq!(parameter_bounds(13, "-value").size.height, px(14.0));
    for (id, width) in [
        (5, 31.0),
        (6, 31.0),
        (7, 64.0),
        (8, 31.0),
        (11, 31.0),
        (12, 64.0),
    ] {
        let bounds = parameter_bounds(id, "");
        assert_eq!(bounds.size.width, px(width));
        assert_eq!(bounds.size.height, px(14.0));
        assert!(bounds.origin.x >= body.origin.x);
        assert!(bounds.origin.x + bounds.size.width <= body.origin.x + body.size.width);
    }
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-2-pan-reset", device.value()).into_boxed_str()
            ))
            .is_some(),
        "balance uses shared pan semantics"
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-10-pan-reset", device.value()).into_boxed_str()
            ))
            .is_none(),
        "mid-side bipolar is not automatically pan"
    );
    let mode_knob = Box::leak(
        format!(
            "device-{}-parameter-{}-knob",
            device.value(),
            mid_side_balance.value()
        )
        .into_boxed_str(),
    );
    let mode_bounds = visual
        .debug_bounds(mode_knob)
        .expect("pending mid-side dial");
    assert!(mode_bounds.origin.x >= input.origin.x);
    assert!(mode_bounds.origin.x + mode_bounds.size.width <= input.origin.x + input.size.width);
    editor.update(cx, |view, cx| {
        view.device_panel.pending_values.remove(&ParameterKey {
            device,
            parameter: mid_side,
        });
        view.worker_requests_pending = 0;
        cx.notify();
    });
    visual.run_until_parked();
    let width_knob = Box::leak(
        format!("device-{}-parameter-{}-knob", device.value(), width.value()).into_boxed_str(),
    );
    assert!(
        visual.debug_bounds(width_knob).is_some(),
        "canonical width mode is shown after pending state clears"
    );
    let mut parameter_bounds = |id: u32, suffix: &str| {
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{id}{suffix}", device.value()).into_boxed_str(),
            ))
            .unwrap_or_else(|| panic!("Utility parameter {id}{suffix} is rendered"))
    };
    assert!(parameter_bounds(3, "-knob").size.width >= px(24.0));
    assert!(parameter_bounds(3, "-value").size.width >= px(60.0));
    let gain_value =
        Box::leak(format!("device-{}-parameter-1-value", device.value()).into_boxed_str());
    let gain_bounds = visual.debug_bounds(gain_value).expect("gain readout");
    assert!(gain_bounds.origin.x >= output.origin.x);
    assert!(gain_bounds.origin.x + gain_bounds.size.width <= output.origin.x + output.size.width);
    let readout = |parameter: u32| {
        editor.read_with(cx, |view, _| {
            let Some(ParameterControl::Float { entity, .. }) =
                view.device_panel.controls.get(&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(parameter),
                })
            else {
                panic!("Utility numeric control is mounted")
            };
            entity.read_with(cx, |control, _| control.display_text().to_owned())
        })
    };
    assert_eq!(readout(1), "0.00 db");
    assert_eq!(readout(2), "c");
    assert_eq!(readout(3), "250%");
    assert_eq!(readout(13), "200 hz");
    let audition = visual
        .debug_bounds(Box::leak(
            format!("utility-bass-mono-audition-{}", device.value()).into_boxed_str(),
        ))
        .expect("headphone audition control");
    assert_eq!(audition.size.width, px(14.0));
    let title = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", device.value()).into_boxed_str(),
        ))
        .expect("Utility title");
    for suffix in ["device-bypass", "device-preset-menu", "device-remove"] {
        let selector = Box::leak(format!("{suffix}-{}", device.value()).into_boxed_str());
        let action = visual.debug_bounds(selector).expect("header action");
        assert!(action.origin.x >= frame.origin.x);
        assert!(action.origin.x + action.size.width <= frame.origin.x + frame.size.width);
        assert_eq!(action.size.height, px(12.0));
        assert!(action.origin.y >= frame.origin.y);
        assert!(action.bottom() <= title.top());
    }

    let dial = visual
        .debug_bounds(Box::leak(
            format!("utility-width-dial-{}", device.value()).into_boxed_str(),
        ))
        .expect("width mode affordance");
    let position = point(dial.center().x, dial.center().y);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    let menu = visual
        .debug_bounds("context-menu-panel")
        .expect("shared popup");
    let item = visual.debug_bounds("context-menu-item").expect("mode item");
    assert!(menu.origin.x >= px(0.0));
    let select = point(item.center().x, item.center().y);
    visual.simulate_mouse_down(select, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(select, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state.project.track(track).is_some_and(|track| {
                hex_engine::native_device_parameter_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    track.devices()[0].state(),
                    mid_side,
                ) == Ok(ParameterValue::Bool(true))
            })
        })
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!(
                    "device-{}-parameter-{}-knob",
                    device.value(),
                    mid_side_balance.value()
                )
                .into_boxed_str(),
            ))
            .is_some()
    );
    let mode_key = ParameterKey {
        device,
        parameter: mid_side,
    };
    editor.read_with(cx, |view, _| {
        assert!(
            !view.device_panel.pending_values.contains_key(&mode_key),
            "the acknowledged mode request retires its pending display value"
        );
    });
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                state.project.track(track).unwrap().devices()[0].state(),
                mid_side,
            ) == Ok(ParameterValue::Bool(false))
        })
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(width_knob).is_some(),
        "undo restores width mode"
    );
    let mid_side_knob = Box::leak(
        format!(
            "device-{}-parameter-{}-knob",
            device.value(),
            mid_side_balance.value()
        )
        .into_boxed_str(),
    );
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.pending_values.contains_key(&mode_key));
    });

    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Redo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                state.project.track(track).unwrap().devices()[0].state(),
                mid_side,
            ) == Ok(ParameterValue::Bool(true))
        })
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(mid_side_knob).is_some(),
        "redo restores mid/side mode"
    );
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.pending_values.contains_key(&mode_key));
    });

    editor.read_with(cx, |view, _| {
        let utility = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()[0]
            .state();
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility,
                width,
            ),
            Ok(ParameterValue::Float(2.5)),
            "switching modes preserves the hidden width value"
        );
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility,
                mid_side_balance,
            ),
            Ok(ParameterValue::Float(-0.4))
        );
    });
    editor.update(cx, |view, cx| {
        assert!(view.enqueue_device_barrier(Request::SetDeviceParameter {
            device,
            parameter: channel,
            value: ParameterValue::Choice(1),
        }));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                state.project.track(track).unwrap().devices()[0].state(),
                channel,
            ) == Ok(ParameterValue::Choice(1))
        })
    });
    editor.read_with(cx, |view, _| {
        let utility = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()[0]
            .state();
        let rendered = hex_engine::native_device_view_for_class(
            hex_engine::NativeDeviceClass::Effect,
            utility,
        )
        .unwrap();
        assert!(
            rendered
                .parameters
                .iter()
                .filter(|parameter| matches!(
                    parameter.descriptor.label,
                    "width" | "mid-side" | "mid-side balance"
                ))
                .all(|parameter| !parameter.enabled)
        );
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility,
                width,
            ),
            Ok(ParameterValue::Float(2.5))
        );
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility,
                mid_side_balance,
            ),
            Ok(ParameterValue::Float(-0.4))
        );
    });
}

#[gpui::test]
fn utility_mode_popup_finalizes_numeric_draft_and_closes_on_escape_or_owner_removal(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let gain_value =
        Box::leak(format!("device-{}-parameter-1-value", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(gain_value).expect("gain readout");
    let click = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("6.00 db");
    visual.run_until_parked();

    let dial = visual
        .debug_bounds(Box::leak(
            format!("utility-width-dial-{}", device.value()).into_boxed_str(),
        ))
        .expect("width dial");
    let position = point(dial.center().x, dial.center().y);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("context-menu-panel").is_some());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                state.project.track(track).unwrap().devices()[0].state(),
                hex_lattice_contract::ParameterId::from_static(1),
            )
            .is_ok_and(|value| {
                matches!(value, ParameterValue::Float(value) if (value - 10.0_f32.powf(6.0 / 20.0)).abs() < 0.0001)
            })
        })
    });

    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));
    let dial = visual
        .debug_bounds(Box::leak(
            format!("utility-width-dial-{}", device.value()).into_boxed_str(),
        ))
        .expect("width dial after escape");
    let position = point(dial.center().x, dial.center().y);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("context-menu-panel").is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));

    let dial = visual
        .debug_bounds(Box::leak(
            format!("utility-width-dial-{}", device.value()).into_boxed_str(),
        ))
        .expect("width dial after escape");
    let position = point(dial.center().x, dial.center().y);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_down(
        point(px(5.0), px(100.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(px(5.0), px(100.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));

    let dial = visual
        .debug_bounds(Box::leak(
            format!("utility-width-dial-{}", device.value()).into_boxed_str(),
        ))
        .expect("width dial after outside dismissal");
    let position = point(dial.center().x, dial.center().y);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("context-menu-panel").is_some());

    editor.update(cx, |view, cx| {
        view.enqueue_device_barrier(Request::Edit(Command::RemoveDevice(device)));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.devices().is_empty())
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));
}

#[gpui::test]
fn return_track_reuses_compact_utility_panel(cx: &mut TestAppContext) {
    let (service, state, track, return_track, device, _) = actor_return_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.device_panel_active = true;
        view.clip_view_open = true;
        view.focused_return = Some(return_track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let panel = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel.size.height, px(229.0));
    let frame = visual
        .debug_bounds(Box::leak(
            format!("effect-device-{}", device.value()).into_boxed_str(),
        ))
        .expect("return Utility frame");
    assert_eq!(
        frame.size.width,
        px(144.0 + hex_widgets::DEVICE_FRAME_HEADER_WIDTH + device_meter::WIDTH)
    );
    assert_eq!(frame.size.height, px(200.0));
    let body = visual
        .debug_bounds(Box::leak(
            format!("device-body-{}", device.value()).into_boxed_str(),
        ))
        .expect("return Utility body");
    assert_eq!(body.size.height, px(198.0));
    let title = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", device.value()).into_boxed_str(),
        ))
        .expect("return Utility title");
    for suffix in ["device-bypass", "return-device-remove"] {
        let action = visual
            .debug_bounds(Box::leak(
                format!("{suffix}-{}", device.value()).into_boxed_str(),
            ))
            .expect("return Utility header action");
        assert_eq!(action.size.height, px(12.0));
        assert!(action.origin.x >= frame.origin.x);
        assert!(action.origin.x + action.size.width <= frame.origin.x + frame.size.width);
        assert!(action.bottom() <= title.top());
    }
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("utility-input-{}", device.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("utility-output-{}", device.value()).into_boxed_str()
            ))
            .is_some()
    );
}

#[gpui::test]
fn utility_default_width_and_audio_readouts_use_reference_units(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        for (parameter, expected) in [(2, "c"), (3, "100%"), (13, "200 hz")] {
            let Some(ParameterControl::Float { entity, .. }) =
                view.device_panel.controls.get(&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(parameter),
                })
            else {
                panic!("Utility default numeric control is mounted")
            };
            entity.read_with(cx, |control, _| {
                assert_eq!(control.display_text(), expected)
            });
        }
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
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let rail = visual
        .debug_bounds("device-chain-scroll")
        .expect("visible rail bounds");
    let outside = device_ids
        .iter()
        .find_map(|device| {
            let selector =
                Box::leak(format!("device-drop-card-{}", device.value()).into_boxed_str());
            let bounds = visual.debug_bounds(selector)?;
            let card_right = bounds.origin.x + bounds.size.width;
            let rail_right = rail.origin.x + rail.size.width;
            let clipped_left = if bounds.origin.x > rail_right {
                bounds.origin.x
            } else {
                rail_right
            };
            let visible_top = if bounds.origin.y > rail.origin.y {
                bounds.origin.y
            } else {
                rail.origin.y
            };
            let rail_bottom = rail.origin.y + rail.size.height;
            let card_bottom = bounds.origin.y + bounds.size.height;
            let visible_bottom = if card_bottom < rail_bottom {
                card_bottom
            } else {
                rail_bottom
            };
            if card_right <= clipped_left || visible_bottom <= visible_top {
                return None;
            }
            Some(point(
                clipped_left + (card_right - clipped_left) / 2.0,
                visible_top + (visible_bottom - visible_top) / 2.0,
            ))
        })
        .expect("fixture reaches a card beyond the visible rail");

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
fn existing_device_header_title_reorders_cards_by_stable_identity(cx: &mut TestAppContext) {
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

    let title_selector = Box::leak(format!("device-drag-title-{}", first.value()).into_boxed_str());
    let target_selector =
        Box::leak(format!("device-drop-card-{}", second.value()).into_boxed_str());
    let title = visual
        .debug_bounds(title_selector)
        .expect("first device title");
    let target = visual
        .debug_bounds(target_selector)
        .expect("second device receiver");
    let down = point(
        title.origin.x + title.size.width / 2.0,
        title.origin.y + title.size.height / 2.0,
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
                before: None,
                replace: None,
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

    let dial = Box::leak(
        format!("device-{}-parameter-{}-knob", device.value(), gain.value()).into_boxed_str(),
    );
    let dial_bounds = visual.debug_bounds(dial).expect("gain dial");
    visual.simulate_click(
        point(dial_bounds.center().x, dial_bounds.center().y),
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
            canonical_audio_source: None,
            comparison_audio: None,
            project: current.project.clone(),
            waveforms: current.waveforms.clone(),
            source_overviews: current.source_overviews.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: current.midi_preview.clone(),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
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
    enter_typed_value(&mut visual, device, parameter, "250%");
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
    enter_typed_value(&mut visual, device, numeric_parameter, "250%");
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

    let dial_selector = Box::leak(
        format!(
            "device-{}-parameter-{}-knob",
            device.value(),
            parameter.value()
        )
        .into_boxed_str(),
    );
    let dial = visual
        .debug_bounds(dial_selector)
        .expect("rendered balance dial");
    let start = point(dial.center().x, dial.center().y);
    let moved = point(start.x, start.y - gpui::px(60.0));
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
    enter_typed_value(&mut visual, device, parameter, "300%");
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
            canonical_audio_source: None,
            comparison_audio: None,
            project: current.project.clone(),
            waveforms: current.waveforms.clone(),
            source_overviews: current.source_overviews.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: current.midi_preview.clone(),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
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
    enter_typed_value(&mut visual, second_device, parameter, "250%");

    let rail = visual.debug_bounds("device-chain-scroll").expect("rail");
    visual.simulate_mouse_down(rail.center(), MouseButton::Middle, Modifiers::default());
    let panned = point(rail.center().x - px(50.0), rail.center().y);
    visual.simulate_mouse_move(panned, Some(MouseButton::Middle), Modifiers::default());
    visual.simulate_mouse_up(panned, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: second_device,
                parameter,
            })
        else {
            panic!("draft control");
        };
        assert!(
            entity.read(cx).is_editing(),
            "middle pan preserves typed draft"
        );
    });

    let toggle = visual
        .debug_bounds("device-panel-bottom-toggle")
        .expect("bottom toggle")
        .center();
    visual.simulate_mouse_down(toggle, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.clip_view_open, "full queue preserves pane and draft")
    });

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
fn command_f_preserves_invalid_device_draft_then_finalizes_repaired_value(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let parameter =
        hex_engine::native_device_view(state.project.track(track).unwrap().devices()[0].state())
            .unwrap()
            .parameters
            .into_iter()
            .find(|item| item.descriptor.label == "width")
            .unwrap()
            .descriptor
            .id;
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.browser.open = false;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, parameter, "invalid");
    let focus = visual.update(|window, cx| window.focused(cx).unwrap());
    for selector in ["clip-detail-tab", "device-detail-tab", "clip-view-toggle"] {
        let target = visual
            .debug_bounds(selector)
            .expect("footer control")
            .center();
        visual.simulate_click(target, Modifiers::default());
        visual.update(|window, cx| {
            assert!(
                focus.is_focused(window),
                "{selector} must retain invalid draft focus"
            );
            editor.read_with(cx, |view, _| {
                assert!(view.clip_view_open && view.device_panel_active);
                assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
            });
        });
    }
    visual.simulate_keystrokes("cmd-f");
    visual.run_until_parked();
    visual.update(|window, cx| {
        assert!(
            focus.is_focused(window),
            "invalid device draft must retain focus"
        );
        editor.read_with(cx, |view, cx| {
            assert!(!view.browser.open);
            assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("finish the device value edit")
            );
            let ParameterControl::Float { entity, .. } =
                &view.device_panel.controls[&ParameterKey { device, parameter }]
            else {
                panic!("numeric control")
            };
            assert!(entity.read(cx).is_editing());
        });
    });
    visual.simulate_keystrokes("escape");
    enter_typed_value(&mut visual, device, parameter, "250%");
    visual.simulate_keystrokes("cmd-f");
    visual.run_until_parked();
    visual.simulate_input("utility");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "utility");
        let ParameterControl::Float { entity, .. } =
            &view.device_panel.controls[&ParameterKey { device, parameter }]
        else {
            panic!("numeric control")
        };
        assert!(!entity.read(cx).is_editing());
    });
    pump_until(&editor, cx, |view| {
        parameter_value(view.state.as_ref().unwrap(), track, device, parameter)
            == ParameterValue::Float(2.5)
    });
}

#[gpui::test]
fn command_f_preserves_invalid_return_send_draft(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, return_track, _, _) = actor_return_utility_fixture();
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::EditorMode::Arrangement;
        view.send_menu_track = Some(track);
        view.browser.open = false;
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
    let value = visual.debug_bounds(selector).unwrap();
    visual.simulate_mouse_down(value.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(value.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("7 db");
    let focus = visual.update(|window, cx| window.focused(cx).unwrap());
    visual.simulate_keystrokes("cmd-f");
    visual.run_until_parked();
    visual.update(|window, cx| {
        assert!(
            focus.is_focused(window),
            "invalid return send must retain focus"
        );
        editor.read_with(cx, |view, cx| {
            assert!(!view.browser.open);
            assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("finish the device value edit")
            );
            assert!(
                view.device_panel.send_controls[&SendControlKey {
                    track,
                    return_track
                }]
                    .entity
                    .read(cx)
                    .is_editing()
            );
        });
    });
    visual.simulate_keystrokes("escape cmd-f");
    visual.simulate_input("utility");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "utility")
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
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(initial_project.clone()),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: std::collections::BTreeMap::new(),
                    midi_preview: Arc::new(Default::default()),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
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
        view.device_panel.operator_pages.insert(
            device,
            super::operator_panel::OperatorPanelState {
                selected_operator: 2,
                page: super::operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
        );
        view.project_generation = view.project_generation.saturating_add(1);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            !view.device_panel.operator_pages.contains_key(&device),
            "project replacement drops device-specific page state"
        );
    });
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
        view.device_panel.operator_pages.insert(
            device,
            super::operator_panel::OperatorPanelState {
                selected_operator: 3,
                page: super::operator_panel::OperatorPage::Envelope,
                ..Default::default()
            },
        );
        let current = view.state.as_ref().expect("view state");
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(next_project.clone()),
            waveforms: current.waveforms.clone(),
            source_overviews: current.source_overviews.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: current.midi_preview.clone(),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
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
        assert!(
            view.device_panel
                .controls
                .keys()
                .all(|key| key.device != device)
        );
        assert!(
            !view.device_panel.operator_pages.contains_key(&device),
            "removed device drops its page state"
        );
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
        assert!(
            view.device_panel
                .controls
                .keys()
                .all(|key| key.device != device)
        );
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
    visual.simulate_input("-8.519374645 db");
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
    assert!(
        value.size.width >= px(46.0),
        "numeric value retains its minimum hit width"
    );
    assert!(value.left() >= menu.left() && value.right() <= menu.right());
    let bar_selector = Box::leak(
        format!(
            "return-send-{}-{}-slider",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    assert_eq!(
        value,
        visual.debug_bounds(bar_selector).unwrap(),
        "value spans the filled rectangle"
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
            view.track_scroll.offset().y,
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
        let _ = super::view(view, None, cx);
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
    visual.simulate_input("-4.082399653 db");
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
    visual.simulate_input("7 db");
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
    visual.simulate_input("7 db");
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
    let moved = point(start.x, start.y - px(36.0));
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

fn operator_parameter_value(
    state: &ViewState,
    track: TrackId,
    device: DeviceId,
    parameter: u32,
) -> ParameterValue {
    let instrument = state
        .project
        .track(track)
        .and_then(hex_project::Track::instrument)
        .filter(|instrument| instrument.id() == device)
        .expect("canonical Operator instrument");
    hex_engine::native_device_view_for_class(NativeDeviceClass::Instrument, instrument.state())
        .expect("Operator parameter view")
        .parameters
        .into_iter()
        .find(|item| item.descriptor.id.value() == parameter)
        .expect("Operator envelope parameter")
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
        canonical_audio_source: None,
        comparison_audio: None,
        project: Arc::new(project_editor.project().clone()),
        waveforms: state.waveforms.clone(),
        source_overviews: state.source_overviews.clone(),
        audio_clip_waveforms: state.audio_clip_waveforms.clone(),
        midi_preview: state.midi_preview.clone(),
        gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
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
        let _panel = super::view(view, None, cx);
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

#[gpui::test]
fn sine_and_effect_place_their_shared_header_to_the_left_of_the_body(cx: &mut TestAppContext) {
    #[cfg(target_os = "macos")]
    {
        cx.use_native_text_system()
            .expect("native bundled-font shaping");
        cx.update(|cx| theme::load_fonts(cx).expect("bundled fonts"));
    }
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("sine")
        .id();
    let (handle, _) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [1280.0, 880.0] {
        visual.simulate_resize(size(px(width), px(540.0)));
        visual.run_until_parked();
        for device in [instrument, effect] {
            let selector =
                Box::leak(format!("device-drag-title-{}", device.value()).into_boxed_str());
            let title = visual.debug_bounds(selector).expect("title");
            let body = visual
                .debug_bounds(Box::leak(
                    format!("device-body-{}", device.value()).into_boxed_str(),
                ))
                .expect("body");
            assert!(title.size.width < px(hex_widgets::DEVICE_FRAME_HEADER_WIDTH));
            assert!(title.size.height > title.size.width);
            #[cfg(target_os = "macos")]
            {
                let glyphs = visual.rendered_vertical_glyph_bounds(selector);
                assert!(glyphs.len() >= 2, "device title must paint rotated glyphs");
                assert!(
                    glyphs[0].origin.y > glyphs[1].origin.y,
                    "title reads bottom to top"
                );
            }
            assert!(title.right() < body.origin.x);
            let shell = visual
                .debug_bounds(Box::leak(
                    format!("device-shell-{}", device.value()).into_boxed_str(),
                ))
                .expect("device frame");
            assert!(shell.origin.y <= title.origin.y && title.bottom() <= shell.bottom());
            assert_eq!(body.size.height, px(DEVICE_FRAME_BODY_HEIGHT));
            assert_eq!(shell.size.height, px(200.0));
            assert_eq!(
                body.bottom() + px(hex_widgets::DEVICE_FRAME_BORDER_WIDTH),
                shell.bottom(),
                "the left header leaves no obsolete horizontal-header allowance"
            );
        }
    }
}

mod main_chain_ui_tests;

#[gpui::test]
fn shared_housing_placeholders_open_in_editor_without_device_commands(cx: &mut TestAppContext) {
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let revision = state.revision;
    let original = state.project.device(effect).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for (toggle, panel) in [
        ("device-macro-toggle", "device-macro-panel"),
        ("device-modulator-toggle", "device-modulator-panel"),
    ] {
        assert!(visual.debug_bounds(panel).is_none());
        let button = visual.debug_bounds(toggle).expect("shared housing control");
        visual.simulate_click(button.center(), Modifiers::default());
        visual.run_until_parked();
        let panel = visual
            .debug_bounds(panel)
            .expect("shared placeholder panel");
        assert!(panel.size.height > px(0.0));
    }
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, revision);
        assert_eq!(state.project.device(effect).unwrap().state(), &original);
        assert_eq!(view.worker_requests_pending, 0);
    });
}

mod navigation_capture_tests;

/// User report (Pulse #3273): after clipping, a one-pixel red line stayed at
/// the top of the device meter. It is the held peak, drawn in the clipping
/// color above unity, which never decayed and could not be cleared. A click
/// now clears the held peaks; quiet repaints keep them cleared and a fresh
/// overload draws the line again.
#[gpui::test]
fn clicking_a_device_meter_clears_its_held_overload_line(cx: &mut TestAppContext) {
    let _output_fixture_guard = eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &state.project,
            &Default::default(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .expect("prepared revision"),
    );
    let meter = Arc::clone(
        prepared
            .arrangement()
            .device_meter(device.value())
            .expect("device meter"),
    );
    let deadline = crate::test_wait::hang_deadline();
    let (mut output, _backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    };
    let client = output.take_client().expect("audio client");
    let (handle, editor) = actor_editor_window(cx, service, Arc::clone(&state), track);
    editor.update(cx, |view, cx| {
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "meter fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audible_playback = Some(super::super::PlaybackCandidate {
            worker_requires_preroll: None,
            observation: None,
            audio_revision: 1,
            project_generation: view.project_generation,
            state: Arc::clone(&state),
            prepared_revision: Some(Arc::clone(&prepared)),
            seek_request_id: None,
            device_preview_identity: None,
            tuning_preview_identity: None,
        });
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    let repaint = |visual: &mut VisualTestContext| {
        editor.update(visual, |_, cx| cx.notify());
        visual.run_until_parked();
    };
    let meter_selector =
        Box::leak(format!("device-{}-output-meter", device.value()).into_boxed_str());
    let left_peak = Box::leak(format!("{meter_selector}-left-peak").into_boxed_str());
    let right_peak = Box::leak(format!("{meter_selector}-right-peak").into_boxed_str());
    let clipping = Some(gpui::rgb(crate::theme::meter_colors().clipping).into());

    meter.observe_output(&[[1.5, 0.5]]);
    repaint(&mut visual);
    assert_eq!(visual.rendered_background(left_peak), clipping);
    assert!(visual.debug_bounds(right_peak).is_some());
    let add = visual
        .debug_bounds(Box::leak(
            format!("device-{}-add-after", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(add.center(), Modifiers::default());
    editor.update(&mut visual, |view, cx| {
        assert!(view.device_panel.add_menu_open);
        view.device_panel.add_menu_open = false;
        cx.notify();
    });
    repaint(&mut visual);
    assert_eq!(
        visual.rendered_background(left_peak),
        clipping,
        "the separate add action does not acknowledge peaks"
    );
    for clear in 0..2 {
        let bounds = visual.debug_bounds(meter_selector).expect("device meter");
        visual.simulate_click(bounds.center(), Modifiers::default());
        repaint(&mut visual);
        assert!(visual.debug_bounds(left_peak).is_none(), "clear {clear}");
        assert!(visual.debug_bounds(right_peak).is_none(), "clear {clear}");
    }
    // Quiet telemetry repaints stay cleared; nothing relatches the old event.
    for _ in 0..3 {
        meter.observe_output(&[[0.0, 0.0]]);
        repaint(&mut visual);
        assert!(visual.debug_bounds(left_peak).is_none());
    }
    // A fresh non-clipping reading draws a normal peak; a fresh overload rearms.
    meter.observe_output(&[[0.5, 0.0]]);
    repaint(&mut visual);
    assert_ne!(visual.rendered_background(left_peak), clipping);
    meter.observe_output(&[[0.0, 2.0]]);
    repaint(&mut visual);
    assert_eq!(visual.rendered_background(right_peak), clipping);
}

/// Open the real optional oscillator controls for tests that edit those fields.
fn open_operator_options(visual: &mut VisualTestContext, device: DeviceId) {
    let panel = Box::leak(format!("operator-options-panel-{}", device.value()).into_boxed_str());
    if visual.debug_bounds(panel).is_none() {
        let button = visual
            .debug_bounds(Box::leak(
                format!("operator-options-{}", device.value()).into_boxed_str(),
            ))
            .expect("oscillator options toggle");
        visual.simulate_click(button.center(), Modifiers::default());
        visual.run_until_parked();
    }
    assert!(visual.debug_bounds(panel).is_some());
}

#[gpui::test]
fn modulation_panel_adds_and_maps_an_lfo_without_editing_base_values(cx: &mut TestAppContext) {
    use hex_devices::utility::parameter;
    assert_modulation_mapping(
        cx,
        "lfo",
        hex_engine::lfo_modulator::TYPE_ID,
        &[parameter::GAIN, parameter::WIDTH],
    );
}

#[gpui::test]
fn modulation_panel_maps_continuous_and_discrete_eq8_controls_without_editing_authored_values(
    cx: &mut TestAppContext,
) {
    use hex_devices::eq8::{EQ8_TYPE_ID, parameter};
    use hex_lattice_contract::ParameterId;
    use hex_project::{DeviceAutomationTarget, ModulationDestination};
    let (service, state, track, device) = actor_native_effect_fixture(EQ8_TYPE_ID);
    let original = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let click = click_modulator_control;
    click(&mut visual, "device-modulator-toggle".into(), false);
    click(&mut visual, format!("add-lfo-{}", device.value()), true);
    let slot = |view: &EditorView| {
        view.state
            .as_ref()?
            .project
            .device(device)?
            .modulation()
            .sources()
            .first()
            .map(|source| source.id)
    };
    pump_until(&editor, cx, |view| slot(view).is_some());
    visual.run_until_parked();
    let source = editor.read_with(cx, |view, _| slot(view).unwrap());
    click(&mut visual, format!("lfo-{}-map", source.value()), true);
    let target_key =
        |parameter: ParameterId| format!("map-target-{}-{}", device.value(), parameter.value());
    let targets = [
        parameter::MODE,
        parameter::enabled(0, 0),
        parameter::kind(0, 0),
        parameter::OUTPUT_GAIN,
        parameter::frequency(0, 0),
        parameter::gain(0, 0),
        parameter::q(0, 0),
    ];
    let route_count = |view: &EditorView| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .routes()
            .len()
    };
    for (index, parameter) in targets.into_iter().enumerate() {
        if parameter == parameter::OUTPUT_GAIN {
            let toolbar = visual
                .debug_bounds(Box::leak(
                    format!("eq8-toolbar-{}", device.value()).into_boxed_str(),
                ))
                .unwrap();
            // Bring the bottom dial row into the scrollable housing's viewport.
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: toolbar.center(),
                delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
                ..Default::default()
            });
            visual.run_until_parked();
        }
        click(&mut visual, target_key(parameter), false);
        assert!(
            editor.read_with(cx, |view, _| {
                view.worker_requests_pending > 0
                    || !view.pending_requests.is_empty()
                    || route_count(view) == index + 1
            }),
            "visible EQ8 dial {} did not dispatch",
            parameter.value()
        );
        pump_until(&editor, cx, |view| route_count(view) == index + 1);
        visual.run_until_parked();
    }
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let current = project.device(device).unwrap();
        for (route, parameter) in current.modulation().routes().iter().zip(targets) {
            assert_eq!(route.source, source);
            assert_eq!(
                route.destination,
                ModulationDestination::Device {
                    device_type: EQ8_TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(parameter),
                    vst3_class: None,
                }
            );
        }
        assert_eq!(current.state(), &original);
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(
            reopened.device(device).unwrap().modulation(),
            current.modulation()
        );
    });
}

#[gpui::test]
fn modulation_panel_adds_and_maps_an_envelope_follower(cx: &mut TestAppContext) {
    assert_modulation_mapping(
        cx,
        "follower",
        hex_engine::follower_modulator::TYPE_ID,
        &[hex_devices::utility::parameter::GAIN],
    );
}

#[gpui::test]
fn modulation_panel_adds_and_maps_a_manual_macro(cx: &mut TestAppContext) {
    use hex_devices::utility::parameter;
    assert_modulation_mapping(
        cx,
        "macro",
        hex_engine::macro_modulator::TYPE_ID,
        &[parameter::GAIN, parameter::WIDTH],
    );
}

fn click_modulator_control(visual: &mut VisualTestContext, key: String, in_strip: bool) {
    let key = Box::leak(key.into_boxed_str());
    if in_strip {
        for _ in 0..3 {
            let target = visual
                .debug_bounds(key)
                .unwrap_or_else(|| panic!("missing {key}"));
            let strip = visual.debug_bounds("device-modulator-scroll").unwrap();
            if strip.contains(&target.center()) {
                break;
            }
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: strip.center(),
                delta: gpui::ScrollDelta::Pixels(point(
                    px(0.0),
                    strip.center().y - target.center().y,
                )),
                ..Default::default()
            });
            visual.run_until_parked();
        }
        let target = visual.debug_bounds(key).unwrap();
        let strip = visual.debug_bounds("device-modulator-scroll").unwrap();
        assert!(
            strip.contains(&target.center()),
            "{key} at {target:?} outside {strip:?}"
        );
    }
    let target = visual.debug_bounds(key).unwrap();
    visual.simulate_click(target.center(), Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn modulation_panel_maps_an_lfo_into_a_macro_and_then_a_device_parameter(cx: &mut TestAppContext) {
    use hex_devices::utility::parameter;
    use hex_project::{DeviceModulation, ModulationDestination};
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bank = |view: &EditorView| -> DeviceModulation {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    click_modulator_control(&mut visual, "device-modulator-toggle".into(), false);
    for (index, kind) in ["lfo", "macro"].into_iter().enumerate() {
        click_modulator_control(&mut visual, format!("add-{kind}-{}", device.value()), true);
        pump_until(&editor, cx, |view| bank(view).sources().len() == index + 1);
        visual.run_until_parked();
    }
    let slots = editor.read_with(cx, |view, _| bank(view));
    let lfo = slots.sources()[0].id;
    let manual = slots.sources()[1].id;
    click_modulator_control(&mut visual, format!("lfo-{}-map", lfo.value()), true);
    click_modulator_control(
        &mut visual,
        format!("map-macro-{}-{}", device.value(), manual.value()),
        true,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 1);
    visual.run_until_parked();
    click_modulator_control(&mut visual, format!("macro-{}-map", manual.value()), true);
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("map-macro-{}-{}", device.value(), manual.value()).into_boxed_str()
            ))
            .is_none()
    );
    click_modulator_control(
        &mut visual,
        format!("map-target-{}-{}", device.value(), parameter::GAIN.value()),
        false,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 2);
    let mapped = editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let mapped = bank(view);
        assert_eq!(mapped.routes()[0].source, lfo);
        assert_eq!(
            mapped.routes()[0].destination,
            ModulationDestination::Modulator {
                source: manual,
                module_type: hex_engine::macro_modulator::TYPE_ID,
                parameter: hex_engine::macro_modulator::VALUE_PARAMETER,
            }
        );
        assert_eq!(mapped.routes()[1].source, manual);
        assert_eq!(mapped.sources(), slots.sources());
        let loaded = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(loaded.device(device).unwrap().modulation(), &mapped);
        mapped
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        bank(view).routes() == &mapped.routes()[..1]
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| bank(view) == mapped);
    visual.run_until_parked();
    click_modulator_control(&mut visual, format!("lfo-{}-remove", lfo.value()), true);
    pump_until(&editor, cx, |view| {
        let bank = bank(view);
        bank.sources() == &mapped.sources()[1..] && bank.routes() == &mapped.routes()[1..]
    });
}

#[gpui::test]
fn modulation_panel_maps_both_lfo_rate_modes_and_preserves_assignments_on_sync(
    cx: &mut TestAppContext,
) {
    use hex_devices::utility::parameter;
    use hex_engine::lfo_modulator as lfo;
    use hex_project::{DeviceModulation, ModulationDestination};

    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bank = |view: &EditorView| -> DeviceModulation {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    let click = click_modulator_control;
    click(&mut visual, "device-modulator-toggle".into(), false);
    for (index, kind) in ["lfo", "lfo", "macro"].into_iter().enumerate() {
        click(&mut visual, format!("add-{kind}-{}", device.value()), true);
        pump_until(&editor, cx, |view| bank(view).sources().len() == index + 1);
        visual.run_until_parked();
    }
    let slots = editor.read_with(cx, |view, _| bank(view));
    let [free, synced, manual] = std::array::from_fn(|index| slots.sources()[index].id);
    let rate_key = |source: hex_project::ModulationSourceId| {
        format!("map-lfo-rate-{}-{}", device.value(), source.value())
    };
    click(&mut visual, format!("lfo-{}-sync", synced.value()), true);
    pump_until(&editor, cx, |view| {
        matches!(
            lfo::Settings::decode(&bank(view).sources()[1].state)
                .unwrap()
                .rate,
            lfo::Rate::BeatsPerCycle(_)
        )
    });
    visual.run_until_parked();

    click(&mut visual, format!("macro-{}-map", manual.value()), true);
    for (index, target) in [synced, free].into_iter().enumerate() {
        click(&mut visual, rate_key(target), true);
        pump_until(&editor, cx, |view| bank(view).routes().len() == index + 1);
        visual.run_until_parked();
    }
    assert!(
        visual
            .debug_bounds(Box::leak(rate_key(free).into_boxed_str()))
            .is_none(),
        "duplicate"
    );
    click(&mut visual, format!("macro-{}-map", manual.value()), true);
    let synced_rate = |view: &EditorView| {
        lfo::Settings::decode(&bank(view).sources()[1].state)
            .unwrap()
            .rate
    };
    let original_synced = editor.read_with(cx, |view, _| synced_rate(view));
    let assigned = editor.read_with(cx, |view, _| bank(view).routes().to_vec());
    for is_free in [true, false] {
        click(&mut visual, format!("lfo-{}-sync", synced.value()), true);
        pump_until(&editor, cx, |view| {
            matches!(synced_rate(view), lfo::Rate::Hertz(_)) == is_free
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(bank(view).routes(), assigned);
            let project = &view.state.as_ref().unwrap().project;
            let version = if is_free {
                hex_storage::LFO_RATE_ROUTE_SCHEMA_VERSION
            } else {
                hex_storage::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION
            };
            assert_eq!(hex_storage::schema_version_for(project), version);
        });
        visual.run_until_parked();
    }
    assert_eq!(
        editor.read_with(cx, |view, _| synced_rate(view)),
        original_synced
    );
    let rate_route = editor.read_with(cx, |view, _| bank(view).routes()[0]);
    assert_eq!(rate_route.source, manual);
    assert_eq!(
        rate_route.destination,
        ModulationDestination::Modulator {
            source: synced,
            module_type: lfo::TYPE_ID,
            parameter: lfo::RATE_PARAMETER,
        }
    );
    for expected in [0.05, 0.0, -0.05] {
        click(
            &mut visual,
            format!("modulation-route-{}-less", rate_route.id.value()),
            true,
        );
        pump_until(&editor, cx, |view| {
            (bank(view).routes()[0].amount.get() - expected).abs() < 1e-6
        });
        visual.run_until_parked();
    }

    click(&mut visual, format!("lfo-{}-map", synced.value()), true);
    assert!(
        visual
            .debug_bounds(Box::leak(rate_key(synced).into_boxed_str()))
            .is_none(),
        "self-route"
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("map-macro-{}-{}", device.value(), manual.value()).into_boxed_str()
            ))
            .is_none(),
        "cycle"
    );
    click(
        &mut visual,
        format!("map-target-{}-{}", device.value(), parameter::GAIN.value()),
        false,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 3);
    let mapped = editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let mapped = bank(view);
        assert_eq!(mapped.routes()[2].source, synced);
        assert_eq!(mapped.sources()[0], slots.sources()[0]);
        assert_eq!(
            hex_storage::schema_version_for(project),
            hex_storage::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION
        );
        let loaded = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(loaded.device(device).unwrap().modulation(), &mapped);
        mapped
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        bank(view).routes() == &mapped.routes()[..2]
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| bank(view) == mapped);
}

#[gpui::test]
fn modulation_panel_adds_and_maps_seeded_random(cx: &mut TestAppContext) {
    assert_modulation_mapping(
        cx,
        "random",
        hex_engine::random_modulator::TYPE_ID,
        &[hex_devices::utility::parameter::GAIN],
    );
}

#[gpui::test]
fn modulation_panel_adds_and_maps_an_audio_rate_modulator(cx: &mut TestAppContext) {
    assert_modulation_mapping(
        cx,
        "audio rate",
        hex_engine::audio_rate_modulator::TYPE_ID,
        &[hex_devices::utility::parameter::GAIN],
    );
}

fn assert_modulation_mapping(
    cx: &mut TestAppContext,
    source_kind: &str,
    type_id: hex_lattice_contract::DeviceTypeId,
    targets: &[hex_lattice_contract::ParameterId],
) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let click = |visual: &mut VisualTestContext, selector: String| {
        let selector: &'static str = Box::leak(selector.into_boxed_str());
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"));
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.run_until_parked();
        bounds
    };
    let scroll_to = |visual: &mut VisualTestContext, key: &str| {
        let key = Box::leak(key.to_string().into_boxed_str());
        let target = visual.debug_bounds(key).unwrap();
        let strip = visual.debug_bounds("device-modulator-scroll").unwrap();
        if !strip.contains(&target.center()) {
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: strip.center(),
                delta: gpui::ScrollDelta::Pixels(point(
                    px(0.0),
                    strip.center().y - target.center().y,
                )),
                ..Default::default()
            });
            visual.run_until_parked();
        }
        let target = visual.debug_bounds(key).unwrap();
        let strip = visual.debug_bounds("device-modulator-scroll").unwrap();
        assert!(
            strip.contains(&target.center()),
            "{key} at {target:?} outside {strip:?}"
        );
    };
    click(&mut visual, "device-modulator-toggle".into());
    click(&mut visual, format!("add-{source_kind}-{}", device.value()));
    let source = |view: &EditorView| {
        view.state
            .as_ref()?
            .project
            .device(device)?
            .modulation()
            .sources()
            .first()
            .map(|source| source.id)
    };
    assert!(
        editor.read_with(cx, |view, _| source(view).is_some()
            || view.worker_requests_pending > 0
            || !view.pending_requests.is_empty()),
        "add did not dispatch"
    );
    pump_until(&editor, cx, |view| source(view).is_some());
    let source_id = editor.read_with(cx, |view, _| source(view).unwrap());
    visual.run_until_parked();
    if type_id == hex_engine::macro_modulator::TYPE_ID {
        click(&mut visual, format!("macro-{}-more", source_id.value()));
        let value = |view: &EditorView| {
            let bank = view.state.as_ref()?.project.device(device)?.modulation();
            let source = bank.sources().first()?;
            Some(
                hex_engine::macro_modulator::Settings::decode(&source.state)
                    .ok()?
                    .value,
            )
        };
        assert!(
            editor.read_with(cx, |view, _| {
                view.worker_requests_pending > 0
                    || !view.pending_requests.is_empty()
                    || value(view) == Some(0.05)
            }),
            "macro value edit did not dispatch"
        );
        pump_until(&editor, cx, |view| value(view) == Some(0.05));
        visual.run_until_parked();
    }

    if type_id == hex_engine::random_modulator::TYPE_ID {
        let settings = |view: &EditorView| {
            let slot = view
                .state
                .as_ref()?
                .project
                .device(device)?
                .modulation()
                .sources()
                .first()?;
            hex_engine::random_modulator::Settings::decode(&slot.state).ok()
        };
        let initial = editor.read_with(cx, |view, _| settings(view).unwrap());
        let pending = |view: &EditorView| {
            view.worker_requests_pending > 0 || !view.pending_requests.is_empty()
        };
        let faster = format!("random-{}-faster", source_id.value());
        scroll_to(&mut visual, &faster);
        click(&mut visual, faster);
        assert!(
            editor.read_with(cx, |view, _| pending(view)
                || settings(view).is_some_and(|s| s.rate_hz == 2.0)),
            "random rate edit did not dispatch"
        );
        pump_until(&editor, cx, |view| {
            settings(view).is_some_and(|value| value.rate_hz == 2.0)
        });
        visual.run_until_parked();
        let reseed = format!("random-{}-reseed", source_id.value());
        scroll_to(&mut visual, &reseed);
        click(&mut visual, reseed);
        assert!(
            editor.read_with(cx, |view, _| pending(view)
                || settings(view).is_some_and(|s| s.seed != initial.seed)),
            "random seed edit did not dispatch"
        );
        pump_until(&editor, cx, |view| {
            settings(view).is_some_and(|value| value.seed != initial.seed)
        });
        visual.run_until_parked();
        scroll_to(&mut visual, &format!("random-{}-map", source_id.value()));
    }
    click(
        &mut visual,
        format!("{source_kind}-{}-map", source_id.value()),
    );
    assert_eq!(
        editor.read_with(cx, |view, _| view.device_panel.mapping_source),
        Some((device, source_id))
    );
    for (count, parameter) in targets.iter().copied().enumerate() {
        let bounds = click(
            &mut visual,
            format!("map-target-{}-{}", device.value(), parameter.value()),
        );
        assert!(
            editor.read_with(cx, |view, _| view.worker_requests_pending > 0
                || !view.pending_requests.is_empty()
                || view
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .unwrap()
                    .modulation()
                    .routes()
                    .len()
                    > count),
            "target {} did not dispatch from {:?}, modulator panel {:?}",
            parameter.value(),
            bounds,
            visual.debug_bounds("device-modulator-panel"),
        );
        pump_until(&editor, cx, |view| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .modulation()
                .routes()
                .len()
                == count + 1
        });
        visual.run_until_parked();
    }
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let current = project.device(device).unwrap();
        if matches!(
            type_id,
            hex_engine::macro_modulator::TYPE_ID | hex_engine::random_modulator::TYPE_ID
        ) {
            let bytes = hex_storage::encode(project).unwrap();
            let reopened = hex_storage::decode(&bytes).unwrap();
            assert_eq!(
                reopened.device(device).unwrap().modulation(),
                current.modulation()
            );
        }
        let device = current;
        assert_eq!(device.state(), &original);
        assert_eq!(device.modulation().sources()[0].state.type_id(), type_id);
        assert!(
            device
                .modulation()
                .routes()
                .iter()
                .all(|route| route.source == source_id)
        );
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("map-target-{}-{}", device.value(), targets[0].value()).into_boxed_str()
            ))
            .is_none()
    );
    if type_id == hex_engine::lfo_modulator::TYPE_ID {
        use hex_project::{ModulationAmount, ModulationRange};
        let routes = |view: &EditorView| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .modulation()
                .routes()
                .to_vec()
        };
        let mut active = editor.read_with(cx, |view, _| routes(view));
        let id = active[0].id.value();
        let mut click_route = |suffix: &str| {
            visual.run_until_parked();
            let selector = format!("modulation-route-{id}-{suffix}");
            scroll_to(&mut visual, &selector);
            click(&mut visual, selector);
        };
        click_route("more");
        active[0].amount = ModulationAmount::new(active[0].amount.get() + 0.05).unwrap();
        pump_until(&editor, cx, |view| routes(view) == active);
        // The control bounds reflect the acknowledged authored state.
        click_route("range");
        active[0].range = ModulationRange {
            start: ModulationAmount::new(0.0).unwrap(),
            end: ModulationAmount::new(1.0).unwrap(),
        };
        pump_until(&editor, cx, |view| routes(view) == active);

        let mut bypassed = active.clone();
        bypassed[0].enabled = false;
        click_route("toggle");
        pump_until(&editor, cx, |view| routes(view) == bypassed);
        editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let loaded = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
            assert_eq!(
                loaded.device(device).unwrap().modulation().routes(),
                bypassed
            );
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| routes(view) == active);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| routes(view) == bypassed);
        click_route("toggle");
        pump_until(&editor, cx, |view| routes(view) == active);

        use hex_engine::lfo_modulator::{Settings, Waveform};
        let shape = |view: &EditorView| {
            let project = &view.state.as_ref().unwrap().project;
            let slot = &project.device(device).unwrap().modulation().sources()[0];
            assert_eq!(slot.id, source_id);
            Settings::decode(&slot.state).unwrap()
        };
        let original = editor.read_with(cx, |view, _| shape(view));
        let control = format!("lfo-{}-waveform", source_id.value());
        for waveform in [Waveform::Triangle, Waveform::RisingRamp, Waveform::Square] {
            visual.run_until_parked();
            scroll_to(&mut visual, &control);
            click(&mut visual, control.clone());
            let expected = Settings {
                waveform,
                ..original
            };
            pump_until(&editor, cx, |view| {
                shape(view) == expected && routes(view) == active
            });
        }
        let square = editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            assert_eq!(
                hex_storage::schema_version_for(project),
                hex_storage::LFO_SHAPE_SCHEMA_VERSION
            );
            let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
            assert_eq!(
                reopened.device(device).unwrap().modulation(),
                project.device(device).unwrap().modulation()
            );
            shape(view)
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            shape(view).waveform == Waveform::RisingRamp
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            shape(view) == square && routes(view) == active
        });

        visual.run_until_parked();
        let map = format!("lfo-{}-map", source_id.value());
        scroll_to(&mut visual, &map);
        click(&mut visual, map);
        let mut previous = active;
        for (target, name) in [
            (
                hex_project::DeviceAutomationTarget::HostOutputGain,
                "output-gain",
            ),
            (
                hex_project::DeviceAutomationTarget::HostInputGain,
                "input-gain",
            ),
        ] {
            let key = format!("map-{name}-{}", device.value());
            scroll_to(&mut visual, &key);
            click(&mut visual, key);
            pump_until(&editor, cx, |view| routes(view).len() == previous.len() + 1);
            let mapped = editor.read_with(cx, |view, _| {
                let project = &view.state.as_ref().unwrap().project;
                let current = project.device(device).unwrap();
                let mapped = routes(view);
                let route = mapped.last().unwrap();
                assert_eq!(route.source, source_id);
                assert!(matches!(
                    route.destination,
                    hex_project::ModulationDestination::Device { target: actual, .. } if actual == target
                ));
                assert_eq!(current.state().output_gain_millidb(), 0);
                assert_eq!(current.state().input_gain_millidb(), 0);
                let loaded = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
                assert_eq!(loaded.device(device).unwrap().modulation().routes(), mapped);
                mapped
            });
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
            pump_until(&editor, cx, |view| routes(view) == previous);
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
            pump_until(&editor, cx, |view| routes(view) == mapped);
            previous = mapped;
        }
    }
}

#[gpui::test]
fn modulation_panel_maps_a_bound_source_effect_on_its_receiver(cx: &mut TestAppContext) {
    use hex_project::{DeviceAutomationTarget, DeviceOwner, ModulationDestination};

    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    let edit = |command| {
        service.submit(Request::Edit(command)).unwrap();
        let event = receive_service_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let add_track = |name: &str| {
        let Some(Created::Track(track)) = edit(Command::AddMidiTrack { name: name.into() }).created
        else {
            panic!("track fixture");
        };
        track
    };
    let receiver = add_track("receiver");
    let contributor = add_track("contributor");
    let device_type = hex_lattice_contract::DeviceTypeId::from_static("hex.utility");
    let base = hex_engine::default_native_device_state(device_type).unwrap();
    let created = edit(Command::CreateNativeSource {
        name: "shared".into(),
        devices: vec![
            hex_project::default_instrument_state().unwrap(),
            base.clone(),
        ],
        instrument_index: 0,
    });
    let Some(Created::NativeSource(source)) = created.created else {
        panic!("source fixture");
    };
    let device = created
        .state
        .project
        .native_source(source)
        .unwrap()
        .post_instrument_effects()[0]
        .id();
    assert!(!hex_engine::project_device_host_gain_modulation_supported(
        &created.state.project,
        device
    ));
    edit(Command::BindTrackSource {
        track: receiver,
        source: Some(source),
    });
    let bound = edit(Command::BindTrackSource {
        track: contributor,
        source: Some(source),
    });
    assert!(hex_engine::project_device_host_gain_modulation_supported(
        &bound.state.project,
        device
    ));
    let gain = hex_lattice_contract::ParameterId::from_static(1);
    assert!(hex_engine::project_device_modulation_target_supported(
        &bound.state.project,
        device,
        gain,
    ));
    assert!(!hex_engine::project_device_modulation_target_supported(
        &bound.state.project,
        device,
        hex_lattice_contract::ParameterId::from_static(11),
    ));

    let (handle, editor) = actor_editor_window(cx, service, bound.state, receiver);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector: &'static str =
        Box::leak(format!("source-effect-modulation-{}", device.value()).into_boxed_str());
    let bounds = visual
        .debug_bounds(selector)
        .expect("source effect on its receiver");
    let click = |visual: &mut VisualTestContext, key: String| {
        let key = Box::leak(key.into_boxed_str());
        let element = visual
            .debug_bounds(key)
            .unwrap_or_else(|| panic!("missing {key}"));
        assert!(
            bounds.contains(&element.center()),
            "{key} belongs to another card"
        );
        visual.simulate_click(element.center(), Modifiers::default());
        visual.run_until_parked();
    };
    click(&mut visual, "device-modulator-toggle".into());
    click(&mut visual, format!("add-lfo-{}", device.value()));
    let source_slot = |view: &EditorView| {
        view.state
            .as_ref()?
            .project
            .device(device)?
            .modulation()
            .sources()
            .first()
            .map(|slot| slot.id)
    };
    pump_until(&editor, cx, |view| source_slot(view).is_some());
    let slot = editor.read_with(cx, |view, _| source_slot(view).unwrap());
    visual.run_until_parked();
    click(&mut visual, format!("lfo-{}-map", slot.value()));
    click(&mut visual, format!("map-output-gain-{}", device.value()));

    let route_count = |view: &EditorView| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .routes()
            .len()
    };
    let dispatched = |cx: &mut TestAppContext, expected| {
        editor.read_with(cx, |view, _| {
            view.worker_requests_pending > 0
                || !view.pending_requests.is_empty()
                || route_count(view) == expected
        })
    };
    assert!(dispatched(cx, 1), "source output trim did not dispatch");
    pump_until(&editor, cx, |view| route_count(view) == 1);
    visual.run_until_parked();
    click(
        &mut visual,
        format!("map-target-{}-{}", device.value(), gain.value()),
    );
    assert!(dispatched(cx, 2), "source provider gain did not dispatch");
    pump_until(&editor, cx, |view| route_count(view) == 2);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let location = project.device_location(device).unwrap();
        assert_eq!(location.owner, DeviceOwner::Source(source));
        assert_eq!(location.device.state(), &base);
        let route = location.device.modulation().routes()[0];
        assert_eq!(route.source, slot);
        assert!(matches!(
            route.destination,
            ModulationDestination::Device {
                target: DeviceAutomationTarget::HostOutputGain,
                ..
            }
        ));
        let matrix_route = location.device.modulation().routes()[1];
        assert_eq!(matrix_route.source, slot);
        assert!(
            matches!(matrix_route.destination, ModulationDestination::Device {
            target: DeviceAutomationTarget::Parameter(id), ..
        } if id == gain)
        );
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(
            reopened.device(device).unwrap().modulation().routes(),
            &[route, matrix_route]
        );
    });

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| route_count(view) == 1);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| route_count(view) == 2);
    editor.update(cx, |view, cx| {
        view.selected_track = Some(contributor);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(selector).is_none());
}

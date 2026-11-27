use super::*;

use crate::editor::worker_ui_fixture::{self, WorkerUiFixture};

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

mod chorus_ui_tests;

mod shifter_ui_tests;

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

mod filter_ui_tests;

mod fold_tests;

mod gate_ui_tests;

mod header_actions_tests;

mod header_drop_tests;

mod header_style_tests;

mod host_slot_layout_ui_tests;

mod keyboard_delete_tests;

mod keyboard_navigation_tests;

mod limiter_ui_tests;

mod navigation_ui_tests;

mod nested_host_ui_tests;

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

mod source_housing_tests;

pub(in crate::editor) mod source_picker_ui_tests;

mod vst3_degraded_tests;

pub(in crate::editor) fn receive_service_event(service: &Service) -> Event {
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

pub(in crate::editor) fn actor_native_effect_fixture(
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

pub(in crate::editor) fn actor_editor_window(
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
                view.device_panel.active = true;
                view.audio_detail.clip_view_open = true;
                view.mode = super::super::EditorMode::Session;
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    (handle, editor)
}

pub(in crate::editor) fn pump_until(
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
                view.device_panel.gesture.snapshot(),
                view.device_panel.gesture_barriers.len(),
                view.error,
                view.state.as_ref().map(|state| state.revision)
            ))
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn device_playback_fixture(cx: &mut TestAppContext) -> (WorkerUiFixture, TrackId, DeviceId) {
    let insert = Command::InsertTrackWithDevice {
        name: "device track".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().unwrap(),
    };
    let mut project = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let Created::Track(track) = project.execute(insert.clone()).unwrap() else {
        panic!("expected an instrument track");
    };
    let fixture = worker_ui_fixture::open_with_section_support(
        cx,
        vec![
            insert,
            Command::InsertDevice {
                track,
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_devices::utility::UTILITY_TYPE_ID,
                )
                .unwrap(),
            },
        ],
        false,
        true,
    );
    let (track, device) = fixture.editor.update(cx, |view, cx| {
        assert!(
            view.normal_start_source().is_some(),
            "worker must install its captured source"
        );
        let track = &view.state.as_ref().unwrap().project.tracks()[0];
        let ids = (track.id(), track.devices()[0].id());
        view.selected_track = Some(ids.0);
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        view.mode = crate::editor::EditorMode::Session;
        cx.notify();
        ids
    });
    (fixture, track, device)
}

fn pump_device_playback(fixture: &mut WorkerUiFixture, cx: &mut TestAppContext) {
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    fixture.editor.update(cx, |view, cx| {
        if view.tick(cx) {
            cx.notify();
        }
    });
}

fn exercise_device_drop_then_space(cx: &mut TestAppContext, cancel_start: bool) {
    use std::sync::atomic::Ordering::Acquire;
    cx.update(crate::editor::bind_editor_keys);
    let (mut fixture, track, existing) = device_playback_fixture(cx);
    let handle = fixture.handle;
    let editor = fixture.editor.clone();
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(800.0)));
    visual.run_until_parked();

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
    if cancel_start {
        visual.simulate_keystrokes("space");
    }
    let deadline = crate::test_wait::hang_deadline();
    loop {
        pump_device_playback(&mut fixture, cx);
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
        if settled {
            editor.read_with(cx, |view, _| {
                assert!(
                    view.error.is_none(),
                    "device installed but play was refused: {:?}",
                    view.error
                );
            });
        }
        let playing = fixture.backend.status().unwrap().playing.load(Acquire);
        if cancel_start {
            assert!(!playing, "second Space must cancel the pending start");
        }
        if settled && (playing || cancel_start) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "drop followed by Space did not both start and apply: {:?}",
            editor.read_with(cx, |view, _| view.error.clone())
        );
    }
    if cancel_start {
        // Consume any command sent by the tick that installed the canonical edit.
        pump_device_playback(&mut fixture, cx);
        assert!(!fixture.backend.status().unwrap().playing.load(Acquire));
        assert!(editor.read_with(cx, |view, _| view.audio_pub.deferred_toggle_play.is_none()));
    }
    fixture.finish(cx);
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

mod main_chain_ui_tests;

mod navigation_capture_tests;

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

fn exercise_sampler_modulation_mapper(cx: &mut TestAppContext, host_output: bool) {
    use super::super::{
        device_automation::host_tests::TestWave,
        worker_ui_fixture::{self, WorkerUiFixture},
    };
    use hex_project::{DeviceAutomationTarget, ModulationDestination};
    use std::time::Instant;

    let file = TestWave::new();
    let type_id = hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sampler");
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        type_id,
    )
    .unwrap();
    let mut fixture = worker_ui_fixture::open(
        cx,
        vec![Command::InsertTrackWithDevice {
            name: "sampler".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state,
        }],
    );
    let editor = fixture.editor.clone();
    let empty = editor.read_with(cx, |view, _| Arc::clone(view.state.as_ref().unwrap()));
    let track = empty.project.tracks()[0].id();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.simulate_resize(size(px(1800.0), px(650.0)));
    editor.update(&mut visual, |view, cx| {
        view.mode = super::super::EditorMode::Session;
        view.selected_track = Some(track);
        view.device_panel.owner = Some(DevicePanelOwner::Track(track));
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        cx.notify();
    });
    visual.run_until_parked();
    let pump = |fixture: &mut WorkerUiFixture,
                visual: &mut VisualTestContext,
                done: &dyn Fn(&EditorView) -> bool| {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let ready = fixture.editor.update(visual, |view, cx| {
                view.tick(cx);
                cx.notify();
                assert!(view.error.is_none(), "{:?}", view.error);
                done(view)
                    && view.state.as_ref().is_some_and(|state| {
                        view.applied_project_revision() == Some(state.revision)
                    })
            });
            if ready {
                break;
            }
            assert!(Instant::now() < deadline, "Sampler action was not applied");
            std::thread::yield_now();
        }
        visual.run_until_parked();
    };
    let toggle = || "device-modulator-toggle".to_owned();
    click_modulator_control(&mut visual, toggle(), false);
    assert!(visual.debug_bounds("device-modulator-scroll").is_none());
    click_modulator_control(&mut visual, toggle(), false);
    let empty_slot = empty.project.track(track).unwrap().instrument().unwrap();
    let slot = hex_session::service::SamplerLoadSlot::Existing {
        device: empty_slot.id(),
        expected_state: empty_slot.state().clone(),
    };
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::LoadFirstSampler {
            request_id: 1,
            project_generation: view.project_generation,
            revision: empty.revision,
            target: hex_session::service::SamplerLoadTarget::Track { track, slot },
            path: file.0.clone(),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }));
    });
    pump(&mut fixture, &mut visual, &|view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .is_some_and(|device| !device.state().asset_references().is_empty())
        })
    });
    visual.run_until_parked();
    let (device, base) = editor.read_with(cx, |view, _| {
        let device = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap();
        (device.id(), device.state().clone())
    });
    let bank = |view: &EditorView| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    let gain = format!("sampler-gain-{}-value", device.value());
    let gain_selector = Box::leak(gain.into_boxed_str());
    assert!(visual.debug_bounds(gain_selector).is_some());
    click_modulator_control(&mut visual, "device-modulator-toggle".into(), false);
    click_modulator_control(
        &mut visual,
        format!("add-modulator-{}", device.value()),
        true,
    );
    assert!(editor.read_with(cx, |view, _| {
        let items = &view.context_menu.as_ref().unwrap().items;
        ["note counter", "ramp", "ahd on release", "adsr"]
            .into_iter()
            .all(|label| items.iter().any(|item| item.label == label))
    }));
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    add_modulator_from_menu(&mut visual, device, "lfo");
    pump(&mut fixture, &mut visual, &|view| {
        bank(view).sources().len() == 1
    });
    visual.run_until_parked();
    let source = editor.read_with(cx, |view, _| bank(view).sources()[0].id);
    click_modulator_control(&mut visual, format!("lfo-{}-map", source.value()), true);
    let parameter = hex_engine::SAMPLER_GAIN_PARAMETER;
    let destination = if host_output {
        DeviceAutomationTarget::HostOutputGain
    } else {
        DeviceAutomationTarget::Parameter(parameter)
    };
    let target = if host_output {
        let input = Box::leak(format!("map-input-gain-{}", device.value()).into_boxed_str());
        assert!(visual.debug_bounds(input).is_none());
        modulator_control_position(
            &mut visual,
            format!("map-output-gain-{}", device.value()),
            true,
        )
    } else {
        let overlay = format!("map-target-{}-{}", device.value(), parameter.value());
        let target_selector = Box::leak(overlay.clone().into_boxed_str());
        let shell = Box::leak(format!("device-shell-{}", device.value()).into_boxed_str());
        // Use the part of the gain overlay visible inside the device rail
        // and scroll the Sampler body vertically when it sits below the fold.
        let rail = visual.debug_bounds("device-chain-scroll").unwrap();
        let visible = visual.debug_bounds(shell).unwrap().intersect(&rail);
        let target = visual.debug_bounds(target_selector).unwrap();
        let hit = target.intersect(&visible);
        assert!(
            hit.size.width > px(0.0),
            "gain has no visible horizontal area"
        );
        if target.bottom() > visible.bottom() - px(6.0) {
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: point(hit.center().x, visible.bottom() - px(14.0)),
                delta: gpui::ScrollDelta::Pixels(point(
                    px(0.0),
                    visible.center().y - target.center().y,
                )),
                ..Default::default()
            });
            visual.run_until_parked();
        }
        let rail = visual.debug_bounds("device-chain-scroll").unwrap();
        let visible = visual.debug_bounds(shell).unwrap().intersect(&rail);
        let hit = visual
            .debug_bounds(target_selector)
            .unwrap()
            .intersect(&visible);
        assert!(
            hit.size.height > px(0.0),
            "gain is outside the visible housing"
        );
        hit.center()
    };
    visual.simulate_mouse_move(target, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::ResizeUpDown),
        "sampler modulation target at {target:?}"
    );
    visual.simulate_click(target, Modifiers::default());
    visual.run_until_parked();
    assert!(
        editor.read_with(cx, |view, _| {
            view.worker_requests_pending > 0
                || !view.pending_requests.is_empty()
                || bank(view).routes().len() == 1
        }),
        "Sampler gain mapping did not submit"
    );
    pump(&mut fixture, &mut visual, &|view| {
        bank(view).routes().len() == 1
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.device(device).unwrap().state(), &base);
        let mapped = bank(view);
        assert_eq!(mapped.routes()[0].source, source);
        assert_eq!(
            mapped.routes()[0].destination,
            ModulationDestination::Device {
                device_type: type_id,
                target: destination,
                vst3_class: None,
            }
        );
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(&reopened, project.as_ref());
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&mut fixture, &mut visual, &|view| {
        bank(view).routes().is_empty()
    });
    fixture.finish(cx);
}

fn exercise_vector_modulation_mapper(cx: &mut TestAppContext, kind: Option<&str>) {
    use hex_devices::operator_state::{OPERATOR_METADATA, OPERATOR_TYPE_ID, parameter};
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{DeviceAutomationTarget, ModulationDestination};

    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    let mut original = OPERATOR_METADATA.default_state().unwrap();
    if kind.is_none() {
        original = OPERATOR_METADATA
            .patch_parameter(&original, parameter::GLOBAL_MODE, ParameterValue::Choice(0))
            .unwrap();
    }
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "vector".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: original.clone(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Some(Created::Track(track)) = event.created else {
        panic!("vector fixture: {:?}", event.error);
    };
    let device = event
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, editor) = actor_editor_window(cx, service, event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(650.0)));
    visual.run_until_parked();
    let bank = |view: &EditorView| {
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
    let Some(kind) = kind else {
        assert!(
            visual
                .debug_bounds("device-modulator-slots-empty-slot")
                .is_some()
        );
        assert!(visual.debug_bounds("device-modulator-scroll").is_none());
        let add = Box::leak(format!("add-modulator-{}", device.value()).into_boxed_str());
        assert!(visual.debug_bounds(add).is_none());
        click_modulator_control(&mut visual, "device-macro-toggle".into(), false);
        let add = Box::leak(format!("add-macro-{}", device.value()).into_boxed_str());
        assert!(visual.debug_bounds(add).is_none());
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .unwrap()
                    .state(),
                &original
            );
            assert!(bank(view).is_empty());
        });
        return;
    };
    click_modulator_control(
        &mut visual,
        format!("add-modulator-{}", device.value()),
        true,
    );
    let index = editor.read_with(cx, |view, _| {
        let items = &view.context_menu.as_ref().unwrap().items;
        assert!(
            ["note counter", "ramp", "ahd on release", "adsr"]
                .into_iter()
                .all(|label| items.iter().any(|item| item.label == label))
        );
        assert!(!items.iter().any(|item| item.label == "note pressure"));
        items.iter().position(|item| item.label == kind).unwrap()
    });
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| bank(view).sources().len() == 1);
    visual.run_until_parked();
    let source = editor.read_with(cx, |view, _| bank(view).sources()[0].id);
    click_modulator_control(&mut visual, format!("{kind}-{}-map", source.value()), true);
    let input = Box::leak(format!("map-input-gain-{}", device.value()).into_boxed_str());
    assert!(visual.debug_bounds(input).is_none());
    click_modulator_control(
        &mut visual,
        format!("map-output-gain-{}", device.value()),
        true,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 1);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let current = project.device(device).unwrap();
        let route = current.modulation().routes()[0];
        assert_eq!(route.source, source);
        assert_eq!(
            route.destination,
            ModulationDestination::Device {
                device_type: OPERATOR_TYPE_ID,
                target: DeviceAutomationTarget::HostOutputGain,
                vst3_class: None,
            }
        );
        assert_eq!(current.state(), &original);
        assert_eq!(
            hex_storage::schema_version_for(project),
            hex_storage::OPERATOR_MODULATION_SCHEMA_VERSION,
        );
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(&reopened, project.as_ref());
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| bank(view).routes().is_empty());
}

enum Fm4MappingFixture {
    Carriers,
    Pressure,
    Counter,
    Ramp,
    AhdRelease,
    Adsr,
    OscillatorLevels,
    OscillatorPhases,
    ConnectionDepths,
    HostOutput,
    FreeSteps,
}

fn exercise_fm4_modulation_mapper(cx: &mut TestAppContext, fixture: Fm4MappingFixture) {
    use hex_devices::fm4::device::{FM4_DEFINITION, FM4_TYPE_ID, parameter};
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{DeviceAutomationTarget, ModulationDestination};

    let pressure = matches!(fixture, Fm4MappingFixture::Pressure);
    let counter = matches!(fixture, Fm4MappingFixture::Counter);
    let ramp = matches!(fixture, Fm4MappingFixture::Ramp);
    let ahd_release = matches!(fixture, Fm4MappingFixture::AhdRelease);
    let adsr = matches!(fixture, Fm4MappingFixture::Adsr);
    let levels = matches!(fixture, Fm4MappingFixture::OscillatorLevels);
    let phases = matches!(fixture, Fm4MappingFixture::OscillatorPhases);
    let edges = matches!(fixture, Fm4MappingFixture::ConnectionDepths);
    let output_trim = matches!(fixture, Fm4MappingFixture::HostOutput);
    let edge_targets = [parameter::modulation(2, 1), parameter::modulation(3, 2)];
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    let mut original = FM4_DEFINITION.default_state().unwrap();
    if output_trim {
        original = original.with_output_gain_millidb(-6_000).unwrap();
    }
    if edges {
        for id in edge_targets {
            original = FM4_DEFINITION
                .patch_parameter(&original, id, ParameterValue::Float(0.25))
                .unwrap();
        }
    }
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "fm4 track".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: original.clone(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Some(Created::Track(track)) = event.created else {
        panic!("track fixture: {:?}", event.error);
    };
    let device = event
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, editor) = actor_editor_window(cx, service, event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let click = click_modulator_control;
    click(&mut visual, "device-modulator-toggle".into(), false);
    click(
        &mut visual,
        format!("add-modulator-{}", device.value()),
        true,
    );
    assert_eq!(
        editor.read_with(cx, |view, _| {
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .map(|item| item.label)
                .collect::<Vec<_>>()
        }),
        [
            "lfo",
            "macro",
            "macro-4",
            "random",
            "note pressure",
            "button",
            "math",
            "buttons",
            "mix",
            "note counter",
            "ramp",
            "ahd on release",
            "adsr",
            "steps",
        ],
    );
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("context-menu-panel").is_none());
    let bank = |view: &EditorView| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    if matches!(fixture, Fm4MappingFixture::FreeSteps) {
        click(
            &mut visual,
            format!("add-modulator-{}", device.value()),
            true,
        );
        let index = editor.read_with(cx, |view, _| {
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .position(|item| item.label == "steps")
                .unwrap()
        });
        for _ in 0..index {
            visual.simulate_keystrokes("down");
        }
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, |view| bank(view).sources().len() == 1);
        visual.run_until_parked();
        let source = editor.read_with(cx, |view, _| {
            let bank = bank(view);
            let source = &bank.sources()[0];
            assert_eq!(source.state.type_id(), hex_engine::steps_modulator::TYPE_ID);
            source.id
        });
        let key = |suffix| Box::leak(format!("steps-{}-{suffix}", source.value()).into_boxed_str());
        assert!(visual.debug_bounds(key("timing-mode")).is_none());
        assert!(visual.debug_bounds(key("step time-shorter")).is_some());
        return;
    }
    let kind = if pressure || counter || ramp || ahd_release || adsr {
        click(
            &mut visual,
            format!("add-modulator-{}", device.value()),
            true,
        );
        let kind = if pressure {
            "note pressure"
        } else if counter {
            "note counter"
        } else if ramp {
            "ramp"
        } else if ahd_release {
            "ahd on release"
        } else {
            "adsr"
        };
        let index = editor.read_with(cx, |view, _| {
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .position(|item| item.label == kind)
                .unwrap()
        });
        for _ in 0..index {
            visual.simulate_keystrokes("down");
        }
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        kind
    } else {
        add_modulator_from_menu(&mut visual, device, "lfo");
        "lfo"
    };
    pump_until(&editor, cx, |view| bank(view).sources().len() == 1);
    visual.run_until_parked();
    let source = editor.read_with(cx, |view, _| bank(view).sources()[0].id);
    if counter {
        use hex_engine::note_counter_modulator::Settings;
        let settings =
            |view: &EditorView| Settings::decode(&bank(view).sources()[0].state).unwrap();
        let edits = [
            (
                "steps-more",
                Settings {
                    steps: 9,
                    increment: 1,
                    bipolar: false,
                },
            ),
            (
                "increment-more",
                Settings {
                    steps: 9,
                    increment: 2,
                    bipolar: false,
                },
            ),
            (
                "polarity",
                Settings {
                    steps: 9,
                    increment: 2,
                    bipolar: true,
                },
            ),
        ];
        for (suffix, expected) in edits {
            click(
                &mut visual,
                format!("note counter-{}-{suffix}", source.value()),
                true,
            );
            pump_until(&editor, cx, |view| settings(view) == expected);
            visual.run_until_parked();
        }
    }
    if ramp {
        use hex_engine::ramp_modulator::{Curve, Settings};
        let settings =
            |view: &EditorView| Settings::decode(&bank(view).sources()[0].state).unwrap();
        for (suffix, expected) in [
            (
                "shorter",
                Settings {
                    duration_micros: 250_000,
                    ..Settings::default()
                },
            ),
            (
                "curve",
                Settings {
                    duration_micros: 250_000,
                    curve: Curve::EaseIn,
                    ..Settings::default()
                },
            ),
            (
                "direction",
                Settings {
                    duration_micros: 250_000,
                    curve: Curve::EaseIn,
                    falling: true,
                    looping: false,
                },
            ),
            (
                "loop",
                Settings {
                    duration_micros: 250_000,
                    curve: Curve::EaseIn,
                    falling: true,
                    looping: true,
                },
            ),
        ] {
            click(
                &mut visual,
                format!("ramp-{}-{suffix}", source.value()),
                true,
            );
            pump_until(&editor, cx, |view| settings(view) == expected);
            visual.run_until_parked();
        }
    }
    if ahd_release {
        use hex_engine::ahd_release_modulator::{Curve, Settings};
        let settings =
            |view: &EditorView| Settings::decode(&bank(view).sources()[0].state).unwrap();
        let mut expected = Settings::default();
        type SettingsEdit = fn(&mut Settings);
        let edits: [(&str, SettingsEdit); 7] = [
            ("attack-shorter", |state| state.durations_micros[0] = 2_500),
            ("hold-longer", |state| state.durations_micros[1] = 40_000),
            ("decay-shorter", |state| state.durations_micros[2] = 100_000),
            ("attack-curve", |state| state.curves[0] = Curve::EaseIn),
            ("decay-curve", |state| state.curves[1] = Curve::EaseIn),
            ("decay-curve", |state| state.curves[1] = Curve::EaseOut),
            ("single-trigger", |state| state.single_trigger = true),
        ];
        for (suffix, update) in edits {
            click(
                &mut visual,
                format!("ahd on release-{}-{suffix}", source.value()),
                true,
            );
            update(&mut expected);
            pump_until(&editor, cx, |view| settings(view) == expected);
            visual.run_until_parked();
        }
    }
    if adsr {
        use hex_engine::adsr_modulator::Settings;
        let settings =
            |view: &EditorView| Settings::decode(&bank(view).sources()[0].state).unwrap();
        let mut expected = Settings::default();
        assert_eq!(editor.read_with(cx, |view, _| settings(view)), expected);
        type SettingsEdit = fn(&mut Settings);
        let edits: [(&str, SettingsEdit); 5] = [
            ("attack-shorter", |state| state.durations_micros[0] = 2_500),
            ("decay-longer", |state| state.durations_micros[1] = 200_000),
            ("release-shorter", |state| {
                state.durations_micros[2] = 100_000
            }),
            ("sustain-more", |state| state.sustain = 0.55),
            ("single-trigger", |state| state.single_trigger = true),
        ];
        for (suffix, edit) in edits {
            click(
                &mut visual,
                format!("adsr-{}-{suffix}", source.value()),
                true,
            );
            edit(&mut expected);
            pump_until(&editor, cx, |view| settings(view) == expected);
            visual.run_until_parked();
        }
    }
    if pressure {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("note pressure-{}-duplicate", source.value()).into_boxed_str()
                ))
                .is_none()
        );
        click(
            &mut visual,
            format!("add-modulator-{}", device.value()),
            true,
        );
        assert!(editor.read_with(cx, |view, _| {
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .all(|item| item.label != "note pressure")
        }));
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
    }
    click(&mut visual, format!("{kind}-{}-map", source.value()), true);
    let host_input: &'static str =
        Box::leak(format!("map-input-gain-{}", device.value()).into_boxed_str());
    let host_output = format!("map-output-gain-{}", device.value());
    assert!(visual.debug_bounds(host_input).is_none());
    if pressure {
        assert!(
            visual
                .debug_bounds(Box::leak(host_output.clone().into_boxed_str()))
                .is_none()
        );
    }
    if output_trim {
        let start = modulator_control_position(&mut visual, host_output, true);
        let end = point(start.x, start.y + px(45.0));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, |view| bank(view).routes().len() == 1);
        editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let current = project.device(device).unwrap();
            let route = current.modulation().routes()[0];
            assert_eq!(route.source, source);
            assert_eq!(route.amount.get(), -0.45);
            assert_eq!(
                route.destination,
                ModulationDestination::Device {
                    device_type: FM4_TYPE_ID,
                    target: DeviceAutomationTarget::HostOutputGain,
                    vst3_class: None,
                }
            );
            assert_eq!(current.state(), &original);
            let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
            assert_eq!(&reopened, project.as_ref());
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| bank(view).routes().is_empty());
        return;
    }
    if !levels && !phases && !edges {
        editor.update(cx, |view, cx| {
            view.device_panel.parameter_rows[&device]
                .scroll
                .set_offset(point(px(0.0), px(-1_000_000.0)));
            cx.notify();
        });
        visual.run_until_parked();
    }

    let map_key = |parameter: hex_lattice_contract::ParameterId| {
        format!("map-target-{}-{}", device.value(), parameter.value())
    };
    let release_control = Box::leak(
        format!(
            "device-{}-parameter-{}-slider",
            device.value(),
            parameter::RELEASE.value()
        )
        .into_boxed_str(),
    );
    if !levels && !phases && !edges {
        assert!(visual.debug_bounds(release_control).is_some());
    }
    assert!(
        visual
            .debug_bounds(Box::leak(map_key(parameter::RELEASE).into_boxed_str()))
            .is_none()
    );

    let controls = if phases {
        [1, 2, 3, 4].map(parameter::oscillator_phase)
    } else if levels {
        [1, 2, 3, 4].map(parameter::oscillator_level)
    } else {
        [1, 2, 3, 4].map(parameter::carrier)
    };
    let targets = if edges {
        &edge_targets[..]
    } else if pressure || counter || ramp || ahd_release || adsr {
        &controls[..1]
    } else {
        &controls[..]
    };
    let body = Box::leak(format!("device-body-{}", device.value()).into_boxed_str());
    let shell = Box::leak(format!("device-shell-{}", device.value()).into_boxed_str());
    let viewport = |visual: &mut VisualTestContext| {
        visual
            .debug_bounds(body)
            .unwrap()
            .intersect(&visual.debug_bounds(shell).unwrap())
    };
    for (index, &target) in targets.iter().enumerate() {
        let key = map_key(target);
        let selector = Box::leak(key.clone().into_boxed_str());
        // The inner row window overscans its scroll area. The outer housing
        // can also clip it when the modulator strip is open.
        for _ in 0..8 {
            let bounds = viewport(&mut visual);
            let position = visual.debug_bounds(selector).map(|target| target.center());
            if position.is_some_and(|position| bounds.contains(&position)) {
                break;
            }
            let delta = position.map_or(px(-56.0), |position| bounds.center().y - position.y);
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: bounds.center(),
                delta: gpui::ScrollDelta::Pixels(point(px(0.0), delta)),
                ..Default::default()
            });
            visual.run_until_parked();
        }
        let bounds = viewport(&mut visual);
        let position = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {key} in {bounds:?}"))
            .center();
        assert!(
            bounds.contains(&position),
            "{key} at {position:?} outside {bounds:?}"
        );
        click(&mut visual, key, false);
        assert!(
            editor.read_with(cx, |view, _| {
                view.worker_requests_pending > 0
                    || !view.pending_requests.is_empty()
                    || bank(view).routes().len() == index + 1
            }),
            "carrier {} did not dispatch: {:?}",
            target.value(),
            editor.read_with(cx, |view, _| view.error.clone())
        );
        pump_until(&editor, cx, |view| bank(view).routes().len() == index + 1);
        visual.run_until_parked();
    }

    if pressure {
        for target in &controls[1..] {
            assert!(
                visual
                    .debug_bounds(Box::leak(map_key(*target).into_boxed_str()))
                    .is_none()
            );
        }
    }
    let mapped = editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let current = project.device(device).unwrap();
        for (route, &target) in current.modulation().routes().iter().zip(targets) {
            assert_eq!(route.source, source);
            if pressure {
                assert_eq!([route.range.start.get(), route.range.end.get()], [0.0, 1.0]);
                assert_eq!(
                    current.modulation().sources()[0].state.type_id(),
                    hex_engine::note_pressure_modulator::TYPE_ID
                );
            }
            assert_eq!(
                route.destination,
                ModulationDestination::Device {
                    device_type: FM4_TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(target),
                    vst3_class: None,
                }
            );
        }
        assert_eq!(current.state(), &original);
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(&reopened, project.as_ref());
        current.modulation().clone()
    });
    let route = mapped.routes()[0].id;
    for _ in 0..3 {
        let strip = visual.debug_bounds("device-modulator-scroll").unwrap();
        let housing = visual.debug_bounds(shell).unwrap();
        if housing.contains(&strip.origin) {
            break;
        }
        let position = viewport(&mut visual).center();
        visual.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(1000.0))),
            ..Default::default()
        });
        visual.run_until_parked();
    }
    let housing = visual.debug_bounds(shell).unwrap();
    let strip = visual.debug_bounds("device-modulator-scroll").unwrap();
    assert!(
        housing.contains(&strip.origin),
        "the modulator strip is clipped"
    );
    for expected in [0.05, 0.0, -0.05] {
        click(
            &mut visual,
            format!("modulation-route-{}-less", route.value()),
            true,
        );
        let adjusted =
            |view: &EditorView| (bank(view).routes()[0].amount.get() - expected).abs() < 1e-6;
        assert!(
            editor.read_with(cx, |view, _| {
                view.worker_requests_pending > 0
                    || !view.pending_requests.is_empty()
                    || adjusted(view)
            }),
            "the depth adjustment did not dispatch"
        );
        pump_until(&editor, cx, adjusted);
        visual.run_until_parked();
    }
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        bank(view).routes()[0].amount.get() == 0.0
    });
}

fn add_modulator_from_menu(visual: &mut VisualTestContext, device: DeviceId, kind: &str) {
    click_modulator_control(visual, format!("add-modulator-{}", device.value()), true);
    let panel = visual
        .debug_bounds("context-menu-panel")
        .expect("modulator menu");
    if kind == "lfo" {
        // Exercise the pointer action on the first row. Keyboard navigation
        // selects the other audio-effect types in the shared mapping tests.
        let position = point(panel.center().x, panel.origin.y + px(10.0));
        visual.simulate_click(position, Modifiers::default());
    } else {
        let steps = [
            "lfo",
            "follower",
            "macro",
            "macro-4",
            "random",
            "audio rate",
            "button",
            "math",
            "buttons",
            "mix",
            "steps",
        ]
        .iter()
        .position(|label| *label == kind)
        .expect("modulator type in audio effect menu");
        for _ in 0..steps {
            visual.simulate_keystrokes("down");
        }
        visual.simulate_keystrokes("enter");
    }
    visual.run_until_parked();
    assert!(visual.debug_bounds("context-menu-panel").is_none());
}

fn modulator_control_position(
    visual: &mut VisualTestContext,
    key: String,
    in_strip: bool,
) -> gpui::Point<gpui::Pixels> {
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
    visual.debug_bounds(key).unwrap().center()
}

fn click_modulator_control(visual: &mut VisualTestContext, key: String, in_strip: bool) {
    let position = modulator_control_position(visual, key, in_strip);
    visual.simulate_click(position, Modifiers::default());
    visual.run_until_parked();
}

fn assert_modulation_mapping(
    cx: &mut TestAppContext,
    device_type: hex_lattice_contract::DeviceTypeId,
    source_kind: &str,
    type_id: hex_lattice_contract::DeviceTypeId,
    targets: &[hex_lattice_contract::ParameterId],
) {
    let (service, state, track, device) = actor_native_effect_fixture(device_type);
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
        let target = visual
            .debug_bounds(key)
            .unwrap_or_else(|| panic!("missing {key}"));
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
        let target = visual
            .debug_bounds(key)
            .unwrap_or_else(|| panic!("missing {key}"));
        let strip = visual.debug_bounds("device-modulator-scroll").unwrap();
        assert!(
            strip.contains(&target.center()),
            "{key} at {target:?} outside {strip:?}"
        );
    };
    click(&mut visual, "device-modulator-toggle".into());
    add_modulator_from_menu(&mut visual, device, source_kind);
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
        click_modulator_control(
            &mut visual,
            format!("macro-{}-more", source_id.value()),
            true,
        );
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
    click_modulator_control(
        &mut visual,
        format!("{source_kind}-{}-map", source_id.value()),
        true,
    );
    assert_eq!(
        editor.read_with(cx, |view, _| view.device_panel.mapping_source),
        Some((device, super::modulators::MappingOutput::main(source_id)))
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
        if device_type == hex_engine::eq::EQ_TYPE_ID
            || matches!(
                type_id,
                hex_engine::macro_modulator::TYPE_ID
                    | hex_engine::random_modulator::TYPE_ID
                    | hex_engine::audio_rate_modulator::TYPE_ID
            )
        {
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
    if type_id == hex_engine::button_modulator::TYPE_ID {
        let on = |view: &EditorView| {
            let project = &view.state.as_ref().unwrap().project;
            let slot = &project.device(device).unwrap().modulation().sources()[0];
            assert_eq!(slot.id, source_id);
            hex_engine::button_modulator::decode(&slot.state).unwrap()
        };
        assert!(!editor.read_with(cx, |view, _| on(view)));
        click_modulator_control(
            &mut visual,
            format!("button-{}-value", source_id.value()),
            true,
        );
        pump_until(&editor, cx, on);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            assert_eq!(
                hex_storage::schema_version_for(project),
                hex_storage::BUTTON_MODULATOR_SCHEMA_VERSION
            );
            assert_eq!(
                hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap(),
                **project
            );
            let route = project.device(device).unwrap().modulation().routes()[0];
            assert_eq!(
                [route.range.start.get(), route.range.end.get()],
                [-1.0, 1.0]
            );
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| !on(view));
        visual.run_until_parked();
    }
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("map-target-{}-{}", device.value(), targets[0].value()).into_boxed_str()
            ))
            .is_none()
    );
    if type_id == hex_engine::audio_rate_modulator::TYPE_ID {
        use hex_project::ModulationAmount;
        let route = |view: &EditorView| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .modulation()
                .routes()[0]
        };
        let mut adjusted = editor.read_with(cx, |view, _| route(view));
        let key = format!("modulation-route-{}-less", adjusted.id.value());
        for _ in 0..3 {
            click_modulator_control(&mut visual, key.clone(), true);
            adjusted.amount = ModulationAmount::new(adjusted.amount.get() - 0.05).unwrap();
            pump_until(&editor, cx, |view| route(view) == adjusted);
            visual.run_until_parked();
        }
        assert!(adjusted.amount.get() < 0.0);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| route(view).amount.get() == 0.0);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| route(view) == adjusted);
        editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let loaded = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
            assert_eq!(
                loaded.device(device).unwrap().modulation().routes(),
                [adjusted]
            );
            assert_eq!(loaded.device(device).unwrap().state(), &original);
        });
    }
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
        // The limiter's lookahead excludes its host trims. Zero-latency EQ
        // supports both host trims in addition to the sparse EQ output gain.
        if device_type == hex_devices::limiter::TYPE_ID {
            for name in ["input-gain", "output-gain"] {
                let selector = Box::leak(format!("map-{name}-{}", device.value()).into_boxed_str());
                assert!(visual.debug_bounds(selector).is_none());
            }
            return;
        }
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
            view.device_panel
                .active_native_drag
                .as_ref()
                .is_some_and(|drag| { drag.destination.get().is_none() })
        );
    });
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
        view.audio_pub.audible_playback = Some(super::super::PlaybackCandidate {
            clip_preview_identity: None,
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

#[gpui::test]
fn modulation_panel_adds_and_maps_an_audio_rate_modulator(cx: &mut TestAppContext) {
    assert_modulation_mapping(
        cx,
        hex_devices::utility::UTILITY_TYPE_ID,
        "audio rate",
        hex_engine::audio_rate_modulator::TYPE_ID,
        &[hex_devices::utility::parameter::GAIN],
    );
}

#[path = "tests/bypass_and_removal_tests.rs"]
mod bypass_and_removal_tests;
#[path = "tests/modulation_and_routing_tests.rs"]
mod modulation_and_routing_tests;
#[path = "tests/parameter_and_preset_tests.rs"]
mod parameter_and_preset_tests;
#[path = "tests/selection_and_reorder_tests.rs"]
mod selection_and_reorder_tests;

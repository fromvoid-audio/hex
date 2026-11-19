use super::*;
use hex_project::DeviceVariant;

fn compressor_mix(state: &hex_lattice_contract::NativeDeviceState) -> f32 {
    let view = hex_engine::native_device_view_for_class(NativeDeviceClass::Effect, state).unwrap();
    match view
        .parameters
        .iter()
        .find(|p| p.descriptor.id.value() == 7)
        .unwrap()
        .value
    {
        ParameterValue::Float(value) => value,
        _ => panic!("compressor mix float"),
    }
}

fn compressor_pair(
    editor: &Entity<EditorView>,
    cx: &TestAppContext,
    device: DeviceId,
    active: DeviceVariant,
    current: f32,
    inactive: f32,
) {
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let entry = view.state.as_ref().unwrap().project.device(device).unwrap();
        assert_eq!(entry.active_variant(), active);
        assert_eq!(compressor_mix(entry.state()), current);
        let other = entry.comparison().unwrap().inactive();
        assert_eq!(compressor_mix(other), inactive);
        for state in [entry.state(), other] {
            assert_eq!(state.input_gain_millidb(), -1234);
            assert_eq!(state.output_gain_millidb(), 4321);
        }
    });
}

fn enter_compressor_mix(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    cx: &mut TestAppContext,
    device: DeviceId,
    value: &str,
) {
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-7-value", device.value()).into_boxed_str(),
        ))
        .expect("actual dry/wet numeric field");
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input(value);
    visual.simulate_keystrokes("enter");
    pump_until(editor, cx, settled);
    visual.run_until_parked();
}

fn compressor_banks_roundtrip(cx: &mut TestAppContext, on_return: bool) {
    let directory = Directory::new();
    let (service, state, track, bus, device) = catalog_fixture(
        NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.compressor"),
        on_return,
    );
    // Global host trims belong to the device, not either provider payload bank.
    let trimmed = state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone()
        .with_input_gain_millidb(-1234)
        .unwrap()
        .with_output_gain_millidb(4321)
        .unwrap();
    service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: trimmed,
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let (handle, editor) = directory.window(cx, service, state, track, bus);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(800.0)));
    visual.run_until_parked();
    enter_compressor_mix(&mut visual, &editor, cx, device, "25%");
    title(&mut visual, device);
    menu(&mut visual, device);
    labels(&editor, cx, &["switch to b", "copy a to b"]);
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    compressor_pair(&editor, cx, device, DeviceVariant::B, 0.25, 0.25);
    enter_compressor_mix(&mut visual, &editor, cx, device, "75%");
    compressor_pair(&editor, cx, device, DeviceVariant::B, 0.75, 0.25);

    // Persist the *distinct* pair before copies erase the discriminator.
    let saved = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let project_path = directory.0.join("distinct-banks.hex");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SaveNew(project_path.clone())))
    });
    pump_until(&editor, cx, settled);
    let reopened = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&reopened);
    reopened.submit(Request::Open(project_path)).unwrap();
    let loaded = receive_service_event(&reopened);
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    assert_eq!(loaded.state.project, saved);
    let preset_path = directory.0.join("distinct-banks.hex-device-preset.json");
    let owner = |view: &EditorView| hex_session::service::preset::Owner {
        project_generation: view.project_generation,
        revision: view.state.as_ref().unwrap().revision,
        device,
        class: NativeDeviceClass::Effect,
        type_id: view
            .state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state()
            .type_id(),
    };
    editor.update(cx, |view, _| {
        let owner = owner(view);
        assert!(view.enqueue(Request::SaveDevicePreset {
            owner,
            path: preset_path.clone()
        }));
    });
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error)
    });
    let preset_bytes = std::fs::read(&preset_path).unwrap();
    enter_compressor_mix(&mut visual, &editor, cx, device, "50%");
    compressor_pair(&editor, cx, device, DeviceVariant::B, 0.5, 0.25);
    let before_load = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    editor.update(cx, |view, _| {
        let owner = owner(view);
        assert!(view.enqueue(Request::LoadDevicePreset {
            owner,
            path: preset_path.clone()
        }));
    });
    pump_until(&editor, cx, settled);
    compressor_pair(&editor, cx, device, DeviceVariant::B, 0.75, 0.25);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before_load)
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, settled);
    compressor_pair(&editor, cx, device, DeviceVariant::B, 0.75, 0.25);
    assert_eq!(std::fs::read(&preset_path).unwrap(), preset_bytes);
    visual.run_until_parked();

    // Both directional menu copies retain the active slot and are exactly one edit.
    for (active, current, inactive, next) in [
        (DeviceVariant::B, 0.75, 0.25, DeviceVariant::A),
        (DeviceVariant::A, 0.25, 0.75, DeviceVariant::B),
    ] {
        title(&mut visual, device);
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        menu(&mut visual, device);
        labels(
            &editor,
            cx,
            if active == DeviceVariant::A {
                &["switch to b", "copy a to b"]
            } else {
                &["switch to a", "copy b to a"]
            },
        );
        visual.simulate_keystrokes("down enter");
        pump_until(&editor, cx, settled);
        compressor_pair(&editor, cx, device, active, current, current);
        let copied = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, before)
        });
        compressor_pair(&editor, cx, device, active, current, inactive);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, copied)
        });
        compressor_pair(&editor, cx, device, active, current, current);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, settled);
        visual.run_until_parked();
        title(&mut visual, device);
        visual.simulate_keystrokes("p");
        pump_until(&editor, cx, settled);
        visual.run_until_parked();
        compressor_pair(&editor, cx, device, next, inactive, current);
    }
    eprintln!(
        "compressor generic A/B: return={on_return}; actual ID7 25/75, both copy directions, project/preset reopen, load+copy one Undo/Redo, global trims preserved"
    );
}

#[gpui::test]
fn compressor_generic_ab_track_mix_banks_copy_and_persistence(cx: &mut TestAppContext) {
    compressor_banks_roundtrip(cx, false);
}

#[gpui::test]
fn compressor_generic_ab_return_mix_banks_copy_and_persistence(cx: &mut TestAppContext) {
    compressor_banks_roundtrip(cx, true);
}

pub(super) struct Directory(pub(super) std::path::PathBuf);
impl Directory {
    pub(super) fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-device-ab-ui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("samples")).unwrap();
        Self(path)
    }
    pub(super) fn window(
        &self,
        cx: &mut TestAppContext,
        service: Service,
        state: Arc<ViewState>,
        track: TrackId,
        bus: Option<hex_project::ReturnId>,
    ) -> (gpui::AnyWindowHandle, Entity<EditorView>) {
        cx.update(crate::editor::bind_editor_keys);
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_browser_directory(window, cx, service, &self.0)
                });
                editor.update(cx, |view, cx| {
                    assert_eq!(view.browser.directory, self.0.join("samples"));
                    view.state = Some(state);
                    view.selected_track = Some(track);
                    view.focused_return = bus;
                    view.device_panel_active = true;
                    view.clip_view_open = true;
                    view.mode = crate::editor::EditorMode::Session;
                    cx.notify();
                });
                editor
            })
            .unwrap();
        (handle, editor)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn settled(view: &EditorView) -> bool {
    view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && !view.device_gesture_pending()
}
fn point_for(
    visual: &mut VisualTestContext,
    prefix: &str,
    device: DeviceId,
) -> gpui::Point<gpui::Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("{prefix}-{}", device.value()).into_boxed_str(),
        ))
        .expect("visible shared device control")
        .center()
}
fn title(visual: &mut VisualTestContext, device: DeviceId) {
    let point = point_for(visual, "device-drag-title", device);
    visual.simulate_click(point, Modifiers::default());
    visual.run_until_parked();
}
fn menu(visual: &mut VisualTestContext, device: DeviceId) {
    let point = point_for(visual, "device-comparison", device);
    visual.simulate_mouse_down(point, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
}
fn labels(editor: &Entity<EditorView>, cx: &TestAppContext, expected: &[&str]) {
    editor.read_with(cx, |view, _| {
        let menu = view.context_menu.as_ref().expect("comparison menu");
        assert_eq!(
            menu.items.iter().map(|item| item.label).collect::<Vec<_>>(),
            expected
        );
    });
}
fn slot(editor: &Entity<EditorView>, cx: &TestAppContext, device: DeviceId) -> DeviceVariant {
    editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .active_variant()
    })
}

#[gpui::test]
fn device_comparison_click_title_p_menu_copy_and_real_undo_are_one_shared_route(
    cx: &mut TestAppContext,
) {
    let directory = Directory::new();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let revision = state.revision;
    let (handle, editor) = directory.window(cx, service, state, track, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    title(&mut visual, device);
    menu(&mut visual, device);
    labels(&editor, cx, &["switch to b", "copy a to b"]);
    visual.simulate_keystrokes("down enter");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().revision,
            revision,
            "identical copy must not publish history"
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    // Focus comes from the real title click/menu restoration, never a fixture-set card focus.
    visual.simulate_keystrokes("p");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    assert_eq!(slot(&editor, cx, device), DeviceVariant::B);
    let changed = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    menu(&mut visual, device);
    labels(&editor, cx, &["switch to a", "copy b to a"]);
    visual.simulate_keystrokes("escape");
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, settled);
    assert_eq!(slot(&editor, cx, device), DeviceVariant::A);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, changed)
    });
}

#[gpui::test]
fn device_comparison_p_respects_browser_ime_musical_and_modal_focus(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = directory.window(cx, service, state, track, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    title(&mut visual, device);
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| view.browser.search.read(cx).focus(window))
    });
    visual.simulate_input("p 🎹");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "p 🎹");
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    visual.update(|window, cx| {
        use gpui::EntityInputHandler;
        let input = editor.read(cx).browser.search.clone();
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "候", Some(0..1), window, cx);
            assert!(input.marked_text_range(window, cx).is_some());
        });
    });
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("p").unwrap(),
        is_held: false,
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
    visual.update(|window, cx| editor.read(cx).focus.focus(window));
    visual.simulate_keystrokes("m");
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("p").unwrap(),
        is_held: false,
    });
    editor.read_with(cx, |view, _| {
        assert!(
            view.computer_keyboard.held().iter().any(Option::is_some),
            "musical p must still sound outside device focus"
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("p").unwrap(),
    });
    title(&mut visual, device);
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("p").unwrap(),
        is_held: true,
    });
    assert_eq!(slot(&editor, cx, device), DeviceVariant::A);
    visual.update(|window, cx| editor.update(cx, |view, cx| view.open_theme_settings(window, cx)));
    visual.simulate_keystrokes("p");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
}

fn catalog_fixture(
    class: NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
    on_return: bool,
) -> (
    Service,
    Arc<ViewState>,
    TrackId,
    Option<hex_project::ReturnId>,
    DeviceId,
) {
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "comparison owner".into(),
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let track = state.project.tracks()[0].id();
    let native = hex_engine::default_native_device_state_for_class(class, type_id).unwrap();
    let bus = if on_return {
        service
            .submit(Request::Edit(Command::AddReturnTrack {
                name: "comparison return".into(),
            }))
            .unwrap();
        let bus = receive_service_event(&service)
            .state
            .project
            .return_tracks()[0]
            .id();
        service
            .submit(Request::Edit(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: native,
            }))
            .unwrap();
        Some(bus)
    } else {
        let command = if class == NativeDeviceClass::Instrument {
            match state.project.track(track).unwrap().instrument() {
                Some(device) => Command::ReplaceDeviceState {
                    device: device.id(),
                    state: native,
                },
                None => Command::InsertInstrument {
                    track,
                    state: native,
                },
            }
        } else {
            Command::InsertDevice {
                track,
                before: None,
                state: native,
            }
        };
        service.submit(Request::Edit(command)).unwrap();
        None
    };
    let event = receive_service_event(&service);
    assert!(event.error.is_none(), "{:?}", event.error);
    let state = event.state;
    let device = if let Some(bus) = bus {
        state
            .project
            .return_tracks()
            .iter()
            .find(|entry| entry.id() == bus)
            .unwrap()
            .devices()[0]
            .id()
    } else if class == NativeDeviceClass::Instrument {
        state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id()
    } else {
        state.project.track(track).unwrap().devices()[0].id()
    };
    (service, state, track, bus, device)
}

#[gpui::test]
fn device_comparison_every_available_catalog_header_and_return_uses_actual_p(
    cx: &mut TestAppContext,
) {
    let mut expected = 0;
    let mut observed = 0;
    for class in [NativeDeviceClass::Instrument, NativeDeviceClass::Effect] {
        let catalog = hex_engine::native_device_catalog_for_class(class).unwrap();
        assert!(!catalog.is_empty());
        expected += catalog.len()
            * if class == NativeDeviceClass::Effect {
                2
            } else {
                1
            };
        for entry in catalog {
            for on_return in if class == NativeDeviceClass::Effect {
                &[false, true][..]
            } else {
                &[false][..]
            } {
                let directory = Directory::new();
                let (service, state, track, bus, device) =
                    catalog_fixture(class, entry.type_id, *on_return);
                let original = state.project.clone();
                let (handle, editor) = directory.window(cx, service, state, track, bus);
                let mut visual = VisualTestContext::from_window(handle, cx);
                visual.simulate_resize(size(px(1600.0), px(800.0)));
                visual.run_until_parked();
                point_for(&mut visual, "device-comparison", device);
                title(&mut visual, device);
                visual.simulate_keystrokes("p");
                pump_until(&editor, cx, settled);
                assert_eq!(
                    slot(&editor, cx, device),
                    DeviceVariant::B,
                    "provider {:?}, return={on_return}",
                    entry.type_id
                );
                visual.run_until_parked();
                menu(&mut visual, device);
                labels(&editor, cx, &["switch to a", "copy b to a"]);
                visual.simulate_keystrokes("escape cmd-z");
                pump_until(&editor, cx, settled);
                editor.read_with(cx, |view, _| {
                    assert_eq!(view.state.as_ref().unwrap().project, original)
                });
                observed += 1;
                visual.update(|window, _| window.remove_window());
            }
        }
    }
    assert_eq!(observed, expected);
    eprintln!("actual rendered A/B catalog role cases: {observed}");
}

fn balance(editor: &Entity<EditorView>, cx: &TestAppContext, device: DeviceId) -> f64 {
    editor.read_with(cx, |view, _| {
        let device = view.state.as_ref().unwrap().project.device(device).unwrap();
        let metadata =
            hex_engine::native_device_view_for_class(NativeDeviceClass::Effect, device.state())
                .unwrap();
        let value = metadata
            .parameters
            .iter()
            .find(|p| p.descriptor.id == hex_lattice_contract::ParameterId::from_static(2))
            .unwrap()
            .value;
        let ParameterValue::Float(value) = value else {
            panic!("balance float")
        };
        f64::from(value)
    })
}
fn enter_balance(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    cx: &mut TestAppContext,
    device: DeviceId,
    value: &str,
) {
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-2-value", device.value()).into_boxed_str(),
        ))
        .expect("balance value");
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input(value);
    visual.simulate_keystrokes("enter");
    pump_until(editor, cx, settled);
    visual.run_until_parked();
}
#[gpui::test]
fn device_comparison_real_numeric_edits_keep_independent_slots_and_copy_is_one_undo(
    cx: &mut TestAppContext,
) {
    let directory = Directory::new();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = directory.window(cx, service, state, track, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    title(&mut visual, device);
    for _ in 0..2 {
        visual.simulate_keystrokes("p");
        pump_until(&editor, cx, settled);
        visual.run_until_parked();
    }
    enter_balance(&mut visual, &editor, cx, device, "25 r");
    assert_eq!(balance(&editor, cx, device), 0.25);
    title(&mut visual, device);
    visual.simulate_keystrokes("p");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    assert_eq!(balance(&editor, cx, device), 0.0);
    enter_balance(&mut visual, &editor, cx, device, "50 l");
    assert_eq!(balance(&editor, cx, device), -0.5);
    title(&mut visual, device);
    visual.simulate_keystrokes("p");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    assert_eq!(balance(&editor, cx, device), 0.25);
    let before_copy = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    menu(&mut visual, device);
    labels(&editor, cx, &["switch to b", "copy a to b"]);
    visual.simulate_keystrokes("down enter");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    assert_eq!(slot(&editor, cx, device), DeviceVariant::A);
    assert_eq!(balance(&editor, cx, device), 0.25);
    let copied = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .comparison()
            .unwrap()
            .inactive()
            .clone()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            &copied,
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state()
        )
    });
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before_copy)
    });
}

#[gpui::test]
fn device_comparison_pending_press_does_not_apply_after_worker_completion(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = directory.window(cx, service, state, track, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    title(&mut visual, device);
    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::RenameTrack {
                track,
                name: "pending comparison".into()
            })))
        });
        window.dispatch_input(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse("p").unwrap(),
                is_held: false,
            }),
            cx,
        );
    });
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    assert_eq!(slot(&editor, cx, device), DeviceVariant::A);
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("p").unwrap(),
        is_held: true,
    });
    assert_eq!(slot(&editor, cx, device), DeviceVariant::A);
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("p").unwrap(),
    });
    visual.simulate_keystrokes("p");
    pump_until(&editor, cx, settled);
    assert_eq!(slot(&editor, cx, device), DeviceVariant::B);
}

#[gpui::test]
fn device_comparison_stale_menu_removal_preserves_real_keyboard_undo(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = directory.window(cx, service, state, track, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    title(&mut visual, device);
    menu(&mut visual, device);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveDevice(device))))
    });
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));
    visual.update(|window, cx| editor.read_with(cx, |view, _| {
        eprintln!("stale menu trace before undo: root_focus={} any_focus={} can_undo={} worker={} error={:?}", view.focus.is_focused(window), window.focused(cx).is_some(), view.state.as_ref().unwrap().can_undo, view.worker_requests_pending, view.error);
    }));
    visual.simulate_keystrokes("cmd-z");
    visual.update(|window, cx| editor.read_with(cx, |view, _| {
        eprintln!("stale menu trace after key: root_focus={} any_focus={} worker={} queued={} error={:?}", view.focus.is_focused(window), window.focused(cx).is_some(), view.worker_requests_pending, view.pending_requests.len(), view.error);
    }));
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project,
            original,
            "stale A/B menu must leave actual keyboard Undo reachable"
        )
    });
}

#[gpui::test]
fn device_comparison_numeric_draft_and_eq_graph_own_p(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = directory.window(cx, service, state, track, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-2-value", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid p");
    visual.simulate_keystrokes("p");
    editor.read_with(cx, |view, cx| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(2),
        }] else {
            panic!("numeric")
        };
        assert!(entity.read(cx).is_editing());
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    visual.simulate_keystrokes("escape");
    let directory_eq = Directory::new();
    let (service, state, track, bus, device) =
        catalog_fixture(NativeDeviceClass::Effect, hex_engine::eq::EQ_TYPE_ID, false);
    let (handle, editor) = directory_eq.window(cx, service, state, track, bus);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    title(&mut visual, device);
    let graph = visual
        .debug_bounds(Box::leak(
            format!("eq-response-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    eq_ui_tests::eq_double_click(&mut visual, graph.center());
    pump_until(&editor, cx, settled);
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    visual.simulate_keystrokes("p");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before)
    });
}

#[gpui::test]
fn device_comparison_title_click_and_p_in_one_dispatch_frame_targets_clicked_card(
    cx: &mut TestAppContext,
) {
    let directory = Directory::new();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, editor) = directory.window(cx, service, state, track, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let point = point_for(&mut visual, "device-drag-title", device);
    visual.update(|window, cx| {
        window.dispatch_input(
            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                position: point,
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_input(
            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                position: point,
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );
        window.dispatch_input(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse("p").unwrap(),
                is_held: false,
            }),
            cx,
        );
        window.dispatch_input(
            gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse("p").unwrap(),
            }),
            cx,
        );
    });
    pump_until(&editor, cx, settled);
    assert_eq!(slot(&editor, cx, device), DeviceVariant::B);
    assert_eq!(slot(&editor, cx, instrument), DeviceVariant::A);
}

#[gpui::test]
fn device_comparison_right_click_nonselected_badge_switches_only_its_target(
    cx: &mut TestAppContext,
) {
    let directory = Directory::new();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, editor) = directory.window(cx, service, state, track, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    title(&mut visual, instrument);
    menu(&mut visual, device);
    labels(&editor, cx, &["switch to b", "copy a to b"]);
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, settled);
    assert_eq!(slot(&editor, cx, device), DeviceVariant::B);
    assert_eq!(slot(&editor, cx, instrument), DeviceVariant::A);
}

#[gpui::test]
fn device_comparison_review_dismiss_nonselected_menu_keeps_p_on_target(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, editor) = directory.window(cx, service, state, track, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    title(&mut visual, instrument);
    menu(&mut visual, device);
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.simulate_keystrokes("p");
    pump_until(&editor, cx, settled);
    assert_eq!(
        slot(&editor, cx, device),
        DeviceVariant::B,
        "dismiss must not restore a different card's P owner"
    );
    assert_eq!(slot(&editor, cx, instrument), DeviceVariant::A);
}

#[gpui::test]
fn device_comparison_review_enqueue_cancels_published_seek_before_callback(
    cx: &mut TestAppContext,
) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    use crate::editor::arrangement_seek::{PendingSeek, SeekIntent, SeekStage};
    use hex_audio::prepared::PublicationPolicy;
    use std::sync::atomic::Ordering::Acquire;
    let directory = Directory::new();
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &state.project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let until = crate::test_wait::hang_deadline();
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && std::time::Instant::now() < until =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("private headless control: {error}"),
        }
    };
    let client = output.take_client().unwrap();
    let playback = || {
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&prepared))
            .unwrap()
    };
    let active = output
        .publication
        .control()
        .unwrap()
        .publish(playback())
        .unwrap();
    let mut block = [[0.0; 2]; 128];
    backend.render(&mut block).unwrap();
    let fence = client.capture_fresh_runtime_fence().unwrap();
    let published = output
        .publication
        .control()
        .unwrap()
        .publish_with_policy(
            playback(),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let mut pending = PendingSeek::from_intent(
        91,
        &SeekIntent {
            kind: crate::editor::arrangement_seek::SeekKind::Fresh,
            recovery_generation: None,
            destination_frame: 128,
            start_playing: true,
            project_generation: 0,
            project_revision: state.revision,
            prepared_revision: Some(prepared),
            device_preview_identity: None,
        },
        fence,
    );
    pending.stage = SeekStage::Published {
        audio_revision: published,
    };
    let target = hex_session::service::preset::Owner {
        project_generation: 0,
        revision: state.revision,
        device,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: state.project.device(device).unwrap().state().type_id(),
    };
    let (_handle, editor) = directory.window(cx, service, state, track, None);
    editor.update(cx, |view, _| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "private control".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.prepared_seek = Some(pending);
        view.enqueue(Request::CompareDevice {
            owner: target,
            expected: DeviceVariant::A,
            action: hex_project::DeviceComparisonAction::Switch,
        });
        assert!(
            view.edit_pending(),
            "Preparing publication is already guarded by pending edits"
        );
    });
    // Run the actual callback before any worker event/render reconciliation.
    backend.render(&mut block).unwrap();
    assert_eq!(
        backend.status().unwrap().applied_revision.load(Acquire),
        active,
        "a queued comparison must cancel the already-published old seek"
    );
    assert_eq!(
        backend.status().unwrap().rejected_revision(),
        Some((
            published,
            hex_engine::playback::PlaybackHandoffRejection::FreshRuntimeCancelled
        ))
    );
    editor.update(cx, |view, _| {
        assert!(view.prepared_seek.as_ref().unwrap().cancelled);
        view.audio = None;
    });
    backend.close();
    drop(output);
}

#[test]
fn device_comparison_review_metadata_keeps_actual_callback_graph_and_worker_arc() {
    use hex_audio::prepared::PublicationPolicy;
    use hex_engine::playback::{Playback, PlaybackCommand};
    use hex_project::{BeatTime, DeviceComparisonAction as Action};
    use std::sync::atomic::Ordering::Acquire;
    let service = Service::new(48_000, 32, 0).unwrap();
    receive_service_event(&service);
    let send = |request| {
        service.submit(request).unwrap();
        receive_service_event(&service)
    };
    let e = send(Request::Edit(Command::InsertTrackWithDevice {
        name: "held".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    }));
    let track = e.state.project.tracks()[0].id();
    let e = send(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(16).unwrap(),
    }));
    let Created::Clip(clip) = e.created.unwrap() else {
        panic!("clip")
    };
    send(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(16).unwrap(),
        pitch: 60,
        channel: 0,
        velocity: 100,
    }));
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let mut initial = send(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state,
    }));
    let device = initial.state.project.track(track).unwrap().devices()[0].id();
    let original = initial
        .playback
        .as_ref()
        .unwrap()
        .prepared_revision()
        .unwrap()
        .clone();
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&original)).unwrap();
    oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = hex_audio::prepared::channel(48_000).unwrap();
    let initial_audio = control.publish(initial.playback.take().unwrap()).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|x| x.abs() > 1e-5));
    let owner = |event: &Event| hex_session::service::preset::Owner {
        project_generation: 0,
        revision: event.state.revision,
        device,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: event
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .type_id(),
    };
    let mut switched = send(Request::CompareDevice {
        owner: owner(&initial),
        expected: DeviceVariant::A,
        action: Action::Switch,
    });
    assert!(switched.error.is_none());
    if let Some(playback) = switched.playback.take() {
        control
            .publish_with_policy(playback, PublicationPolicy::InheritRuntime)
            .unwrap();
    }
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        initial_audio,
        "identical first switch must not publish a replacement graph"
    );
    assert_eq!(switched.state.revision, initial.state.revision + 1);
    assert_eq!(
        switched
            .state
            .project
            .device(device)
            .unwrap()
            .active_variant(),
        DeviceVariant::B
    );
    let restored = send(Request::RestorePlayback { generation: 91 });
    assert!(Arc::ptr_eq(
        restored
            .playback
            .as_ref()
            .unwrap()
            .prepared_revision()
            .unwrap(),
        &original
    ));
    let mut changed = send(Request::SetDeviceParameter {
        device,
        parameter: hex_lattice_contract::ParameterId::from_static(1),
        value: hex_lattice_contract::ParameterValue::Float(0.25),
    });
    assert!(changed.error.is_none());
    let changed_arc = changed
        .playback
        .as_ref()
        .unwrap()
        .prepared_revision()
        .unwrap()
        .clone();
    assert!(
        !Arc::ptr_eq(&changed_arc, &original),
        "audible edits still prepare a graph"
    );
    let applied = control
        .publish_with_policy(
            changed.playback.take().unwrap(),
            PublicationPolicy::InheritRuntime,
        )
        .unwrap();
    // The changed static matrix ramps over `rate.div_ceil(200)` frames
    // (240 at 48 kHz, Pulse #4101), so the first block is still in transit.
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert!(
        actual
            .iter()
            .flatten()
            .zip(expected.iter().flatten())
            .any(|(a, e)| (a - e * 0.25).abs() > 1e-6),
        "the gain edit ramps instead of stepping"
    );
    for _ in 0..2 {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
    }
    for (a, e) in actual.iter().flatten().zip(expected.iter().flatten()) {
        assert!((a - e * 0.25).abs() < 1e-6);
    }
    let before_copy = changed.state.project.clone();
    let copied = send(Request::CompareDevice {
        owner: owner(&changed),
        expected: DeviceVariant::B,
        action: Action::CopyToInactive,
    });
    assert!(copied.error.is_none());
    assert!(
        copied.playback.is_none(),
        "copy changes authored metadata only"
    );
    assert_eq!(copied.state.revision, changed.state.revision + 1);
    assert_ne!(copied.state.project, before_copy);
    let restored = send(Request::RestorePlayback { generation: 92 });
    assert!(Arc::ptr_eq(
        restored
            .playback
            .as_ref()
            .unwrap()
            .prepared_revision()
            .unwrap(),
        &changed_arc
    ));
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(control.status.applied_revision.load(Acquire), applied);
    // Two extra 128-frame blocks let the gain edit's matrix ramp settle.
    assert_eq!(control.status.position.load(Acquire), 768);
    for (a, e) in actual.iter().flatten().zip(expected.iter().flatten()) {
        assert!((a - e * 0.25).abs() < 1e-6);
    }
    let noop = send(Request::CompareDevice {
        owner: owner(&copied),
        expected: DeviceVariant::B,
        action: Action::CopyToInactive,
    });
    assert_eq!(noop.state.revision, copied.state.revision);
    assert!(noop.playback.is_none());
    let undone = send(Request::Undo);
    assert_eq!(undone.state.project, before_copy);
    let redone = send(Request::Redo);
    assert_eq!(redone.state.project, copied.state.project);
}

#[gpui::test]
fn device_comparison_metadata_pending_callback_then_applied_authoring_is_not_a_new_ack(
    cx: &mut TestAppContext,
) {
    comparison_callback_fixture(cx, false);
}

#[gpui::test]
fn device_comparison_metadata_recovery_error_preserves_real_pending_and_applied_correlation(
    cx: &mut TestAppContext,
) {
    comparison_callback_fixture(cx, true);
}

fn comparison_callback_fixture(cx: &mut TestAppContext, recovery_error: bool) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    use crate::editor::audio_admission::{AdmissionState, ProjectRevision};
    use std::sync::atomic::Ordering::Acquire;
    let directory = Directory::new();
    let service = if recovery_error {
        // A real worker-owned checkpoint failure, after a successful model edit.
        // No fabricated Event.error, proof, callback ACK or preparation outcome.
        let parent = directory.0.join("not-a-recovery-directory");
        std::fs::write(&parent, b"owned fixture obstruction").unwrap();
        Service::with_recovery(48_000, 32, 0, parent.join("untitled.hexjournal")).unwrap()
    } else {
        Service::new(48_000, 32, 0).unwrap()
    };
    receive_service_event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "held".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        }))
        .unwrap();
    receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddScene {
            name: "launch".into(),
        }))
        .unwrap();
    let mut initial = receive_service_event(&service);
    let scene = initial.state.project.scenes()[0].id();
    let state = Arc::clone(&initial.state);
    let track = state.project.tracks()[0].id();
    let device = state.project.tracks()[0].instrument().unwrap().id();
    let keyboard = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    keyboard.set_key(0, Some((60, 100, 1))).unwrap();
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: state.revision,
            request_id: 1,
            track,
            input: keyboard.clone(),
        })
        .unwrap();
    initial = receive_service_event(&service);
    assert!(initial.error.is_none());
    let source = state.canonical_audio_source.as_ref().unwrap().clone();
    let prepared = initial
        .playback
        .as_ref()
        .unwrap()
        .prepared_revision()
        .unwrap()
        .clone();
    let deadline = crate::test_wait::hang_deadline();
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("private control: {error}"),
        }
    };
    let client = output.take_client().unwrap();
    let audio_revision = output
        .publication
        .control()
        .unwrap()
        .publish(initial.playback.take().unwrap())
        .unwrap();
    let (_handle, editor) = directory.window(cx, service, state.clone(), track, None);
    editor.update(cx, |view, _| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "private".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.computer_keyboard.enabled = true;
        view.computer_keyboard.input = Some(keyboard.clone());
        view.computer_keyboard.target = Some((0, state.revision, track));
        view.computer_keyboard.request_id = 1;
        assert!(view.computer_keyboard.key_down("a", false));
        view.audio_admission.canonical_revision(ProjectRevision {
            generation: 0,
            revision: state.revision,
        });
        view.audio_admission.published(
            ProjectRevision {
                generation: 0,
                revision: state.revision,
            },
            audio_revision,
        );
        view.playback_candidates
            .push_back(crate::editor::PlaybackCandidate {
                clip_preview_identity: None,
                worker_requires_preroll: None,
                observation: None,
                audio_revision,
                project_generation: 0,
                state: state.clone(),
                prepared_revision: Some(prepared.clone()),
                seek_request_id: None,
                device_preview_identity: None,
                tuning_preview_identity: None,
            });
        if recovery_error {
            view.service
                .submit(Request::CompareDevice {
                    owner: hex_session::service::preset::Owner {
                        project_generation: 0,
                        revision: state.revision,
                        device,
                        class: hex_engine::NativeDeviceClass::Instrument,
                        type_id: state.project.device(device).unwrap().state().type_id(),
                    },
                    expected: DeviceVariant::B, // Actual A: refusal, not a commit.
                    action: hex_project::DeviceComparisonAction::Switch,
                })
                .unwrap();
            let refused = receive_service_event(&view.service);
            assert!(refused.error.is_some());
            assert!(refused.state.comparison_audio.is_none());
            assert_eq!(refused.state.revision, state.revision);
            assert!(refused.playback.is_none());
            view.receive(refused);
            assert_eq!(view.playback_candidates[0].state.revision, state.revision);
            assert_eq!(
                view.audio_admission.current().unwrap().state,
                AdmissionState::Pending
            );
            assert_eq!(view.applied_project_revision(), None);
        }
        view.service
            .submit(Request::CompareDevice {
                owner: hex_session::service::preset::Owner {
                    project_generation: 0,
                    revision: state.revision,
                    device,
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: state.project.device(device).unwrap().state().type_id(),
                },
                expected: DeviceVariant::A,
                action: hex_project::DeviceComparisonAction::Switch,
            })
            .unwrap();
        let event = receive_service_event(&view.service);
        assert!(event.playback.is_none());
        if recovery_error {
            assert_eq!(
                event.error.as_deref(),
                Some("automatic recovery: recovery directory must be a real directory")
            );
            assert_eq!(event.state.revision, state.revision + 1);
            assert!(event.state.comparison_audio.as_ref().unwrap().follows(
                &state,
                &event.state,
                0
            ));
            assert_eq!(
                event.state.project.device(device).unwrap().active_variant(),
                DeviceVariant::B
            );
            assert!(event.state.can_undo && event.state.unsaved);
            // A synthetic wrong-generation delivery of an ACTUAL proof cannot
            // change candidates, inputs or authoring. The valid event follows.
            view.service
                .submit(Request::CompareDevice {
                    owner: hex_session::service::preset::Owner {
                        project_generation: 0,
                        revision: event.state.revision,
                        device,
                        class: hex_engine::NativeDeviceClass::Instrument,
                        type_id: event
                            .state
                            .project
                            .device(device)
                            .unwrap()
                            .state()
                            .type_id(),
                    },
                    expected: DeviceVariant::A,
                    action: hex_project::DeviceComparisonAction::Switch,
                })
                .unwrap();
            let invalid = receive_service_event(&view.service);
            assert!(invalid.error.is_some()); // Wrong active slot: no new commit.
            assert_eq!(invalid.state.revision, event.state.revision);
            view.project_generation = 1;
            view.receive(invalid);
            view.project_generation = 0;
            assert_eq!(view.state.as_ref().unwrap().revision, state.revision);
            assert_eq!(view.playback_candidates[0].state.revision, state.revision);
            assert_eq!(
                view.computer_keyboard.target,
                Some((0, state.revision, track))
            );
        }
        view.receive(event);
        if recovery_error {
            assert_eq!(
                view.error.as_deref(),
                Some("automatic recovery: recovery directory must be a real directory")
            );
        }
        let next = view.state.as_ref().unwrap();
        assert_eq!(next.revision, state.revision + 1);
        assert!(Arc::ptr_eq(
            next.canonical_audio_source.as_ref().unwrap(),
            &source
        ));
        assert!(Arc::ptr_eq(
            view.playback_candidates[0]
                .prepared_revision
                .as_ref()
                .unwrap(),
            &prepared
        ));
        assert_eq!(view.playback_candidates[0].audio_revision, audio_revision);
        assert_eq!(view.playback_candidates[0].state.revision, next.revision);
        assert_eq!(
            view.audio_admission.current().unwrap().state,
            AdmissionState::Pending
        );
        assert_eq!(
            view.applied_project_revision(),
            None,
            "metadata proof cannot invent a callback ACK"
        );
    });
    let mut block = [[0.0; 2]; 128];
    backend.render(&mut block).unwrap();
    if recovery_error {
        assert!(
            block.iter().flatten().any(|sample| *sample != 0.0),
            "the real callback must keep rendering the held note"
        );
    }
    assert_eq!(
        backend.status().unwrap().applied_revision.load(Acquire),
        audio_revision
    );
    editor.update(cx, |view, cx| {
        view.tick(cx);
        assert!(
            !keyboard.is_cancelled(),
            "comparison-only authoring must preserve held keyboard input"
        );
        assert!(Arc::ptr_eq(
            view.computer_keyboard.input.as_ref().unwrap(),
            &keyboard
        ));
        assert!(view.computer_keyboard.held()[0].is_some());
        let current = view.state.as_ref().unwrap().clone();
        assert_eq!(view.applied_project_revision(), Some(current.revision));
        assert_eq!(
            view.audio_admission.current().unwrap().state,
            AdmissionState::Applied
        );
        let pending = view.worker_requests_pending;
        view.service
            .submit(Request::CompareDevice {
                owner: hex_session::service::preset::Owner {
                    project_generation: 0,
                    revision: current.revision,
                    device,
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: current.project.device(device).unwrap().state().type_id(),
                },
                expected: DeviceVariant::B,
                action: hex_project::DeviceComparisonAction::Switch,
            })
            .unwrap();
        let event = receive_service_event(&view.service);
        let next = event.state.clone();
        if recovery_error {
            assert_eq!(event.error.as_deref(), Some("automatic recovery: recovery directory must be a real directory"));
            assert!(event.playback.is_none());
            assert!(next.comparison_audio.as_ref().unwrap().follows(&current, &next, 0));
        }
        view.receive(event);
        if recovery_error {
            assert_eq!(view.error.as_deref(), Some("automatic recovery: recovery directory must be a real directory"));
            assert_eq!(view.computer_keyboard.target, Some((0, next.revision, track)));
            assert!(!keyboard.is_cancelled());
            assert!(Arc::ptr_eq(next.canonical_audio_source.as_ref().unwrap(), &source));
        }
        assert_eq!(view.applied_project_revision(), Some(next.revision));
        assert_eq!(
            view.audio_admission.current().unwrap().state,
            AdmissionState::Applied
        );
        assert_eq!(
            view.audio_admission.current().unwrap().audio_revision,
            Some(audio_revision)
        );
        assert!(view.pending_playback.is_none());
        assert_eq!(view.worker_requests_pending, pending);
        assert!(Arc::ptr_eq(
            view.audible_playback
                .as_ref()
                .unwrap()
                .prepared_revision
                .as_ref()
                .unwrap(),
            &prepared
        ));
        if recovery_error {
            // A same-revision Save/status must not need to replay the lost edge.
            view.service.submit(Request::SaveNew(directory.0.join("after-recovery.hex"))).unwrap();
            let saved = receive_service_event(&view.service);
            assert!(saved.error.is_none());
            assert!(saved.playback.is_none());
            assert_eq!(saved.state.revision, next.revision);
            assert!(Arc::ptr_eq(saved.state.canonical_audio_source.as_ref().unwrap(), &source));
            view.receive(saved);
            assert_eq!(view.applied_project_revision(), Some(next.revision));
            assert_eq!(view.audio_admission.current().unwrap().audio_revision, Some(audio_revision));
            assert!(!keyboard.is_cancelled());
            assert!(view.computer_keyboard.held()[0].is_some());
        }
        // Repeated status reconstruction is idempotent, not a new publication.
        view.tick(cx);
        assert_eq!(view.applied_project_revision(), Some(next.revision));
        view.queue_session_launch_batch(
            hex_engine::SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
            next.revision,
        );
        assert!(view.pending_audio_commands.iter().any(|pending| matches!(pending.action,
            hex_engine::playback::PlaybackAction::LaunchAndPlay(hex_engine::SessionPlaybackCommand::LaunchScene { scene: target, .. }) if target == scene)),
            "launch must queue exactly its actual scene: error={:?}", view.error);
        view.tick(cx); // The normal pump assigns the real command ticket.
        assert!(view.pending_session_ticket.is_some(),
            "current equivalent audio must submit actual Session launch: error={:?}", view.error);
        assert_eq!(view.worker_requests_pending, pending);
        view.stop();
        assert!(
            view.stopped_playback_restore.is_none(),
            "Stop must not recompile equivalent canonical audio"
        );
        assert_eq!(view.worker_requests_pending, pending);
        // Actual audio edit -> comparison while its graph is still unsubmitted.
        view.service
            .submit(Request::Edit(Command::Mute { track, muted: true }))
            .unwrap();
        let audio_event = receive_service_event(&view.service);
        let actual_prepared = audio_event
            .playback
            .as_ref()
            .unwrap()
            .prepared_revision()
            .unwrap()
            .clone();
        let audio_state = audio_event.state.clone();
        view.service
            .submit(Request::CompareDevice {
                owner: hex_session::service::preset::Owner {
                    project_generation: 0,
                    revision: audio_state.revision,
                    device,
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: audio_state
                        .project
                        .device(device)
                        .unwrap()
                        .state()
                        .type_id(),
                },
                expected: DeviceVariant::A,
                action: hex_project::DeviceComparisonAction::Switch,
            })
            .unwrap();
        let metadata_event = receive_service_event(&view.service);
        let metadata_state = metadata_event.state.clone();
        view.receive(metadata_event); // Deliberately out of order: cannot skip its actual audio source.
        assert_eq!(view.state.as_ref().unwrap().revision, next.revision);
        assert!(view.pending_playback.is_none());
        view.receive(audio_event);
        assert_eq!(view.state.as_ref().unwrap().revision, audio_state.revision);
        view.service
            .submit(Request::SaveNew(directory.0.join("queued.hex")))
            .unwrap();
        let saved = receive_service_event(&view.service);
        view.receive(saved); // Persistent, ordered proof now correlates the pending graph.
        assert_eq!(
            view.state.as_ref().unwrap().revision,
            metadata_state.revision
        );
        assert_eq!(
            view.pending_playback.as_ref().unwrap().project_revision,
            metadata_state.revision
        );
        assert!(Arc::ptr_eq(
            view.pending_playback
                .as_ref()
                .unwrap()
                .prepared_revision
                .as_ref()
                .unwrap(),
            &actual_prepared
        ));
        assert_eq!(
            view.audio_admission.current().unwrap().state,
            AdmissionState::Pending
        );
        assert_eq!(
            view.applied_project_revision(),
            Some(next.revision),
            "a pending graph cannot become applied through metadata"
        );
        view.service.submit(Request::SaveCurrent).unwrap();
        let delayed_metadata = receive_service_event(&view.service);
        view.service
            .submit(Request::Edit(Command::Mute {
                track,
                muted: false,
            }))
            .unwrap();
        let later_audio = receive_service_event(&view.service);
        let later_state = later_audio.state.clone();
        view.receive(later_audio);
        view.receive(delayed_metadata); // Old proof cannot resurrect the retired pending graph.
        assert_eq!(view.state.as_ref().unwrap().revision, later_state.revision);
        assert!(view.state.as_ref().unwrap().comparison_audio.is_none());
        assert_eq!(
            view.pending_playback.as_ref().unwrap().project_revision,
            later_state.revision
        );
        view.audio = None;
    });
    backend.close();
    drop(output);
}

fn auto_pan_shape(state: &hex_lattice_contract::NativeDeviceState) -> u32 {
    let view = hex_engine::native_device_view_for_class(NativeDeviceClass::Effect, state).unwrap();
    match view
        .parameters
        .iter()
        .find(|p| p.descriptor.id.value() == 4)
        .unwrap()
        .value
    {
        ParameterValue::Choice(value) => value,
        _ => panic!("auto pan shape choice"),
    }
}

fn auto_pan_pair(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    cx: &TestAppContext,
    device: DeviceId,
    active: DeviceVariant,
    current: u32,
    inactive: u32,
) {
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let entry = view.state.as_ref().unwrap().project.device(device).unwrap();
        assert_eq!(entry.active_variant(), active);
        assert_eq!(auto_pan_shape(entry.state()), current);
        let other = entry.comparison().unwrap().inactive();
        assert_eq!(auto_pan_shape(other), inactive);
        assert_eq!(
            auto_pan_panel::cached_preview(view, device)
                .unwrap()
                .as_ref()
                .unwrap(),
            &hex_engine::native_auto_pan_waveform_preview(entry.state()).unwrap(),
            "actual rendered cached preview follows the selected canonical bank"
        );
        for state in [entry.state(), other] {
            assert_eq!(state.input_gain_millidb(), -1234);
            assert_eq!(state.output_gain_millidb(), 4321);
        }
    });
}

fn choose_auto_pan_shape(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    cx: &mut TestAppContext,
    device: DeviceId,
    value: u32,
) {
    let chooser = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-4", device.value()).into_boxed_str(),
        ))
        .expect("actual waveform selector");
    visual.simulate_click(chooser.center(), Modifiers::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-4-option-{value}", device.value()).into_boxed_str(),
        ))
        .expect("actual waveform option");
    visual.simulate_click(option.center(), Modifiers::default());
    pump_until(editor, cx, settled);
    visual.run_until_parked();
}

fn auto_pan_banks_roundtrip(cx: &mut TestAppContext, on_return: bool) {
    let directory = Directory::new();
    let (service, state, track, bus, device) = catalog_fixture(
        NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.auto-pan"),
        on_return,
    );
    // Global host trims belong to the device, not either provider payload bank.
    let trimmed = state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone()
        .with_input_gain_millidb(-1234)
        .unwrap()
        .with_output_gain_millidb(4321)
        .unwrap();
    service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: trimmed,
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let (handle, editor) = directory.window(cx, service, state, track, bus);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(800.0)));
    visual.run_until_parked();
    choose_auto_pan_shape(&mut visual, &editor, cx, device, 1);
    title(&mut visual, device);
    menu(&mut visual, device);
    labels(&editor, cx, &["switch to b", "copy a to b"]);
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    auto_pan_pair(&mut visual, &editor, cx, device, DeviceVariant::B, 1, 1);
    choose_auto_pan_shape(&mut visual, &editor, cx, device, 4);
    auto_pan_pair(&mut visual, &editor, cx, device, DeviceVariant::B, 4, 1);

    // Persist the *distinct* pair before copies erase the discriminator.
    let saved = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let project_path = directory.0.join("distinct-banks.hex");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SaveNew(project_path.clone())))
    });
    pump_until(&editor, cx, settled);
    let reopened = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&reopened);
    reopened
        .submit(Request::Open(project_path.clone()))
        .unwrap();
    let loaded = receive_service_event(&reopened);
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    assert_eq!(loaded.state.project, saved);
    // Reopen through THIS actual controller too: identical IDs/payloads in a
    // new generation must retire the old panel cache and draw the real bank.
    let generation = editor.read_with(cx, |view, _| view.project_generation);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Open(project_path.clone())))
    });
    pump_until(&editor, cx, settled);
    editor.update(cx, |view, cx| {
        assert!(view.project_generation > generation);
        view.selected_track = Some(track);
        view.focused_return = bus;
        view.clip_view_open = true;
        view.device_panel_active = true;
        cx.notify();
    });
    auto_pan_pair(&mut visual, &editor, cx, device, DeviceVariant::B, 4, 1);
    let preset_path = directory.0.join("distinct-banks.hex-device-preset.json");
    let owner = |view: &EditorView| hex_session::service::preset::Owner {
        project_generation: view.project_generation,
        revision: view.state.as_ref().unwrap().revision,
        device,
        class: NativeDeviceClass::Effect,
        type_id: view
            .state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state()
            .type_id(),
    };
    editor.update(cx, |view, _| {
        let owner = owner(view);
        assert!(view.enqueue(Request::SaveDevicePreset {
            owner,
            path: preset_path.clone()
        }));
    });
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error)
    });
    let preset_bytes = std::fs::read(&preset_path).unwrap();
    choose_auto_pan_shape(&mut visual, &editor, cx, device, 2);
    auto_pan_pair(&mut visual, &editor, cx, device, DeviceVariant::B, 2, 1);
    let before_load = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    editor.update(cx, |view, _| {
        let owner = owner(view);
        assert!(view.enqueue(Request::LoadDevicePreset {
            owner,
            path: preset_path.clone()
        }));
    });
    pump_until(&editor, cx, settled);
    auto_pan_pair(&mut visual, &editor, cx, device, DeviceVariant::B, 4, 1);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before_load)
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, settled);
    auto_pan_pair(&mut visual, &editor, cx, device, DeviceVariant::B, 4, 1);
    assert_eq!(std::fs::read(&preset_path).unwrap(), preset_bytes);
    visual.run_until_parked();

    // Both directional menu copies retain the active slot and are exactly one edit.
    for (active, current, inactive, next) in [
        (DeviceVariant::B, 4, 1, DeviceVariant::A),
        (DeviceVariant::A, 1, 4, DeviceVariant::B),
    ] {
        title(&mut visual, device);
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        menu(&mut visual, device);
        labels(
            &editor,
            cx,
            if active == DeviceVariant::A {
                &["switch to b", "copy a to b"]
            } else {
                &["switch to a", "copy b to a"]
            },
        );
        visual.simulate_keystrokes("down enter");
        pump_until(&editor, cx, settled);
        auto_pan_pair(&mut visual, &editor, cx, device, active, current, current);
        let copied = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, before)
        });
        auto_pan_pair(&mut visual, &editor, cx, device, active, current, inactive);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, copied)
        });
        auto_pan_pair(&mut visual, &editor, cx, device, active, current, current);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, settled);
        visual.run_until_parked();
        title(&mut visual, device);
        visual.simulate_keystrokes("p");
        pump_until(&editor, cx, settled);
        visual.run_until_parked();
        auto_pan_pair(&mut visual, &editor, cx, device, next, inactive, current);
    }
    eprintln!(
        "auto pan generic A/B: return={on_return}; actual ID4 shapes1/4, both copy directions, project/preset reopen, load+copy one Undo/Redo, global trims preserved"
    );
}

#[gpui::test]
fn auto_pan_generic_ab_track_shapes_banks_copy_and_persistence(cx: &mut TestAppContext) {
    auto_pan_banks_roundtrip(cx, false);
}

#[gpui::test]
fn auto_pan_generic_ab_return_shapes_banks_copy_and_persistence(cx: &mut TestAppContext) {
    auto_pan_banks_roundtrip(cx, true);
}

fn rms_bank_values(state: &hex_lattice_contract::NativeDeviceState) -> (u32, f32) {
    let ParameterValue::Choice(mode) = hex_engine::native_device_parameter_for_class(
        NativeDeviceClass::Effect,
        state,
        hex_lattice_contract::ParameterId::from_static(8),
    )
    .unwrap() else {
        panic!("actual detector choice")
    };
    (mode, compressor_mix(state))
}

fn assert_rms_banks(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    cx: &TestAppContext,
    device: DeviceId,
    active: DeviceVariant,
    pair: [(u32, f32); 2],
) {
    visual.run_until_parked();
    let mode = if pair[0].0 == 1 { "rms" } else { "peak" };
    let other = if pair[0].0 == 1 { "peak" } else { "rms" };
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("compressor-{mode}-detector-{}", device.value()).into_boxed_str()
            ))
            .is_some(),
        "actual selected-bank caption"
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("compressor-{other}-detector-{}", device.value()).into_boxed_str()
            ))
            .is_none(),
        "no stale opposite-bank caption"
    );
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let entry = view.state.as_ref().unwrap().project.device(device).unwrap();
        assert_eq!(entry.active_variant(), active);
        let inactive = entry.comparison().unwrap().inactive();
        assert_eq!(rms_bank_values(entry.state()), pair[0]);
        assert_eq!(rms_bank_values(inactive), pair[1]);
        assert_eq!(
            compressor_panel::cached_curve(view, device).unwrap(),
            &hex_engine::native_compressor_transfer_curve(entry.state()).unwrap(),
            "actual rendered cached transfer follows the selected bank and mix"
        );
        if pair[0].1 != pair[1].1 {
            assert_ne!(
                hex_engine::native_compressor_transfer_curve(entry.state()).unwrap(),
                hex_engine::native_compressor_transfer_curve(inactive).unwrap(),
                "distinct mix makes stale-bank curve an observable discriminator"
            );
        }
        for state in [entry.state(), inactive] {
            assert_eq!(state.input_gain_millidb(), -1234);
            assert_eq!(state.output_gain_millidb(), 4321);
        }
    });
}

fn choose_compressor_detector(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    cx: &mut TestAppContext,
    device: DeviceId,
    mode: u32,
) {
    let chooser = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-8", device.value()).into_boxed_str(),
        ))
        .expect("actual detector selector");
    visual.simulate_click(chooser.center(), Modifiers::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-8-option-{mode}", device.value()).into_boxed_str(),
        ))
        .expect("actual detector option");
    visual.simulate_click(option.center(), Modifiers::default());
    pump_until(editor, cx, settled);
    visual.run_until_parked();
}

fn compressor_rms_banks_roundtrip(cx: &mut TestAppContext, on_return: bool) {
    let directory = Directory::new();
    let (service, state, track, bus, device) = catalog_fixture(
        NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.compressor"),
        on_return,
    );
    // Global host trims belong to the device, not either provider payload bank.
    let trimmed = state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone()
        .with_input_gain_millidb(-1234)
        .unwrap()
        .with_output_gain_millidb(4321)
        .unwrap();
    service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: trimmed,
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let (handle, editor) = directory.window(cx, service, state, track, bus);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(800.0)));
    visual.run_until_parked();
    enter_compressor_mix(&mut visual, &editor, cx, device, "25%");
    title(&mut visual, device);
    menu(&mut visual, device);
    labels(&editor, cx, &["switch to b", "copy a to b"]);
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    assert_rms_banks(
        &mut visual,
        &editor,
        cx,
        device,
        DeviceVariant::B,
        [(0, 0.25), (0, 0.25)],
    );
    choose_compressor_detector(&mut visual, &editor, cx, device, 1);
    enter_compressor_mix(&mut visual, &editor, cx, device, "75%");
    assert_rms_banks(
        &mut visual,
        &editor,
        cx,
        device,
        DeviceVariant::B,
        [(1, 0.75), (0, 0.25)],
    );

    // Persist the *distinct* pair before copies erase the discriminator.
    let saved = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let project_path = directory.0.join("distinct-rms-mix-banks.hex");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SaveNew(project_path.clone())))
    });
    pump_until(&editor, cx, settled);
    let reopened = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&reopened);
    reopened
        .submit(Request::Open(project_path.clone()))
        .unwrap();
    let loaded = receive_service_event(&reopened);
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    assert_eq!(loaded.state.project, saved);
    let generation = editor.read_with(cx, |view, _| view.project_generation);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Open(project_path.clone())))
    });
    pump_until(&editor, cx, settled);
    editor.update(cx, |view, cx| {
        assert!(view.project_generation > generation);
        view.selected_track = Some(track);
        view.focused_return = bus;
        view.clip_view_open = true;
        view.device_panel_active = true;
        cx.notify();
    });
    assert_rms_banks(
        &mut visual,
        &editor,
        cx,
        device,
        DeviceVariant::B,
        [(1, 0.75), (0, 0.25)],
    );
    let preset_path = directory
        .0
        .join("distinct-rms-mix-banks.hex-device-preset.json");
    let owner = |view: &EditorView| hex_session::service::preset::Owner {
        project_generation: view.project_generation,
        revision: view.state.as_ref().unwrap().revision,
        device,
        class: NativeDeviceClass::Effect,
        type_id: view
            .state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state()
            .type_id(),
    };
    editor.update(cx, |view, _| {
        let owner = owner(view);
        assert!(view.enqueue(Request::SaveDevicePreset {
            owner,
            path: preset_path.clone()
        }));
    });
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error)
    });
    let preset_bytes = std::fs::read(&preset_path).unwrap();
    enter_compressor_mix(&mut visual, &editor, cx, device, "50%");
    choose_compressor_detector(&mut visual, &editor, cx, device, 0);
    assert_rms_banks(
        &mut visual,
        &editor,
        cx,
        device,
        DeviceVariant::B,
        [(0, 0.5), (0, 0.25)],
    );
    let before_load = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    editor.update(cx, |view, _| {
        let owner = owner(view);
        assert!(view.enqueue(Request::LoadDevicePreset {
            owner,
            path: preset_path.clone()
        }));
    });
    pump_until(&editor, cx, settled);
    assert_rms_banks(
        &mut visual,
        &editor,
        cx,
        device,
        DeviceVariant::B,
        [(1, 0.75), (0, 0.25)],
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before_load)
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, settled);
    assert_rms_banks(
        &mut visual,
        &editor,
        cx,
        device,
        DeviceVariant::B,
        [(1, 0.75), (0, 0.25)],
    );
    assert_eq!(std::fs::read(&preset_path).unwrap(), preset_bytes);
    visual.run_until_parked();

    // A real keyboard switch is one edit and re-renders mode caption and curve.
    title(&mut visual, device);
    let before_switch =
        editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    visual.simulate_keystrokes("p");
    pump_until(&editor, cx, settled);
    assert_rms_banks(
        &mut visual,
        &editor,
        cx,
        device,
        DeviceVariant::A,
        [(0, 0.25), (1, 0.75)],
    );
    let after_switch = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before_switch)
    });
    assert_rms_banks(
        &mut visual,
        &editor,
        cx,
        device,
        DeviceVariant::B,
        [(1, 0.75), (0, 0.25)],
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, after_switch)
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, settled);
    visual.run_until_parked();

    // Both directional menu copies retain the active slot and are exactly one edit.
    for (active, current, inactive, next) in [
        (DeviceVariant::B, (1, 0.75), (0, 0.25), DeviceVariant::A),
        (DeviceVariant::A, (0, 0.25), (1, 0.75), DeviceVariant::B),
    ] {
        title(&mut visual, device);
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        menu(&mut visual, device);
        labels(
            &editor,
            cx,
            if active == DeviceVariant::A {
                &["switch to b", "copy a to b"]
            } else {
                &["switch to a", "copy b to a"]
            },
        );
        visual.simulate_keystrokes("down enter");
        pump_until(&editor, cx, settled);
        assert_rms_banks(&mut visual, &editor, cx, device, active, [current, current]);
        let copied = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, before)
        });
        assert_rms_banks(
            &mut visual,
            &editor,
            cx,
            device,
            active,
            [current, inactive],
        );
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, copied)
        });
        assert_rms_banks(&mut visual, &editor, cx, device, active, [current, current]);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, settled);
        visual.run_until_parked();
        title(&mut visual, device);
        visual.simulate_keystrokes("p");
        pump_until(&editor, cx, settled);
        visual.run_until_parked();
        assert_rms_banks(&mut visual, &editor, cx, device, next, [inactive, current]);
    }
    eprintln!(
        "compressor RMS generic A/B: return={on_return}; actual ID8 Peak/RMS plus ID7 25/75, both copy directions, project/preset reopen, load+copy one Undo/Redo, global trims preserved"
    );
}

#[gpui::test]
fn compressor_generic_ab_track_rms_mix_banks_copy_and_persistence(cx: &mut TestAppContext) {
    compressor_rms_banks_roundtrip(cx, false);
}

#[gpui::test]
fn compressor_generic_ab_return_rms_mix_banks_copy_and_persistence(cx: &mut TestAppContext) {
    compressor_rms_banks_roundtrip(cx, true);
}

//! Actual shared controls/worker history and delayed terminal display custody.
use super::*;
use hex_engine::delay::*;
use hex_lattice_contract::ParameterId;
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
#[derive(Default)]
struct Gate {
    armed: AtomicBool,
    reached: AtomicBool,
    released: Mutex<bool>,
    changed: Condvar,
}
struct Release(Arc<Gate>);
impl Release {
    fn arm(&self) {
        *self.0.released.lock().unwrap() = false;
        self.0.reached.store(false, Ordering::Release);
        self.0.armed.store(true, Ordering::Release);
    }
    fn release(&self) {
        *self.0.released.lock().unwrap() = true;
        self.0.changed.notify_all();
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.release();
    }
}
struct Sink(Arc<Gate>);
impl hex_session::service::WorkerAudioSink for Sink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(48000)
    }
    fn canonical_changed(&mut self, _: Arc<hex_session::service::CanonicalAudioSourceSnapshot>) {
        if self.0.armed.swap(false, Ordering::AcqRel) {
            self.0.reached.store(true, Ordering::Release);
            let lock = self.0.released.lock().unwrap();
            let _ = self
                .0
                .changed
                .wait_timeout_while(lock, std::time::Duration::from_secs(3), |released| {
                    !*released
                })
                .unwrap();
        }
    }
    fn poll(&mut self) {}
    fn close(&mut self) {
        *self.0.released.lock().unwrap() = true;
        self.0.changed.notify_all();
    }
}
fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    DeviceId,
    hex_project::ReturnId,
    DeviceId,
    Release,
) {
    #[cfg(target_os = "macos")]
    cx.use_native_text_system().unwrap();
    cx.update(|cx| {
        theme::load_fonts(cx).unwrap();
        hex_widgets::bind_numeric_control_keys(cx);
    });
    let gate = Arc::new(Gate::default());
    let service =
        Service::with_audio_sink(32, 8_000_000, None, Box::new(Sink(Arc::clone(&gate)))).unwrap();
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "delay".into(),
        }))
        .unwrap();
    let Created::Track(track) = receive_service_event(&service).created.unwrap() else {
        panic!("track")
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(TYPE_ID),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).unwrap().devices()[0].id();
    let authored = state.project.device(device).unwrap().state().clone();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "delay return".into(),
        }))
        .unwrap();
    let Created::ReturnTrack(bus) = receive_service_event(&service).created.unwrap() else {
        panic!("return")
    };
    service
        .submit(Request::Edit(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: authored,
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let ret = state.project.return_tracks()[0].devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = None;
        cx.notify();
    });
    (handle, editor, device, bus, ret, Release(gate))
}
fn settled(view: &EditorView) -> bool {
    view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.device_gesture_barriers.is_empty()
        && view.device_gesture.snapshot().is_none()
}
fn name(device: DeviceId, id: ParameterId, suffix: &str) -> &'static str {
    Box::leak(
        format!("device-{}-parameter-{}{suffix}", device.value(), id.value()).into_boxed_str(),
    )
}
fn read(view: &EditorView, device: DeviceId, id: ParameterId) -> ParameterValue {
    hex_engine::native_device_view(
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state(),
    )
    .unwrap()
    .parameters
    .iter()
    .find(|p| p.descriptor.id == id)
    .unwrap()
    .value
}
fn click(visual: &mut VisualTestContext, device: DeviceId, id: ParameterId, suffix: &str) {
    let bounds = visual
        .debug_bounds(name(device, id, suffix))
        .expect("actual shared control");
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
}
#[gpui::test]
fn delay_reference_panel_groups_fit_with_expanded_lfo_on_track_and_return(cx: &mut TestAppContext) {
    let (handle, editor, device, bus, ret, _gate) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [960.0, 1500.0] {
        visual.simulate_resize(size(px(width), px(650.0)));
        visual.run_until_parked();
        for (device, owner) in [(device, None), (ret, Some(bus))] {
            editor.update(cx, |view, cx| {
                view.focused_return = owner;
                view.device_panel
                    .chain_window
                    .scroll
                    .set_offset(point(px(0.0), px(0.0)));
                cx.notify();
            });
            visual.run_until_parked();
            for expanded in [false, true] {
                let body = visual
                    .debug_bounds(Box::leak(
                        format!("device-body-{}", device.value()).into_boxed_str(),
                    ))
                    .unwrap();
                assert_eq!(body.size.height, px(crate::device_composition::BODY_HEIGHT));
                let mut ids = vec![
                    LEFT_SYNC,
                    RIGHT_SYNC,
                    LEFT_DIVISION,
                    RIGHT_DIVISION,
                    LEFT_OFFSET,
                    RIGHT_OFFSET,
                    LINK,
                    FEEDBACK,
                    FREEZE,
                    SMOOTHING,
                    PING_PONG,
                    DRY_WET,
                    FILTER,
                    FREQUENCY,
                    WIDTH,
                    LFO_DELAY,
                    LFO_FILTER,
                ];
                if expanded {
                    ids.extend([
                        LFO_MODE,
                        LFO_RATE,
                        LFO_WAVE,
                        LFO_MORPH,
                        HIGH_QUALITY,
                        EQUAL_LOUDNESS,
                    ]);
                }
                let mut placed: Vec<Bounds<Pixels>> = Vec::new();
                for id in ids {
                    let field = visual.debug_bounds(name(device, id, "")).unwrap();
                    assert!(
                        field.left() >= body.left()
                            && field.right() <= body.right()
                            && field.top() >= body.top()
                            && field.bottom() <= body.bottom(),
                        "{width}px {id:?}: {field:?} inside {body:?}"
                    );
                    for previous in &placed {
                        assert!(
                            field.right() <= previous.left()
                                || field.left() >= previous.right()
                                || field.bottom() <= previous.top()
                                || field.top() >= previous.bottom(),
                            "overlap {field:?}/{previous:?}"
                        );
                    }
                    placed.push(field);
                }
                let graph = editor.read_with(cx, |v, _| delay_panel::graph_for_test(v, device));
                let plot = graph.read_with(cx, |g, _| g.plot_bounds().unwrap());
                assert!(plot.size.height >= px(60.0));
                assert!(plot.size.width >= px(200.0));
                if !expanded {
                    let disclosure = visual
                        .debug_bounds(Box::leak(
                            format!("delay-lfo-expand-{}", device.value()).into_boxed_str(),
                        ))
                        .unwrap();
                    visual.simulate_click(disclosure.center(), Modifiers::default());
                    visual.run_until_parked();
                }
            }
            // Restore the view-only disclosure before the next measured owner/width.
            let disclosure = visual
                .debug_bounds(Box::leak(
                    format!("delay-lfo-expand-{}", device.value()).into_boxed_str(),
                ))
                .unwrap();
            visual.simulate_click(disclosure.center(), Modifiers::default());
            visual.run_until_parked();
        }
    }
}
#[gpui::test]
fn delay_grid_keyboard_link_offsets_timing_and_history_use_real_shared_bindings(
    cx: &mut TestAppContext,
) {
    let (handle, editor, device, _, _, _gate) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    click(&mut visual, device, LEFT_DIVISION, "-option-6");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device, LEFT_DIVISION), ParameterValue::Choice(6));
        assert_eq!(read(v, device, RIGHT_DIVISION), ParameterValue::Choice(6));
    });
    visual.simulate_keystrokes("right");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device, RIGHT_DIVISION), ParameterValue::Choice(7))
    });
    click(&mut visual, device, LINK, "");
    pump_until(&editor, cx, settled);
    click(&mut visual, device, LEFT_DIVISION, "-option-0");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device, RIGHT_DIVISION), ParameterValue::Choice(7))
    });
    click(&mut visual, device, RIGHT_SYNC, "");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, RIGHT_TIME, "1234.5");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device, RIGHT_TIME), ParameterValue::Float(1234.5));
        assert_eq!(read(v, device, LEFT_SYNC), ParameterValue::Bool(true));
    });
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device, RIGHT_TIME), ParameterValue::Float(375.0))
    });
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Redo)));
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device, RIGHT_TIME), ParameterValue::Float(1234.5))
    });
    enter_typed_value(&mut visual, device, RIGHT_TIME, "5001");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(read(v, device, RIGHT_TIME), ParameterValue::Float(1234.5))
    });
    visual.simulate_keystrokes("escape");
}
#[gpui::test]
fn delay_filter_xy_release_keeps_final_display_across_three_repaints_until_real_ack(
    cx: &mut TestAppContext,
) {
    let (handle, editor, device, _, _, gate) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let graph = editor.read_with(cx, |v, _| delay_panel::graph_for_test(v, device));
    let (plot, position) = graph.read_with(cx, |g, _| {
        (
            g.plot_bounds().expect("painted filter plot"),
            g.node_position(1).expect("provider filter node"),
        )
    });
    let point_at = |p: [f32; 2]| {
        point(
            plot.left() + plot.size.width * p[0],
            plot.bottom() - plot.size.height * p[1],
        )
    };
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
    visual.simulate_mouse_down(point_at(position), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point_at([0.7, 0.5]),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    pump_until(&editor, cx, |v| v.device_gesture.snapshot().is_some());
    gate.arm();
    visual.simulate_mouse_up(
        point_at([0.7, 0.5]),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump_until(&editor, cx, |v| {
        v.device_gesture.has_pending_terminal() && gate.0.reached.load(Ordering::Acquire)
    });
    let final_position = graph.read_with(cx, |g, _| g.node_position(1).unwrap());
    assert_ne!(final_position, position);
    for _ in 0..3 {
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(
            graph.read_with(cx, |g, _| g.node_position(1).unwrap()),
            final_position
        );
        editor.read_with(cx, |v, _| {
            assert_eq!(&v.state.as_ref().unwrap().project, &before)
        });
    }
    gate.release();
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    assert_eq!(
        graph.read_with(cx, |g, _| g.node_position(1).unwrap()),
        final_position
    );
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |v, _| {
        assert_eq!(&v.state.as_ref().unwrap().project, &before)
    });
    visual.run_until_parked();
    let original = graph.read_with(cx, |g, _| g.node_position(1).unwrap());
    visual.simulate_mouse_down(point_at(original), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point_at([0.3, 0.3]),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(
        point_at([0.3, 0.3]),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |v, _| {
        assert_eq!(&v.state.as_ref().unwrap().project, &before)
    });
    visual.run_until_parked();
    assert_eq!(
        graph.read_with(cx, |g, _| g.node_position(1).unwrap()),
        original,
        "escape restores the displayed node even without an intervening repaint"
    );
}

#[gpui::test]
fn delay_filter_refused_begin_retires_capture_without_moving_another_gesture(
    cx: &mut TestAppContext,
) {
    let (handle, editor, device, _, _, _gate) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let graph = editor.read_with(cx, |v, _| delay_panel::graph_for_test(v, device));
    let (plot, original) = graph.read_with(cx, |g, _| {
        (g.plot_bounds().unwrap(), g.node_position(1).unwrap())
    });
    editor.update(cx, |v, _| {
        v.begin_device_gesture(
            GestureTarget::DeviceParameter {
                device,
                parameter: FEEDBACK,
            },
            ParameterValue::Float(0.5),
        )
        .unwrap()
    });
    let key = editor.read_with(cx, |v, _| v.device_gesture.snapshot().unwrap().key);
    let point_at = |p: [f32; 2]| {
        point(
            plot.left() + plot.size.width * p[0],
            plot.bottom() - plot.size.height * p[1],
        )
    };
    visual.simulate_mouse_down(point_at(original), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point_at([0.3, 0.3]),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point_at([0.3, 0.3]),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(
        graph.read_with(cx, |g, _| g.node_position(1).unwrap()),
        original
    );
    assert!(!graph.read_with(cx, |g, _| g.is_dragging()));
    editor.update(cx, |v, _| {
        assert_eq!(v.device_gesture.snapshot().unwrap().key, key);
        v.cancel_device_gesture().unwrap();
    });
    pump_until(&editor, cx, settled);
}

#[gpui::test]
fn delay_catalog_growth_keeps_all_add_choices_clickable_on_track_return_and_main(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, bus, _, _gate) = fixture(cx);
    let track = editor.read_with(cx, |v, _| v.selected_track.unwrap());
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [960.0, 1500.0] {
        visual.simulate_resize(size(px(width), px(650.0)));
        for owner in [
            None,
            Some(EffectBusOwner::Return(bus)),
            Some(EffectBusOwner::Main),
        ] {
            editor.update(cx, |v, cx| {
                v.focused_return = match owner {
                    Some(EffectBusOwner::Return(id)) => Some(id),
                    _ => None,
                };
                v.selected_track = owner.is_none().then_some(track);
                v.device_panel.main_selected =
                    (owner == Some(EffectBusOwner::Main)).then_some(v.project_generation);
                v.device_panel.add_menu_open = true;
                cx.notify();
            });
            visual.run_until_parked();
            let panel = visual.debug_bounds("device-panel").unwrap();
            let menu = visual.debug_bounds("device-add-menu").unwrap();
            assert!(
                menu.left() >= panel.left()
                    && menu.right() <= panel.right()
                    && menu.top() >= panel.top()
                    && menu.bottom() <= panel.bottom(),
                "{width}/{owner:?}: {menu:?} in {panel:?}"
            );
            let mut choices: Vec<Bounds<Pixels>> = Vec::new();
            for class in [NativeDeviceClass::Instrument, NativeDeviceClass::Effect] {
                if class == NativeDeviceClass::Instrument && owner.is_some() {
                    continue;
                }
                for entry in hex_engine::native_device_catalog_for_class(class).unwrap() {
                    let selector = if let Some(owner) = owner {
                        format!(
                            "add-{}-effect-{}",
                            owner.prefix(),
                            entry.type_id.as_str().unwrap()
                        )
                    } else {
                        format!(
                            "add-native-{}-{}",
                            if class == NativeDeviceClass::Effect {
                                "effect"
                            } else {
                                "instrument"
                            },
                            entry.type_id.as_str().unwrap()
                        )
                    };
                    let bounds = visual
                        .debug_bounds(Box::leak(selector.into_boxed_str()))
                        .unwrap();
                    assert!(
                        bounds.left() >= menu.left()
                            && bounds.right() <= menu.right()
                            && bounds.top() >= menu.top()
                            && bounds.bottom() <= menu.bottom(),
                        "{width}/{owner:?}: {bounds:?} in {menu:?}"
                    );
                    for other in &choices {
                        assert!(
                            bounds.right() <= other.left()
                                || bounds.left() >= other.right()
                                || bounds.bottom() <= other.top()
                                || bounds.top() >= other.bottom()
                        );
                    }
                    choices.push(bounds);
                }
            }
            let count = |v: &EditorView| {
                let p = &v.state.as_ref().unwrap().project;
                if let Some(owner) = owner {
                    owner.devices(p).unwrap().len()
                } else {
                    p.track(track).unwrap().devices().len()
                }
            };
            let before = editor.read_with(cx, |v, _| count(v));
            let before_project =
                editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
            let selector = if let Some(owner) = owner {
                format!("add-{}-effect-hex.delay", owner.prefix())
            } else {
                "add-native-effect-hex.delay".into()
            };
            let bounds = visual
                .debug_bounds(Box::leak(selector.into_boxed_str()))
                .unwrap();
            visual.simulate_click(bounds.center(), Modifiers::default());
            pump_until(&editor, cx, settled);
            editor.read_with(cx, |v, _| {
                assert!(v.error.is_none(), "{width}/{owner:?}: {:?}", v.error);
                assert_eq!(
                    count(v),
                    before + 1,
                    "{width}/{owner:?}: add={:?} message={:?} open={}",
                    v.last_enqueued_native_device_add,
                    v.device_panel.message,
                    v.device_panel.add_menu_open
                );
            });
            editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
            pump_until(&editor, cx, settled);
            visual.run_until_parked();
            editor.read_with(cx, |v, _| {
                assert_eq!(count(v), before);
                let actual = &v.state.as_ref().unwrap().project;
                let mut expected = before_project.snapshot();
                expected.next_id = actual.next_identity();
                assert_eq!(
                    actual.snapshot(),
                    expected,
                    "a menu click must not also edit a control underneath it"
                );
            });
        }
    }
}

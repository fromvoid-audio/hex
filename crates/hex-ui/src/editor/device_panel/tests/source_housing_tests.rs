//! The shared source housing must not absorb track-local devices.
use super::*;

#[gpui::test]
fn shared_source_housing_excludes_local_prefix_and_suffix(cx: &mut TestAppContext) {
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    let edit = |command| {
        service.submit(Request::Edit(command)).unwrap();
        let event = receive_service_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let mut tracks = Vec::new();
    for name in ["receiver", "contributor"] {
        let Some(Created::Track(track)) = edit(Command::AddMidiTrack { name: name.into() }).created
        else {
            panic!("track")
        };
        tracks.push(track);
    }
    let effect = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let created = edit(Command::CreateNativeSource {
        name: "shared".into(),
        devices: vec![
            hex_project::default_instrument_state().unwrap(),
            effect.clone(),
        ],
        instrument_index: 0,
    });
    let Some(Created::NativeSource(source)) = created.created else {
        panic!("source")
    };
    let definition = created.state.project.native_source(source).unwrap();
    let instrument = definition.instrument().id();
    let source_effect = definition.post_instrument_effects()[0].id();
    let mut locals = Vec::new();
    let mut last = created;
    for track in &tracks {
        edit(Command::BindTrackSource {
            track: *track,
            source: Some(source),
        });
        let prefix = edit(Command::InsertDevice {
            track: *track,
            before: Some(instrument),
            state: effect.clone(),
        });
        let Some(Created::Device(prefix)) = prefix.created else {
            panic!("prefix")
        };
        last = edit(Command::InsertDevice {
            track: *track,
            before: None,
            state: effect.clone(),
        });
        let Some(Created::Device(suffix)) = last.created else {
            panic!("suffix")
        };
        locals.push((prefix, suffix));
    }
    let original = last.state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, last.state, tracks[0]);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(2400.0), px(700.0)));
    for (track, (prefix, suffix)) in tracks.into_iter().zip(locals) {
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.select_track(track, window);
                cx.notify();
            })
        });
        visual.run_until_parked();
        let housing = visual
            .debug_bounds("source-chain-housing")
            .expect("source housing");
        let bounds = |visual: &mut VisualTestContext, key: String| {
            let key = Box::leak(key.into_boxed_str());
            visual
                .debug_bounds(key)
                .unwrap_or_else(|| panic!("missing {key}"))
        };
        let prefix_bounds = bounds(&mut visual, format!("device-drop-card-{}", prefix.value()));
        let instrument_bounds = bounds(
            &mut visual,
            format!("device-drop-card-{}", instrument.value()),
        );
        let source_bounds = bounds(
            &mut visual,
            format!("source-effect-modulation-{}", source_effect.value()),
        );
        let suffix_bounds = bounds(&mut visual, format!("device-drop-card-{}", suffix.value()));
        assert!(prefix_bounds.origin.x + prefix_bounds.size.width <= housing.origin.x);
        assert!(housing.contains(&instrument_bounds.center()));
        assert!(housing.contains(&source_bounds.center()));
        assert!(suffix_bounds.origin.x >= housing.origin.x + housing.size.width);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, original)
        });
    }
}

macro_rules! source_matrix_case {
    ($name:ident, $parameter:expr, $filter:expr) => {
        #[gpui::test]
        fn $name(cx: &mut TestAppContext) {
            shared_source_utility_matrix_with_filter(cx, $filter, $parameter);
        }
    };
}

source_matrix_case!(shared_source_gain_plain, 1, None);
source_matrix_case!(
    shared_source_gain_dc,
    1,
    Some(hex_devices::utility::parameter::DC_REMOVE)
);
source_matrix_case!(
    shared_source_gain_bass,
    1,
    Some(hex_devices::utility::parameter::BASS_MONO)
);
source_matrix_case!(shared_source_balance_plain, 2, None);
source_matrix_case!(
    shared_source_balance_dc,
    2,
    Some(hex_devices::utility::parameter::DC_REMOVE)
);
source_matrix_case!(
    shared_source_balance_bass,
    2,
    Some(hex_devices::utility::parameter::BASS_MONO)
);
source_matrix_case!(shared_source_width_plain, 3, None);
source_matrix_case!(
    shared_source_width_dc,
    3,
    Some(hex_devices::utility::parameter::DC_REMOVE)
);
source_matrix_case!(
    shared_source_width_bass,
    3,
    Some(hex_devices::utility::parameter::BASS_MONO)
);
source_matrix_case!(shared_source_mid_side_plain, 10, None);
source_matrix_case!(
    shared_source_mid_side_dc,
    10,
    Some(hex_devices::utility::parameter::DC_REMOVE)
);
source_matrix_case!(
    shared_source_mid_side_bass,
    10,
    Some(hex_devices::utility::parameter::BASS_MONO)
);

fn shared_source_utility_matrix_with_filter(
    cx: &mut TestAppContext,
    filter: Option<hex_lattice_contract::ParameterId>,
    parameter: u32,
) {
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    let edit = |command| {
        service.submit(Request::Edit(command)).unwrap();
        let event = receive_service_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let Some(Created::Track(receiver)) = edit(Command::AddMidiTrack {
        name: "receiver".into(),
    })
    .created
    else {
        panic!("receiver")
    };
    let Some(Created::Track(contributor)) = edit(Command::AddMidiTrack {
        name: "contributor".into(),
    })
    .created
    else {
        panic!("contributor")
    };
    let mut utility = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    if let Some(filter) = filter {
        use hex_lattice_contract::NativeDeviceMetadata;
        utility = hex_devices::utility::UTILITY_DEFINITION
            .patch_parameter(&utility, filter, ParameterValue::Bool(true))
            .unwrap();
    }
    if parameter == 10 {
        use hex_lattice_contract::NativeDeviceMetadata;
        utility = hex_devices::utility::UTILITY_DEFINITION
            .patch_parameter(
                &utility,
                hex_devices::utility::parameter::MID_SIDE_ENABLED,
                ParameterValue::Bool(true),
            )
            .unwrap();
    }
    let created = edit(Command::CreateNativeSource {
        name: "shared".into(),
        devices: vec![
            hex_project::default_instrument_state().unwrap(),
            utility.clone(),
        ],
        instrument_index: 0,
    });
    let Some(Created::NativeSource(source)) = created.created else {
        panic!("source")
    };
    let device = created
        .state
        .project
        .native_source(source)
        .unwrap()
        .post_instrument_effects()[0]
        .id();
    edit(Command::BindTrackSource {
        track: receiver,
        source: Some(source),
    });
    edit(Command::BindTrackSource {
        track: contributor,
        source: Some(source),
    });
    let other = edit(Command::CreateNativeSource {
        name: "other source".into(),
        devices: vec![hex_project::default_instrument_state().unwrap()],
        instrument_index: 0,
    });
    let Some(Created::NativeSource(other)) = other.created else {
        panic!("other source")
    };
    let bound = edit(Command::InsertDevice {
        track: contributor,
        before: None,
        state: utility,
    });
    let local = bound
        .state
        .project
        .track(contributor)
        .unwrap()
        .devices()
        .to_vec();
    let (handle, editor) = actor_editor_window(cx, service, bound.state, contributor);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let gain = hex_lattice_contract::ParameterId::from_static(parameter);
    let (original, desired, typed) = match parameter {
        1 => (1.0, 10.0_f32.powf(6.0 / 20.0), "6 db"),
        2 => (0.0, 0.5, "50 r"),
        3 => (1.0, 0.0, "0%"),
        10 => (0.0, 0.5, "50 s"),
        _ => panic!("unsupported test parameter"),
    };
    let selector = Box::leak(
        format!("device-{}-parameter-{}-value", device.value(), gain.value()).into_boxed_str(),
    );
    visual
        .debug_bounds(selector)
        .expect("source Utility control");
    let card = visual
        .debug_bounds(Box::leak(
            format!("source-effect-modulation-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    for parameter in [1, 2, if parameter == 10 { 10 } else { 3 }] {
        let key = Box::leak(
            format!("device-{}-parameter-{}", device.value(), parameter).into_boxed_str(),
        );
        let control = visual.debug_bounds(key).expect("source matrix control");
        assert!(
            control.origin.x >= card.origin.x
                && control.origin.y >= card.origin.y
                && control.origin.x + control.size.width <= card.origin.x + card.size.width
                && control.origin.y + control.size.height <= card.origin.y + card.size.height,
            "control {parameter} outside source card: {control:?} vs {card:?}"
        );
        assert!(
            control.size.height >= px(hex_widgets::COMPACT_DIAL_HEIGHT),
            "control {parameter} was compressed: {control:?}"
        );
    }
    let inactive = if parameter == 10 { 3 } else { 10 };
    let inactive_selector =
        Box::leak(format!("device-{}-parameter-{inactive}-value", device.value()).into_boxed_str());
    assert!(visual.debug_bounds(inactive_selector).is_none());
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let choice = editor.read_with(&visual, |view, _| {
        let item = view
            .context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .find(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("other source")
            })
            .unwrap();
        let crate::editor::context_menu::EditorContextAction::SourcePicker(choice) = &item.action
        else {
            panic!("source choice")
        };
        choice.clone()
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    enter_typed_value(&mut visual, device, gain, typed);
    editor.update(cx, |view, cx| {
        assert!(super::super::finalize_controls(view, cx))
    });
    let value = |view: &EditorView| {
        let device = view.state.as_ref()?.project.device(device)?;
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            device.state(),
            gain,
        )
        .ok()
    };
    // The Service is deliberately not polled while the terminal is pending.
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            choice.clone().perform(view, window, cx);
        })
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert!(view.device_gesture.snapshot().is_some());
            let Some(ParameterControl::Float { entity, .. }) =
                view.device_panel.controls.get(&ParameterKey {
                    device,
                    parameter: gain,
                })
            else {
                panic!("pending source matrix control")
            };
            assert!((entity.read(cx).value() - f64::from(desired)).abs() < 0.00001);
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(contributor)
                    .unwrap()
                    .source_id(),
                Some(source)
            );
            assert_ne!(source, other);
        });
    }
    pump_until(
        &editor,
        cx,
        |view| matches!(value(view), Some(ParameterValue::Float(n)) if (n - desired).abs() < 0.00001),
    );
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(
            project.device_location(device).unwrap().owner,
            hex_project::DeviceOwner::Source(source)
        );
        assert_eq!(project.track(contributor).unwrap().devices(), local);
        assert_eq!(project.track(receiver).unwrap().source_id(), Some(source));
        assert_eq!(
            project.track(contributor).unwrap().source_id(),
            Some(source)
        );
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            choice.perform(view, window, cx);
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(contributor)
                    .unwrap()
                    .source_id(),
                Some(source)
            );
        })
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(
        &editor,
        cx,
        |view| matches!(value(view), Some(ParameterValue::Float(n)) if (n - original).abs() < 0.00001),
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(
        &editor,
        cx,
        |view| matches!(value(view), Some(ParameterValue::Float(n)) if (n - desired).abs() < 0.00001),
    );
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.select_track(receiver, window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    visual
        .debug_bounds(selector)
        .expect("same Utility control on receiver");
    editor.read_with(cx, |view, _| {
        assert!(
            matches!(value(view), Some(ParameterValue::Float(n)) if (n - desired).abs() < 0.00001)
        )
    });
}

#[test]
fn source_utility_gain_is_limited_to_unautomated_active_processing() {
    use hex_lattice_contract::{DeviceAutomationTarget, NativeDeviceMetadata};
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "source".into(),
            devices: vec![
                hex_project::default_instrument_state().unwrap(),
                hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            ],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let device = editor
        .project()
        .native_source(source)
        .unwrap()
        .post_instrument_effects()[0]
        .id();
    editor
        .execute(Command::BindTrackSource {
            track,
            source: Some(source),
        })
        .unwrap();
    let available = |editor: &Editor| {
        let gain = super::super::modulators::source_utility_gain_available(
            editor.project(),
            editor.project().device(device).unwrap(),
        );
        let balance = hex_engine::project_source_utility_matrix_edit_supported(
            editor.project(),
            device,
            hex_devices::utility::parameter::BALANCE,
        );
        assert_eq!(gain, balance);
        gain
    };
    assert!(available(&editor));
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    assert!(!available(&editor));
    assert!(editor.undo());
    let state = hex_devices::utility::UTILITY_DEFINITION
        .patch_parameter(
            editor.project().device(device).unwrap().state(),
            hex_devices::utility::parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    assert!(available(&editor));
    assert!(editor.undo());
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Parameter(hex_devices::utility::parameter::GAIN),
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
        .unwrap();
    assert!(!available(&editor));
    assert!(!hex_engine::project_source_utility_mute_edit_supported(
        editor.project(),
        device
    ));
}

macro_rules! source_mode_case {
    ($name:ident, $filter:expr, $initial:expr) => {
        #[gpui::test]
        fn $name(cx: &mut TestAppContext) {
            shared_source_mode_with_filter(
                cx,
                $filter,
                $initial,
                false,
                hex_devices::utility::parameter::MID_SIDE_ENABLED,
            );
        }
    };
}
source_mode_case!(shared_source_mode_on_plain, None, false);
source_mode_case!(shared_source_mode_off_plain, None, true);
source_mode_case!(
    shared_source_mode_on_dc,
    Some(hex_devices::utility::parameter::DC_REMOVE),
    false
);
source_mode_case!(
    shared_source_mode_off_dc,
    Some(hex_devices::utility::parameter::DC_REMOVE),
    true
);
source_mode_case!(
    shared_source_mode_on_bass,
    Some(hex_devices::utility::parameter::BASS_MONO),
    false
);
source_mode_case!(
    shared_source_mode_off_bass,
    Some(hex_devices::utility::parameter::BASS_MONO),
    true
);

#[gpui::test]
fn shared_source_mode_restores_after_worker_refusal(cx: &mut TestAppContext) {
    shared_source_mode_with_filter(
        cx,
        None,
        false,
        true,
        hex_devices::utility::parameter::MID_SIDE_ENABLED,
    );
}

macro_rules! source_mono_case {
    ($name:ident, $filter:expr, $initial:expr, $reject:expr) => {
        #[gpui::test]
        fn $name(cx: &mut TestAppContext) {
            shared_source_mode_with_filter(
                cx,
                $filter,
                $initial,
                $reject,
                hex_devices::utility::parameter::MONO,
            );
        }
    };
}
source_mono_case!(shared_source_mono_on_plain, None, false, false);
source_mono_case!(shared_source_mono_off_plain, None, true, false);
source_mono_case!(
    shared_source_mono_on_dc,
    Some(hex_devices::utility::parameter::DC_REMOVE),
    false,
    false
);
source_mono_case!(
    shared_source_mono_off_dc,
    Some(hex_devices::utility::parameter::DC_REMOVE),
    true,
    false
);
source_mono_case!(
    shared_source_mono_on_bass,
    Some(hex_devices::utility::parameter::BASS_MONO),
    false,
    false
);
source_mono_case!(
    shared_source_mono_off_bass,
    Some(hex_devices::utility::parameter::BASS_MONO),
    true,
    false
);
source_mono_case!(shared_source_mono_refusal, None, false, true);

#[gpui::test]
fn shared_source_invert_left_with_dc(cx: &mut TestAppContext) {
    shared_source_mode_with_filter(
        cx,
        Some(hex_devices::utility::parameter::DC_REMOVE),
        false,
        false,
        hex_devices::utility::parameter::INVERT_LEFT,
    );
}

#[gpui::test]
fn shared_source_invert_right_with_bass(cx: &mut TestAppContext) {
    shared_source_mode_with_filter(
        cx,
        Some(hex_devices::utility::parameter::BASS_MONO),
        true,
        false,
        hex_devices::utility::parameter::INVERT_RIGHT,
    );
}

#[gpui::test]
fn shared_source_automated_invert_is_disabled(cx: &mut TestAppContext) {
    shared_source_mode_inner(
        cx,
        Some(hex_devices::utility::parameter::DC_REMOVE),
        false,
        false,
        hex_devices::utility::parameter::INVERT_LEFT,
        true,
    );
}

#[gpui::test]
fn shared_source_mute_on_and_off(cx: &mut TestAppContext) {
    shared_source_mode_with_filter(
        cx,
        None,
        false,
        false,
        hex_devices::utility::parameter::MUTE,
    );
}

#[gpui::test]
fn shared_source_unmute_and_mute(cx: &mut TestAppContext) {
    shared_source_mode_with_filter(cx, None, true, false, hex_devices::utility::parameter::MUTE);
}

#[gpui::test]
fn shared_source_dc_mute_and_unmute(cx: &mut TestAppContext) {
    shared_source_mode_with_filter(
        cx,
        Some(hex_devices::utility::parameter::DC_REMOVE),
        false,
        false,
        hex_devices::utility::parameter::MUTE,
    );
}

#[gpui::test]
fn shared_source_bass_unmute_and_mute(cx: &mut TestAppContext) {
    shared_source_mode_with_filter(
        cx,
        Some(hex_devices::utility::parameter::BASS_MONO),
        true,
        false,
        hex_devices::utility::parameter::MUTE,
    );
}

#[gpui::test]
fn shared_source_automated_mute_is_disabled(cx: &mut TestAppContext) {
    shared_source_mode_inner(
        cx,
        Some(hex_devices::utility::parameter::DC_REMOVE),
        false,
        false,
        hex_devices::utility::parameter::MUTE,
        true,
    );
}

fn shared_source_mode_with_filter(
    cx: &mut TestAppContext,
    filter: Option<hex_lattice_contract::ParameterId>,
    initial: bool,
    reject: bool,
    parameter: hex_lattice_contract::ParameterId,
) {
    shared_source_mode_inner(cx, filter, initial, reject, parameter, false);
}

fn shared_source_mode_inner(
    cx: &mut TestAppContext,
    filter: Option<hex_lattice_contract::ParameterId>,
    initial: bool,
    reject: bool,
    parameter: hex_lattice_contract::ParameterId,
    unsupported: bool,
) {
    use hex_lattice_contract::NativeDeviceMetadata;
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    let edit = |command| {
        service.submit(Request::Edit(command)).unwrap();
        let event = receive_service_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let Some(Created::Track(receiver)) = edit(Command::AddMidiTrack {
        name: "receiver".into(),
    })
    .created
    else {
        panic!("receiver")
    };
    let Some(Created::Track(contributor)) = edit(Command::AddMidiTrack {
        name: "contributor".into(),
    })
    .created
    else {
        panic!("contributor")
    };
    let mut utility = hex_devices::utility::UTILITY_DEFINITION
        .default_state()
        .unwrap();
    if let Some(filter) = filter {
        utility = hex_devices::utility::UTILITY_DEFINITION
            .patch_parameter(&utility, filter, ParameterValue::Bool(true))
            .unwrap();
    }
    utility = hex_devices::utility::UTILITY_DEFINITION
        .patch_parameter(&utility, parameter, ParameterValue::Bool(initial))
        .unwrap();
    let created = edit(Command::CreateNativeSource {
        name: "shared".into(),
        devices: vec![
            hex_project::default_instrument_state().unwrap(),
            utility.clone(),
        ],
        instrument_index: 0,
    });
    let Some(Created::NativeSource(source)) = created.created else {
        panic!("source")
    };
    let device = created
        .state
        .project
        .native_source(source)
        .unwrap()
        .post_instrument_effects()[0]
        .id();
    edit(Command::BindTrackSource {
        track: receiver,
        source: Some(source),
    });
    edit(Command::BindTrackSource {
        track: contributor,
        source: Some(source),
    });
    let other = edit(Command::CreateNativeSource {
        name: "other source".into(),
        devices: vec![hex_project::default_instrument_state().unwrap()],
        instrument_index: 0,
    });
    let Some(Created::NativeSource(other)) = other.created else {
        panic!("other source")
    };
    let bound = edit(Command::InsertDevice {
        track: contributor,
        before: None,
        state: utility,
    });
    let bound = if unsupported {
        use hex_lattice_contract::DeviceAutomationTarget;
        use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime};
        edit(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Parameter(hex_devices::utility::parameter::GAIN),
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
    } else {
        bound
    };
    let local = bound
        .state
        .project
        .track(contributor)
        .unwrap()
        .devices()
        .to_vec();
    let (handle, editor) = actor_editor_window(cx, service, bound.state, contributor);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let choice = editor.read_with(&visual, |view, _| {
        let item = view
            .context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .find(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("other source")
            })
            .unwrap();
        let crate::editor::context_menu::EditorContextAction::SourcePicker(choice) = &item.action
        else {
            panic!("choice")
        };
        choice.clone()
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let mode = parameter;
    let button =
        Box::leak(format!("device-{}-parameter-{}", device.value(), mode.value()).into_boxed_str());
    let target = visual.debug_bounds(button).expect("mode button");
    if unsupported {
        visual.simulate_click(target.center(), Modifiers::default());
        visual.run_until_parked();
        let toggle = editor.read_with(&visual, |view, _| {
            let Some(ParameterControl::Bool { entity, .. }) =
                view.device_panel.controls.get(&ParameterKey {
                    device,
                    parameter: mode,
                })
            else {
                panic!("mute control")
            };
            entity.clone()
        });
        // Simulate an already queued activation reaching the old control after
        // its source state became ineligible.
        toggle.update(&mut visual, |_, cx| {
            cx.emit(ToggleChanged { checked: true })
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.device_gesture.snapshot().is_none());
            assert!(view.error.is_some());
            assert_eq!(
                hex_engine::native_device_parameter_for_class(
                    NativeDeviceClass::Effect,
                    view.state
                        .as_ref()
                        .unwrap()
                        .project
                        .device(device)
                        .unwrap()
                        .state(),
                    mode,
                )
                .unwrap(),
                ParameterValue::Bool(initial)
            );
        });
        return;
    }
    if mode == hex_devices::utility::parameter::MUTE && !initial {
        use crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::{
            headless_status_output, status_fixture_guard,
        };
        use std::sync::atomic::Ordering;
        let _guard = status_fixture_guard();
        let (mut output, _backend) = headless_status_output();
        let control = output.take_client().unwrap();
        control
            .status()
            .unwrap()
            .playing
            .store(true, Ordering::Release);
        editor.update(&mut visual, |view, cx| {
            view.audio = Some(crate::editor_backend::EditorAudio {
                control,
                sample_rate: 48_000,
                label: "private control".into(),
                spectrum_error: None,
                mode: hex_session::DocumentAudioMode::Resident,
            });
            cx.notify();
        });
        visual.run_until_parked();
        visual.simulate_click(target.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.device_gesture.snapshot().is_some())
        });
        editor.update(&mut visual, |view, cx| {
            view.audio = None;
            cx.notify();
        });
    } else {
        visual.simulate_click(target.center(), Modifiers::default());
    }
    visual.run_until_parked();
    let key = ParameterKey {
        device,
        parameter: mode,
    };
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| choice.clone().perform(view, window, cx))
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            let snapshot = view
                .device_gesture
                .snapshot()
                .expect("pending mode gesture");
            assert_eq!(snapshot.value, ParameterValue::Bool(!initial));
            let Some(ParameterControl::Bool { entity, .. }) = view.device_panel.controls.get(&key)
            else {
                panic!("mode control")
            };
            assert_eq!(entity.read(cx).checked(), !initial);
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(contributor)
                    .unwrap()
                    .source_id(),
                Some(source)
            );
            assert_ne!(source, other);
        });
        if mode == hex_devices::utility::parameter::MID_SIDE_ENABLED {
            let selected = if initial { 3 } else { 10 };
            let selected = Box::leak(
                format!("device-{}-parameter-{selected}-value", device.value()).into_boxed_str(),
            );
            let selected_bounds = visual
                .debug_bounds(selected)
                .expect("pending selected control");
            visual.simulate_click(selected_bounds.center(), Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |view, cx| {
                let selected_parameter =
                    hex_lattice_contract::ParameterId::from_static(if initial { 3 } else { 10 });
                let Some(ParameterControl::Float { entity, .. }) =
                    view.device_panel.controls.get(&ParameterKey {
                        device,
                        parameter: selected_parameter,
                    })
                else {
                    panic!("pending numeric control")
                };
                assert!(!entity.read(cx).is_editing());
                assert_eq!(
                    view.device_gesture.snapshot().unwrap().target,
                    hex_session::device_gesture::GestureTarget::DeviceParameter {
                        device,
                        parameter: mode
                    }
                );
            });
        }
    }
    let value = |view: &EditorView| {
        let device = view.state.as_ref()?.project.device(device)?;
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            device.state(),
            mode,
        )
        .ok()
    };
    if reject {
        editor.update(cx, |view, _| {
            view.service
                .shutdown(std::time::Duration::from_secs(2))
                .unwrap()
        });
        pump_until(&editor, cx, |view| {
            view.device_gesture.snapshot().is_none() && view.error.is_some()
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert_eq!(value(view), Some(ParameterValue::Bool(initial)));
            let Some(ParameterControl::Bool { entity, .. }) = view.device_panel.controls.get(&key)
            else {
                panic!("restored mode")
            };
            assert_eq!(entity.read(cx).checked(), initial);
        });
        if mode == hex_devices::utility::parameter::MID_SIDE_ENABLED {
            let original = if initial { 10 } else { 3 };
            let selector = Box::leak(
                format!("device-{}-parameter-{original}-value", device.value()).into_boxed_str(),
            );
            visual
                .debug_bounds(selector)
                .expect("canonical control restored");
        }
        return;
    }
    pump_until(&editor, cx, |view| {
        value(view) == Some(ParameterValue::Bool(!initial))
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(contributor).unwrap().devices(), local);
        assert_eq!(project.track(receiver).unwrap().source_id(), Some(source));
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        value(view) == Some(ParameterValue::Bool(initial))
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        value(view) == Some(ParameterValue::Bool(!initial))
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.select_track(receiver, window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    let receiver_button = visual.debug_bounds(button).expect("mode on receiver");
    visual.simulate_click(receiver_button.center(), Modifiers::default());
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        value(view) == Some(ParameterValue::Bool(initial))
            && view.device_gesture.snapshot().is_none()
    });
}

#[gpui::test]
fn shared_source_channel_selector_keeps_linked_identity_and_history(cx: &mut TestAppContext) {
    use hex_lattice_contract::{DeviceAutomationTarget, ParameterId};
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime};
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_selector_keys);
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    let edit = |command| {
        service.submit(Request::Edit(command)).unwrap();
        let event = receive_service_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let Some(Created::Track(receiver)) = edit(Command::InsertTrackWithDevice {
        name: "receiver".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().unwrap(),
    })
    .created
    else {
        panic!("receiver")
    };
    let added = edit(Command::AddMidiTrack {
        name: "contributor".into(),
    });
    let Some(Created::Track(contributor)) = added.created else {
        panic!("contributor")
    };
    let source = added
        .state
        .project
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let instrument = added
        .state
        .project
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    edit(Command::BindTrackSource {
        track: contributor,
        source: Some(source),
    });
    let utility = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let inserted = edit(Command::InsertNestedDevice {
        parent: instrument,
        state: utility.clone(),
    });
    let Some(Created::Device(effect)) = inserted.created else {
        panic!("effect")
    };
    let local = edit(Command::InsertDevice {
        track: contributor,
        before: None,
        state: utility,
    });
    let local_devices = local
        .state
        .project
        .track(contributor)
        .unwrap()
        .devices()
        .to_vec();
    let (handle, editor) = actor_editor_window(cx, service, local.state, contributor);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let key = ParameterKey {
        device: effect,
        parameter: ParameterId::from_static(4),
    };
    let selector = Box::leak(format!("device-{}-parameter-4", effect.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).expect("channel selector");
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.simulate_keystrokes("down");
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            let snapshot = view
                .device_gesture
                .snapshot()
                .expect("pending channel gesture");
            assert_eq!(snapshot.value, ParameterValue::Choice(1));
            let Some(ParameterControl::Choice { entity, .. }) =
                view.device_panel.controls.get(&key)
            else {
                panic!("channel selector")
            };
            assert_eq!(entity.read(cx).selected(), 1);
        });
    }
    let value = |view: &EditorView| {
        let device = view.state.as_ref()?.project.device(effect)?;
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            device.state(),
            key.parameter,
        )
        .ok()
    };
    pump_until(&editor, cx, |view| {
        value(view) == Some(ParameterValue::Choice(1)) && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(contributor).unwrap().devices(), local_devices);
        assert_eq!(project.track(receiver).unwrap().source_id(), Some(source));
        assert_eq!(
            project.track(contributor).unwrap().source_id(),
            Some(source)
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        value(view) == Some(ParameterValue::Choice(0))
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        value(view) == Some(ParameterValue::Choice(1))
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.select_track(receiver, window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    let bounds = visual
        .debug_bounds(selector)
        .expect("receiver channel selector");
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.simulate_keystrokes("down");
    pump_until(&editor, cx, |view| {
        value(view) == Some(ParameterValue::Choice(2)) && view.device_gesture.snapshot().is_none()
    });
    visual.run_until_parked();
    let bounds = visual.debug_bounds(selector).expect("channel selector");
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.simulate_keystrokes("down");
    pump_until(&editor, cx, |view| {
        value(view) == Some(ParameterValue::Choice(3)) && view.device_gesture.snapshot().is_none()
    });
    editor.update(cx, |view, _| {
        assert!(
            view.enqueue(Request::Edit(Command::ReplaceDeviceAutomationTarget {
                device: effect,
                target: DeviceAutomationTarget::Parameter(hex_devices::utility::parameter::GAIN),
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.5).unwrap(),
                    outgoing: AutomationInterpolation::Step
                }],
            }))
        )
    });
    pump_until(&editor, cx, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(effect)
                .unwrap()
                .automation()
                .len()
                == 1
    });
    visual.run_until_parked();
    let choice = editor.read_with(&visual, |view, _| {
        match view.device_panel.controls.get(&key) {
            Some(ParameterControl::Choice { entity, .. }) => entity.clone(),
            _ => panic!("channel control"),
        }
    });
    choice.update(&mut visual, |_, cx| {
        cx.emit(SelectorChanged {
            index: 0,
            label: "stereo".into(),
        })
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(value(view), Some(ParameterValue::Choice(3)));
        assert!(view.device_gesture.snapshot().is_none());
        assert!(view.error.is_some());
    });
}

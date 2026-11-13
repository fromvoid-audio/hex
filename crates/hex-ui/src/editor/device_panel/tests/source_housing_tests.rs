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

#[gpui::test]
fn shared_source_utility_gain_edits_from_contributor(cx: &mut TestAppContext) {
    for filter in [
        None,
        Some(hex_devices::utility::parameter::DC_REMOVE),
        Some(hex_devices::utility::parameter::BASS_MONO),
    ] {
        shared_source_utility_gain_with_filter(cx, filter);
    }
}

fn shared_source_utility_gain_with_filter(
    cx: &mut TestAppContext,
    filter: Option<hex_lattice_contract::ParameterId>,
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
    let gain = hex_lattice_contract::ParameterId::from_static(1);
    let selector = Box::leak(
        format!("device-{}-parameter-{}-value", device.value(), gain.value()).into_boxed_str(),
    );
    visual.debug_bounds(selector).expect("source Utility gain");
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
    enter_typed_value(&mut visual, device, gain, "6 db");
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
    let six_db = 10.0_f32.powf(6.0 / 20.0);
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
                panic!("pending source gain control")
            };
            assert!((entity.read(cx).value() - f64::from(six_db)).abs() < 0.00001);
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
        |view| matches!(value(view), Some(ParameterValue::Float(n)) if (n - six_db).abs() < 0.00001),
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
        |view| matches!(value(view), Some(ParameterValue::Float(n)) if (n - 1.0).abs() < 0.00001),
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(
        &editor,
        cx,
        |view| matches!(value(view), Some(ParameterValue::Float(n)) if (n - six_db).abs() < 0.00001),
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
        .expect("same Utility gain on receiver");
    editor.read_with(cx, |view, _| {
        assert!(
            matches!(value(view), Some(ParameterValue::Float(n)) if (n - six_db).abs() < 0.00001)
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
        super::super::modulators::source_utility_gain_available(
            editor.project(),
            editor.project().device(device).unwrap(),
        )
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
}

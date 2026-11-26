use super::*;

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
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
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
        let _panel = super::super::view(view, None, cx);
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

#[gpui::test]
fn hovering_a_device_meter_updates_its_tooltip_live_and_ticks_while_decaying(
    cx: &mut TestAppContext,
) {
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
            label: "meter tooltip fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audio_pub.audible_playback = Some(super::super::super::PlaybackCandidate {
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
    visual.run_until_parked();

    let meter_selector =
        Box::leak(format!("device-{}-output-meter", device.value()).into_boxed_str());
    let tooltip_selector = Box::leak(format!("{meter_selector}-tooltip").into_boxed_str());
    let meter_bounds = visual.debug_bounds(meter_selector).expect("device meter");

    // Hover the meter and wait for GPUI's tooltip show delay.
    visual.simulate_mouse_move(meter_bounds.center(), None, Modifiers::default());
    visual.run_until_parked();
    visual
        .executor()
        .advance_clock(std::time::Duration::from_secs(1));
    visual.run_until_parked();

    let quiet_bounds = visual
        .debug_bounds(tooltip_selector)
        .expect("hovered device meter tooltip");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.device_panel.hovered_meter, Some(device));
        assert!(
            !super::super::tick_hovered_meter(view),
            "silent meter needs no decay redraws"
        );
    });

    // Feed live output peaks while the pointer stays stationary: the visible
    // tooltip updates in place (wider than `−∞`) and requests decay ticks.
    meter.observe_output(&[[0.125, 0.0625]]);
    editor.update(&mut visual, |_, cx| cx.notify());
    visual.run_until_parked();

    let active_bounds = visual
        .debug_bounds(tooltip_selector)
        .expect("tooltip remains visible during live peak update");
    assert!(
        active_bounds.size.width > quiet_bounds.size.width,
        "expected tooltip width to grow for live dBFS numbers: quiet={quiet_bounds:?}, active={active_bounds:?}"
    );
    editor.read_with(&visual, |view, _| {
        assert!(
            super::super::tick_hovered_meter(view),
            "hovered decaying meter keeps repainting until it settles"
        );
    });

    // Moving the pointer off the meter stops decay ticks and hides the tooltip.
    visual.simulate_mouse_move(point(px(4.0), px(4.0)), None, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(tooltip_selector).is_none());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.device_panel.hovered_meter, None);
        assert!(!super::super::tick_hovered_meter(view));
    });
}

#[gpui::test]
fn modulation_panel_maps_loaded_sampler_gain_through_its_authored_control(cx: &mut TestAppContext) {
    exercise_sampler_modulation_mapper(cx, false);
}

#[gpui::test]
fn modulation_panel_maps_loaded_sampler_host_output(cx: &mut TestAppContext) {
    exercise_sampler_modulation_mapper(cx, true);
}

#[gpui::test]
fn modulation_panel_maps_limiter_input_gain(cx: &mut TestAppContext) {
    use hex_devices::limiter::{TYPE_ID, parameter::GAIN};
    assert_modulation_mapping(
        cx,
        TYPE_ID,
        "lfo",
        hex_engine::lfo_modulator::TYPE_ID,
        &[GAIN],
    );
}

#[gpui::test]
fn modulation_panel_maps_sparse_eq_output_gain(cx: &mut TestAppContext) {
    use hex_engine::eq::{EQ_TYPE_ID, OUTPUT_GAIN};
    assert_modulation_mapping(
        cx,
        EQ_TYPE_ID,
        "lfo",
        hex_engine::lfo_modulator::TYPE_ID,
        &[OUTPUT_GAIN],
    );
}

#[gpui::test]
fn modulation_panel_omits_note_sources_from_an_effect_without_events(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click_modulator_control(&mut visual, "device-modulator-toggle".into(), false);
    click_modulator_control(
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
            .all(|item| {
                !matches!(
                    item.label,
                    "note counter" | "ramp" | "ahd on release" | "adsr"
                )
            })
    }));
}

#[gpui::test]
fn modulation_panel_adds_and_maps_an_lfo_without_editing_base_values(cx: &mut TestAppContext) {
    use hex_devices::utility::parameter;
    assert_modulation_mapping(
        cx,
        hex_devices::utility::UTILITY_TYPE_ID,
        "lfo",
        hex_engine::lfo_modulator::TYPE_ID,
        &[parameter::GAIN, parameter::WIDTH],
    );
}

#[gpui::test]
fn modulation_panel_drags_signed_depth_and_keeps_released_preview_until_ack(
    cx: &mut TestAppContext,
) {
    use hex_devices::utility::parameter::GAIN;
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let base = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
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
    add_modulator_from_menu(&mut visual, device, "lfo");
    pump_until(&editor, cx, |view| bank(view).sources().len() == 1);
    let (source, revision) = editor.read_with(cx, |view, _| {
        (
            bank(view).sources()[0].id,
            view.state.as_ref().unwrap().revision,
        )
    });
    visual.run_until_parked();
    click_modulator_control(&mut visual, format!("lfo-{}-map", source.value()), true);
    let target: &'static str =
        Box::leak(format!("map-target-{}-{}", device.value(), GAIN.value()).into_boxed_str());
    let selector = |label| -> &'static str {
        Box::leak(format!("map-depth-{}-{}-{label}", device.value(), GAIN.value()).into_boxed_str())
    };
    let negative_depth = selector("-30%");
    let positive_depth = selector("+30%");
    let bounds = visual.debug_bounds(target).unwrap();
    let start = bounds.center();
    let down = point(start.x, start.y + px(30.0));
    assert!(!bounds.contains(&down));
    let modifiers = Modifiers::default();
    visual.simulate_mouse_down(start, MouseButton::Left, modifiers);
    visual.run_until_parked();
    visual.simulate_mouse_move(down, Some(MouseButton::Left), modifiers);
    visual.run_until_parked();
    let preview = visual.debug_bounds(negative_depth).unwrap();
    assert!(editor.read_with(cx, |view, _| bank(view).routes().is_empty()));

    visual.simulate_mouse_up(down, MouseButton::Left, modifiers);
    // Leave the worker terminal queued while completing additional repaints.
    for _ in 0..3 {
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(negative_depth), Some(preview));
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().revision, revision);
            assert!(bank(view).routes().is_empty());
        });
    }
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().revision == revision + 1
    });
    visual.run_until_parked();
    let first = editor.read_with(cx, |view, _| bank(view).routes()[0]);
    assert_eq!(first.amount.get(), -0.3);
    assert_eq!(visual.debug_bounds(negative_depth), Some(preview));

    let start = visual.debug_bounds(target).unwrap().center();
    let up = point(start.x, start.y - px(60.0));
    visual.simulate_mouse_down(start, MouseButton::Left, modifiers);
    visual.run_until_parked();
    visual.simulate_mouse_move(up, Some(MouseButton::Left), modifiers);
    visual.run_until_parked();
    assert!(visual.debug_bounds(positive_depth).is_some());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(up, MouseButton::Left, modifiers);
    visual.run_until_parked();
    assert!(visual.debug_bounds(positive_depth).is_none());
    assert!(editor.read_with(cx, |view, _| !view.edit_pending()));

    let start = visual.debug_bounds(target).unwrap().center();
    let up = point(start.x, start.y - px(60.0));
    visual.simulate_mouse_down(start, MouseButton::Left, modifiers);
    visual.simulate_mouse_move(up, Some(MouseButton::Left), modifiers);
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.device_panel.mapping_source = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(up, MouseButton::Left, modifiers);
    assert!(editor.read_with(cx, |view, _| !view.edit_pending()));
    click_modulator_control(&mut visual, format!("lfo-{}-map", source.value()), true);

    let start = visual.debug_bounds(target).unwrap().center();
    let up = point(start.x, start.y - px(60.0));
    visual.simulate_mouse_down(start, MouseButton::Left, modifiers);
    visual.run_until_parked();
    visual.simulate_mouse_up(up, MouseButton::Left, modifiers);
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().revision == revision + 2
    });
    editor.read_with(cx, |view, _| {
        let bank = bank(view);
        assert_eq!(bank.routes().len(), 1);
        let route = bank.routes()[0];
        assert_eq!(route.id, first.id);
        assert_eq!(route.amount.get(), 0.3);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state(),
            &base
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| bank(view).routes() == [first]);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| bank(view).routes().is_empty());
}

#[gpui::test]
fn modulation_panel_drags_host_input_and_output_depths_independently(cx: &mut TestAppContext) {
    use hex_project::{DeviceAutomationTarget, ModulationDestination};

    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
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
    add_modulator_from_menu(&mut visual, device, "lfo");
    pump_until(&editor, cx, |view| bank(view).sources().len() == 1);
    let source = editor.read_with(cx, |view, _| bank(view).sources()[0].id);
    visual.run_until_parked();
    click_modulator_control(&mut visual, format!("lfo-{}-map", source.value()), true);

    let drag = |visual: &mut VisualTestContext, name, delta| {
        let key = Box::leak(format!("map-{name}-gain-{}", device.value()).into_boxed_str());
        let bounds = visual
            .debug_bounds(key)
            .unwrap_or_else(|| panic!("missing {key}"));
        let start = bounds.center();
        assert!(
            visual
                .debug_bounds("device-modulator-scroll")
                .unwrap()
                .contains(&start)
        );
        let end = point(start.x, start.y + px(delta));
        assert!(!bounds.contains(&end));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    };
    drag(&mut visual, "input", -40.0);
    pump_until(&editor, cx, |view| bank(view).routes().len() == 1);
    visual.run_until_parked();
    drag(&mut visual, "output", 25.0);
    pump_until(&editor, cx, |view| bank(view).routes().len() == 2);
    visual.run_until_parked();
    let first = editor.read_with(cx, |view, _| bank(view));
    for (route, (target, amount)) in first.routes().iter().zip([
        (DeviceAutomationTarget::HostInputGain, 0.4),
        (DeviceAutomationTarget::HostOutputGain, -0.25),
    ]) {
        assert_eq!(route.source, source);
        assert_eq!(route.amount.get(), amount);
        assert!(
            matches!(route.destination, ModulationDestination::Device { target: actual, .. } if actual == target)
        );
    }
    drag(&mut visual, "output", -75.0);
    pump_until(&editor, cx, |view| {
        bank(view).routes()[1].amount.get() == 0.5
    });
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let bank = bank(view);
        assert_eq!(bank.routes()[0], first.routes()[0]);
        assert_eq!(bank.routes()[1].id, first.routes()[1].id);
        assert_eq!(project.device(device).unwrap().state(), &original);
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(&reopened, project.as_ref());
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| bank(view) == first);
}

#[gpui::test]
fn modulation_panel_edits_steps_and_reopens_the_exact_signed_pattern(cx: &mut TestAppContext) {
    use hex_devices::utility::parameter::GAIN;
    use hex_engine::steps_modulator::{Direction, Settings, Timing};

    let (service, state, track, device, _, _) = actor_utility_fixture();
    let base = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
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
    let read = |view: &EditorView| Settings::decode(&bank(view).sources()[0].state).unwrap();
    let click = click_modulator_control;
    click(&mut visual, "device-modulator-toggle".into(), false);
    add_modulator_from_menu(&mut visual, device, "steps");
    pump_until(&editor, cx, |view| bank(view).sources().len() == 1);
    let source = editor.read_with(cx, |view, _| bank(view).sources()[0].id);
    let mut expected = Settings::default();
    assert_eq!(editor.read_with(cx, |view, _| read(view)), expected);
    type Edit = fn(&mut Settings);
    let edits: [(&str, Edit); 5] = [
        ("step time-shorter", |settings| {
            settings.timing = Timing::FreeMicros(62_500)
        }),
        ("direction", |settings| {
            settings.direction = Direction::Backward
        }),
        ("length-more", |settings| settings.pattern.push(0)),
        ("step-1-more", |settings| settings.pattern[0] += 3277),
        ("step-2-less", |settings| settings.pattern[1] -= 3277),
    ];
    visual.run_until_parked();
    for (suffix, edit) in edits {
        click(
            &mut visual,
            format!("steps-{}-{suffix}", source.value()),
            true,
        );
        edit(&mut expected);
        pump_until(&editor, cx, |view| read(view) == expected);
        visual.run_until_parked();
    }
    click(&mut visual, format!("steps-{}-map", source.value()), true);
    click(
        &mut visual,
        format!("map-target-{}-{}", device.value(), GAIN.value()),
        false,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 1);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.device(device).unwrap().state(), &base);
        assert_eq!(read(view), expected);
        assert_eq!(bank(view).routes()[0].source, source);
        assert_eq!(
            hex_storage::schema_version_for(project),
            hex_storage::STEPS_MODULATOR_SCHEMA_VERSION,
        );
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(&reopened, project.as_ref());
    });

    let free = editor.read_with(cx, |view, _| bank(view));
    click(
        &mut visual,
        format!("steps-{}-timing-mode", source.value()),
        true,
    );
    expected.timing = Timing::DEFAULT_ARRANGEMENT;
    pump_until(&editor, cx, |view| read(view) == expected);
    visual.run_until_parked();
    let key = |suffix| Box::leak(format!("steps-{}-{suffix}", source.value()).into_boxed_str());
    assert!(visual.debug_bounds(key("arrangement-time")).is_some());
    assert!(visual.debug_bounds(key("step time-shorter")).is_none());

    click(
        &mut visual,
        format!("steps-{}-division-shorter", source.value()),
        true,
    );
    expected.timing = Timing::Arrangement { steps_per_beat: 8 };
    pump_until(&editor, cx, |view| read(view) == expected);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(bank(view).routes(), free.routes());
        assert_eq!(
            hex_storage::schema_version_for(project),
            hex_storage::ARRANGEMENT_STEPS_SCHEMA_VERSION
        );
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(&reopened, project.as_ref());
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    expected.timing = Timing::DEFAULT_ARRANGEMENT;
    pump_until(&editor, cx, |view| read(view) == expected);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| bank(view) == free);
}

#[gpui::test]
fn modulation_panel_maps_delay_feedback_mix_and_oscillator_without_changing_authored_values(
    cx: &mut TestAppContext,
) {
    use hex_devices::delay::{DRY_WET, FEEDBACK, LFO_DELAY, LFO_RATE, TIME, TYPE_ID};
    use hex_lattice_contract::ParameterId;
    use hex_project::{DeviceAutomationTarget, ModulationDestination};
    let (service, state, track, device) = actor_native_effect_fixture(TYPE_ID);
    let original = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let click = click_modulator_control;
    click(&mut visual, "device-modulator-toggle".into(), false);
    add_modulator_from_menu(&mut visual, device, "lfo");
    let slot = |view: &EditorView| {
        view.state
            .as_ref()?
            .project
            .device(device)?
            .modulation()
            .sources()
            .first()
            .map(|row| row.id)
    };
    pump_until(&editor, cx, |view| slot(view).is_some());
    visual.run_until_parked();
    let source = editor.read_with(cx, |view, _| slot(view).unwrap());
    click(&mut visual, format!("lfo-{}-map", source.value()), true);
    let key = |id: ParameterId| format!("map-target-{}-{}", device.value(), id.value());
    assert!(
        visual
            .debug_bounds(Box::leak(key(TIME).into_boxed_str()))
            .is_none()
    );
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
    assert_eq!(
        editor.read_with(cx, |view, _| view.device_panel.mapping_source),
        Some((
            device,
            super::super::modulators::MappingOutput::main(source)
        ))
    );
    let strip = visual.debug_bounds("device-modulator-panel").unwrap();
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: point(strip.right() + px(5.0), strip.center().y),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-100.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    click(
        &mut visual,
        format!("delay-lfo-expand-{}", device.value()),
        false,
    );
    visual.run_until_parked();
    let targets = [FEEDBACK, DRY_WET, LFO_DELAY, LFO_RATE];
    let body = Box::leak(format!("device-body-{}", device.value()).into_boxed_str());
    let shell = Box::leak(format!("device-shell-{}", device.value()).into_boxed_str());
    let viewport = |visual: &mut VisualTestContext| {
        visual
            .debug_bounds(body)
            .unwrap()
            .intersect(&visual.debug_bounds(shell).unwrap())
    };
    for (index, parameter) in targets.into_iter().enumerate() {
        let selector = Box::leak(key(parameter).into_boxed_str());
        let position = visual.debug_bounds(selector).unwrap().center();
        let visible = viewport(&mut visual);
        if !visible.contains(&position) {
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: visible.center(),
                delta: gpui::ScrollDelta::Pixels(point(px(0.0), visible.center().y - position.y)),
                ..Default::default()
            });
            visual.run_until_parked();
        }
        let bounds = visual.debug_bounds(selector).unwrap();
        let visible = viewport(&mut visual);
        assert!(
            visible.contains(&bounds.center()),
            "{parameter:?} at {bounds:?} outside {visible:?}"
        );
        click(&mut visual, key(parameter), false);
        assert!(
            editor.read_with(cx, |view, _| view.worker_requests_pending > 0
                || !view.pending_requests.is_empty()
                || routes(view).len() == index + 1),
            "Delay parameter {parameter:?} at {bounds:?} did not dispatch",
        );
        pump_until(&editor, cx, |view| routes(view).len() == index + 1);
        visual.run_until_parked();
    }
    editor.read_with(cx, |view, _| {
        let current = view.state.as_ref().unwrap().project.device(device).unwrap();
        assert_eq!(current.state(), &original);
        for (route, id) in routes(view).iter().zip(targets) {
            assert_eq!(route.source, source);
            assert_eq!(
                route.destination,
                ModulationDestination::Device {
                    device_type: TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(id),
                    vst3_class: None,
                }
            );
        }
    });
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
    add_modulator_from_menu(&mut visual, device, "lfo");
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
fn modulation_panel_maps_all_fm4_carriers_through_the_scrolled_instrument_body(
    cx: &mut TestAppContext,
) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::Carriers);
}

#[gpui::test]
fn modulation_panel_maps_unipolar_fm4_pressure_to_one_carrier(cx: &mut TestAppContext) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::Pressure);
}

#[gpui::test]
fn modulation_panel_maps_fm4_oscillator_levels(cx: &mut TestAppContext) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::OscillatorLevels);
}

#[gpui::test]
fn modulation_panel_maps_fm4_note_counter_and_edits_its_settings(cx: &mut TestAppContext) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::Counter);
}

#[gpui::test]
fn modulation_panel_maps_fm4_ramp_and_edits_all_controls(cx: &mut TestAppContext) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::Ramp);
}

#[gpui::test]
fn modulation_panel_maps_all_four_fm4_oscillator_phases(cx: &mut TestAppContext) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::OscillatorPhases);
}

#[gpui::test]
fn modulation_panel_maps_fm4_ahd_release_and_edits_all_controls(cx: &mut TestAppContext) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::AhdRelease);
}

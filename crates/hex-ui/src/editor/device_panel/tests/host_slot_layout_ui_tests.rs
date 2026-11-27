//! Real shared host macro/modulator slot layout, uniform 2/3/4-high sizing,
//! interactive focus/arming/editing, delayed-ACK receipt custody, effective-value
//! feedback, and explicit unsupported source presentation (#3893).
use super::*;

#[gpui::test]
fn real_macro_and_modulator_slots_obey_horizontal_order_uniform_sizes_focus_and_effective_feedback(
    cx: &mut TestAppContext,
) {
    use hex_devices::utility::parameter::GAIN;

    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original_revision = state.revision;
    let original_device_state = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(700.0)));
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

    // 1. Opening both panels changes zero sound/canonical state and preserves sidebar -> macros -> modulators -> body order at 200 px height.
    click_modulator_control(&mut visual, "device-macro-toggle".into(), false);
    click_modulator_control(&mut visual, "device-modulator-toggle".into(), false);
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, original_revision);
        assert_eq!(
            state.project.device(device).unwrap().state(),
            &original_device_state
        );
        assert_eq!(view.worker_requests_pending, 0);
    });
    let header = visual.debug_bounds("device-frame-header").unwrap();
    let macro_panel = visual.debug_bounds("device-macro-panel").unwrap();
    let mod_panel = visual.debug_bounds("device-modulator-panel").unwrap();
    let body = visual
        .debug_bounds(Box::leak(
            format!("device-body-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    assert!(header.right() <= macro_panel.left());
    assert!(macro_panel.right() <= mod_panel.left());
    assert!(mod_panel.right() <= body.left());
    let card = visual
        .debug_bounds(Box::leak(
            format!("device-drop-card-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    assert_eq!(card.size.height, px(DEVICE_FRAME_HEIGHT));

    // 2. Add 2 real macros via macro panel header `+` -> 2-high uniform squares, NOT duplicated in modulator slot grid.
    for count in 1..=2 {
        click_modulator_control(&mut visual, format!("add-macro-{}", device.value()), false);
        pump_until(&editor, cx, |view| bank(view).sources().len() == count);
        visual.run_until_parked();
    }
    let macros: Vec<_> = editor.read_with(cx, |view, _| bank(view).sources().to_vec());
    assert_eq!(macros.len(), 2);
    assert_ne!(
        macros[0].color, macros[1].color,
        "real slots get distinct colors"
    );
    let m1 = visual
        .debug_bounds(Box::leak(
            format!("host-slot-macro-{}", macros[0].id.value()).into_boxed_str(),
        ))
        .expect("macro 1 slot");
    let m2 = visual
        .debug_bounds(Box::leak(
            format!("host-slot-macro-{}", macros[1].id.value()).into_boxed_str(),
        ))
        .expect("macro 2 slot");
    assert_eq!(
        m1.size.width, m1.size.height,
        "2-high macro slot must be square"
    );
    assert_eq!(m1.size, m2.size, "macro slots share one uniform size");
    assert!(
        visual
            .debug_bounds("device-modulator-slots-empty-slot")
            .is_some(),
        "macros must not duplicate into the modulator slot grid"
    );

    // 3. Click macro 1 square slot to arm mod mode + step value via keyboard `up` + drag depth with delayed-ACK receipt (no release flicker) + verify effective-value feedback.
    let m1_action: &'static str =
        Box::leak(format!("host-slot-action-macro-{}", macros[0].id.value()).into_boxed_str());
    let m1_armed: &'static str =
        Box::leak(format!("host-slot-armed-macro-{}", macros[0].id.value()).into_boxed_str());
    let m1_bounds = visual
        .debug_bounds(m1_action)
        .expect("macro 1 interactive overlay");
    visual.simulate_click(m1_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(m1_armed).is_some(),
        "clicked slot arms mod mode"
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("macro-done-{}", device.value()).into_boxed_str()
            ))
            .is_some(),
        "armed macro header exposes done action"
    );

    // Raise macro 1 value to 50% via keyboard up (10 * 5%)? Do 4 steps (20%) to keep test fast!
    for step in 1..=4 {
        visual.simulate_keystrokes("up");
        let expected = step as f32 * 0.05;
        pump_until(&editor, cx, |view| {
            hex_engine::macro_modulator::Settings::decode(&bank(view).sources()[0].state)
                .is_ok_and(|s| (s.value - expected).abs() < 1e-4)
        });
        visual.run_until_parked();
    }

    // Drag signed depth on GAIN target (+50%) and assert delayed-ACK receipt keeps preview across repaints.
    let gain_target: &'static str =
        Box::leak(format!("map-target-{}-{}", device.value(), GAIN.value()).into_boxed_str());
    let depth_badge: &'static str =
        Box::leak(format!("map-depth-{}-{}-+50%", device.value(), GAIN.value()).into_boxed_str());
    let rev_before_drag = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().revision);
    let target_bounds = visual.debug_bounds(gain_target).expect("gain mod target");
    let start = target_bounds.center();
    let up = point(start.x, start.y - px(50.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_move(up, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let preview = visual
        .debug_bounds(depth_badge)
        .expect("live drag depth preview");
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds(depth_badge),
            Some(preview),
            "released preview must stay frozen across repaints until worker ACK"
        );
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().revision, rev_before_drag);
        });
    }
    pump_until(&editor, cx, |view| bank(view).routes().len() == 1);
    visual.run_until_parked();

    // Exit mod mode with Escape while focused on slot; verify actual effective-value feedback (+10% = 20% macro * +50% depth).
    let m1_bounds = visual.debug_bounds(m1_action).unwrap();
    visual.simulate_click(m1_bounds.center(), Modifiers::default()); // disarms & focuses
    visual.run_until_parked();
    assert!(visual.debug_bounds(m1_armed).is_none());
    let eff_selector: &'static str = Box::leak(
        format!(
            "param-mod-effective-{}-{}-+10%",
            device.value(),
            GAIN.value()
        )
        .into_boxed_str(),
    );
    assert!(
        visual.debug_bounds(eff_selector).is_some(),
        "effective modulation feedback must reflect macro 20% * +50% = +10%"
    );

    // 4. Add 5 real LFOs -> verify 3-high at 3 LFOs and 2-column 4-high at 5 LFOs (never mixing sizes).
    let mut lfo_ids = Vec::new();
    for count in 1..=5 {
        add_modulator_from_menu(&mut visual, device, "lfo");
        pump_until(&editor, cx, |view| bank(view).sources().len() == 2 + count);
        visual.run_until_parked();
        let id = editor.read_with(cx, |view, _| bank(view).sources().last().unwrap().id);
        lfo_ids.push(id);
        if count == 3 {
            let s1 = visual
                .debug_bounds(Box::leak(
                    format!("host-slot-mod-{}", lfo_ids[0].value()).into_boxed_str(),
                ))
                .expect("3-high lfo 1");
            let s3 = visual
                .debug_bounds(Box::leak(
                    format!("host-slot-mod-{}", lfo_ids[2].value()).into_boxed_str(),
                ))
                .expect("3-high lfo 3");
            assert_eq!(s1.size.width, s1.size.height);
            assert_eq!(s1.size, s3.size);
            assert!(
                s1.size.width < m1.size.width,
                "3-high slots are smaller than 2-high"
            );
        }
    }
    let f1 = visual
        .debug_bounds(Box::leak(
            format!("host-slot-mod-{}", lfo_ids[0].value()).into_boxed_str(),
        ))
        .expect("4-high lfo 1");
    let f4 = visual
        .debug_bounds(Box::leak(
            format!("host-slot-mod-{}", lfo_ids[3].value()).into_boxed_str(),
        ))
        .expect("4-high lfo 4");
    let f5 = visual
        .debug_bounds(Box::leak(
            format!("host-slot-mod-{}", lfo_ids[4].value()).into_boxed_str(),
        ))
        .expect("4-high lfo 5");
    assert_eq!(f1.size.width, f1.size.height);
    assert_eq!(f1.size, f4.size);
    assert_eq!(
        f4.size, f5.size,
        "5th slot keeps exact uniform 4-high square size"
    );
    assert!(f5.left() > f1.right(), "5th slot starts column 2");
    assert_eq!(
        visual
            .debug_bounds(Box::leak(
                format!("device-drop-card-{}", device.value()).into_boxed_str(),
            ))
            .unwrap()
            .size
            .height,
        px(DEVICE_FRAME_HEIGHT),
        "device host height stays 200 px at every slot column count"
    );

    // 5. Removing focused macro 1 via Backspace removes its route atomically in one undoable edit; Undo restores both.
    let m1_bounds = visual.debug_bounds(m1_action).unwrap();
    visual.simulate_click(m1_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.simulate_keystrokes("backspace");
    pump_until(&editor, cx, |view| bank(view).routes().is_empty());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(eff_selector).is_none(),
        "removing source clears effective modulation feedback"
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        bank(view).routes().len() == 1 && bank(view).sources().len() == 7
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(eff_selector).is_some(),
        "undo restores source, route, and effective feedback atomically"
    );
}

#[gpui::test]
fn unsupported_modulator_family_renders_explicitly_without_decorative_controls(
    cx: &mut TestAppContext,
) {
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    use hex_project::{DeviceModulation, ElementColor, ModulationSourceId, Modulator};

    let (service, state, track, device, _, _) = actor_utility_fixture();
    let unknown_id = ModulationSourceId::new(state.project.next_identity()).unwrap();
    let unknown_state = NativeDeviceState::new(
        DeviceTypeId::from_static("hex.modulator.future"),
        1,
        vec![1],
    )
    .unwrap();
    let bank = DeviceModulation::new(
        vec![Modulator {
            id: unknown_id,
            color: ElementColor::Cyan,
            enabled: true,
            state: unknown_state,
        }],
        Vec::new(),
    )
    .unwrap();
    let mut project_editor = Editor::new((*state.project).clone(), 8).unwrap();
    project_editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: bank,
        })
        .unwrap();
    let patched = Arc::new(ViewState {
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
    let (handle, _editor) = actor_editor_window(cx, service, patched, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1400.0), px(700.0)));
    visual.run_until_parked();

    click_modulator_control(&mut visual, "device-modulator-toggle".into(), false);
    let unsupported_marker: &'static str =
        Box::leak(format!("host-slot-unsupported-mod-{}", unknown_id.value()).into_boxed_str());
    let unsupported_action: &'static str =
        Box::leak(format!("host-slot-action-mod-{}", unknown_id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(unsupported_marker).is_some(),
        "unsupported source family must render explicitly"
    );
    assert!(
        visual.debug_bounds(unsupported_action).is_none(),
        "unsupported source family must not expose fake enabled controls"
    );
}

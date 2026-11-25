//! Hosted VST3 parameter panel UI tests.
use super::render::{follow_display, row_automated};
use super::*;

mod gesture_release;

#[gpui::test]
fn live_pages_retain_unchanged_inventory_and_update_search(cx: &mut gpui::TestAppContext) {
    let (project, track, device) = parameter_project();
    let (_, editor) = crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.device_panel.owner = Some(DevicePanelOwner::Track(track));
        ensure_panel(view, device, cx);
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        let binding = hex_lattice_contract::ForeignProcessorBinding {
            root: 1,
            instance: device.value(),
            graph: 3,
            epoch: 4,
        };
        let signature = [7; 32];
        panel.source = Some(binding);
        panel.descriptor_signature = Some(signature);
        panel.total = Some(1);
        let mut row = layout_row(0.25, true, 1);
        row.name = "filter cutoff".into();
        panel.filter = "filter".into();
        let inventory = Arc::new(vec![row.clone()]);
        panel.result = Some(Arc::clone(&inventory));
        panel.refilter(false);
        let matches = Arc::clone(&panel.matched);
        let query = PluginParameterQuery {
            page_first: Some(0),
            ..panel.query
        };
        let source = AppliedAudioSource {
            context: query.context,
            output_identity: query.output_identity,
            audio_revision: 1,
        };
        let apply_page = |view: &mut EditorView, row, cx: &mut Context<EditorView>| {
            view.device_panel.vst3_parameters.in_flight = Some((query, source));
            apply_result(
                view,
                PluginParameterResult {
                    query,
                    source: Some(binding),
                    descriptor_signature: Some(signature),
                    total: 1,
                    rows: Ok(vec![row]),
                },
                query.context.editor_revision,
                Some(source),
                cx,
            );
        };
        apply_page(view, row.clone(), cx);
        let panel = &view.device_panel.vst3_parameters.panels[&device];
        assert!(Arc::ptr_eq(panel.result.as_ref().unwrap(), &inventory));
        assert!(Arc::ptr_eq(&panel.matched, &matches));

        row.normalized = 0.75;
        apply_page(view, row.clone(), cx);
        let panel = &view.device_panel.vst3_parameters.panels[&device];
        assert_eq!(panel.rows().unwrap(), &[row.clone()]);
        assert_eq!(inventory[0].normalized, 0.25);
        assert!(Arc::ptr_eq(&panel.matched, &matches));

        row.name = "resonance".into();
        apply_page(view, row, cx);
        assert!(
            view.device_panel.vst3_parameters.panels[&device]
                .matched
                .is_empty()
        );
    });
}

#[gpui::test]
fn vst3_refresh_waits_for_an_installed_output(cx: &mut gpui::TestAppContext) {
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let (project, track, device) = parameter_project();
    let (_, editor) = crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "vst3 refresh fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.error = None;
        view.device_panel.owner = Some(DevicePanelOwner::Track(track));
        view.device_panel.active = true;
        ensure_panel(view, device, cx);
        view.device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap()
            .cache_loading = false;
        tick(view, cx);
        assert!(
            !query_pending(view),
            "an open output is not an installed VST3 source"
        );
        assert!(view.error.is_none());

        let query = view.device_panel.vst3_parameters.panels[&device].query;
        let first = AppliedAudioSource {
            context: query.context,
            output_identity: query.output_identity,
            audio_revision: 1,
        };
        let recovered = AppliedAudioSource {
            audio_revision: 2,
            ..first
        };
        let failed = || PluginParameterResult {
            query,
            source: None,
            total: 0,
            descriptor_signature: None,
            rows: Err("vst3 parameter source unavailable".into()),
        };
        view.device_panel.vst3_parameters.in_flight = Some((query, first));
        apply_result(
            view,
            failed(),
            query.context.editor_revision,
            Some(recovered),
            cx,
        );
        assert!(!query_pending(view));
        assert!(
            view.device_panel.vst3_parameters.panels[&device]
                .cache_error
                .is_none()
        );

        view.device_panel.vst3_parameters.in_flight = Some((query, recovered));
        apply_result(
            view,
            failed(),
            query.context.editor_revision,
            Some(recovered),
            cx,
        );
        assert!(report_panel_state(view));
        assert_eq!(
            view.error.as_deref(),
            Some("vst3 parameter source unavailable")
        );
    });
    drop(backend);
    output.publication.close();
}

#[cfg(feature = "ui-test-support")]
#[gpui::test]
fn vst3_header_and_list_bounds_stay_fixed_across_touch_states(cx: &mut gpui::TestAppContext) {
    use crate::editor::{clip_drag_ui_test, device_panel::DevicePanelOwner};
    use gpui::VisualTestContext;
    let (project, track, device) = parameter_project();
    let automation_class = project
        .device(device)
        .unwrap()
        .automation_target_class(hex_project::DeviceAutomationTarget::Vst3 { parameter: 7 })
        .unwrap();
    let (handle, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
    // Exercise the parameter card itself without an audio publication. The
    // full editor intentionally hides this card until a native source exists.
    struct ParameterCard {
        editor: Entity<EditorView>,
        device: DeviceId,
        _observe: Subscription,
    }
    impl gpui::Render for ParameterCard {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.editor.update(cx, |view, cx| {
                let body = body(view, self.device, cx);
                div()
                    .w(px(286.0))
                    .h(px(340.0))
                    .flex()
                    .flex_col()
                    .child(body)
            })
        }
    }
    cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, cx| ParameterCard {
            editor: editor.clone(),
            device,
            _observe: cx.observe(&editor, |_, _, cx| cx.notify()),
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel.owner = Some(DevicePanelOwner::Track(track));
        view.device_panel.selected_device = Some(device);
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        ensure_panel(view, device, cx);
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .expect("panel created from project device");
        panel.cache_loading = false;
        panel.source = Some(binding);
        panel.result = Some(Arc::new(vec![
            PluginParameterRow {
                automation_class,
                ..layout_row(0.2, true, 1)
            },
            PluginParameterRow {
                id: 8,
                automation_class,
                ..layout_row(0.3, true, 1)
            },
        ]));
        panel.matched = Arc::new(vec![0, 1]);
        panel.refilter(false);
        cx.notify();
    });

    let bounds = |visual: &mut VisualTestContext| {
        let header = visual.debug_bounds("vst3-search-row").unwrap();
        let search = visual.debug_bounds("vst3-parameter-search-input").unwrap();
        let touch = visual.debug_bounds("vst3-last-touch-row").unwrap();
        let list = visual.debug_bounds("vst3-parameter-list-area").unwrap();
        let first_row = visual.debug_bounds("vst3-parameter-row-7").unwrap();
        let second_row = visual.debug_bounds("vst3-parameter-row-8").unwrap();
        [header, search, touch, list, first_row, second_row].map(|bounds| {
            [
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ]
        })
    };
    visual.run_until_parked();
    let baseline = bounds(&mut visual);
    let value_bounds = visual.debug_bounds("vst3-last-touch-value").unwrap();
    assert!(
        visual.debug_bounds("vst3-last-touch-label").is_none(),
        "the empty last-touch label must not paint placeholder text"
    );
    for selector in ["vst3-parameter-row-7", "vst3-parameter-row-8"] {
        assert!(
            visual.rendered_border(selector).is_some(),
            "each parameter needs a section"
        );
    }
    assert_eq!(baseline[0][3], 56.0);
    assert_eq!(baseline[2][3], hex_widgets::MICRO_DIAL_HEIGHT,);
    assert!(baseline[4][3] >= hex_widgets::MICRO_DIAL_HEIGHT);
    assert!(baseline[5][3] >= hex_widgets::MICRO_DIAL_HEIGHT);
    assert!(baseline[5][1] >= baseline[4][1] + baseline[4][3] + 4.0);
    assert!(visual.debug_bounds("vst3-last-touched-knob").is_some());
    let placeholder =
        Box::leak(format!("vst3-follow-placeholder-{}-knob", device.value()).into_boxed_str());
    let down = visual
        .debug_bounds(placeholder)
        .expect("disabled follow dial is visible")
        .center();
    let modifiers = gpui::Modifiers::default();
    visual.simulate_mouse_down(down, gpui::MouseButton::Left, modifiers);
    visual.simulate_mouse_move(
        gpui::point(down.x, down.y - gpui::px(20.0)),
        gpui::MouseButton::Left,
        modifiers,
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let control = view
            .device_panel
            .vst3_parameters
            .panels
            .get(&device)
            .unwrap()
            .placeholder_knob
            .as_ref()
            .unwrap()
            .read(cx);
        assert!(
            !control.is_interacting(),
            "an unbound dial cannot capture a gesture"
        );
        assert_eq!(
            control.value(),
            0.5,
            "an unbound dial cannot change its placeholder value"
        );
        assert!(view.device_panel.gesture.snapshot().is_none());
    });
    visual.simulate_mouse_up(down, gpui::MouseButton::Left, modifiers);
    visual.run_until_parked();

    let (revision, before) = editor.update(&mut visual, |view, cx| {
        view.automation_view.enabled = true;
        let state = view.state.as_ref().unwrap();
        cx.notify();
        (state.revision, state.project.clone())
    });
    visual.run_until_parked();
    let click = |visual: &mut VisualTestContext, selector: String| {
        let point = visual
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .unwrap()
            .center();
        visual.simulate_mouse_down(point, gpui::MouseButton::Left, modifiers);
        visual.simulate_mouse_up(point, gpui::MouseButton::Left, modifiers);
        visual.run_until_parked();
    };
    let owner = DevicePanelOwner::Track(track);
    click(&mut visual, format!("vst3-knob-{}-8-knob", device.value()));
    editor.update(&mut visual, |view, cx| {
        assert!(
            view.test_touched_vst3_lane(owner, device, 8),
            "a hosted parameter click selects its lane without a value gesture"
        );
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        assert!(
            panel.last_touched.is_none(),
            "lane selection keeps native touch status"
        );
        panel.last_touched = Some(7);
        cx.notify();
    });
    visual.run_until_parked();
    click(
        &mut visual,
        format!("vst3-follow-knob-{}-7-knob", device.value()),
    );
    editor.update(&mut visual, |view, cx| {
        assert!(view.test_touched_vst3_lane(owner, device, 7));
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, revision);
        assert_eq!(state.project, before);
        assert!(view.device_panel.gesture.snapshot().is_none());
        assert!(view.pending_requests.is_empty());
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        assert_eq!(panel.rows().unwrap()[0].normalized, 0.2);
        assert_eq!(panel.rows().unwrap()[1].normalized, 0.3);
        panel.last_touched = None;
        view.automation_view.enabled = false;
        cx.notify();
    });
    visual.run_until_parked();

    editor.update(&mut visual, |view, _| {
        report_panel_state(view);
        assert_eq!(view.status, "no vst parameter touched");
        let no_touch_status = view.status.clone();
        view.device_panel.vst3_parameters.last_touched_device = Some(device);
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.queued_touch = Some(TouchObservation {
            source: AppliedAudioSource {
                context: panel.query.context,
                audio_revision: 1,
                output_identity: panel.query.output_identity,
            },
            binding,
            parameter: 7,
        });
        report_panel_state(view);
        assert_eq!(view.status, no_touch_status);

        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.queued_touch = None;
        panel.last_touched = Some(7);
        panel.source = Some(binding);
        panel.result = Some(Arc::new(vec![layout_row(0.6, false, 7)]));
        report_panel_state(view);
        assert_eq!(view.status, "last touched vst3 parameter is read only");

        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.cache_error = Some("cache failed".into());
        report_panel_state(view);
        assert!(view.status.starts_with("vst3 parameter refresh failed"));
        assert_eq!(view.error.as_deref(), Some("cache failed"));

        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.cache_error = None;
        panel.pending_touch = Some(TouchObservation {
            source: AppliedAudioSource {
                context: panel.query.context,
                audio_revision: 2,
                output_identity: panel.query.output_identity,
            },
            binding,
            parameter: 7,
        });
        report_panel_state(view);
        assert!(view.status.starts_with("vst3 parameter refresh failed"));
        assert_eq!(view.error.as_deref(), Some("cache failed"));
        view.error = Some("save failed".into());
        view.device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap()
            .cache_error = Some("another read failure".into());
        assert!(report_panel_state(view));
        assert!(
            view.status.contains("another read failure"),
            "the parameter error still reaches status/logging"
        );
        assert_eq!(
            view.error.as_deref(),
            Some("save failed"),
            "background reads cannot replace an unrelated error"
        );
        assert!(
            !report_panel_state(view),
            "unchanged notices do not repaint or spam logs"
        );
    });
    for (row, touched, pending, error) in [
        (layout_row(0.6, false, 7), Some(7), false, None),
        (layout_row(0.7, true, 7), None, true, None),
        (
            layout_row(0.75, true, 7),
            Some(7),
            false,
            Some("cache failed"),
        ),
        (
            layout_row(0.8, true, 7),
            Some(7),
            false,
            Some("vst3 controller is busy · try again"),
        ),
        (layout_row(0.8, true, 7), Some(7), false, None),
    ] {
        editor.update(&mut visual, |view, cx| {
            let panel = view
                .device_panel
                .vst3_parameters
                .panels
                .get_mut(&device)
                .unwrap();
            panel.source = Some(binding);
            panel.descriptor_signature = Some([8; 32]);
            panel.total = Some(1);
            panel.result = Some(Arc::new(vec![
                row,
                PluginParameterRow {
                    id: 8,
                    ..layout_row(0.35, true, 7)
                },
            ]));
            panel.matched = Arc::new(vec![0, 1]);
            panel.last_touched = touched;
            panel.touch_unavailable = false;
            panel.pending_touch = pending.then_some(TouchObservation {
                source: AppliedAudioSource {
                    context: panel.query.context,
                    audio_revision: 1,
                    output_identity: panel.query.output_identity,
                },
                binding,
                parameter: 7,
            });
            panel.cache_error = None;
            panel.retry_count = 0;
            let refresh_due = Instant::now() + PAGE_REFRESH_INTERVAL;
            panel.next_query_at = refresh_due;
            if let Some(error) = error {
                record_refresh_failure(panel, error.into());
                if error == "vst3 controller is busy · try again" {
                    assert_eq!(panel.next_query_at, refresh_due);
                    assert_eq!(panel.retry_count, 0);
                }
            }
            panel.queued_touch = None;
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual), baseline);
        if touched.is_some() {
            assert!(visual.debug_bounds("vst3-last-touch-label").is_some());
        }
        if error == Some("cache failed") {
            assert!(visual.debug_bounds("vst3-parameter-retry").is_some());
        } else {
            assert_eq!(
                visual.debug_bounds("vst3-last-touch-value"),
                Some(value_bounds)
            );
        }
    }
    // A local pointer cannot capture another dial while the prior edit's
    // terminal is outstanding. Deliver the typed ACKs in this headless
    // card without starting a plug-in or publishing audio.
    let (active, other_control) = editor.update(&mut visual, |view, cx| {
        let panel = &view.device_panel.vst3_parameters.panels[&device];
        let origin = parameter_origin(panel.query, &panel.rows().unwrap()[0]).unwrap();
        let other = panel.knobs[&8].control.entity_id();
        let active = view.device_panel.gesture.begin_plugin(origin).unwrap();
        view.device_panel
            .gesture
            .finish_plugin(origin.initial)
            .unwrap();
        cx.notify();
        (active, other)
    });
    visual.run_until_parked();
    let other_selector = Box::leak(format!("vst3-knob-{}-8-knob", device.value()).into_boxed_str());
    let other_dial = visual.debug_bounds(other_selector).unwrap().center();
    visual.simulate_mouse_down(other_dial, gpui::MouseButton::Left, modifiers);
    visual.simulate_mouse_move(
        gpui::point(other_dial.x, other_dial.y - px(20.0)),
        gpui::MouseButton::Left,
        modifiers,
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let knob = &view.device_panel.vst3_parameters.panels[&device].knobs[&8];
        assert_eq!(knob.control.entity_id(), other_control);
        let control = knob.control.read(cx);
        assert!(!control.is_interacting());
        assert_eq!(control.value(), 0.35);
        assert_eq!(view.device_panel.gesture.snapshot().unwrap().key, active);
        assert!(view.device_panel.gesture.has_pending_terminal());
    });
    visual.simulate_mouse_up(other_dial, gpui::MouseButton::Left, modifiers);
    editor.update(&mut visual, |view, cx| {
        use hex_session::device_gesture::{GestureAck, GesturePhase};
        for phase in [GesturePhase::Begin, GesturePhase::Commit] {
            let request = view.device_panel.gesture.next_request().unwrap();
            assert_eq!((request.key(), request.phase()), (active, phase));
            view.device_panel.gesture.mark_submitted();
            assert!(view.device_panel.gesture.acknowledge(&GestureAck {
                preview_retained: false,
                source_lost: false,
                key: active,
                phase,
                sequence: request.sequence(),
                accepted: true,
                reason: None,
            }));
        }
        assert!(view.device_panel.gesture.snapshot().is_none());
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(other_dial, gpui::MouseButton::Left, modifiers);
    editor.read_with(&visual, |view, cx| {
        let knob = &view.device_panel.vst3_parameters.panels[&device].knobs[&8];
        assert_eq!(knob.control.entity_id(), other_control);
        assert!(knob.control.read(cx).is_interacting());
    });
    visual.simulate_mouse_up(other_dial, gpui::MouseButton::Left, modifiers);
    visual.run_until_parked();
    let (previous_control, previous_value) = editor.update(&mut visual, |view, cx| {
        let control = follow_display(view, device, cx).0.unwrap();
        (control.entity_id(), control.read(cx).value())
    });
    editor.update(&mut visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        let touch = TouchObservation {
            source: AppliedAudioSource {
                context: panel.query.context,
                audio_revision: 1,
                output_identity: panel.query.output_identity,
            },
            binding,
            parameter: 8,
        };
        let row = panel
            .rows()
            .unwrap()
            .iter()
            .find(|row| row.id == 8)
            .unwrap()
            .clone();
        queue_touch_refresh(panel, touch, Some(&row));
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("vst3-last-touch-label").is_some(),
        "a pending touch must retain the last validated label across repaints"
    );
    let pending_dial = visual
        .debug_bounds(Box::leak(
            format!("vst3-follow-knob-{}-7-knob", device.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(pending_dial, gpui::MouseButton::Left, modifiers);
    visual.simulate_mouse_move(
        gpui::point(pending_dial.x, pending_dial.y - gpui::px(20.0)),
        gpui::MouseButton::Left,
        modifiers,
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get(&device)
            .unwrap();
        let control = panel.follow_knob.as_ref().unwrap().1.control.read(cx);
        assert!(
            !control.is_interacting(),
            "pending display must not start a gesture"
        );
        assert_eq!(control.value(), previous_value);
        assert!(view.device_panel.gesture.snapshot().is_none());
    });
    visual.simulate_mouse_up(pending_dial, gpui::MouseButton::Left, modifiers);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        let (control, row, _) = follow_display(view, device, cx);
        assert_eq!(control.unwrap().entity_id(), previous_control);
        assert_eq!(row.unwrap().id, 7);
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        let row = panel
            .rows()
            .unwrap()
            .iter()
            .find(|row| row.id == 8)
            .unwrap()
            .clone();
        resolve_pending_touch(panel, binding, &[row], Some(0), true).unwrap();
        assert_eq!(follow_display(view, device, cx).1.unwrap().id, 8);
        // Restore the existing stale-value assertions below.
        view.device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap()
            .last_touched = Some(7);
    });
    let (row_control_before_renewal, follow_control_before_renewal) =
        editor.update(&mut visual, |view, cx| {
            let panel = view
                .device_panel
                .vst3_parameters
                .panels
                .get(&device)
                .unwrap();
            let row = panel
                .rows()
                .unwrap()
                .iter()
                .find(|row| row.id == 7)
                .unwrap()
                .clone();
            let query = panel.query;
            let row = knob(view, device, query, &row, cx).unwrap().entity_id();
            let follow = follow_display(view, device, cx).0.unwrap().entity_id();
            (row, follow)
        });
    editor.update(&mut visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        let known_rows = panel.rows().unwrap().len();
        let unknown = TouchObservation {
            source: AppliedAudioSource {
                context: panel.query.context,
                audio_revision: 1,
                output_identity: panel.query.output_identity,
            },
            binding,
            parameter: 999,
        };
        queue_touch_refresh(panel, unknown, None);
        assert_eq!(
            panel.rows().map(<[_]>::len),
            Some(known_rows),
            "an unknown touch must not blank the parameter inventory"
        );
        assert_eq!(panel.source, Some(binding));
        panel.pending_touch = None;
        panel.mark_values_stale();
        let row = panel
            .rows()
            .unwrap()
            .iter()
            .find(|row| row.id == 7)
            .unwrap();
        assert_eq!(row.normalized, 0.8);
        assert!(!row.editable && row.metadata_revision == 0);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(bounds(&mut visual), baseline);
    editor.read_with(&visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get(&device)
            .unwrap();
        assert_eq!(
            panel.knobs.get(&7).unwrap().control.entity_id(),
            row_control_before_renewal,
            "renewal must retain the rendered row control"
        );
        assert_eq!(
            panel.follow_knob.as_ref().unwrap().1.control.entity_id(),
            follow_control_before_renewal
        );
        assert_eq!(panel.knobs[&7].control.read(cx).value(), 0.8);
        assert_eq!(
            panel
                .follow_knob
                .as_ref()
                .unwrap()
                .1
                .control
                .read(cx)
                .value(),
            0.8
        );
    });
    let stale_dial = visual
        .debug_bounds(Box::leak(
            format!("vst3-knob-{}-7-knob", device.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(stale_dial, gpui::MouseButton::Left, modifiers);
    visual.simulate_mouse_move(
        gpui::point(stale_dial.x, stale_dial.y - px(20.0)),
        gpui::MouseButton::Left,
        modifiers,
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get(&device)
            .unwrap();
        assert!(!panel.knobs[&7].control.read(cx).is_interacting());
        assert_eq!(panel.knobs[&7].control.read(cx).value(), 0.8);
        assert!(view.device_panel.gesture.snapshot().is_none());
    });
    visual.simulate_mouse_up(stale_dial, gpui::MouseButton::Left, modifiers);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        let mut row = panel
            .rows()
            .unwrap()
            .iter()
            .find(|row| row.id == 7)
            .unwrap()
            .clone();
        row.editable = true;
        row.metadata_revision = 9;
        row.automation_class = Some([2; 16]);
        panel.source = Some(binding);
        let query = panel.query;
        let renewed = knob(view, device, query, &row, cx).unwrap();
        assert_eq!(renewed.entity_id(), row_control_before_renewal);
        assert_eq!(
            view.device_panel.vst3_parameters.panels[&device].knobs[&7]
                .origin
                .get()
                .metadata_revision,
            9
        );
        let mut changed_source = binding;
        changed_source.epoch += 1;
        view.device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap()
            .source = Some(changed_source);
        let replaced = knob(view, device, query, &row, cx).unwrap();
        assert_ne!(replaced.entity_id(), renewed.entity_id());
        let mut query = query;
        query.output_identity += 1;
        let new_output = knob(view, device, query, &row, cx).unwrap();
        assert_ne!(new_output.entity_id(), replaced.entity_id());
        row.name = "new descriptor".into();
        let descriptor = knob(view, device, query, &row, cx).unwrap();
        assert_ne!(descriptor.entity_id(), new_output.entity_id());
    });
    editor.update(&mut visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.result = Some(Arc::new(
            (0..40)
                .map(|index| PluginParameterRow {
                    id: 100 + index,
                    page_first: ((index as usize / PAGE_SIZE) * PAGE_SIZE) as u16,
                    ..layout_row(0.35, false, 0)
                })
                .collect(),
        ));
        panel.refilter(true);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel
            .scroll
            .0
            .borrow()
            .base_handle
            .set_offset(gpui::point(px(0.0), px(-1164.0)));
    });
    visual.update(|window, cx| window.draw(cx).clear());
    assert!(visual.debug_bounds("vst3-parameter-row-124").is_some());
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("vst3-placeholder-{}-124-knob", device.value()).into_boxed_str()
            ))
            .is_some(),
        "a newly visible row has its dial in the first frame"
    );
    editor.read_with(&visual, |view, _| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get(&device)
            .unwrap();
        assert!(panel.row_placeholders.len() <= 128);
        assert!(!panel.knobs.contains_key(&124));
    });
    let dial = Box::leak(format!("vst3-placeholder-{}-124-knob", device.value()).into_boxed_str());
    let down = visual.debug_bounds(dial).unwrap().center();
    visual.simulate_mouse_down(down, gpui::MouseButton::Left, modifiers);
    visual.simulate_mouse_move(
        gpui::point(down.x, down.y - px(20.0)),
        gpui::MouseButton::Left,
        modifiers,
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get(&device)
            .unwrap();
        let control = panel.row_placeholders[&124].control.read(cx);
        assert!(!control.is_interacting());
        assert_eq!(control.value(), 0.35);
        assert!(view.device_panel.gesture.snapshot().is_none());
    });
    visual.simulate_mouse_up(down, gpui::MouseButton::Left, modifiers);
    editor.update(&mut visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.source = Some(binding);
        let rows = Arc::make_mut(panel.result.as_mut().unwrap());
        rows[24] = PluginParameterRow {
            id: 124,
            page_first: rows[24].page_first,
            ..layout_row(0.35, true, 7)
        };
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("vst3-knob-{}-124-knob", device.value()).into_boxed_str()
            ))
            .is_some()
    );
    editor.read_with(&visual, |view, _| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get(&device)
            .unwrap();
        assert!(!panel.row_placeholders.contains_key(&124));
        assert!(panel.knobs.contains_key(&124));
    });
    editor.update(&mut visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.result = Some(Arc::new(
            (0..40)
                .map(|index| PluginParameterRow {
                    id: 100 + index,
                    page_first: ((index as usize / PAGE_SIZE) * PAGE_SIZE) as u16,
                    ..layout_row(0.35, false, 0)
                })
                .collect(),
        ));
        panel.refilter(true);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("vst3-parameter-scrollbar").is_some(),
        "a long parameter list has a scrollbar"
    );
    let geometry = |visual: &VisualTestContext| {
        editor.read_with(visual, |view, _| {
            view.device_panel.vst3_parameters.panels[&device]
                .scrollbar
                .test_geometry()
                .unwrap()
        })
    };
    let (track, thumb, maximum) = geometry(&visual);
    assert!(maximum > 0.0);
    assert_eq!(thumb.top(), track.top());
    let row = visual.debug_bounds("vst3-parameter-row-100").unwrap();
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: row.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-150.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let (track, thumb, _) = geometry(&visual);
    assert!(thumb.top() > track.top());
    visual.simulate_mouse_down(thumb.center(), gpui::MouseButton::Left, modifiers);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.mark_values_stale();
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(geometry(&visual).1, thumb);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.device_panel.vst3_parameters.panels[&device]
                .scrollbar
                .test_dragging(),
            "value refresh must retain the scrollbar drag"
        )
    });
    let outside = gpui::point(track.left(), track.bottom() + px(300.0));
    visual.simulate_mouse_move(outside, gpui::MouseButton::Left, modifiers);
    visual.simulate_mouse_up(outside, gpui::MouseButton::Left, modifiers);
    visual.run_until_parked();
    let (track, thumb, _) = geometry(&visual);
    assert!((f32::from(thumb.bottom() - track.bottom())).abs() < 0.01);
    editor.read_with(&visual, |view, _| {
        assert!(
            !view.device_panel.vst3_parameters.panels[&device]
                .scrollbar
                .test_dragging()
        );
        assert!(view.device_panel.gesture.snapshot().is_none());
    });
    visual.simulate_mouse_down(thumb.center(), gpui::MouseButton::Left, modifiers);
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        assert!(panel.scrollbar.test_dragging());
        panel.filter = "no matching parameter".into();
        panel.refilter(true);
        assert!(!panel.scrollbar.test_dragging());
        cx.notify();
    });
    visual.simulate_mouse_move(
        gpui::point(track.left(), track.top()),
        gpui::MouseButton::Left,
        modifiers,
    );
    visual.simulate_mouse_up(
        gpui::point(track.left(), track.top()),
        gpui::MouseButton::Left,
        modifiers,
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            !view.device_panel.vst3_parameters.panels[&device]
                .scrollbar
                .test_dragging()
        );
        assert!(view.device_panel.gesture.snapshot().is_none());
    });
}

#[cfg(feature = "ui-test-support")]
#[gpui::test]
fn repeated_native_performs_keep_the_inflight_touch_and_established_target(
    cx: &mut gpui::TestAppContext,
) {
    use crate::editor::{clip_drag_ui_test, device_panel::DevicePanelOwner};
    let (project, track, device) = parameter_project();
    let (_, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.device_panel.owner = Some(DevicePanelOwner::Track(track));
        ensure_panel(view, device, cx);
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.cache_loading = false;
        panel.query.output_identity = 7;
        let binding = hex_lattice_contract::ForeignProcessorBinding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let touch = TouchObservation {
            source: AppliedAudioSource {
                context: panel.query.context,
                audio_revision: 1,
                output_identity: 7,
            },
            binding,
            parameter: 7,
        };
        let row = layout_row(0.2, true, 7);
        panel.source = Some(binding);
        panel.result = Some(Arc::new(vec![row.clone()]));
        queue_touch_refresh(panel, touch, Some(&row));
        panel.pending_touch_request = Some(9);
        for _ in 0..8 {
            queue_native_touch(panel, touch);
            assert_eq!(panel.pending_touch_request, Some(9));
            assert_eq!(panel.queued_touch, None);
        }
        let intermediate = layout_row(0.4, true, 7);
        let is_touch_query = panel.pending_touch_request == Some(9);
        let (_, observed) = resolve_pending_touch(
            panel,
            binding,
            std::slice::from_ref(&intermediate),
            Some(0),
            is_touch_query,
        )
        .expect("an in-flight page can resolve before the touch stream ends");
        assert_eq!(observed.normalized, 0.4);
        panel.result = Some(Arc::new(vec![intermediate]));
        let refresh_due = Instant::now() + PAGE_REFRESH_INTERVAL;
        panel.next_query_at = refresh_due;
        for _ in 0..8 {
            queue_native_touch(panel, touch);
            assert_eq!(panel.last_touched, Some(7));
            assert_eq!(panel.pending_touch, None);
            assert_eq!(panel.queued_touch, None);
            assert_eq!(panel.priority_page, Some(0));
            assert_eq!(panel.next_query_at, refresh_due);
        }
        // A main-lane page may observe the latest native edit before the
        // processing queue delivers an older Perform for the same parameter.
        let mut row = panel.rows().unwrap()[0].clone();
        row.editor_sequence = 10;
        row.value = "current".into();
        panel.result = Some(Arc::new(vec![row.clone()]));
        panel.observe_value(touch, 2, Some(PluginParameterValue::new(0.25).unwrap()));
        assert_eq!(panel.display_value(&row), 0.4);
        // The next Perform leads the page. An older receipt or a lagging
        // page must not undo it or flicker the SDK's formatted text.
        panel.observe_value(touch, 12, Some(PluginParameterValue::new(0.7).unwrap()));
        panel.observe_value(touch, 11, Some(PluginParameterValue::new(0.55).unwrap()));
        assert_eq!(panel.display_value(&row), 0.7);
        assert_eq!(
            parameter_origin(panel.query, &row)
                .unwrap()
                .initial
                .normalized(),
            0.4
        );
        let lagging = PluginParameterRow {
            value: "earlier".into(),
            normalized: 0.55,
            editor_sequence: 11,
            ..row.clone()
        };
        panel.reconcile_live_values(binding, std::slice::from_ref(&lagging));
        assert_eq!(panel.display_value(&lagging), 0.7);
        assert_eq!(panel.display_text(&lagging), "current");
        let current = PluginParameterRow {
            value: "formatted".into(),
            normalized: 0.7,
            editor_sequence: 12,
            ..row.clone()
        };
        panel.reconcile_live_values(binding, std::slice::from_ref(&current));
        assert_eq!(panel.display_value(&current), 0.7);
        assert_eq!(panel.display_text(&current), "formatted");
        panel.observe_value(touch, 13, Some(PluginParameterValue::new(0.8).unwrap()));
        panel.source = Some(hex_lattice_contract::ForeignProcessorBinding {
            epoch: 5,
            ..binding
        });
        assert_eq!(panel.display_value(&row), 0.4);
        panel.source = Some(binding);
        panel.live_values.clear();

        let query = panel.query;
        panel.observe_value(touch, 14, Some(PluginParameterValue::new(0.9).unwrap()));
        let control = knob(view, device, query, &row, cx).unwrap();
        assert_eq!(control.read(cx).value(), 0.9);
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        assert_eq!(panel.knobs[&row.id].origin.get().initial.normalized(), 0.4);

        // Different raw IDs and source identities must restart validation.
        let other = TouchObservation {
            parameter: 0,
            ..touch
        };
        queue_native_touch(panel, other);
        assert_eq!(panel.queued_touch, Some(other));
        queue_native_touch(panel, touch);
        assert_eq!(panel.queued_touch, None);
        let replacement = TouchObservation {
            binding: hex_lattice_contract::ForeignProcessorBinding {
                instance: 8,
                ..binding
            },
            ..touch
        };
        queue_native_touch(panel, replacement);
        assert_eq!(panel.queued_touch, Some(replacement));
        queue_touch_refresh(panel, replacement, Some(&row));
        panel.pending_touch_request = Some(10);
        assert!(resolve_pending_touch(panel, binding, &[row], Some(0), true).is_none());
        assert_eq!(panel.last_touched, None);
    });
}

#[cfg(feature = "ui-test-support")]
#[gpui::test]
fn vst3_modulation_maps_only_fresh_continuous_rows_and_keeps_signed_pending_depth(
    cx: &mut gpui::TestAppContext,
) {
    use crate::editor::{clip_drag_ui_test, device_panel::DevicePanelOwner};
    use gpui::{Modifiers, MouseButton, VisualTestContext, point};
    use hex_project::{
        Command, DeviceAutomationTarget, DeviceModulation, Editor, ModulationDestination,
        ModulationSourceId, Modulator,
    };

    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let root = client.output_identity().unwrap();
    let (project, track, device) = parameter_project();
    let source = ModulationSourceId::new(project.next_identity()).unwrap();
    let mut project = Editor::new(project, 8).unwrap();
    project
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: DeviceModulation::new(
                vec![Modulator {
                    id: source,
                    color: Default::default(),
                    enabled: true,
                    state: hex_engine::lfo_modulator::Settings::default()
                        .encode()
                        .unwrap(),
                }],
                Vec::new(),
            )
            .unwrap(),
        })
        .unwrap();
    let (handle, editor) =
        clip_drag_ui_test::editor_window(cx, project.project().clone(), Arc::default());
    struct Card {
        editor: Entity<EditorView>,
        device: DeviceId,
        _observe: Subscription,
    }
    impl gpui::Render for Card {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.editor.update(cx, |view, cx| {
                let device = view
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(self.device)
                    .unwrap()
                    .clone();
                let body = body(view, self.device, cx);
                let panel = super::super::modulators::vst3_panel(view, cx, &device);
                hex_widgets::device_frame_with_modulators(
                    theme::text("vst3"),
                    theme::device_frame_style(true),
                    div(),
                    div(),
                    body,
                    panel,
                )
                .w(px(300.0))
                .h(px(360.0))
            })
        }
    }
    cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, cx| Card {
            editor: editor.clone(),
            device,
            _observe: cx.observe(&editor, |_, _, cx| cx.notify()),
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "headless".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.selected_track = Some(track);
        view.device_panel.owner = Some(DevicePanelOwner::Track(track));
        view.device_panel.selected_device = Some(device);
        view.device_panel.active = true;
        view.audio_detail.clip_view_open = true;
        ensure_panel(view, device, cx);
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.cache_loading = false;
        panel.source = Some(hex_lattice_contract::ForeignProcessorBinding {
            root,
            instance: device.value(),
            graph: 1,
            epoch: 1,
        });
        panel.query.output_identity = root;
        let live = PluginParameterRow {
            id: 0,
            automation_class: Some([2; 16]),
            ..layout_row(0.25, true, 1)
        };
        panel.result = Some(Arc::new(vec![
            live.clone(),
            PluginParameterRow {
                id: 8,
                steps: 1,
                ..live.clone()
            },
            PluginParameterRow {
                id: 9,
                metadata_revision: 0,
                editor_sequence: 0,
                ..live
            },
        ]));
        panel.matched = Arc::new(vec![0, 1, 2]);
        cx.notify();
    });
    editor.update(&mut visual, |view, _| {
        let current = {
            let panel = &view.device_panel.vst3_parameters.panels[&device];
            mapping_target(view, device, &panel.rows().unwrap()[0]).unwrap()
        };
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        let original = panel.source.unwrap();
        panel.source = Some(hex_lattice_contract::ForeignProcessorBinding {
            epoch: original.epoch + 1,
            ..original
        });
        assert!(!mapping_target_is_current(view, device, current));
        view.device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap()
            .source = Some(original);
    });
    let click = |visual: &mut VisualTestContext, key: &'static str| {
        let center = visual.debug_bounds(key).unwrap().center();
        visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    };
    visual.run_until_parked();
    click(&mut visual, "device-modulator-toggle");
    click(
        &mut visual,
        Box::leak(format!("lfo-{}-map", source.value()).into_boxed_str()),
    );
    let key = |suffix| -> &'static str {
        Box::leak(format!("map-vst3-row-{}-{suffix}", device.value()).into_boxed_str())
    };
    let target = visual.debug_bounds(key(0)).unwrap().center();
    assert!(visual.debug_bounds(key(8)).is_none());
    assert!(visual.debug_bounds(key(9)).is_none());
    let down = point(target.x, target.y + px(30.0));
    visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(down, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let depth = Box::leak(format!("map-vst3-row-depth-{}-0--30%", device.value()).into_boxed_str());
    assert!(visual.debug_bounds(depth).is_some());
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        let bank = view
            .device_panel
            .modulation_target_drag
            .pending_bank_for_test()
            .unwrap();
        assert_eq!(bank.routes()[0].amount.get(), -0.3);
        assert_eq!(
            bank.routes()[0].destination,
            ModulationDestination::Device {
                device_type: hex_lattice_contract::VST3_DEVICE_TYPE_ID,
                target: DeviceAutomationTarget::Vst3 { parameter: 0 },
                vst3_class: Some([2; 16]),
            }
        );
    });
    editor.update(&mut visual, |view, _| view.audio = None);
    backend.close();
}

#[test]
fn hosted_automation_indicator_requires_authored_points_and_the_current_class() {
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime,
        DeviceAutomationTarget, Editor,
    };

    let (project, _, device) = parameter_project();
    let mut editor = Editor::new(project, 8).unwrap();
    let row = PluginParameterRow {
        automation_class: Some([2; 16]),
        ..layout_row(0.25, true, 1)
    };
    assert!(!row_automated(editor.project().device(device), &row));

    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Vst3 { parameter: row.id },
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Linear,
            }],
        })
        .unwrap();

    let canonical = editor.project().device(device);
    assert!(row_automated(canonical, &row));
    for automation_class in [None, Some([3; 16])] {
        let stale_row = PluginParameterRow {
            automation_class,
            ..row.clone()
        };
        assert!(!row_automated(canonical, &stale_row));
    }
}

#[cfg(feature = "ui-test-support")]
fn parameter_project() -> (hex_project::Project, hex_project::TrackId, DeviceId) {
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    use hex_project::{Command, Created, Editor, Project};
    let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = model
        .execute(Command::AddMidiTrack {
            name: "owner".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut payload = vec![0; 96];
    payload[..4].copy_from_slice(b"HXVS");
    payload[4] = 1;
    payload[8..24].fill(2);
    payload[24..88].fill(3);
    let state = NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, payload).unwrap();
    let Created::Device(device) = model
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    (model.project().clone(), track, device)
}

#[cfg(feature = "ui-test-support")]
fn layout_row(normalized: f64, editable: bool, metadata_revision: u64) -> PluginParameterRow {
    PluginParameterRow {
        id: 7,
        name: "a long parameter name that stays in its slot".into(),
        value: format!("{normalized:.3}"),
        normalized,
        default_normalized: 0.2,
        steps: 0,
        editable,
        automation_class: Some([6; 16]).filter(|_| editable),
        page_first: 0,
        metadata_revision,
        editor_sequence: 0,
    }
}

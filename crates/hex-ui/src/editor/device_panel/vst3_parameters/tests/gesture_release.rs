//! A real numeric pointer release must reach the edit admitted before a refresh.
use super::*;
use gpui::{AppContext, Modifiers, MouseButton, VisualTestContext};
use hex_session::device_gesture::{GestureAck, GesturePhase};

struct Card {
    editor: Entity<EditorView>,
    device: DeviceId,
    _observe: Subscription,
}

impl gpui::Render for Card {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (focus, body) = self.editor.update(cx, |view, cx| {
            let body = if view.audio_pub.stopped_playback_restore.is_some() {
                let device = view
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(self.device)
                    .unwrap()
                    .clone();
                super::super::super::vst3_status_body(view, &device, cx).into_any_element()
            } else {
                body(view, self.device, cx).into_any_element()
            };
            (view.focus.clone(), body)
        });
        let editor = self.editor.downgrade();
        div()
            .track_focus(&focus)
            .on_key_down(move |event, window, cx| {
                let _ = editor.update(cx, |view, cx| {
                    if view.arrangement_view_key(event, window, cx) {
                        cx.stop_propagation();
                    }
                });
            })
            .w(px(286.0))
            .h(px(340.0))
            .flex()
            .flex_col()
            .child(body)
    }
}

#[gpui::test]
fn vst3_knob_release_after_refresh_unblocks_automation(cx: &mut gpui::TestAppContext) {
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, _backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let root = client.output_identity().unwrap();
    let (project, track, device) = parameter_project();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, cx| Card {
            editor: editor.clone(),
            device,
            _observe: cx.observe(&editor, |_, _, cx| cx.notify()),
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root,
        instance: device.value(),
        graph: 1,
        epoch: 1,
    };
    let row = PluginParameterRow {
        automation_class: Some([2; 16]),
        ..layout_row(0.25, true, 1)
    };
    let install_row = |view: &mut EditorView| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.cache_loading = false;
        panel.source = Some(binding);
        panel.result = Some(Arc::new(vec![row.clone()]));
        panel.refilter(false);
    };
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
        install_row(view);
        cx.notify();
    });
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    let selector = Box::leak(format!("vst3-knob-{}-7-knob", device.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    let release = gpui::point(down.x + px(8.0), down.y);
    let mods = Modifiers::default();
    visual.simulate_mouse_down(down, MouseButton::Left, mods);
    visual.simulate_mouse_move(release, MouseButton::Left, mods);
    let key = editor.update(&mut visual, |view, _| {
        let key = view
            .device_panel
            .gesture
            .snapshot()
            .expect("admitted pointer")
            .key;
        refresh(view);
        key
    });
    visual.simulate_mouse_up(release, MouseButton::Left, mods);
    editor.update(&mut visual, |view, cx| {
        let acknowledge = |phase, sequence| GestureAck {
            key,
            phase,
            sequence,
            accepted: true,
            preview_retained: false,
            source_lost: false,
            reason: None,
        };
        assert!(
            view.device_panel
                .gesture
                .acknowledge(&acknowledge(GesturePhase::Begin, 0))
        );
        let terminal = view
            .device_panel
            .gesture
            .next_request()
            .expect("mouse release must finish the edit");
        assert_eq!(
            (terminal.key(), terminal.phase()),
            (key, GesturePhase::Commit)
        );
        view.device_panel.gesture.mark_submitted();
        assert!(
            view.device_panel
                .gesture
                .acknowledge(&acknowledge(GesturePhase::Commit, terminal.sequence()))
        );
        view.worker_requests_pending = 0;
        install_row(view);
        cx.notify();
    });
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    visual.simulate_keystrokes("a");
    editor.read_with(&visual, |view, _| assert!(view.automation_view.enabled));
    let row = visual.debug_bounds("vst3-parameter-row-7").unwrap();
    let label = gpui::point(row.left() + px(4.0), row.center().y);
    visual.simulate_mouse_down(label, MouseButton::Right, mods);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .any(|item| item.label == "show automation")
        );
    });
}

#[gpui::test]
fn typed_vst3_value_reaches_the_matching_gesture_terminal(cx: &mut gpui::TestAppContext) {
    typed_value_case(cx, true, false, false);
}

#[gpui::test]
fn refused_typed_vst3_value_returns_to_the_observed_value(cx: &mut gpui::TestAppContext) {
    typed_value_case(cx, false, false, false);
}

#[gpui::test]
fn typed_vst3_value_survives_page_refresh_before_terminal(cx: &mut gpui::TestAppContext) {
    typed_value_case(cx, true, true, false);
}

#[gpui::test]
fn replaced_vst3_source_never_inherits_old_gesture_preview(cx: &mut gpui::TestAppContext) {
    typed_value_case(cx, true, false, true);
}

fn typed_value_case(
    cx: &mut gpui::TestAppContext,
    accepted: bool,
    refresh_before_ack: bool,
    replace_before_ack: bool,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, _backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let root = client.output_identity().unwrap();
    let (project, track, device) = parameter_project();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, cx| Card {
            editor: editor.clone(),
            device,
            _observe: cx.observe(&editor, |_, _, cx| cx.notify()),
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root,
        instance: device.value(),
        graph: 1,
        epoch: 1,
    };
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
        panel.source = Some(binding);
        panel.result = Some(Arc::new(vec![
            layout_row(0.25, true, 1),
            PluginParameterRow {
                id: 8,
                ..layout_row(0.6, true, 1)
            },
        ]));
        panel.refilter(false);
        cx.notify();
    });
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let untouched = Box::leak(format!("vst3-knob-{}-8-knob", device.value()).into_boxed_str());
    let untouched_bounds = visual.debug_bounds(untouched).expect("untouched dial");
    let down = untouched_bounds.center();
    let modifiers = Modifiers::default();
    visual.simulate_mouse_move(down, None, modifiers);
    visual.run_until_parked();
    let available_cursor = Some(gpui::CursorStyle::PointingHand);
    assert_eq!(visual.rendered_cursor_style(), available_cursor);
    let selector = Box::leak(format!("vst3-knob-{}-7-value", device.value()).into_boxed_str());
    // This headless fixture has no installed SDK source. Exercise the gesture
    // subscription independently; the hosted text path has its own SDK test.
    editor.update(&mut visual, |view, cx| {
        view.device_panel.vst3_parameters.panels[&device].knobs[&7]
            .control
            .as_numeric()
            .unwrap()
            .update(cx, |control, cx| control.set_deferred_text(false, cx));
    });
    let value = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(value, Modifiers::default());
    editor.read_with(&visual, |view, cx| {
        let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
            .control
            .as_numeric()
            .unwrap()
            .read(cx);
        assert!(control.is_editing(), "value field must start a text edit");
    });
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.75");
    editor.read_with(&visual, |view, cx| {
        let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
            .control
            .as_numeric()
            .unwrap()
            .read(cx);
        assert_eq!(control.display_text(), "0.75");
    });
    visual.simulate_keystrokes("enter");
    editor.read_with(&visual, |view, cx| {
        let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
            .control
            .as_numeric()
            .unwrap()
            .read(cx);
        assert!(!control.is_editing(), "enter must commit the numeric field");
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
                .control
                .as_numeric()
                .unwrap()
                .read(cx);
            assert_eq!(
                control.value(),
                0.75,
                "pending text edit must remain visible"
            );
            assert!(view.device_panel.gesture.has_pending_terminal());
        });
    }
    if refresh_before_ack {
        editor.update(&mut visual, |view, cx| {
            refresh(view);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
                .control
                .as_numeric()
                .unwrap()
                .read(cx);
            assert_eq!(
                control.value(),
                0.75,
                "page refresh must retain the admitted preview"
            );
        });
    }
    if replace_before_ack {
        editor.update(&mut visual, |view, cx| {
            let panel = view
                .device_panel
                .vst3_parameters
                .panels
                .get_mut(&device)
                .unwrap();
            panel.source = Some(hex_lattice_contract::ForeignProcessorBinding {
                epoch: binding.epoch + 1,
                ..binding
            });
            panel.result = Some(Arc::new(vec![layout_row(0.33, true, 2)]));
            panel.refilter(false);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
                .control
                .as_numeric()
                .unwrap()
                .read(cx);
            assert_eq!(
                control.value(),
                0.33,
                "replacement must not inherit the old preview"
            );
        });
    }
    editor.update(&mut visual, |view, cx| {
        let panel = &view.device_panel.vst3_parameters.panels[&device];
        assert_eq!(current_context(view), Some(panel.query.context), "current context");
        let snapshot = view.device_panel.gesture.snapshot().expect("typed value began a gesture");
        assert!(view.device_panel.gesture.has_pending_terminal(), "typed value must finish its admitted gesture");
        assert_eq!(snapshot.plugin_value.unwrap().normalized(), 0.75);
        assert!(view.device_panel.gesture.acknowledge(&GestureAck {
            key: snapshot.key, phase: GesturePhase::Begin, sequence: 0, accepted: true,
            preview_retained: false, source_lost: false, reason: None,
        }));
        let terminal = view.device_panel.gesture.next_request().expect("typed value terminal");
        assert!(matches!(terminal, hex_session::device_gesture::GestureRequest::CommitPlugin { key, value, .. } if key == snapshot.key && value.normalized() == 0.75));
        view.device_panel.gesture.mark_submitted();
        let mut wrong_key = snapshot.key;
        wrong_key.token += 1;
        assert!(!view.device_panel.gesture.acknowledge(&GestureAck {
            key: wrong_key, phase: GesturePhase::Commit, sequence: terminal.sequence(), accepted: true,
            preview_retained: false, source_lost: false, reason: None,
        }));
        assert!(view.device_panel.gesture.has_pending_terminal());
        assert!(view.acknowledge_device_gesture(&GestureAck {
            key: snapshot.key, phase: GesturePhase::Commit, sequence: terminal.sequence(), accepted,
            preview_retained: false, source_lost: false, reason: None,
        }));
        let panel = &view.device_panel.vst3_parameters.panels[&device];
        assert!(panel.source.is_none(), "the old page cannot authorize another edit");
        assert_eq!(panel.rows().unwrap()[0].normalized, if replace_before_ack { 0.33 } else if accepted { 0.75 } else { 0.25 });
        cx.notify();
    });
    visual.run_until_parked();
    if !replace_before_ack {
        assert_eq!(visual.debug_bounds(untouched), Some(untouched_bounds));
        // The dial's hover cursor and paint use the same presentation state.
        visual.simulate_mouse_move(down, None, modifiers);
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            available_cursor,
            "an unrelated dial must not dim after the terminal"
        );
        let end = gpui::point(down.x + px(10.0), down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(end, MouseButton::Left, modifiers);
        visual.simulate_mouse_up(end, MouseButton::Left, modifiers);
        editor.read_with(&visual, |view, cx| {
            let control = view.device_panel.vst3_parameters.panels[&device].knobs[&8]
                .control
                .as_numeric()
                .unwrap()
                .read(cx);
            assert!(!control.is_interacting());
            assert_eq!(control.value(), 0.6);
            assert!(view.device_panel.gesture.snapshot().is_none());
        });
        editor.update(&mut visual, |view, cx| {
            view.audio_pub.stopped_playback_restore =
                Some(crate::editor::audio_admission::StoppedPlaybackRestore::Preparing);
            cx.notify();
        });
        visual.run_until_parked();
        let pending = Box::leak(format!("device-vst3-pending-{}", device.value()).into_boxed_str());
        assert!(visual.debug_bounds(pending).is_some());
        assert_eq!(visual.debug_bounds(untouched), Some(untouched_bounds));
        visual.simulate_mouse_move(down, None, modifiers);
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), available_cursor);
    }
    editor.read_with(&visual, |view, cx| {
        let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
            .control
            .as_numeric()
            .unwrap()
            .read(cx);
        assert_eq!(
            control.value(),
            if replace_before_ack {
                0.33
            } else if accepted {
                0.75
            } else {
                0.25
            },
            "terminal outcome must survive the page refresh"
        );
    });
}

#[gpui::test]
fn vst3_knob_text_draft_conversion_and_plugin_gesture_protect_fold_finalize_and_mount(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, _backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let root = client.output_identity().unwrap();
    let (project, track, device) = parameter_project();
    let other_device = {
        let mut scratch = hex_project::Editor::new(project.clone(), 8).unwrap();
        let utility = hex_engine::default_native_device_state(
            hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let hex_project::Created::Device(other) = scratch
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: utility,
            })
            .unwrap()
        else {
            panic!("utility");
        };
        other
    };
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, cx| Card {
            editor: editor.clone(),
            device,
            _observe: cx.observe(&editor, |_, _, cx| cx.notify()),
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root,
        instance: device.value(),
        graph: 1,
        epoch: 1,
    };
    let row = PluginParameterRow {
        automation_class: Some([2; 16]),
        ..layout_row(0.25, true, 1)
    };
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
        panel.source = Some(binding);
        panel.result = Some(Arc::new(vec![row.clone()]));
        panel.refilter(false);
        // Simulate a long chain where `device` sits far past the viewport + overscan.
        let mut geometry = vec![(other_device, 286.0); 20];
        geometry.push((device, 286.0));
        view.device_panel.chain_window.prepare(&geometry, None);
        cx.notify();
    });
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    // Baseline: offscreen idle card does not report pending draft or forced mount.
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.device_panel.has_pending_draft(cx));
            assert!(super::super::super::finalize_controls(view, cx));
            assert!(!super::super::super::chain_window::must_mount(
                view,
                device,
                Some(window),
                cx,
            ));
        });
    });

    // 1. Entering a text edit on a VST3 parameter knob blocks fold, blocks finalize_controls,
    //    sets has_pending_draft, and pins chain_window::must_mount.
    let selector = Box::leak(format!("vst3-knob-{}-7-value", device.value()).into_boxed_str());
    let value_center = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(value_center, Modifiers::default());
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
                .control
                .as_numeric()
                .unwrap()
                .clone();
            assert!(control.read(cx).is_editing(), "vst3 knob must be editing");
            assert!(view.device_panel.has_pending_draft(cx));
            assert!(
                !super::super::super::finalize_controls(view, cx),
                "finalize_controls must guard active vst3 text draft"
            );
            super::super::super::fold::set_selected(view, device, true, cx);
            assert!(
                !view.device_panel.collapsed.contains(&device),
                "folding must not discard an active vst3 text edit"
            );
            assert!(
                super::super::super::chain_window::must_mount(view, device, Some(window), cx),
                "active vst3 knob text edit must keep card mounted while scrolled offscreen"
            );
        });
    });

    // 2. An in-flight async SDK conversion (`vst3_parameters.conversion`) also
    //    blocks fold, blocks finalize_controls, sets has_pending_draft, and pins
    //    mount even when no knob is actively editing and focus has moved elsewhere.
    editor.update(&mut visual, |view, cx| {
        let knob = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap()
            .knobs
            .get_mut(&7)
            .unwrap();
        let control = knob.control.as_numeric().unwrap().clone();
        knob._text_observation = Some(cx.observe(&control, |_, _, _| {}));
    });
    visual.simulate_input("0.5");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let knob = &view.device_panel.vst3_parameters.panels[&device].knobs[&7];
            let control = knob.control.as_numeric().unwrap().clone();
            let origin = knob.origin.get();
            let request = control
                .update(cx, |c, _| c.take_deferred_text_request())
                .expect("deferred vst3 text request");
            control.update(cx, |c, cx| c.cancel(cx));
            assert!(!control.read(cx).is_editing());
            view.focus.focus(window);
            let source = AppliedAudioSource {
                context: origin.context,
                audio_revision: 1,
                output_identity: root,
            };
            let query = PluginParameterConversionQuery {
                context: origin.context,
                device: origin.device,
                output_identity: root,
                request_id: 1,
                source: binding,
                parameter: origin.parameter,
                operation: PluginParameterConversion::Parse(
                    PluginParameterText::new("0.5").unwrap(),
                ),
            };
            view.device_panel.vst3_parameters.conversion = Some(PendingConversion {
                query,
                source,
                origin,
                control: control.clone(),
                draft: request.id,
            });
            assert!(view.device_panel.has_pending_draft(cx));
            assert!(!super::super::super::finalize_controls(view, cx));
            super::super::super::fold::set_selected(view, device, true, cx);
            assert!(!view.device_panel.collapsed.contains(&device));
            assert!(super::super::super::chain_window::must_mount(
                view,
                device,
                Some(window),
                cx,
            ));
            view.device_panel.vst3_parameters.conversion = None;
            assert!(!view.device_panel.has_pending_draft(cx));
            assert!(super::super::super::finalize_controls(view, cx));
            assert!(!super::super::super::chain_window::must_mount(
                view,
                device,
                Some(window),
                cx,
            ));
        });
    });

    // 3. Active `GestureTarget::PluginParameter` pins `chain_window::must_mount` even when
    //    focus is elsewhere and no knob entity is currently interacting.
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let origin = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
                .origin
                .get();
            view.device_panel.gesture.begin_plugin(origin).unwrap();
            assert!(
                super::super::super::chain_window::must_mount(view, device, Some(window), cx),
                "active GestureTarget::PluginParameter must keep card mounted offscreen"
            );
            view.device_panel.gesture.reset();
        });
    });
}

#[test]
fn vst3_step_and_toggle_conversion_helpers_snap_without_fractional_values() {
    assert!(!is_toggle_parameter(0));
    assert!(is_toggle_parameter(1));
    assert!(!is_toggle_parameter(5));
    assert!(!toggle_checked(0.49));
    assert!(toggle_checked(0.5));
    assert_eq!(toggle_label(false, None), "off");
    assert_eq!(toggle_label(true, None), "on");
    assert_eq!(toggle_label(true, Some("Active")), "Active");

    for step in 0..=5_u32 {
        let normalized = f64::from(step) / 5.0;
        assert_eq!(step_index_from_normalized(5, normalized), step);
        assert_eq!(
            control_value_from_normalized(5, normalized),
            f64::from(step)
        );
        assert!((normalized_from_control_value(5, f64::from(step)) - normalized).abs() < 1e-12);
    }
    assert_eq!(control_value_from_normalized(5, 0.39), 2.0);
    assert!((normalized_from_control_value(5, 2.0) - 0.4).abs() < 1e-12);
}

#[gpui::test]
fn vst3_toggle_stepped_live_format_and_commit_without_updating_flash(
    cx: &mut gpui::TestAppContext,
) {
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, _backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let root = client.output_identity().unwrap();
    let (project, track, device) = parameter_project();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, cx| Card {
            editor: editor.clone(),
            device,
            _observe: cx.observe(&editor, |_, _, cx| cx.notify()),
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root,
        instance: device.value(),
        graph: 1,
        epoch: 1,
    };
    let toggle_row = PluginParameterRow {
        id: 10,
        name: "Auto".into(),
        value: "off".into(),
        normalized: 0.0,
        default_normalized: 0.0,
        steps: 1,
        ..layout_row(0.0, true, 1)
    };
    let stepped_row = PluginParameterRow {
        id: 20,
        name: "Voice Stack".into(),
        value: "1".into(),
        normalized: 0.0,
        default_normalized: 0.0,
        steps: 5,
        ..layout_row(0.0, true, 1)
    };
    let applied_source = AppliedAudioSource {
        context: AudioSourceContext {
            project_generation: 0,
            editor_revision: 0,
        },
        audio_revision: 1,
        output_identity: root,
    };
    let context = editor.update(&mut visual, |view, cx| {
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
        panel.source = Some(binding);
        panel.last_touched = Some(10);
        panel.result = Some(Arc::new(vec![toggle_row.clone(), stepped_row.clone()]));
        panel.record_row_displays(&[toggle_row.clone(), stepped_row.clone()]);
        panel.refilter(false);
        cx.notify();
        panel.query.context
    });
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    // 1. Boolean (`steps == 1`) parameter renders as a Toggle, flips directly
    //    to 1.0 on click, and commits without flashing "updating".
    let toggle_selector = Box::leak(format!("vst3-toggle-{}-10", device.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(toggle_selector).is_some(),
        "steps == 1 must render as a Toggle control"
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("vst3-knob-{}-10-knob", device.value()).into_boxed_str()
            ))
            .is_none(),
        "steps == 1 must not render as a dial"
    );
    let toggle_center = visual.debug_bounds(toggle_selector).unwrap().center();
    visual.simulate_mouse_down(toggle_center, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle_center, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    editor.update(&mut visual, |view, cx| {
        let panel = &view.device_panel.vst3_parameters.panels[&device];
        let toggle = panel.knobs[&10].control.as_toggle().unwrap().read(cx);
        assert!(toggle.checked());
        assert_eq!(toggle.label(), "on");
        let snapshot = view
            .device_panel
            .gesture
            .snapshot()
            .expect("toggle click must start and commit a gesture");
        assert_eq!(snapshot.plugin_value.unwrap().normalized(), 1.0);
        assert!(view.device_panel.gesture.has_pending_terminal());
        assert!(view.device_panel.gesture.acknowledge(&GestureAck {
            key: snapshot.key,
            phase: GesturePhase::Begin,
            sequence: 0,
            accepted: true,
            preview_retained: false,
            source_lost: false,
            reason: None,
        }));
        let terminal = view.device_panel.gesture.next_request().unwrap();
        view.device_panel.gesture.mark_submitted();
        assert!(view.acknowledge_device_gesture(&GestureAck {
            key: snapshot.key,
            phase: GesturePhase::Commit,
            sequence: terminal.sequence(),
            accepted: true,
            preview_retained: false,
            source_lost: false,
            reason: None,
        }));
        let panel = &view.device_panel.vst3_parameters.panels[&device];
        let stale_toggle = &panel.rows().unwrap()[0];
        assert_eq!(stale_toggle.normalized, 1.0);
        assert_eq!(
            stale_toggle.value, "on",
            "post-commit stale row must retain formatted label without flashing updating"
        );
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("vst3-follow-toggle-{}-10", device.value()).into_boxed_str()
            ))
            .is_some(),
        "follow slot must also render a Toggle for steps == 1"
    );

    // 2. Stepped (`steps == 5`) parameter snaps strictly to integer steps and
    //    applies live Format results without raw floats or "updating" flash.
    editor.update(&mut visual, |view, cx| {
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.source = Some(binding);
        let refreshed = vec![
            PluginParameterRow {
                normalized: 1.0,
                value: "on".into(),
                metadata_revision: 2,
                ..toggle_row.clone()
            },
            PluginParameterRow {
                metadata_revision: 2,
                ..stepped_row.clone()
            },
        ];
        panel.record_row_displays(&refreshed);
        panel.result = Some(Arc::new(refreshed));
        panel.refilter(false);
        cx.notify();
    });
    visual.run_until_parked();

    let stepped_selector =
        Box::leak(format!("vst3-knob-{}-20-knob", device.value()).into_boxed_str());
    let down = visual.debug_bounds(stepped_selector).unwrap().center();
    let drag_to = gpui::point(down.x, down.y - px(48.0));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(drag_to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    let dragged_step = editor.update(&mut visual, |view, cx| {
        let panel = &view.device_panel.vst3_parameters.panels[&device];
        let control = panel.knobs[&20].control.as_numeric().unwrap().read(cx);
        let step = control.value();
        assert!((1.0..=5.0).contains(&step));
        assert_eq!(step.fract(), 0.0, "stepped control must snap to integers");
        assert_eq!(
            control.presented_text(),
            "1",
            "must hold previous plugin-formatted label until Format replies instead of raw float"
        );
        assert!(!view.device_panel.vst3_parameters.has_pending_text_edit(cx));
        let normalized = normalized_from_control_value(5, step);
        let formatted_label = format!("{}", (step as u32) + 1);
        let format_query = PluginParameterConversionQuery {
            context,
            device,
            output_identity: root,
            request_id: 99,
            source: binding,
            parameter: 20,
            operation: PluginParameterConversion::Format(normalized),
        };
        view.device_panel.vst3_parameters.live_format = Some(PendingFormat {
            query: format_query.clone(),
            source: AppliedAudioSource {
                context,
                ..applied_source
            },
            steps: 5,
        });
        apply_conversion_result(
            view,
            PluginParameterConversionResult {
                query: format_query,
                value: Ok(hex_session::service::PluginParameterConvertedValue {
                    normalized,
                    display: Some(formatted_label.clone()),
                }),
            },
            context.editor_revision,
            cx,
        );
        // Simulate live format result with refresh_source absent in headless unit test:
        // verify apply_live_format_result / cache directly on the panel too.
        let panel = view
            .device_panel
            .vst3_parameters
            .panels
            .get_mut(&device)
            .unwrap();
        panel.cache_step_label(20, 5, normalized, &formatted_label);
        panel.drag_preview = Some(DragPreview {
            source: binding,
            parameter: 20,
            normalized,
            display: formatted_label.clone(),
        });
        let row_control = panel.knobs[&20].control.clone();
        row_control.set_external_display_for_current(&formatted_label, cx);
        let control = panel.knobs[&20].control.as_numeric().unwrap().read(cx);
        assert_eq!(control.presented_text(), formatted_label);
        assert_eq!(
            panel.display_text(&panel.rows().unwrap()[1]),
            formatted_label
        );
        assert!(view.error.is_none());
        (step, normalized, formatted_label)
    });

    visual.simulate_mouse_up(drag_to, MouseButton::Left, Modifiers::default());
    editor.update(&mut visual, |view, cx| {
        let snapshot = view.device_panel.gesture.snapshot().unwrap();
        assert!((snapshot.plugin_value.unwrap().normalized() - dragged_step.1).abs() < 1e-12);
        assert!(view.device_panel.gesture.acknowledge(&GestureAck {
            key: snapshot.key,
            phase: GesturePhase::Begin,
            sequence: 0,
            accepted: true,
            preview_retained: false,
            source_lost: false,
            reason: None,
        }));
        while let Some(request) = view.device_panel.gesture.next_request() {
            let phase = request.phase();
            let seq = request.sequence();
            view.device_panel.gesture.mark_submitted();
            assert!(view.acknowledge_device_gesture(&GestureAck {
                key: snapshot.key,
                phase,
                sequence: seq,
                accepted: true,
                preview_retained: false,
                source_lost: false,
                reason: None,
            }));
            if phase == GesturePhase::Commit {
                break;
            }
        }
        let panel = &view.device_panel.vst3_parameters.panels[&device];
        let stale_stepped = &panel.rows().unwrap()[1];
        assert_eq!(
            stale_stepped.value, dragged_step.2,
            "post-commit stale stepped row must retain plugin step label without flashing updating"
        );
        assert_ne!(stale_stepped.value, "updating");
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let control = view.device_panel.vst3_parameters.panels[&device].knobs[&20]
            .control
            .as_numeric()
            .unwrap()
            .read(cx);
        assert_eq!(
            control.presented_text(),
            dragged_step.2,
            "control must retain formatted step label across post-commit refresh"
        );
    });
}

#[gpui::test]
fn inflight_commit_locks_released_knob_and_stranded_commit_recovers_on_click(
    cx: &mut gpui::TestAppContext,
) {
    use crate::editor::{clip_drag_ui_test, device_panel::DevicePanelOwner};
    use gpui::{Modifiers, MouseButton, VisualTestContext};
    use hex_session::device_gesture::{GestureAck, GesturePhase};

    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, _backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let root = client.output_identity().unwrap();
    let (project, track, device) = parameter_project();
    let (handle, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, cx| Card {
            editor: editor.clone(),
            device,
            _observe: cx.observe(&editor, |_, _, cx| cx.notify()),
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root,
        instance: device.value(),
        graph: 1,
        epoch: 1,
    };
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
        panel.source = Some(binding);
        panel.result = Some(Arc::new(vec![
            layout_row(0.25, true, 1),
            PluginParameterRow {
                id: 8,
                name: "mix".into(),
                value: "0.50".into(),
                normalized: 0.5,
                default_normalized: 0.5,
                steps: 0,
                editable: true,
                page_first: 0,
                metadata_revision: 1,
                editor_sequence: 0,
                automation_class: None,
            },
        ]));
        panel.refilter(false);
        cx.notify();
    });
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    let knob_7_selector =
        Box::leak(format!("vst3-knob-{}-7-knob", device.value()).into_boxed_str());
    let knob_7 = visual.debug_bounds(knob_7_selector).unwrap().center();
    let drag_end = gpui::point(knob_7.x, knob_7.y - px(24.0));
    visual.simulate_mouse_down(knob_7, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(drag_end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(drag_end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    // Repaint while Commit is still in flight; the released knob must be Busy so
    // re-dragging it cannot visually spin and snap back with silently dropped events.
    let committed_preview = editor.read_with(&visual, |view, cx| {
        assert!(view.device_panel.gesture.has_pending_terminal());
        let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
            .control
            .as_numeric()
            .unwrap()
            .read(cx);
        let value = control.value();
        assert!(value > 0.25, "initial drag must have advanced knob 7");
        value
    });
    let redrag_end = gpui::point(knob_7.x, knob_7.y + px(40.0));
    visual.simulate_mouse_down(knob_7, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(redrag_end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(redrag_end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
            .control
            .as_numeric()
            .unwrap()
            .read(cx);
        assert_eq!(
            control.value(),
            committed_preview,
            "re-dragging the released knob while its commit is in flight must not phantom-spin"
        );
    });

    // Now refuse the Commit with preview_retained=true (stranded commit).
    let key = editor.update(&mut visual, |view, cx| {
        let snapshot = view.device_panel.gesture.snapshot().unwrap();
        assert!(view.device_panel.gesture.acknowledge(&GestureAck {
            key: snapshot.key,
            phase: GesturePhase::Begin,
            sequence: 0,
            accepted: true,
            preview_retained: false,
            source_lost: false,
            reason: None,
        }));
        while let Some(request) = view.device_panel.gesture.next_request() {
            let phase = request.phase();
            let seq = request.sequence();
            view.device_panel.gesture.mark_submitted();
            if phase == GesturePhase::Commit {
                assert!(view.acknowledge_device_gesture_with_snapshot(
                    &GestureAck {
                        key: snapshot.key,
                        phase,
                        sequence: seq,
                        accepted: false,
                        preview_retained: true,
                        source_lost: false,
                        reason: Some("vst3 edit playback refused".into()),
                    },
                    Some(snapshot),
                ));
                break;
            }
            assert!(view.acknowledge_device_gesture(&GestureAck {
                key: snapshot.key,
                phase,
                sequence: seq,
                accepted: true,
                preview_retained: false,
                source_lost: false,
                reason: None,
            }));
        }
        assert!(view.device_panel.gesture.is_recovery_pending());
        assert!(view.device_panel.gesture.has_stranded_commit());
        assert!(!view.device_panel.gesture.has_pending_requests());
        cx.notify();
        snapshot.key
    });
    visual.run_until_parked();

    // Clicking an unrelated non-automated parameter row (row 8) while stranded
    // must immediately initiate Cancel instead of deadlocking all knobs forever.
    let knob_8_selector =
        Box::leak(format!("vst3-knob-{}-8-knob", device.value()).into_boxed_str());
    let knob_8 = visual.debug_bounds(knob_8_selector).unwrap().center();
    visual.simulate_mouse_down(knob_8, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(knob_8, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    editor.update(&mut visual, |view, cx| {
        assert_eq!(
            view.status,
            "previous vst3 edit is canceling · drag again when it finishes"
        );
        let (phase, seq) = view
            .device_panel
            .gesture
            .terminal()
            .expect("clicking a vst3 row while stranded must submit Cancel");
        assert_eq!(phase, GesturePhase::Cancel);
        assert!(view.acknowledge_device_gesture(&GestureAck {
            key,
            phase: GesturePhase::Cancel,
            sequence: seq,
            accepted: true,
            preview_retained: false,
            source_lost: false,
            reason: None,
        }));
        assert!(!view.device_panel.gesture.is_recovery_pending());
        assert!(view.device_panel.gesture.snapshot().is_none());
        cx.notify();
    });
    output.publication.close();
}

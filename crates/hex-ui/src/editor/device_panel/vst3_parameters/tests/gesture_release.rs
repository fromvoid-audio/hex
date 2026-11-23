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
            let body = if view.stopped_playback_restore.is_some() {
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
        view.device_panel_active = true;
        view.clip_view_open = true;
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
            .device_gesture
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
            view.device_gesture
                .acknowledge(&acknowledge(GesturePhase::Begin, 0))
        );
        let terminal = view
            .device_gesture
            .next_request()
            .expect("mouse release must finish the edit");
        assert_eq!(
            (terminal.key(), terminal.phase()),
            (key, GesturePhase::Commit)
        );
        view.device_gesture.mark_submitted();
        assert!(
            view.device_gesture
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
        view.device_panel_active = true;
        view.clip_view_open = true;
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
            .update(cx, |control, cx| control.set_deferred_text(false, cx));
    });
    let value = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(value, Modifiers::default());
    editor.read_with(&visual, |view, cx| {
        let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
            .control
            .read(cx);
        assert!(control.is_editing(), "value field must start a text edit");
    });
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.75");
    editor.read_with(&visual, |view, cx| {
        let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
            .control
            .read(cx);
        assert_eq!(control.display_text(), "0.75");
    });
    visual.simulate_keystrokes("enter");
    editor.read_with(&visual, |view, cx| {
        let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
            .control
            .read(cx);
        assert!(!control.is_editing(), "enter must commit the numeric field");
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            let control = view.device_panel.vst3_parameters.panels[&device].knobs[&7]
                .control
                .read(cx);
            assert_eq!(
                control.value(),
                0.75,
                "pending text edit must remain visible"
            );
            assert!(view.device_gesture.has_pending_terminal());
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
        let snapshot = view.device_gesture.snapshot().expect("typed value began a gesture");
        assert!(view.device_gesture.has_pending_terminal(), "typed value must finish its admitted gesture");
        assert_eq!(snapshot.plugin_value.unwrap().normalized(), 0.75);
        assert!(view.device_gesture.acknowledge(&GestureAck {
            key: snapshot.key, phase: GesturePhase::Begin, sequence: 0, accepted: true,
            preview_retained: false, source_lost: false, reason: None,
        }));
        let terminal = view.device_gesture.next_request().expect("typed value terminal");
        assert!(matches!(terminal, hex_session::device_gesture::GestureRequest::CommitPlugin { key, value, .. } if key == snapshot.key && value.normalized() == 0.75));
        view.device_gesture.mark_submitted();
        let mut wrong_key = snapshot.key;
        wrong_key.token += 1;
        assert!(!view.device_gesture.acknowledge(&GestureAck {
            key: wrong_key, phase: GesturePhase::Commit, sequence: terminal.sequence(), accepted: true,
            preview_retained: false, source_lost: false, reason: None,
        }));
        assert!(view.device_gesture.has_pending_terminal());
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
                .read(cx);
            assert!(!control.is_interacting());
            assert_eq!(control.value(), 0.6);
            assert!(view.device_gesture.snapshot().is_none());
        });
        editor.update(&mut visual, |view, cx| {
            view.stopped_playback_restore =
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
        view.device_panel_active = true;
        view.clip_view_open = true;
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
        let control = knob.control.clone();
        knob._text_observation = cx.observe(&control, |_, _, _| {});
    });
    visual.simulate_input("0.5");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let knob = &view.device_panel.vst3_parameters.panels[&device].knobs[&7];
            let control = knob.control.clone();
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
            view.device_gesture.begin_plugin(origin).unwrap();
            assert!(
                super::super::super::chain_window::must_mount(view, device, Some(window), cx),
                "active GestureTarget::PluginParameter must keep card mounted offscreen"
            );
            view.device_gesture.reset();
        });
    });
}

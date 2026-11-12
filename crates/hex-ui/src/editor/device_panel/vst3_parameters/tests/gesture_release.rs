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
            (view.focus.clone(), body(view, self.device, cx))
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

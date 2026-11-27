//! Compact descriptor-driven native device chain in the shared detail pane.
use super::{EditorView, device_comparison, drag};
use crate::theme;
use gpui::{
    App, Bounds, Context, Div, ElementId, Entity, MouseButton, Pixels, Stateful, Subscription,
    Window, div, prelude::*, px, rgb,
};
use hex_engine::{
    NativeDeviceClass, NativeDeviceParameterView, NativeDeviceView, PreparedDeviceProcessing,
};
use hex_lattice_contract::{
    NativeDeviceState, ParameterDescriptor, ParameterKind, ParameterPresentation, ParameterValue,
};
use hex_project::{Command, DeviceId, TrackId};
use hex_session::device_gesture::GestureTarget;
use hex_session::service::{NativeDeviceDropSource, Request, ViewState};
use hex_widgets::{
    ControlStyle, DialLayout, NumericControl, NumericControlConfig, NumericControlEvent,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec, Selector,
    SelectorAppearance, SelectorChanged, SelectorConfig, Toggle, ToggleAppearance, ToggleChanged,
    ToggleConfig, device_frame,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, atomic::Ordering::Acquire},
};

mod auto_pan_panel;
mod automation_playback;
mod automation_tint;
mod bindings;
mod bypass;
mod chorus_panel;
mod delay_panel;
mod overdrive_panel;
mod rattle_panel;
mod shifter_panel;
use bindings::{
    ControlPresentation, ControlText, ParameterControl, cancel_control,
    parameter_control_matches_identity,
};
pub(super) use bindings::{effective_parameter_value, parameter_display_override};
mod gesture_adapter;
pub(super) use gesture_adapter::{
    NumericGestureKey, NumericGestureLease, commit_owned_numeric_edit,
    dispatch_bound_numeric_gesture, dispatch_numeric_gesture, owns_numeric_gesture,
};
mod chain_window;
mod effect_bus;
pub(super) use effect_bus::{EffectBusOwner, select_main, selected_owner};
mod compressor_panel;
mod effect_preview;
pub(super) use compressor_panel::sidechain::drop_track_key;
mod eq8_panel;
mod eq_automation;
mod eq_panel;
mod limiter_panel;
pub(super) use eq_panel::after_scroll_worker_event as eq_scroll_after_worker_event;
pub(super) use eq_panel::reconcile_spectrum as reconcile_eq_spectrum;
mod device_add;
mod device_picker;
mod source_manager;
mod source_picker;
pub(super) use device_picker::{
    bind_keys as bind_picker_keys, is_open as picker_is_open, overlay as picker_overlay,
};
pub(super) use source_manager::Action as SourceManagerAction;
pub(super) use source_manager::bind_keys as bind_source_manager_keys;
pub(super) use source_manager::focus_from_workspace as focus_source_manager_from_workspace;
pub(super) use source_manager::{name_overlay as source_name_overlay, naming as source_naming};
pub(super) use source_picker::Choice as SourcePickerChoice;
pub(super) use source_picker::bind_keys as bind_source_picker_keys;
pub(super) use source_picker::focus_from_workspace as focus_source_picker_from_workspace;
mod device_meter;
#[cfg(all(test, feature = "ui-test-support"))]
pub(super) use device_meter::tick_hovered as tick_hovered_meter;
pub(in crate::editor) use device_meter::tick_hovered_isolated as tick_hovered_meter_isolated;

/// Push live EQ / EQ8 analyzer traces directly into mounted `ResponseGraph`
/// entities without forcing a root `EditorView::render`. Returns
/// `(any_graph_updated, needs_root_redraw)`.
pub(in crate::editor) fn push_live_spectra(
    view: &EditorView,
    cx: &mut Context<EditorView>,
) -> (bool, bool) {
    let mut updated = false;
    for (&device, panel) in &view.device_panel.eq_panels {
        updated |= eq_panel::push_live_spectrum(view, device, panel, cx);
    }
    for (&device, control) in &view.device_panel.eq8_graphs {
        updated |= eq8_panel::push_live_spectrum(view, device, control, cx);
    }
    let needs_root = view.audio_detail.clip_view_open
        && view.device_panel.active
        && view.device_panel.eq_panels.is_empty()
        && view.device_panel.eq8_graphs.is_empty();
    (updated, needs_root)
}
mod filter_panel;
mod fold;
mod gate_panel;
mod keyboard;
mod navigation;
mod nested;
mod reverb_panel;
pub(super) use keyboard::{bind_keys as bind_keyboard_keys, key_up as selection_key_up};
mod numeric_display;
pub(super) use navigation::{bottom_bar, capture_pointer_down as capture_navigation_pointer_down};
mod modulators;
pub(super) use modulators::AddModulatorAction;
#[cfg(feature = "ui-test-support")]
pub(super) mod operator_focus_fixture;
mod operator_panel;
pub(in crate::editor) mod package_panel;
mod parameter_rows;
pub(super) mod vst3_parameters;
pub(super) use operator_panel::bind_keys as bind_operator_keys;

pub(super) fn has_rattle_preview(view: &EditorView) -> bool {
    !view.device_panel.rattle_previews.is_empty()
}
mod return_drag;
mod sampler_overview;
pub(super) use sampler_overview::{cancel_sampler_boundary, sampler_boundary_preview};
mod saturator_panel;
mod send_controls;
mod snapshot;
mod source_summary;
pub(super) mod spectrum_display;
mod utility_panel;
pub(super) use send_controls::finalize_send_controls_for_track;
pub(super) use send_controls::pending_send;
pub(super) use send_controls::return_send_numeric_control;

mod header_style;
use header_style::{
    pointer_within_visible_device_drop_bounds, retained_source_path_badge,
    retained_source_path_label, source_summary_tooltip, title_label,
};
mod actions;
mod chain_render;
mod drop_targets;
mod state;
use actions::*;
use chain_render::*;
use drop_targets::*;
use state::*;

pub(super) use actions::{action_button, segment_button};
pub(super) use chain_render::{trace_vst3_track_panel, view};
pub(super) use state::{
    DEVICE_CHAIN_PADDING, DEVICE_FRAME_HEIGHT, DEVICE_PANEL_HEIGHT, DeviceDragSelection,
    DevicePanelOwner, DevicePanelState, PROPOSED_SLOT_WIDTH, ParameterKey, UTILITY_FRAME_WIDTH,
    balance_spec, clear_eq8_audition_gesture_flags, control_style, device_layout_style,
    displayed_owner, enqueue_return_edit, finalize_controls, retire_removed_eq8_graphs,
    retire_removed_send_controls, source_owner_current, source_sampler_edit_active,
};

mod return_view;

#[cfg(all(test, not(feature = "ui-test-support")))]
mod chain_layout_unit_tests {
    mod keyboard_navigation_tests {
        use super::super::DevicePanelState;
        use gpui::Modifiers;
        use hex_project::{Command, Created, DeviceId};

        fn ids(n: u64) -> Vec<DeviceId> {
            let mut editor =
                hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Track(track) = editor
                .execute(Command::AddTrack { name: "ids".into() })
                .unwrap()
            else {
                panic!("track");
            };
            for _ in 0..n {
                editor
                    .execute(Command::InsertDevice {
                        track,
                        before: None,
                        state: hex_engine::default_native_device_state(
                            hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                        )
                        .unwrap(),
                    })
                    .unwrap();
            }
            editor
                .project()
                .track(track)
                .unwrap()
                .devices()
                .iter()
                .map(|device| device.id())
                .collect()
        }

        #[test]
        fn step_target_walks_selection_and_insertion_gaps() {
            let order = ids(3);
            let mut panel = DevicePanelState::default();
            assert_eq!(panel.step_target(&order, true), Some(order[0]));
            assert_eq!(panel.step_target(&order, false), Some(order[2]));
            assert_eq!(panel.step_target(&[], true), None);

            panel.select_only(Some(order[1]));
            assert_eq!(panel.step_target(&order, true), Some(order[2]));
            assert_eq!(panel.step_target(&order, false), Some(order[0]));
            panel.select_only(Some(order[2]));
            assert_eq!(panel.step_target(&order, true), None, "end does not wrap");

            panel.select_insertion(Some(order[1]));
            assert!(panel.selected_device.is_none(), "a gap replaces selection");
            assert_eq!(panel.step_target(&order, false), Some(order[0]));
            assert_eq!(panel.step_target(&order, true), Some(order[1]));
            panel.select_insertion(Some(order[0]));
            assert_eq!(panel.step_target(&order, false), None);
            panel.select_insertion(None);
            assert_eq!(panel.step_target(&order, false), Some(order[2]));
            assert_eq!(panel.step_target(&order, true), None);

            // A removed gap device leaves no stale paste target.
            panel.select_insertion(Some(order[1]));
            panel.reconcile_selection(&[order[0], order[2]].into_iter().collect());
            assert_eq!(panel.insertion_before(&order), None);
            panel.select_insertion(Some(order[1]));
            panel.select_click(order[0], &order, Modifiers::default());
            assert_eq!(
                panel.insertion_before(&order),
                None,
                "selection replaces a gap"
            );
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod test_support {
    pub(super) struct RestoreTheme(pub(super) hex_preferences::ThemePreferences);
    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            crate::theme::apply_preferences(self.0);
        }
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod header_style_unit_tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    #[test]
    fn full_metadata_cache_keeps_unavailable_parameters_and_unsupported_errors_without_entities() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "metadata".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        for type_id in ["hex.utility", "hex.test.unsupported"] {
            let state = if type_id == "hex.utility" {
                hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static(type_id),
                )
                .unwrap()
            } else {
                NativeDeviceState::new(
                    hex_lattice_contract::DeviceTypeId::from_static(type_id),
                    1,
                    Vec::new(),
                )
                .unwrap()
            };
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state,
                })
                .unwrap();
        }
        let devices = editor.project().track(track).unwrap().devices();
        let mut panel = DevicePanelState::default();
        for device in devices {
            cache_device_metadata(&mut panel, device, NativeDeviceClass::Effect);
        }
        assert_eq!(panel.views.len(), 2);
        assert!(panel.controls.is_empty());
        assert!(panel.views[&devices[1].id()].2.is_err());
        let expected =
            hex_engine::native_device_view_for_class(NativeDeviceClass::Effect, devices[0].state())
                .unwrap();
        let cached = panel.views[&devices[0].id()].2.as_ref().unwrap();
        assert_eq!(cached.parameters.len(), expected.parameters.len());
        for (first, second) in cached.parameters.iter().zip(&expected.parameters) {
            assert_eq!(first.descriptor, second.descriptor);
            assert_eq!(first.value, second.value);
            assert_eq!(first.enabled, second.enabled);
        }
        // Availability is metadata, not a reason to discard a descriptor.
        panel
            .views
            .get_mut(&devices[0].id())
            .unwrap()
            .2
            .as_mut()
            .unwrap()
            .parameters[0]
            .enabled = false;
        cache_device_metadata(&mut panel, &devices[0], NativeDeviceClass::Effect);
        assert!(!panel.views[&devices[0].id()].2.as_ref().unwrap().parameters[0].enabled);
        assert!(panel.controls.is_empty());
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) fn set_hovered_meter_for_test(
    view: &mut EditorView,
    device: Option<DeviceId>,
) {
    view.device_panel.hovered_meter = device;
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) fn has_mounted_eq8_and_meter_for_test(
    view: &EditorView,
    device: DeviceId,
) -> bool {
    view.device_panel.eq8_graphs.contains_key(&device)
        && view.device_panel.meter_views.contains_key(&device)
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) fn install_spectrum_test_channel(
    view: &mut EditorView,
) -> hex_audio::spectrum_target::SpectrumTargetCallback {
    let (control, callback) = hex_audio::spectrum_target::channel();
    view.device_panel.eq_spectrum.test_channel = Some(control);
    callback
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) fn desired_spectrum_target_for_test(
    view: &EditorView,
    identity: hex_engine::PostNodeIdentity,
) -> Option<hex_audio::spectrum_target::SpectrumTarget> {
    view.device_panel
        .eq_spectrum
        .test_channel
        .as_ref()
        .and_then(|channel| {
            channel
                .desired_targets()
                .find(identity)
                .map(|(_, target)| target)
        })
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) mod tests;

#[cfg(all(test, feature = "ui-test-support"))]
mod owner_reset_tests {
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};
    use hex_engine::{NativeDeviceClass, default_native_device_state_for_class};
    use hex_lattice_contract::DeviceTypeId;
    use hex_project::{Command, Editor, Project};

    fn effect_state(type_id: &str) -> NativeDeviceState {
        default_native_device_state_for_class(
            NativeDeviceClass::Effect,
            DeviceTypeId::new(type_id).unwrap(),
        )
        .unwrap()
    }

    fn setup_window(
        cx: &mut TestAppContext,
    ) -> (
        gpui::AnyWindowHandle,
        gpui::Entity<EditorView>,
        [hex_project::TrackId; 2],
        DeviceId,
        DeviceId,
        DeviceId,
    ) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        editor
            .execute(Command::AddTrack { name: "a".into() })
            .unwrap();
        editor
            .execute(Command::AddTrack { name: "b".into() })
            .unwrap();
        let tracks = [
            editor.project().tracks()[0].id(),
            editor.project().tracks()[1].id(),
        ];
        for id in ["hex.rattle", "hex.eq", "hex.eq8"] {
            editor
                .execute(Command::InsertDevice {
                    track: tracks[0],
                    before: None,
                    state: effect_state(id),
                })
                .unwrap();
        }
        let devices = editor.project().track(tracks[0]).unwrap().devices();
        let (rattle, eq, eq8) = (devices[0].id(), devices[1].id(), devices[2].id());
        // Add one node to EQ so we have a draggable node on the EQ response graph.
        let mut eq_state = hex_devices::eq::EqState::decode(devices[1].state()).unwrap();
        let node_id = hex_devices::eq::NodeId::new(1).unwrap();
        eq_state
            .add(hex_devices::eq::Node::new(node_id, 1_000.0, 0.0, 0).unwrap())
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: eq,
                state: eq_state.encode().unwrap(),
            })
            .unwrap();
        let project = editor.project().clone();
        let (handle, view) = crate::editor::rendered_test_fixture::editor_window(
            cx,
            project,
            std::sync::Arc::default(),
        );
        (handle, view, tracks, rattle, eq, eq8)
    }

    fn start_graph_drag(
        visual: &mut VisualTestContext,
        graph: &gpui::Entity<hex_widgets::ResponseGraph>,
        node_id: u32,
    ) {
        let (bounds, position) = graph.read_with(visual, |g, _| {
            (
                g.plot_bounds().expect("painted plot bounds"),
                g.node_position(node_id).expect("draggable node position"),
            )
        });
        let target = point(
            bounds.origin.x + bounds.size.width * position[0],
            bounds.origin.y + bounds.size.height * (1.0 - position[1]),
        );
        visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(target.x + px(8.0), target.y),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        assert!(
            graph.read_with(visual, |g, _| g.is_dragging()),
            "expected graph node {node_id} drag to begin"
        );
    }

    #[gpui::test]
    fn owner_switch_and_project_replacement_clear_rattle_meter_hover_and_cancel_graph_captures(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, tracks, rattle, eq, eq8) = setup_window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1800.0), px(900.0)));
        editor.update(&mut visual, |view, cx| {
            view.selected_track = Some(tracks[0]);
            view.device_panel.active = true;
            view.audio_detail.clip_view_open = true;
            view.mode = crate::editor::EditorMode::Arrangement;
            cx.notify();
        });
        visual.run_until_parked();

        // Populate meter/hover state and verify rattle preview + EQ/EQ8 graphs exist.
        let (eq_graph, eq8_graph) = editor.update(&mut visual, |view, _| {
            view.device_panel
                .meter_states
                .insert(rattle, device_meter::MeterState::default());
            view.device_panel.hovered_meter = Some(rattle);
            assert!(view.device_panel.rattle_previews.contains_key(&rattle));
            assert!(view.device_panel.eq_panels.contains_key(&eq));
            assert!(view.device_panel.eq8_graphs.contains_key(&eq8));
            (
                eq_panel::graph_for_test(view, eq),
                view.device_panel.eq8_graphs[&eq8].entity.clone(),
            )
        });

        // Start an active drag on the EQ graph, cancel the device gesture so owner switch is
        // allowed to proceed, and verify owner switch clears caches and cancels the graph capture.
        start_graph_drag(&mut visual, &eq_graph, 1);
        editor.update(&mut visual, |view, cx| {
            view.device_panel.gesture.reset();
            view.selected_track = Some(tracks[1]);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.device_panel.rattle_previews.is_empty());
            assert!(view.device_panel.meter_states.is_empty());
            assert!(view.device_panel.hovered_meter.is_none());
            assert!(view.device_panel.eq_panels.is_empty());
            assert!(view.device_panel.eq8_graphs.is_empty());
        });
        assert!(!eq_graph.read_with(&visual, |g, _| g.is_dragging()));

        // Switch back to Track 0, start a drag on EQ8 graph, and test project generation change.
        editor.update(&mut visual, |view, cx| {
            view.selected_track = Some(tracks[0]);
            cx.notify();
        });
        visual.run_until_parked();
        let (eq_graph_2, eq8_graph_2) = editor.update(&mut visual, |view, _| {
            view.device_panel
                .meter_states
                .insert(rattle, device_meter::MeterState::default());
            view.device_panel.hovered_meter = Some(rattle);
            assert!(view.device_panel.rattle_previews.contains_key(&rattle));
            (
                eq_panel::graph_for_test(view, eq),
                view.device_panel.eq8_graphs[&eq8].entity.clone(),
            )
        });
        let eq8_node = (0..8u32)
            .find(|id| eq8_graph_2.read_with(&visual, |g, _| g.node_position(*id).is_some()))
            .expect("enabled eq8 band");
        start_graph_drag(&mut visual, &eq8_graph_2, eq8_node);
        editor.update(&mut visual, |view, cx| {
            view.project_generation += 1;
            view.selected_track = None;
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.device_panel.rattle_previews.is_empty());
            assert!(view.device_panel.meter_states.is_empty());
            assert!(view.device_panel.hovered_meter.is_none());
        });
        assert!(!eq8_graph_2.read_with(&visual, |g, _| g.is_dragging()));
        let _ = (eq8_graph, eq_graph_2);
    }

    #[gpui::test]
    fn retire_removed_eq8_graphs_cancels_removed_eq_panel_capture(cx: &mut TestAppContext) {
        let (handle, editor, tracks, _rattle, eq, _eq8) = setup_window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1800.0), px(900.0)));
        editor.update(&mut visual, |view, cx| {
            view.selected_track = Some(tracks[0]);
            view.device_panel.active = true;
            view.audio_detail.clip_view_open = true;
            view.mode = crate::editor::EditorMode::Arrangement;
            cx.notify();
        });
        visual.run_until_parked();
        let eq_graph = editor.read_with(&visual, |view, _| eq_panel::graph_for_test(view, eq));
        start_graph_drag(&mut visual, &eq_graph, 1);
        editor.update(&mut visual, |view, cx| {
            retire_removed_eq8_graphs(view, cx, &HashSet::new());
            assert!(view.device_panel.eq_panels.is_empty());
        });
        assert!(!eq_graph.read_with(&visual, |g, _| g.is_dragging()));
    }
}

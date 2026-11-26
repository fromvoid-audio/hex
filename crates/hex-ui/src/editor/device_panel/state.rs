//! State, constants, and lifecycle finalization for the device panel.
use super::*;

pub(in crate::editor) const DEVICE_PANEL_BORDER_WIDTH: f32 = 1.0;
pub(in crate::editor) const DEVICE_PANEL_HEADER_HEIGHT: f32 = 22.0;
pub(in crate::editor) const DEVICE_CHAIN_PADDING: f32 = 3.0;
pub(in crate::editor) const DEVICE_PANEL_HEIGHT: f32 = crate::device_composition::FRAME_HEIGHT
    + DEVICE_PANEL_BORDER_WIDTH
    + DEVICE_PANEL_HEADER_HEIGHT
    + DEVICE_CHAIN_PADDING * 2.0;
pub(in crate::editor) const DEVICE_FRAME_WIDTH: f32 =
    crate::device_composition::GENERIC_FRAME_WIDTH;
pub(in crate::editor) const UTILITY_FRAME_WIDTH: f32 =
    crate::device_composition::UTILITY_FRAME_WIDTH;
pub(in crate::editor) const DEVICE_FRAME_HEIGHT: f32 = crate::device_composition::FRAME_HEIGHT;
pub(in crate::editor) const PROPOSED_SLOT_WIDTH: f32 = 200.0;
pub(in crate::editor) const DEVICE_FRAME_BODY_HEIGHT: f32 = crate::device_composition::BODY_HEIGHT;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::editor) struct ParameterKey {
    pub(in crate::editor) device: DeviceId,
    pub(in crate::editor) parameter: hex_lattice_contract::ParameterId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::editor) struct SendControlKey {
    pub(in crate::editor) track: TrackId,
    pub(in crate::editor) return_track: hex_project::ReturnId,
}

pub(in crate::editor) struct SendControl {
    pub(in crate::editor) entity: Entity<NumericControl>,
    pub(in crate::editor) _subscription: Subscription,
}

pub(in crate::editor) struct Eq8AuditionToggle {
    pub(in crate::editor) entity: Entity<Toggle>,
    pub(in crate::editor) _subscription: Subscription,
}

#[derive(Clone, Copy)]
pub(in crate::editor) struct DeviceControlIdentity {
    pub(in crate::editor) class: NativeDeviceClass,
    pub(in crate::editor) type_id: hex_lattice_contract::DeviceTypeId,
}

#[derive(Default)]
pub(in crate::editor) struct DevicePanelState {
    pub(super) bypass: bypass::State,
    pub(in crate::editor) owner: Option<DevicePanelOwner>,
    pub(in crate::editor) main_selected: Option<u64>,
    pub(super) chain_window: chain_window::ChainWindow,
    pub(super) source_summary: source_summary::SourceSummaryCache,
    pub(super) source_picker: source_picker::State,
    pub(super) source_manager: source_manager::State,
    pub(super) parameter_rows: HashMap<DeviceId, parameter_rows::RowWindow>,
    pub(in crate::editor) package_schemas:
        HashMap<DeviceId, Result<hex_lattice_contract::package_ui::PackageUiSchema, String>>,
    pub(in crate::editor) package_snapshots:
        HashMap<DeviceId, hex_lattice_contract::package_ui::PackagePresentationSnapshot>,
    pub(in crate::editor) mapping_source: Option<(DeviceId, modulators::MappingOutput)>,
    pub(super) modulation_target_drag: modulators::TargetDragState,
    pub(super) host_gain_mapping_availability: Option<modulators::HostGainMappingAvailability>,
    pub(super) vst3_parameters: vst3_parameters::State,
    pub(in crate::editor) navigation: navigation::Navigation,
    pub(super) generation: u64,
    pub(super) reconciled_track_chain: Option<(
        u64,
        u64,
        std::sync::Weak<hex_project::Project>,
        DevicePanelOwner,
    )>,
    pub(in crate::editor) selected_device: Option<DeviceId>,
    pub(super) selected_others: HashSet<DeviceId>,
    pub(super) collapsed: HashSet<DeviceId>,
    pub(super) selection_anchor: Option<DeviceId>,
    /// View-only chain gap chosen for paste; exclusive with device selection.
    pub(super) insertion: Option<InsertionPoint>,
    pub(in crate::editor) drag_selection: Option<DeviceDragSelection>,
    pub(in crate::editor) pending_duplicate: Option<keyboard::PendingDuplicate>,
    pub(super) controls: HashMap<ParameterKey, ParameterControl>,
    pub(super) playback_values: automation_playback::Values,
    pub(super) meter_states: HashMap<DeviceId, device_meter::MeterState>,
    pub(super) meter_views: HashMap<DeviceId, Entity<device_meter::DeviceMeterView>>,
    pub(super) hovered_meter: Option<DeviceId>,
    pub(super) send_controls: HashMap<SendControlKey, SendControl>,
    pub(super) views: HashMap<
        DeviceId,
        (
            NativeDeviceClass,
            NativeDeviceState,
            Result<NativeDeviceView, String>,
        ),
    >,
    pub(super) effect_catalog: Option<Result<Vec<hex_engine::NativeDeviceCatalogEntry>, String>>,
    pub(super) instrument_catalog:
        Option<Result<Vec<hex_engine::NativeDeviceCatalogEntry>, String>>,
    pub(in crate::editor) pending_values: HashMap<ParameterKey, ParameterValue>,
    pub(super) operator_pages: HashMap<DeviceId, operator_panel::OperatorPanelState>,
    pub(super) operator_navigation_focus:
        HashMap<(DeviceId, operator_panel::NavigationTarget), gpui::FocusHandle>,
    pub(super) operator_section_focus:
        HashMap<(DeviceId, operator_panel::Section), operator_panel::context::SectionFocus>,
    pub(super) operator_previews: HashMap<DeviceId, operator_panel::preview::State>,
    pub(super) operator_preview_inventory: Option<OperatorPreviewInventory>,
    pub(super) operator_harmonics: HashMap<(DeviceId, usize), operator_panel::harmonics::Control>,
    pub(super) operator_envelope_graphs:
        HashMap<operator_panel::envelope::GraphKey, operator_panel::envelope::GraphControl>,
    pub(super) operator_envelope_gestures:
        HashMap<operator_panel::envelope::GraphKey, operator_panel::envelope::GraphGesture>,
    pub(super) eq_panels: HashMap<DeviceId, eq_panel::Panel>,
    pub(super) delay_panels: HashMap<DeviceId, delay_panel::Panel>,
    pub(super) eq_spectrum: eq_panel::SpectrumSelection,
    pub(super) sampler_cursor: sampler_overview::CursorState,
    pub(super) eq8_panels: HashMap<DeviceId, eq8_panel::Eq8PanelState>,
    pub(super) eq8_graphs: HashMap<DeviceId, eq8_panel::GraphControl>,
    pub(super) eq8_responses: HashMap<DeviceId, eq8_panel::ResponseCache>,
    pub(super) saturator_previews: HashMap<DeviceId, saturator_panel::PreviewCache>,
    pub(super) compressor_previews: HashMap<DeviceId, compressor_panel::PreviewCache>,
    pub(super) compressor_modes: HashMap<DeviceId, compressor_panel::display::Mode>,
    pub(super) compressor_displays: HashMap<DeviceId, compressor_panel::display::DisplayState>,
    pub(super) filter_previews: HashMap<DeviceId, filter_panel::Preview>,
    pub(in crate::editor) compressor_sidechains: compressor_panel::sidechain::Controls,
    pub(super) auto_pan_previews: HashMap<DeviceId, auto_pan_panel::PreviewCache>,
    pub(super) reverb_previews: HashMap<DeviceId, reverb_panel::PreviewCache>,
    pub(super) overdrive_previews: HashMap<DeviceId, overdrive_panel::PreviewCache>,
    pub(super) overdrive_graphs: HashMap<DeviceId, overdrive_panel::GraphControl>,
    pub(super) rattle_previews: HashMap<DeviceId, rattle_panel::PreviewCache>,
    pub(super) eq8_graph_gestures: HashMap<DeviceId, eq8_panel::GraphGestureState>,
    pub(super) eq8_audition_toggles: HashMap<DeviceId, Eq8AuditionToggle>,
    pub(in crate::editor) spectrum_results: HashMap<
        hex_audio::spectrum_worker::SpectrumTargetId,
        hex_audio::spectrum_worker::PublishedSpectra,
    >,
    /// Retained per-tick read buffer for the latest spectrum pairs.
    pub(in crate::editor) spectrum_scratch: Vec<hex_audio::spectrum_worker::PublishedSpectra>,
    pub(super) add_menu_open: bool,
    pub(super) add_menu_target: Option<device_add::Target>,
    pub(super) add_search: Option<device_picker::Search>,
    pub(super) message: Option<String>,
    pub(in crate::editor) active: bool,
    pub(in crate::editor) gesture: hex_session::device_gesture::GestureClient,
    pub(in crate::editor) gesture_barriers: std::collections::VecDeque<Request>,
    pub(in crate::editor) catalog_barrier: super::super::device_gesture::CatalogBarrierState,
    pub(in crate::editor) insert_confirmation:
        Option<super::super::plugin_effect::DeviceConfirmation>,
    pub(in crate::editor) confirmation_restore_focus:
        Option<super::super::plugin_effect::DeviceConfirmationFocusRestore>,
    /// Vst3 devices the current output reports as degraded.
    pub(in crate::editor) degraded_plugins: crate::editor_backend::DegradedDevices,
    /// Degraded vst3 devices with an in-flight per-plugin reload request.
    pub(in crate::editor) reloading_plugins: std::collections::BTreeSet<DeviceId>,
    pub(in crate::editor) plugin_editor_windows: Vec<u64>,
    pub(in crate::editor) pending_plugin_effect:
        Option<super::super::plugin_effect::PendingPluginEffect>,
    pub(in crate::editor) failed_plugin_proposal:
        Option<super::super::plugin_effect::FailedPluginProposal>,
    pub(in crate::editor) proposed_plugin_action_focus: Option<gpui::FocusHandle>,
    pub(in crate::editor) sampler_load: super::super::sampler_load::State,
    pub(in crate::editor) sampler_parameter: super::super::sampler_parameter::State,
    // Streamed documents replace their sampler output stopped; this frame
    // resumes playback once that edit settles (Pulse #3784).
    pub(in crate::editor) sampler_edit_resume: Option<u64>,
    pub(in crate::editor) sample_recorder: super::super::sample_recorder::Recorder,
    pub(in crate::editor) project_spectrum: super::super::project_spectrum::ProjectSpectrumState,
    // One existing read-only view per transient protocol, never a prepared graph/resource set.
    pub(in crate::editor) resident_preview: Option<hex_session::service::DevicePreview>,
    pub(in crate::editor) arrangement_drop_preview:
        Option<super::super::arrangement_device_drop::ArrangementDevicePreview>,
    pub(in crate::editor) active_native_drag: Option<drag::NativeDeviceDrag>,
    pub(in crate::editor) preset_dialog_open: bool,
}

pub(in crate::editor) struct OperatorPreviewInventory {
    pub(in crate::editor) generation: u64,
    pub(in crate::editor) revision: u64,
    pub(in crate::editor) devices: Arc<HashSet<DeviceId>>,
}

#[derive(Clone)]
pub(in crate::editor) struct DeviceDragSelection {
    pub(in crate::editor) generation: u64,
    pub(in crate::editor) revision: u64,
    pub(in crate::editor) owner: DevicePanelOwner,
    pub(in crate::editor) device: DeviceId,
    pub(in crate::editor) selected: hex_project::DeviceSelectionIds,
}

/// A gap in the current owner's chain; `before: None` is the chain end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) struct InsertionPoint {
    pub(in crate::editor) before: Option<DeviceId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::editor) enum DevicePanelOwner {
    Main,
    Track(TrackId),
    Return(hex_project::ReturnId),
    Source(hex_project::SourceId),
}

impl DevicePanelState {
    pub(in crate::editor) fn select_only(&mut self, device: Option<DeviceId>) {
        self.cancel_duplicate_selection_update();
        self.selected_device = device;
        self.selected_others.clear();
        self.selection_anchor = device;
        self.insertion = None;
    }

    pub(in crate::editor) fn select_insertion(&mut self, before: Option<DeviceId>) {
        self.select_only(None);
        self.insertion = Some(InsertionPoint { before });
    }

    pub(in crate::editor) fn insertion_is(&self, before: Option<DeviceId>) -> bool {
        self.insertion == Some(InsertionPoint { before })
    }

    /// The insertion point's `before` while it still names a gap in `order`.
    pub(in crate::editor) fn insertion_before(
        &self,
        order: &[DeviceId],
    ) -> Option<Option<DeviceId>> {
        self.insertion
            .map(|point| point.before)
            .filter(|before| before.is_none_or(|id| order.contains(&id)))
    }

    /// Arrow-key target: the neighbor of the primary selection or of the
    /// insertion gap. With neither, the chain is entered from its near end.
    pub(in crate::editor) fn step_target(
        &self,
        order: &[DeviceId],
        forward: bool,
    ) -> Option<DeviceId> {
        let position = |id: DeviceId| order.iter().position(|item| *item == id);
        let index = if let Some(before) = self.insertion_before(order) {
            // The device after the gap, or one past the end.
            let next = before.map_or(Some(order.len()), position)?;
            if forward { next } else { next.checked_sub(1)? }
        } else if let Some(current) = self.selected_device.and_then(position) {
            if forward {
                current + 1
            } else {
                current.checked_sub(1)?
            }
        } else if forward {
            0
        } else {
            order.len().checked_sub(1)?
        };
        order.get(index).copied()
    }

    pub(in crate::editor) fn is_selected(&self, device: DeviceId) -> bool {
        self.selected_device == Some(device) || self.selected_others.contains(&device)
    }

    pub(in crate::editor) fn reconcile_selection(&mut self, alive: &HashSet<DeviceId>) {
        self.insertion = self
            .insertion
            .filter(|point| point.before.is_none_or(|id| alive.contains(&id)));
        self.selected_device = self.selected_device.filter(|id| alive.contains(id));
        self.selected_others.retain(|id| alive.contains(id));
        self.selection_anchor = self.selection_anchor.filter(|id| alive.contains(id));
        if self.selected_device.is_none() {
            self.selected_device = self
                .selected_others
                .iter()
                .copied()
                .min_by_key(|id| id.value());
            if let Some(primary) = self.selected_device {
                self.selected_others.remove(&primary);
            }
        }
        if self.selection_anchor.is_none() {
            self.selection_anchor = self.selected_device;
        }
    }

    pub(in crate::editor) fn select_click(
        &mut self,
        device: DeviceId,
        order: &[DeviceId],
        modifiers: gpui::Modifiers,
    ) {
        if !order.contains(&device) {
            return;
        }
        self.cancel_duplicate_selection_update();
        self.insertion = None;
        if modifiers.shift {
            let anchor = self
                .selection_anchor
                .or(self.selected_device)
                .unwrap_or(device);
            let Some(a) = order.iter().position(|id| *id == anchor) else {
                self.select_only(Some(device));
                return;
            };
            let Some(b) = order.iter().position(|id| *id == device) else {
                return;
            };
            if modifiers.platform {
                if let Some(previous) = self.selected_device {
                    self.selected_others.insert(previous);
                }
            } else {
                self.selected_others.clear();
            }
            self.selected_others
                .extend(order[a.min(b)..=a.max(b)].iter().copied());
            self.selected_others.remove(&device);
            self.selected_device = Some(device);
            self.selection_anchor = Some(anchor);
        } else if modifiers.platform {
            if self.is_selected(device) {
                self.selected_others.remove(&device);
                if self.selected_device == Some(device) {
                    self.selected_device = order
                        .iter()
                        .copied()
                        .find(|id| self.selected_others.contains(id));
                    if let Some(primary) = self.selected_device {
                        self.selected_others.remove(&primary);
                    }
                }
                self.selection_anchor = self.selected_device;
            } else {
                if let Some(previous) = self.selected_device {
                    self.selected_others.insert(previous);
                }
                self.selected_device = Some(device);
                self.selection_anchor = Some(device);
            }
        } else {
            self.select_only(Some(device));
        }
    }
    pub(in crate::editor) fn add_menu_is_open(&self, state: Option<&ViewState>) -> bool {
        self.add_menu_open
            && self
                .owner
                .is_some_and(|owner| state.is_some_and(|state| owner_exists(state, owner)))
    }

    /// A sidebar pointer action leaves descendant editors on its owning card.
    pub(in crate::editor) fn focus_action_card(
        &mut self,
        device: DeviceId,
        window: &mut Window,
        cx: &App,
    ) {
        self.chain_window.focus(device, cx).focus(window);
    }

    /// A menu selecting another card restores that card only when a card owned
    /// focus. Nested numeric/text/graph and browser focus remain untouched.
    pub(in crate::editor) fn comparison_return_focus(
        &mut self,
        device: DeviceId,
        window: &Window,
        cx: &App,
    ) -> Option<gpui::FocusHandle> {
        let previous = self.selected_device?;
        self.chain_window
            .device_is_focused(previous, window)
            .then(|| self.chain_window.focus(device, cx))
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in crate::editor) fn has_control_for(&self, device: DeviceId) -> bool {
        self.controls.keys().any(|key| key.device == device)
    }

    pub(in crate::editor) fn has_pending_draft(&self, cx: &App) -> bool {
        self.controls.values().any(|control| match control {
            ParameterControl::Float { entity, .. } => entity.read_with(cx, |control, _| {
                control.is_interacting() || control.is_editing()
            }),
            ParameterControl::Bool { .. } | ParameterControl::Choice { .. } => false,
        }) || self.send_controls.values().any(|control| {
            control.entity.read_with(cx, |control, _| {
                control.is_interacting() || control.is_editing()
            })
        }) || self.vst3_parameters.has_pending_draft(cx)
    }
}

pub(in crate::editor) fn cache_device_metadata(
    panel: &mut DevicePanelState,
    device: &hex_project::Device,
    class: NativeDeviceClass,
) {
    let current = panel
        .views
        .get(&device.id())
        .is_some_and(|(cached_class, state, _)| *cached_class == class && state == device.state());
    if !current {
        let metadata =
            hex_engine::native_device_view_for_class(class, device.state()).map_err(|error| {
                hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
            });
        panel
            .views
            .insert(device.id(), (class, device.state().clone(), metadata));
    }
}

pub(in crate::editor) fn control_style() -> ControlStyle {
    theme::control_style()
}

// Reuse the canonical device-side balance display/parser for track mixer fields.
pub(in crate::editor) fn balance_spec(spec: NumericSpec) -> NumericSpec {
    numeric_display::NumericDisplayCodec::Balance.apply(spec)
}

pub(in crate::editor) fn device_layout_style() -> ControlStyle {
    theme::device_style(theme::DeviceStyleRole::Layout)
}

/// A source-owned Sampler edit retains its visible owner until its terminal.
pub(in crate::editor) fn source_sampler_edit_active(view: &EditorView) -> bool {
    matches!(view.device_panel.owner, Some(DevicePanelOwner::Source(_)))
        && (view.sampler_parameter_pending()
            || view.device_panel.sampler_parameter.numeric_draft_active())
}

/// Commit every live numeric draft before a save, panel transition, or destructive edit.
/// A bad typed value stays in its field and blocks the caller.
pub(in crate::editor) fn finalize_controls(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> bool {
    finalize_controls_with_excluded_graph(view, cx, None)
}

/// Commit every live numeric draft and release other graph captures while
/// preserving the pointer capture of the graph handling the current input event.
pub(super) fn finalize_controls_except_graph(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    excluded_graph: gpui::EntityId,
) -> bool {
    finalize_controls_with_excluded_graph(view, cx, Some(excluded_graph))
}

fn finalize_controls_with_excluded_graph(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    excluded_graph: Option<gpui::EntityId>,
) -> bool {
    if source_sampler_edit_active(view) {
        view.error = Some("finish the sampler edit before changing the source view".into());
        cx.notify();
        return false;
    }
    if source_manager::naming(view) {
        view.error = Some("finish or cancel the source name before continuing".into());
        cx.notify();
        return false;
    }
    finalize_controls_except(view, cx, &[], excluded_graph)
}

/// Finish every device edit except controls whose pointer-down must continue
/// into a new gesture in the same update (for example an Operator shell row
/// that is changing which oscillator is shown in the center panel) and an
/// optional response graph whose active pointer capture initiated this check.
pub(super) fn finalize_controls_except(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    excluded: &[ParameterKey],
    excluded_graph: Option<gpui::EntityId>,
) -> bool {
    retire_removed_send_controls(view, cx);
    // A canonical replacement can arrive before the next render reconciles
    // cached widgets. Retire those drafts before synchronous owner submission.
    let stale: Vec<_> = view
        .device_panel
        .controls
        .iter()
        .filter_map(|(key, control)| (!control.binding().current(view, true)).then_some(*key))
        .collect();
    for key in stale {
        if let Some(mut control) = view.device_panel.controls.remove(&key) {
            cancel_control(view, cx, key, &mut control);
        }
        view.device_panel.pending_values.remove(&key);
    }
    let mut first_error = None;
    let active = view
        .device_panel
        .gesture
        .snapshot()
        .map(|snapshot| snapshot.target);
    let mut controls = Vec::new();
    for (key, control) in &view.device_panel.controls {
        if excluded.contains(key) {
            continue;
        }
        if let ParameterControl::Float { entity, lease, .. } = control {
            controls.push((
                GestureTarget::DeviceParameter {
                    device: key.device,
                    parameter: key.parameter,
                },
                entity.clone(),
                Some((*key, lease.clone())),
            ));
        }
    }
    for (key, control) in &view.device_panel.send_controls {
        controls.push((
            GestureTarget::ReturnSend {
                track: key.track,
                return_track: key.return_track,
            },
            control.entity.clone(),
            None,
        ));
    }
    controls.extend(
        crate::editor::track_gain::controls_for_finalize(view, cx)
            .into_iter()
            .map(|(target, entity)| (target, entity, None)),
    );
    controls.sort_by_key(|(target, _, _)| u8::from(Some(*target) != active));
    for (target, entity, parameter_key) in controls {
        let finalized =
            match entity.update(cx, |control, cx| control.prepare_finish_edit_for_owner(cx)) {
                Ok(value) => value,
                Err(error) => {
                    first_error.get_or_insert_with(|| error.to_string());
                    continue;
                }
            };
        if let Some(finalized) = finalized {
            let active_gesture_matches = match parameter_key.as_ref() {
                Some((_, lease)) => gesture_adapter::owns_numeric_gesture(view, target, lease),
                None => {
                    view.device_panel
                        .gesture
                        .snapshot()
                        .is_some_and(|snapshot| snapshot.target == target)
                        && !view.device_panel.gesture.has_pending_terminal()
                }
            };
            if finalized.previous != finalized.value || active_gesture_matches {
                let result = match parameter_key.as_ref() {
                    Some((_, lease)) => {
                        gesture_adapter::commit_bound_numeric_edit(view, target, lease, &finalized)
                    }
                    None => commit_owned_numeric_edit(view, target, &finalized),
                };
                if let Err(error) = result {
                    first_error.get_or_insert_with(|| error.to_owned());
                    continue;
                }
                if let Some((key, _)) = parameter_key.as_ref() {
                    view.device_panel
                        .pending_values
                        .insert(*key, ParameterValue::Float(finalized.value as f32));
                }
            }
            if let Some((key, _)) = parameter_key.as_ref() {
                bindings::fence_finished_delivery(view, cx, *key, entity.entity_id());
            }
            entity.update(cx, |control, cx| {
                control.accept_finish_edit_for_owner(finalized, cx)
            });
        }
    }
    if view.device_panel.vst3_parameters.has_pending_text_edit(cx) {
        first_error.get_or_insert_with(|| {
            "finish or cancel the vst3 parameter value edit first".to_owned()
        });
    }
    if let Some(error) = first_error {
        view.error = Some(
            format!("finish the device value edit: {error}")
                .to_lowercase()
                .into(),
        );
        false
    } else {
        release_active_graph_captures(view, cx, excluded_graph);
        true
    }
}

fn release_active_graph_captures(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    excluded_graph: Option<gpui::EntityId>,
) {
    for panel in view.device_panel.eq_panels.values_mut() {
        if Some(panel.entity_id()) != excluded_graph {
            panel.release_pointer_capture(cx);
        }
    }
    let mut excluded_eq8_device = None;
    for (&device, graph) in &mut view.device_panel.eq8_graphs {
        if Some(graph.entity_id()) == excluded_graph {
            excluded_eq8_device = Some(device);
        } else {
            graph.release_pointer_capture(cx);
        }
    }
    if let Some(device) = excluded_eq8_device {
        view.device_panel
            .eq8_graph_gestures
            .retain(|&gesture_device, _| gesture_device == device);
    } else {
        view.device_panel.eq8_graph_gestures.clear();
    }
    for panel in view.device_panel.delay_panels.values_mut() {
        if Some(panel.entity_id()) != excluded_graph {
            panel.release_pointer_capture(cx);
        }
    }
    for graph in view.device_panel.overdrive_graphs.values() {
        if Some(graph.entity_id()) != excluded_graph {
            graph.release_pointer_capture(cx);
        }
    }
    for preview in view.device_panel.operator_previews.values() {
        if let Some(control) = preview.filter_response.as_ref()
            && Some(control.entity_id()) != excluded_graph
        {
            control.release_pointer_capture(cx);
        }
    }
}

pub(in crate::editor) fn retire_removed_send_controls(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) {
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let tracks: HashSet<_> = state
        .project
        .tracks()
        .iter()
        .map(hex_project::Track::id)
        .collect();
    let returns: HashSet<_> = state
        .project
        .return_tracks()
        .iter()
        .map(hex_project::ReturnTrack::id)
        .collect();
    let stale: Vec<_> = view
        .device_panel
        .send_controls
        .keys()
        .filter(|key| !tracks.contains(&key.track) || !returns.contains(&key.return_track))
        .copied()
        .collect();
    for key in stale {
        let target = GestureTarget::ReturnSend {
            track: key.track,
            return_track: key.return_track,
        };
        if let Some(control) = view.device_panel.send_controls.remove(&key) {
            control.entity.update(cx, |control, cx| control.cancel(cx));
        }
        if view
            .device_panel
            .gesture
            .snapshot()
            .is_some_and(|snapshot| {
                snapshot.target == target && !view.device_panel.gesture.has_pending_terminal()
            })
        {
            let _ = view.cancel_device_gesture();
        }
    }
}

/// Serialize a return-owned project edit after any active native parameter edit.
pub(in crate::editor) fn enqueue_return_edit(
    view: &mut EditorView,
    command: Command,
    cx: &mut Context<EditorView>,
) -> bool {
    finalize_controls(view, cx) && view.enqueue_device_barrier(Request::Edit(command))
}

pub(in crate::editor) fn clear_owner_state(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    full_reset: bool,
    next_owner: Option<DevicePanelOwner>,
) {
    if full_reset {
        cancel_all_controls(view, cx);
        cancel_send_controls(view, cx);
        operator_panel::preview::cancel_all(view, cx);
        view.device_panel.operator_preview_inventory = None;
    } else {
        operator_panel::preview::cancel_interactions(view, cx);
        operator_panel::envelope::cancel_all(view, cx);
        operator_panel::harmonics::retire(view, cx, |_| false);
        retire_removed_eq8_graphs(view, cx, &HashSet::new());
    }
    let delay_panels: Vec<_> = view.device_panel.delay_panels.drain().collect();
    for (_, panel) in delay_panels {
        panel.cancel(cx);
    }
    view.device_panel.controls.clear();
    view.device_panel.views.clear();
    view.device_panel.pending_values.clear();
    view.device_panel.operator_pages.clear();
    view.device_panel.operator_navigation_focus.clear();
    view.device_panel.operator_section_focus.clear();
    view.device_panel.eq8_panels.clear();
    view.device_panel.eq_panels.clear();
    view.device_panel.eq8_audition_toggles.clear();
    view.device_panel.eq8_graphs.clear();
    view.device_panel.eq8_responses.clear();
    view.device_panel.saturator_previews.clear();
    view.device_panel.compressor_previews.clear();
    view.device_panel.filter_previews.clear();
    view.device_panel.compressor_displays.clear();
    if full_reset {
        view.device_panel.compressor_modes.clear();
    }
    view.device_panel.auto_pan_previews.clear();
    view.device_panel.reverb_previews.clear();
    view.device_panel.overdrive_previews.clear();
    view.device_panel.overdrive_graphs.clear();
    view.device_panel.rattle_previews.clear();
    view.device_panel.meter_states.clear();
    view.device_panel.meter_views.clear();
    view.device_panel.hovered_meter = None;
    view.device_panel.eq8_graph_gestures.clear();
    view.device_panel.spectrum_results.clear();
    view.device_panel.chain_window = Default::default();
    view.device_panel.parameter_rows.clear();
    view.device_panel.package_schemas.clear();
    view.device_panel.package_snapshots.clear();
    view.device_panel.owner = next_owner;
    view.device_panel.reconciled_track_chain = None;
    view.device_panel.drag_selection = None;
    view.device_panel.select_only(None);
    if full_reset {
        if let Some(audio) = view.audio.as_mut() {
            let _ = audio.control.spectrum_clear();
            let _ = audio.control.audition_cancel();
        }
        view.device_panel.collapsed.clear();
        view.device_panel.generation = view.project_generation;
    } else {
        view.device_panel.message = None;
    }
}

pub(in crate::editor) fn clear_eq8_audition_gesture_flags(view: &mut EditorView) {
    for panel in view.device_panel.eq8_panels.values_mut() {
        panel.audition_gesture_active = false;
    }
}

pub(in crate::editor) fn owner_exists(state: &ViewState, owner: DevicePanelOwner) -> bool {
    match owner {
        DevicePanelOwner::Main => true,
        DevicePanelOwner::Track(track) => state.project.track(track).is_some(),
        DevicePanelOwner::Return(return_track) => state
            .project
            .return_tracks()
            .iter()
            .any(|candidate| candidate.id() == return_track),
        DevicePanelOwner::Source(source) => state.project.native_source(source).is_some(),
    }
}

/// Applied display custody can lag selection while a control is finalized.
pub(in crate::editor) fn displayed_owner(view: &EditorView) -> Option<DevicePanelOwner> {
    view.device_panel.owner
}

pub(in crate::editor) fn source_owner_current(
    view: &EditorView,
    source: hex_project::SourceId,
) -> bool {
    if !view.audio_detail.clip_view_open
        || !view.device_panel.active
        || view.device_panel.owner != Some(DevicePanelOwner::Source(source))
    {
        return false;
    }
    let Some(state) = view.state.as_ref() else {
        return false;
    };
    view.device_panel
        .source_manager
        .source_view
        .is_some_and(|(generation, selected, anchor)| {
            generation == view.project_generation
                && selected == source
                && anchor == effect_bus::selected_track_owner(view, state)
        })
        && state.project.native_source(source).is_some()
        && !state
            .project
            .tracks()
            .iter()
            .any(|track| track.source_id() == Some(source))
}

pub(in crate::editor) fn cancel_all_controls(view: &mut EditorView, cx: &mut Context<EditorView>) {
    let keys: Vec<_> = view.device_panel.controls.keys().copied().collect();
    for key in keys {
        if let Some(mut control) = view.device_panel.controls.remove(&key) {
            cancel_control(view, cx, key, &mut control);
        }
    }
    operator_panel::envelope::cancel_all(view, cx);
    operator_panel::harmonics::retire(view, cx, |_| false);
    view.device_panel.views.clear();
    view.device_panel.pending_values.clear();
    retire_removed_eq8_graphs(view, cx, &HashSet::new());
}

pub(in crate::editor) fn retire_removed_eq8_graphs(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    alive: &HashSet<DeviceId>,
) {
    let stale_eq: Vec<_> = view
        .device_panel
        .eq_panels
        .keys()
        .filter(|device| !alive.contains(device))
        .copied()
        .collect();
    for device in stale_eq {
        if let Some(panel) = view.device_panel.eq_panels.remove(&device) {
            panel.cancel(cx);
        }
    }
    let stale: Vec<_> = view
        .device_panel
        .eq8_graphs
        .keys()
        .filter(|device| !alive.contains(device))
        .copied()
        .collect();
    for device in stale {
        if let Some(graph) = view.device_panel.eq8_graphs.remove(&device) {
            graph.entity.update(cx, |graph, cx| graph.cancel(cx));
        }
        view.device_panel.eq8_responses.remove(&device);
        view.device_panel.eq8_panels.remove(&device);
        view.device_panel.eq8_audition_toggles.remove(&device);
        view.device_panel.eq8_graph_gestures.remove(&device);
    }
}

pub(in crate::editor) fn clear_removed_processor_audition_target(
    view: &mut EditorView,
    owner: hex_engine::ProcessorChainOwner,
    identities: &HashMap<DeviceId, DeviceControlIdentity>,
) {
    let stale = view.audio.as_ref().and_then(|audio| {
        audio.control.audition_desired().map(|target| {
            target.identity.owner == owner
                && !identities.iter().any(|(device, identity)| {
                    device.value() == target.identity.device_id
                        && identity.class == NativeDeviceClass::Effect
                        && identity.type_id == target.identity.device_type
                        && matches!(identity.type_id.as_str(), Ok("hex.eq8" | "hex.eq"))
                })
        })
    }) == Some(true);
    if stale && let Some(audio) = view.audio.as_mut() {
        let _ = audio.control.audition_cancel();
    }
}

pub(in crate::editor) fn cancel_send_controls(view: &mut EditorView, cx: &mut Context<EditorView>) {
    let controls = std::mem::take(&mut view.device_panel.send_controls);
    for (_, control) in controls {
        control.entity.update(cx, |control, cx| control.cancel(cx));
    }
}

/// Keep only previews belonging to live effects of the matching registered type.
pub(in crate::editor) fn retire_transfer_previews(
    panel: &mut DevicePanelState,
    type_of: impl Fn(DeviceId) -> Option<hex_lattice_contract::DeviceTypeId>,
) {
    panel.delay_panels.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.delay"))
    });
    panel.saturator_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.saturator"))
    });
    panel.filter_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.filter"))
    });
    panel.compressor_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.compressor"))
    });
    panel.compressor_modes.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.compressor"))
    });
    panel.compressor_displays.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.compressor"))
    });
    panel.overdrive_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.overdrive"))
    });
    panel.overdrive_graphs.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.overdrive"))
    });
    panel.rattle_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.rattle"))
    });
    panel.auto_pan_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.auto-pan"))
    });
    panel.reverb_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.reverb"))
    });
}

#[cfg(all(test, feature = "ui-test-support"))]
mod graph_capture_tests {
    use super::super::tests::{actor_editor_window, pump_until, receive_service_event};
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};
    use hex_engine::eq::{EQ_TYPE_ID, EqState};
    use hex_project::{Command, Created};
    use hex_session::service::{Request, Service};

    fn read_eq(view: &EditorView, device: DeviceId) -> EqState {
        EqState::decode(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state(),
        )
        .unwrap()
    }

    fn settled(view: &EditorView) -> bool {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture_barriers.is_empty()
            && view.device_panel.gesture.snapshot().is_none()
    }

    #[gpui::test]
    fn eq_graph_create_drag_and_mid_drag_undo_barrier_preserve_and_release_capture(
        cx: &mut TestAppContext,
    ) {
        #[cfg(target_os = "macos")]
        cx.use_native_text_system().unwrap();
        cx.update(|cx| {
            theme::load_fonts(cx).unwrap();
            hex_widgets::bind_numeric_control_keys(cx);
        });
        let service = Service::new(48_000, 16, 8_000_000).unwrap();
        let _ = receive_service_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack { name: "eq".into() }))
            .unwrap();
        let Created::Track(track) = receive_service_event(&service).created.unwrap() else {
            panic!("expected created track");
        };
        service
            .submit(Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Effect,
                type_id: Box::new(EQ_TYPE_ID),
                before: None,
            })
            .unwrap();
        let state = receive_service_event(&service).state;
        let device = state.project.track(track).unwrap().devices()[0].id();

        let (handle, editor) = actor_editor_window(cx, service, state, track);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.simulate_resize(size(px(1500.0), px(650.0)));
        visual.run_until_parked();

        let graph = editor.read_with(cx, |view, _| eq_panel::graph_for_test(view, device));
        let plot = graph.read_with(cx, |g, _| g.plot_bounds().expect("painted eq plot"));
        let point_at = |x: f32, y: f32| {
            point(
                plot.left() + plot.size.width * x,
                plot.bottom() - plot.size.height * y,
            )
        };
        let modifiers = Modifiers::default();

        // 1. Double-click to create a node on the newly added EQ, drag while held, then release.
        let create_start = point_at(0.45, 0.50);
        let create_end = point_at(0.50, 0.60);
        visual.simulate_click(create_start, modifiers);
        visual.simulate_event(gpui::MouseDownEvent {
            position: create_start,
            modifiers,
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        visual.simulate_mouse_move(create_end, Some(MouseButton::Left), modifiers);
        visual.simulate_event(gpui::MouseUpEvent {
            position: create_end,
            modifiers,
            button: MouseButton::Left,
            click_count: 2,
        });
        pump_until(&editor, cx, |view| {
            settled(view) && read_eq(view, device).nodes.len() == 1
        });
        let created = editor.read_with(cx, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert!(
                view.device_panel.message.is_none(),
                "{:?}",
                view.device_panel.message
            );
            read_eq(view, device).nodes[0]
        });

        // 2. Click and drag that newly created node across multiple moves and commit on release.
        let node_pos = graph
            .read_with(cx, |g, _| g.node_position(created.id.value()))
            .expect("created node position");
        let drag_start = point_at(node_pos[0], node_pos[1]);
        let drag_mid = point_at(0.65, 0.70);
        let drag_end = point_at(0.72, 0.78);
        visual.simulate_mouse_down(drag_start, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(drag_mid, Some(MouseButton::Left), modifiers);
        visual.simulate_mouse_move(drag_end, Some(MouseButton::Left), modifiers);
        visual.simulate_mouse_up(drag_end, MouseButton::Left, modifiers);
        pump_until(&editor, cx, settled);
        let moved = editor.read_with(cx, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert!(
                view.device_panel.message.is_none(),
                "{:?}",
                view.device_panel.message
            );
            read_eq(view, device).nodes[0]
        });
        assert!(
            moved.frequency_hz > created.frequency_hz * 1.5
                && moved.gain_db > created.gain_db + 3.0,
            "dragging a newly created eq node must move its frequency and gain: {created:?} -> {moved:?}"
        );

        // 3. External barrier (Undo) mid-drag still releases pointer capture cleanly.
        let moved_pos = graph
            .read_with(cx, |g, _| g.node_position(moved.id.value()))
            .expect("moved node position");
        visual.simulate_mouse_down(
            point_at(moved_pos[0], moved_pos[1]),
            MouseButton::Left,
            modifiers,
        );
        visual.simulate_mouse_move(point_at(0.30, 0.30), Some(MouseButton::Left), modifiers);
        visual.run_until_parked();
        assert!(graph.read_with(cx, |g, _| g.is_dragging()));
        editor.update(cx, |view, cx| {
            view.history_action(Request::Undo, cx);
        });
        assert!(
            !graph.read_with(cx, |g, _| g.is_dragging()),
            "external Undo barrier must release eq graph pointer capture"
        );
        visual.simulate_mouse_move(point_at(0.20, 0.20), Some(MouseButton::Left), modifiers);
        visual.simulate_mouse_up(point_at(0.20, 0.20), MouseButton::Left, modifiers);
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_ne!(
                view.device_panel.message.as_deref(),
                Some("device gesture is already finishing")
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_panel_state_defaults_empty_and_clears_selection() {
        let mut state = DevicePanelState::default();
        assert!(state.owner.is_none());
        assert!(state.selected_device.is_none());
        state.select_insertion(None);
        assert!(state.insertion_is(None));
        state.select_only(None);
        assert!(!state.insertion_is(None));
    }
}

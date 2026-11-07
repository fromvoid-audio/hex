//! Compact descriptor-driven native device chain in the shared detail pane.
use super::{EditorView, drag};
use crate::theme;
use gpui::{
    App, Bounds, Context, Div, ElementId, Entity, MouseButton, Pixels, Stateful, Subscription,
    Window, div, prelude::*, px, rgb,
};
use hex_engine::{NativeDeviceClass, NativeDeviceParameterView, NativeDeviceView};
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
    sync::Arc,
};

mod auto_pan_panel;
mod bindings;
mod bypass;
mod delay_panel;
mod overdrive_panel;
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
pub(super) use compressor_panel::sidechain::drop_track_key;
mod eq8_panel;
mod eq_panel;
mod limiter_panel;
pub(super) use eq_panel::after_scroll_worker_event as eq_scroll_after_worker_event;
pub(super) use eq_panel::reconcile_spectrum as reconcile_eq_spectrum;
mod device_meter;
mod gate_panel;
mod keyboard;
mod navigation;
mod reverb_panel;
pub(super) use keyboard::{bind_keys as bind_keyboard_keys, key_up as selection_key_up};
mod numeric_display;
pub(super) use navigation::{bottom_bar, capture_pointer_down as capture_navigation_pointer_down};
mod operator_panel;
mod parameter_rows;
pub(super) mod vst3_parameters;
pub(super) use operator_panel::bind_keys as bind_operator_keys;
mod return_drag;
mod sampler_overview;
pub(super) use sampler_overview::{cancel_sampler_boundary, sampler_boundary_preview};
mod saturator_panel;
mod send_controls;
mod snapshot;
pub(super) mod spectrum_display;
mod utility_panel;
pub(super) use send_controls::finalize_send_controls_for_track;
pub(super) use send_controls::return_send_numeric_control;

const DEVICE_PANEL_BORDER_WIDTH: f32 = 1.0;
const DEVICE_PANEL_HEADER_HEIGHT: f32 = 22.0;
const DEVICE_CHAIN_PADDING: f32 = 3.0;
pub(super) const DEVICE_PANEL_HEIGHT: f32 = crate::device_composition::FRAME_HEIGHT
    + DEVICE_PANEL_BORDER_WIDTH
    + DEVICE_PANEL_HEADER_HEIGHT
    + DEVICE_CHAIN_PADDING * 2.0;
const DEVICE_FRAME_WIDTH: f32 = crate::device_composition::GENERIC_FRAME_WIDTH;
pub(super) const UTILITY_FRAME_WIDTH: f32 = crate::device_composition::UTILITY_FRAME_WIDTH;
const DEVICE_FRAME_HEIGHT: f32 = crate::device_composition::FRAME_HEIGHT;
const DEVICE_FRAME_BODY_HEIGHT: f32 = crate::device_composition::BODY_HEIGHT;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ParameterKey {
    pub(super) device: DeviceId,
    pub(super) parameter: hex_lattice_contract::ParameterId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SendControlKey {
    track: TrackId,
    return_track: hex_project::ReturnId,
}

struct SendControl {
    entity: Entity<NumericControl>,
    _subscription: Subscription,
}

struct Eq8AuditionToggle {
    entity: Entity<Toggle>,
    _subscription: Subscription,
}

#[derive(Clone, Copy)]
struct DeviceControlIdentity {
    class: NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
}

#[derive(Default)]
pub(super) struct DevicePanelState {
    bypass: bypass::State,
    pub(super) owner: Option<DevicePanelOwner>,
    pub(super) main_selected: Option<u64>,
    chain_window: chain_window::ChainWindow,
    parameter_rows: HashMap<DeviceId, parameter_rows::RowWindow>,
    vst3_parameters: vst3_parameters::State,
    pub(super) navigation: navigation::Navigation,
    generation: u64,
    pub(super) selected_device: Option<DeviceId>,
    selected_others: HashSet<DeviceId>,
    selection_anchor: Option<DeviceId>,
    /// View-only chain gap chosen for paste; exclusive with device selection.
    insertion: Option<InsertionPoint>,
    pub(super) drag_copy: Option<DeviceDragCopy>,
    controls: HashMap<ParameterKey, ParameterControl>,
    meter_states: HashMap<DeviceId, device_meter::MeterState>,
    send_controls: HashMap<SendControlKey, SendControl>,
    views: HashMap<
        DeviceId,
        (
            NativeDeviceClass,
            NativeDeviceState,
            Result<NativeDeviceView, String>,
        ),
    >,
    effect_catalog: Option<Result<Vec<hex_engine::NativeDeviceCatalogEntry>, String>>,
    instrument_catalog: Option<Result<Vec<hex_engine::NativeDeviceCatalogEntry>, String>>,
    pub(super) pending_values: HashMap<ParameterKey, ParameterValue>,
    operator_pages: HashMap<DeviceId, operator_panel::OperatorPanelState>,
    operator_navigation_focus:
        HashMap<(DeviceId, operator_panel::NavigationTarget), gpui::FocusHandle>,
    operator_section_focus:
        HashMap<(DeviceId, operator_panel::Section), operator_panel::context::SectionFocus>,
    operator_context_focus: HashMap<DeviceId, HashMap<gpui::EntityId, Subscription>>,
    operator_previews: HashMap<DeviceId, operator_panel::preview::State>,
    operator_preview_inventory: Option<OperatorPreviewInventory>,
    operator_harmonics: HashMap<(DeviceId, usize), operator_panel::harmonics::Control>,
    operator_envelope_graphs:
        HashMap<operator_panel::envelope::GraphKey, operator_panel::envelope::GraphControl>,
    operator_envelope_gestures:
        HashMap<operator_panel::envelope::GraphKey, operator_panel::envelope::GraphGesture>,
    eq_panels: HashMap<DeviceId, eq_panel::Panel>,
    delay_panels: HashMap<DeviceId, delay_panel::Panel>,
    eq_spectrum: eq_panel::SpectrumSelection,
    sampler_cursor: sampler_overview::CursorState,
    eq8_panels: HashMap<DeviceId, eq8_panel::Eq8PanelState>,
    eq8_graphs: HashMap<DeviceId, eq8_panel::GraphControl>,
    eq8_responses: HashMap<DeviceId, eq8_panel::ResponseCache>,
    saturator_previews: HashMap<DeviceId, saturator_panel::PreviewCache>,
    compressor_previews: HashMap<DeviceId, compressor_panel::PreviewCache>,
    pub(super) compressor_sidechains: compressor_panel::sidechain::Controls,
    auto_pan_previews: HashMap<DeviceId, auto_pan_panel::PreviewCache>,
    reverb_previews: HashMap<DeviceId, reverb_panel::PreviewCache>,
    overdrive_previews: HashMap<DeviceId, overdrive_panel::PreviewCache>,
    eq8_graph_gestures: HashMap<DeviceId, eq8_panel::GraphGestureState>,
    eq8_audition_toggles: HashMap<DeviceId, Eq8AuditionToggle>,
    pub(super) spectrum_results: HashMap<
        hex_audio::spectrum_worker::SpectrumTargetId,
        hex_audio::spectrum_worker::PublishedSpectra,
    >,
    /// Retained per-tick read buffer for the latest spectrum pairs.
    pub(super) spectrum_scratch: Vec<hex_audio::spectrum_worker::PublishedSpectra>,
    add_menu_open: bool,
    message: Option<String>,
}

struct OperatorPreviewInventory {
    generation: u64,
    revision: u64,
    devices: Arc<HashSet<DeviceId>>,
}

#[derive(Clone)]
pub(super) struct DeviceDragCopy {
    generation: u64,
    revision: u64,
    owner: DevicePanelOwner,
    device: DeviceId,
    selected: hex_project::DeviceSelectionIds,
}

/// A gap in the current owner's chain; `before: None` is the chain end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InsertionPoint {
    before: Option<DeviceId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DevicePanelOwner {
    Main,
    Track(TrackId),
    Return(hex_project::ReturnId),
}

impl DevicePanelState {
    pub(super) fn select_only(&mut self, device: Option<DeviceId>) {
        self.selected_device = device;
        self.selected_others.clear();
        self.selection_anchor = device;
        self.insertion = None;
    }

    fn select_insertion(&mut self, before: Option<DeviceId>) {
        self.select_only(None);
        self.insertion = Some(InsertionPoint { before });
    }

    fn insertion_is(&self, before: Option<DeviceId>) -> bool {
        self.insertion == Some(InsertionPoint { before })
    }

    /// The insertion point's `before` while it still names a gap in `order`.
    fn insertion_before(&self, order: &[DeviceId]) -> Option<Option<DeviceId>> {
        self.insertion
            .map(|point| point.before)
            .filter(|before| before.is_none_or(|id| order.contains(&id)))
    }

    /// Arrow-key target: the neighbor of the primary selection or of the
    /// insertion gap. With neither, the chain is entered from its near end.
    fn step_target(&self, order: &[DeviceId], forward: bool) -> Option<DeviceId> {
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

    fn is_selected(&self, device: DeviceId) -> bool {
        self.selected_device == Some(device) || self.selected_others.contains(&device)
    }

    fn reconcile_selection(&mut self, alive: &HashSet<DeviceId>) {
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

    fn select_click(&mut self, device: DeviceId, order: &[DeviceId], modifiers: gpui::Modifiers) {
        if !order.contains(&device) {
            return;
        }
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
    pub(super) fn add_menu_is_open(&self, state: Option<&ViewState>) -> bool {
        self.add_menu_open
            && self
                .owner
                .is_some_and(|owner| state.is_some_and(|state| owner_exists(state, owner)))
    }

    /// A menu selecting another card restores that card only when a card owned
    /// focus. Nested numeric/text/graph and browser focus remain untouched.
    pub(super) fn comparison_return_focus(
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
    pub(super) fn has_control_for(&self, device: DeviceId) -> bool {
        self.controls.keys().any(|key| key.device == device)
    }

    pub(super) fn has_pending_draft(&self, cx: &App) -> bool {
        self.controls.values().any(|control| match control {
            ParameterControl::Float { entity, .. } => entity.read_with(cx, |control, _| {
                control.is_interacting() || control.is_editing()
            }),
            ParameterControl::Bool { .. } | ParameterControl::Choice { .. } => false,
        }) || self.send_controls.values().any(|control| {
            control.entity.read_with(cx, |control, _| {
                control.is_interacting() || control.is_editing()
            })
        })
    }
}

fn cache_device_metadata(
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

fn title_label(title: String, warning: bool) -> Div {
    let preferred = theme::color(if warning {
        theme::PaletteRole::Primary
    } else {
        theme::PaletteRole::Muted
    });
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_color(rgb(theme::readable_text(
            theme::color(theme::PaletteRole::Surface),
            preferred,
        )))
        .child(theme::text(title))
}

fn retained_source_path_label(track: &hex_project::Track) -> Option<&'static str> {
    track
        .retained_source_path()
        .then_some("source path · printed audio bypasses")
}

fn retained_source_path_badge(label: &'static str) -> Div {
    let background = theme::color(theme::PaletteRole::Surface);
    div()
        .debug_selector(|| "retained-source-path".into())
        .flex_shrink()
        .min_w_0()
        .truncate()
        .text_color(rgb(theme::readable_text(
            background,
            theme::color(theme::PaletteRole::SecondaryText),
        )))
        .child(theme::text(label))
}

pub(super) fn control_style() -> ControlStyle {
    theme::control_style()
}

// Reuse the canonical device-side balance display/parser for track mixer fields.
pub(super) fn balance_spec(spec: NumericSpec) -> NumericSpec {
    numeric_display::NumericDisplayCodec::Balance.apply(spec)
}

pub(super) fn device_layout_style() -> ControlStyle {
    theme::device_style(theme::DeviceStyleRole::Layout)
}

/// Commit every live numeric draft before a save, panel transition, or destructive edit.
/// A bad typed value stays in its field and blocks the caller.
pub(super) fn finalize_controls(view: &mut EditorView, cx: &mut Context<EditorView>) -> bool {
    finalize_controls_except(view, cx, &[])
}

/// Finish every device edit except controls whose pointer-down must continue
/// into a new gesture in the same update (for example an Operator shell row
/// that is changing which oscillator is shown in the center panel).
pub(super) fn finalize_controls_except(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    excluded: &[ParameterKey],
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
        .device_gesture
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
        super::track_gain::controls_for_finalize(view, cx)
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
                    view.device_gesture
                        .snapshot()
                        .is_some_and(|snapshot| snapshot.target == target)
                        && !view.device_gesture.has_pending_terminal()
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
    if let Some(error) = first_error {
        view.error = Some(
            format!("finish the device value edit: {error}")
                .to_lowercase()
                .into(),
        );
        false
    } else {
        true
    }
}

pub(super) fn retire_removed_send_controls(view: &mut EditorView, cx: &mut Context<EditorView>) {
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
        if view.device_gesture.snapshot().is_some_and(|snapshot| {
            snapshot.target == target && !view.device_gesture.has_pending_terminal()
        }) {
            let _ = view.cancel_device_gesture();
        }
    }
}

/// Serialize a return-owned project edit after any active native parameter edit.
pub(super) fn enqueue_return_edit(
    view: &mut EditorView,
    command: Command,
    cx: &mut Context<EditorView>,
) -> bool {
    finalize_controls(view, cx) && view.enqueue_device_barrier(Request::Edit(command))
}

pub(super) fn view(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    retire_removed_send_controls(view, cx);
    if view.device_panel.generation != view.project_generation {
        // Project replacement retires the worker gesture. Drop old subscriptions
        // before stale controls can send events into the replacement project.
        cancel_all_controls(view, cx);
        cancel_send_controls(view, cx);
        view.device_panel.controls.clear();
        view.device_panel.views.clear();
        view.device_panel.pending_values.clear();
        view.device_panel.operator_pages.clear();
        view.device_panel.operator_navigation_focus.clear();
        view.device_panel.operator_section_focus.clear();
        view.device_panel.operator_context_focus.clear();
        operator_panel::preview::cancel_all(view, cx);
        view.device_panel.operator_preview_inventory = None;
        operator_panel::envelope::cancel_all(view, cx);
        operator_panel::harmonics::retire(view, cx, |_| false);
        view.device_panel.eq8_panels.clear();
        view.device_panel.eq_panels.clear();
        view.device_panel.delay_panels.clear();
        view.device_panel.eq8_audition_toggles.clear();
        view.device_panel.eq8_graphs.clear();
        view.device_panel.eq8_responses.clear();
        view.device_panel.saturator_previews.clear();
        view.device_panel.compressor_previews.clear();
        view.device_panel.auto_pan_previews.clear();
        view.device_panel.reverb_previews.clear();
        view.device_panel.overdrive_previews.clear();
        view.device_panel.eq8_graph_gestures.clear();
        view.device_panel.spectrum_results.clear();
        if let Some(audio) = view.audio.as_mut() {
            let _ = audio.control.spectrum_clear();
            let _ = audio.control.audition_cancel();
        }
        view.device_panel.chain_window = Default::default();
        view.device_panel.parameter_rows.clear();
        view.device_panel.owner = None;
        view.device_panel.drag_copy = None;
        view.device_panel.select_only(None);
        view.device_panel.generation = view.project_generation;
    }
    reconcile_eq_spectrum(view);
    sampler_overview::reconcile_cursor(view);
    if window.is_some_and(|window| !view.focus.is_focused(window)) {
        cancel_sampler_boundary(view);
    }
    let Some(state) = view.state.as_ref().cloned() else {
        return empty_panel("project is still loading");
    };
    let inventory_is_current = view
        .device_panel
        .operator_preview_inventory
        .as_ref()
        .is_some_and(|inventory| {
            inventory.generation == view.project_generation && inventory.revision == state.revision
        });
    if !inventory_is_current {
        let devices = state
            .project
            .tracks()
            .iter()
            .flat_map(|track| track.ordered_device_chain())
            .filter_map(|device| match device {
                hex_project::TrackDeviceRef::Instrument(device)
                    if device.state().type_id().as_str().ok()
                        == Some("hex.instrument.operator") =>
                {
                    Some(device.id())
                }
                _ => None,
            })
            .collect();
        view.device_panel.operator_preview_inventory = Some(OperatorPreviewInventory {
            generation: view.project_generation,
            revision: state.revision,
            devices: Arc::new(devices),
        });
    }
    let alive_operator = view
        .device_panel
        .operator_preview_inventory
        .as_ref()
        .map_or_else(
            || Arc::new(HashSet::new()),
            |inventory| Arc::clone(&inventory.devices),
        );
    operator_panel::preview::retire_removed(view, cx, alive_operator.as_ref());
    if view.selected_track.is_some() || view.focused_return.is_some() {
        view.device_panel.main_selected = None;
    }
    let desired_owner = selected_owner(view, &state);
    if desired_owner != view.device_panel.owner {
        if let Some(audio) = view.audio.as_mut() {
            let _ = audio.control.audition_cancel();
        }
        if let Some(previous) = view.device_panel.owner {
            if !owner_exists(&state, previous) {
                // A history restore or external project replacement can remove
                // an owner while its view is focused. Cancel its drafts before
                // switching; never commit an edit against a stale device ID.
                cancel_all_controls(view, cx);
                if view.device_gesture.snapshot().is_some() {
                    let _ = view.cancel_device_gesture();
                    view.device_panel.message = Some("canceling removed device edit".into());
                    return empty_panel("canceling removed device edit");
                }
            } else if !finalize_controls(view, cx) {
                view.device_panel.message = Some("finish the active device value first".into());
                return render_owner(view, window, cx, &state, previous);
            } else if view.device_gesture.snapshot().is_some() {
                view.device_panel.message = Some("finishing device edit".into());
                return render_owner(view, window, cx, &state, previous);
            }
        }
        if let Some(audio) = view.audio.as_mut() {
            let _ = audio.control.spectrum_clear();
        }
        view.device_panel.controls.clear();
        view.device_panel.views.clear();
        view.device_panel.pending_values.clear();
        view.device_panel.operator_pages.clear();
        view.device_panel.operator_navigation_focus.clear();
        view.device_panel.operator_section_focus.clear();
        view.device_panel.operator_context_focus.clear();
        operator_panel::preview::cancel_interactions(view, cx);
        operator_panel::envelope::cancel_all(view, cx);
        operator_panel::harmonics::retire(view, cx, |_| false);
        view.device_panel.eq8_panels.clear();
        view.device_panel.eq_panels.clear();
        view.device_panel.delay_panels.clear();
        view.device_panel.eq8_audition_toggles.clear();
        view.device_panel.eq8_graphs.clear();
        view.device_panel.eq8_responses.clear();
        view.device_panel.saturator_previews.clear();
        view.device_panel.compressor_previews.clear();
        view.device_panel.auto_pan_previews.clear();
        view.device_panel.reverb_previews.clear();
        view.device_panel.overdrive_previews.clear();
        view.device_panel.eq8_graph_gestures.clear();
        view.device_panel.spectrum_results.clear();
        view.device_panel.chain_window = Default::default();
        view.device_panel.parameter_rows.clear();
        view.device_panel.owner = desired_owner;
        view.device_panel.drag_copy = None;
        view.device_panel.select_only(None);
        view.device_panel.message = None;
    }
    match view.device_panel.owner {
        Some(DevicePanelOwner::Main) => {
            return_view::render(view, window, cx, &state, EffectBusOwner::Main)
        }
        Some(DevicePanelOwner::Track(track)) => render_track(view, window, cx, &state, track),
        Some(DevicePanelOwner::Return(return_track)) => return_view::render(
            view,
            window,
            cx,
            &state,
            EffectBusOwner::Return(return_track),
        ),
        None => empty_panel("select a track or return to view devices"),
    }
}

pub(super) fn clear_eq8_audition_gesture_flags(view: &mut EditorView) {
    for panel in view.device_panel.eq8_panels.values_mut() {
        panel.audition_gesture_active = false;
    }
}

fn owner_exists(state: &ViewState, owner: DevicePanelOwner) -> bool {
    match owner {
        DevicePanelOwner::Main => true,
        DevicePanelOwner::Track(track) => state.project.track(track).is_some(),
        DevicePanelOwner::Return(return_track) => state
            .project
            .return_tracks()
            .iter()
            .any(|candidate| candidate.id() == return_track),
    }
}

/// Applied display custody can lag selection while a control is finalized.
pub(super) fn displayed_owner(view: &EditorView) -> Option<DevicePanelOwner> {
    view.device_panel.owner
}

fn render_owner(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    state: &ViewState,
    owner: DevicePanelOwner,
) -> Stateful<Div> {
    match owner {
        DevicePanelOwner::Main => {
            return_view::render(view, window, cx, state, EffectBusOwner::Main)
        }
        DevicePanelOwner::Track(track) => render_track(view, window, cx, state, track),
        DevicePanelOwner::Return(return_track) => return_view::render(
            view,
            window,
            cx,
            state,
            EffectBusOwner::Return(return_track),
        ),
    }
}

fn cancel_all_controls(view: &mut EditorView, cx: &mut Context<EditorView>) {
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

pub(super) fn retire_removed_eq8_graphs(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    alive: &HashSet<DeviceId>,
) {
    view.device_panel
        .eq_panels
        .retain(|device, _| alive.contains(device));
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

fn clear_removed_processor_audition_target(
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

fn cancel_send_controls(view: &mut EditorView, cx: &mut Context<EditorView>) {
    let controls = std::mem::take(&mut view.device_panel.send_controls);
    for (_, control) in controls {
        control.entity.update(cx, |control, cx| control.cancel(cx));
    }
}

fn render_track(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    state: &ViewState,
    track_id: TrackId,
) -> Stateful<Div> {
    let Some(track) = state.project.track(track_id) else {
        return empty_panel("selected track is no longer available");
    };
    let instrument = track.instrument().cloned();
    let identities: HashMap<_, _> = track
        .ordered_device_chain()
        .map(|node| match node {
            hex_project::TrackDeviceRef::Instrument(device) => (
                device.id(),
                DeviceControlIdentity {
                    class: NativeDeviceClass::Instrument,
                    type_id: device.state().type_id(),
                },
            ),
            hex_project::TrackDeviceRef::Effect(device) => (
                device.id(),
                DeviceControlIdentity {
                    class: NativeDeviceClass::Effect,
                    type_id: device.state().type_id(),
                },
            ),
        })
        .collect();
    let alive: HashSet<_> = identities.keys().copied().collect();
    view.device_panel.reconcile_selection(&alive);

    clear_removed_processor_audition_target(
        view,
        hex_engine::ProcessorChainOwner::Track(track_id.value()),
        &identities,
    );
    view.device_panel
        .meter_states
        .retain(|id, _| identities.contains_key(id));
    let stale_keys: Vec<_> = view
        .device_panel
        .controls
        .iter()
        .filter(|(key, control)| {
            identities
                .get(&key.device)
                .is_none_or(|identity| !parameter_control_matches_identity(control, *identity))
        })
        .map(|(key, _)| *key)
        .collect();
    let mut changed_device_ids: HashSet<_> = stale_keys.iter().map(|key| key.device).collect();
    changed_device_ids.extend(view.device_panel.views.iter().filter_map(
        |(device, (class, old_state, _))| {
            identities.get(device).and_then(|identity| {
                (*class != identity.class || old_state.type_id() != identity.type_id)
                    .then_some(*device)
            })
        },
    ));
    let active_target = view
        .device_gesture
        .snapshot()
        .map(|snapshot| snapshot.target);
    snapshot::retire_invalid_owner(view, &alive);
    let stale_target_has_work = active_target.is_some_and(|target| match target {
        GestureTarget::DeviceParameter { device, .. } => !alive.contains(&device),
        GestureTarget::DeviceParameterPair { device, .. } => !alive.contains(&device),
        GestureTarget::DeviceParameterSnapshot { device }
        | GestureTarget::PluginParameter { device, .. }
        | GestureTarget::EqNodeCreation { device } => !alive.contains(&device),
        GestureTarget::ReturnSend { .. }
        | GestureTarget::TrackGain { .. }
        | GestureTarget::TrackGains { .. }
        | GestureTarget::TrackBalance { .. }
        | GestureTarget::ReturnGain { .. } => false,
    }) || stale_keys.iter().any(|key| {
        matches!(
            view.device_panel.controls.get(key),
            Some(ParameterControl::Float { entity, .. })
                if entity.read_with(cx, |control, _| {
                    control.is_interacting() || control.is_editing()
                })
        )
    });
    if !stale_keys.is_empty() {
        for key in stale_keys {
            if let Some(mut control) = view.device_panel.controls.remove(&key) {
                cancel_control(view, cx, key, &mut control);
            }
        }
        view.device_panel
            .controls
            .retain(|key, _| alive.contains(&key.device));
        if stale_target_has_work {
            view.device_panel.message = Some("device target is no longer available".into());
        } else if view.device_panel.message.as_deref()
            == Some("device target is no longer available")
        {
            view.device_panel.message = None;
        }
    }
    view.device_panel.views.retain(|id, _| alive.contains(id));
    retire_transfer_previews(&mut view.device_panel, |device| {
        identities
            .get(&device)
            .filter(|identity| identity.class == NativeDeviceClass::Effect)
            .map(|identity| identity.type_id)
    });
    view.device_panel
        .pending_values
        .retain(|key, _| alive.contains(&key.device) && !changed_device_ids.contains(&key.device));
    view.device_panel.operator_pages.retain(|device, _| {
        identities.get(device).is_some_and(|identity| {
            identity.class == NativeDeviceClass::Instrument
                && identity.type_id.as_str().ok() == Some("hex.instrument.operator")
        })
    });
    view.device_panel
        .operator_navigation_focus
        .retain(|(device, _), _| view.device_panel.operator_pages.contains_key(device));
    view.device_panel
        .operator_section_focus
        .retain(|(device, _), _| view.device_panel.operator_pages.contains_key(device));
    let alive_operator: HashSet<_> = identities
        .iter()
        .filter_map(|(device, identity)| {
            (identity.class == NativeDeviceClass::Instrument
                && identity.type_id.as_str().ok() == Some("hex.instrument.operator"))
            .then_some(*device)
        })
        .collect();
    view.device_panel
        .operator_context_focus
        .retain(|device, _| alive_operator.contains(device));
    operator_panel::envelope::retire_removed(view, cx, &alive_operator);
    operator_panel::harmonics::retire(view, cx, |key| alive_operator.contains(&key.0));
    let alive_eq8: HashSet<_> = identities
        .iter()
        .filter_map(|(device, identity)| {
            (identity.class == NativeDeviceClass::Effect
                && matches!(identity.type_id.as_str(), Ok("hex.eq8" | "hex.eq")))
            .then_some(*device)
        })
        .collect();
    retire_removed_eq8_graphs(view, cx, &alive_eq8);
    view.device_panel
        .eq8_panels
        .retain(|device, _| alive_eq8.contains(device));
    view.device_panel
        .eq8_responses
        .retain(|device, _| alive_eq8.contains(device));
    view.device_panel
        .eq8_graph_gestures
        .retain(|device, _| alive_eq8.contains(device));

    // Prepare the full read-only inventory, including unavailable descriptors
    // and unsupported-state errors, without constructing offscreen controls.
    for node in track.ordered_device_chain() {
        let (device, class) = match node {
            hex_project::TrackDeviceRef::Instrument(device) => {
                (device, NativeDeviceClass::Instrument)
            }
            hex_project::TrackDeviceRef::Effect(device) => (device, NativeDeviceClass::Effect),
        };
        cache_device_metadata(&mut view.device_panel, device, class);
    }

    parameter_rows::reconcile(view, cx);

    if view.device_panel.effect_catalog.is_none() {
        view.device_panel.effect_catalog = Some(
            hex_engine::native_device_catalog_for_class(NativeDeviceClass::Effect).map_err(
                |error| hex_session::service::text::lowercase_outside_verbatim(&error.to_string()),
            ),
        );
    }
    if view.device_panel.instrument_catalog.is_none() {
        view.device_panel.instrument_catalog = Some(
            hex_engine::native_device_catalog_for_class(NativeDeviceClass::Instrument).map_err(
                |error| hex_session::service::text::lowercase_outside_verbatim(&error.to_string()),
            ),
        );
    }
    let effect_catalog = view
        .device_panel
        .effect_catalog
        .clone()
        .unwrap_or_else(|| Ok(Vec::new()));
    let instrument_catalog = view
        .device_panel
        .instrument_catalog
        .clone()
        .unwrap_or_else(|| Ok(Vec::new()));

    let mut root = div()
        .id("device-panel")
        .debug_selector(|| "device-panel".into())
        .h(px(DEVICE_PANEL_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .relative()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .border_t_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)));

    let mut chain = chain_window::rail(view, cx)
        .id("device-chain-scroll")
        .track_scroll(&view.device_panel.chain_window.scroll)
        .relative()
        .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
        .debug_selector(|| "device-chain-scroll".into())
        .flex_1()
        .min_h_0()
        .flex()
        .p(px(DEVICE_CHAIN_PADDING))
        .overflow_x_scroll()
        .overflow_y_hidden();
    chain = navigation::rail(view, chain);
    if retained_source_path_label(track).is_some() {
        chain = chain
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)));
    }
    let chain_entity = cx.entity();
    let visible_chain_bounds = std::rc::Rc::new(std::cell::RefCell::new(None));
    let chain_visible_bounds = visible_chain_bounds.clone();
    chain = chain.on_drag_move::<drag::NativeDeviceDrag>(move |event, _, cx| {
        let Some(payload) = event
            .dragged_item()
            .downcast_ref::<drag::NativeDeviceDrag>()
        else {
            return;
        };
        let pointer = event.event.position;
        let bounds = event.bounds;
        *chain_visible_bounds.borrow_mut() = Some(bounds);
        let inside = bounds.contains(&pointer);
        if inside {
            payload.return_destination.set(None);
        }
        if !inside && payload.destination.take().is_some() {
            chain_entity.update(cx, |_, cx| cx.notify());
        }
    });

    let nodes: Vec<_> = track.ordered_device_chain().collect();
    let device_ids: Vec<_> = nodes
        .iter()
        .map(|node| match node {
            hex_project::TrackDeviceRef::Instrument(device)
            | hex_project::TrackDeviceRef::Effect(device) => device.id(),
        })
        .collect();
    let geometry: Vec<_> = nodes
        .iter()
        .map(|node| {
            let (device, class) = match node {
                hex_project::TrackDeviceRef::Instrument(device) => {
                    (*device, NativeDeviceClass::Instrument)
                }
                hex_project::TrackDeviceRef::Effect(device) => (*device, NativeDeviceClass::Effect),
            };
            (
                device.id(),
                device_card_width(class, device.state().type_id()),
            )
        })
        .collect();
    view.device_panel.chain_window.prepare(&geometry);
    let spectrum_identities: Vec<_> = nodes
        .iter()
        .filter_map(|node| {
            let hex_project::TrackDeviceRef::Effect(device) = node else {
                return None;
            };
            (eq_panel::is_spectrum_eq(device.state().type_id())
                && chain_window::must_mount(view, device.id(), window, cx))
            .then(|| {
                hex_engine::PostNodeIdentity::new(
                    hex_engine::ProcessorChainOwner::Track(track_id.value()),
                    device.id().value(),
                    device.state().type_id(),
                )
            })
        })
        .collect();
    eq_panel::sync_visible_spectra(view, &spectrum_identities);
    let first_device = device_ids.first().copied();
    for (index, node) in nodes.into_iter().enumerate() {
        let (device, class) = match node {
            hex_project::TrackDeviceRef::Instrument(device) => {
                (device, NativeDeviceClass::Instrument)
            }
            hex_project::TrackDeviceRef::Effect(device) => (device, NativeDeviceClass::Effect),
        };
        let mounted = chain_window::must_mount(view, device.id(), window, cx);
        if !mounted {
            let card = chain_window::spacer(
                device.id(),
                device_card_width(class, device.state().type_id()),
            )
            .mr(px(DEVICE_CHAIN_PADDING));
            chain = chain.child(card);
            continue;
        }
        let rendered = match class {
            NativeDeviceClass::Instrument => render_instrument(view, window, cx, track_id, device),
            NativeDeviceClass::Effect => render_effect(view, window, cx, track_id, device),
        };
        let focus = view.device_panel.chain_window.focus(device.id(), cx);
        let card = render_device_card_target(
            view,
            cx,
            DeviceCardDropGeometry {
                track: track_id,
                device: device.id(),
                class,
                width: device_card_width(class, device.state().type_id()),
                next: device_ids.get(index + 1).copied(),
                first: first_device,
                visible_chain_bounds: visible_chain_bounds.clone(),
                is_sampler: class == NativeDeviceClass::Instrument
                    && device.state().type_id().as_str() == Ok("hex.instrument.sampler"),
            },
            rendered,
        )
        .track_focus(&focus);
        let card = keyboard::card(view, cx, device.id(), focus.clone(), card);
        let card = super::device_comparison::card(view, cx, device.id(), class, focus, card);
        chain = chain.child(card);
    }
    chain = chain.child(render_device_end_target(
        view,
        cx,
        drag::NativeDeviceDropTarget {
            track: track_id,
            before: None,
            replace: None,
        },
        first_device,
        visible_chain_bounds,
    ));

    let add_open = view.device_panel.add_menu_open;
    let add_button = div()
        .id("device-add-toggle")
        .debug_selector(|| "device-add-toggle".into())
        .size(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Raised))))
        .tooltip(|_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: "add native device".into(),
            })
            .into()
        })
        .child(theme::text(if add_open { "×" } else { "+" }))
        .on_click(cx.listener(|view, _, _, cx| {
            view.device_panel.add_menu_open = !view.device_panel.add_menu_open;
            cx.notify();
        }));
    let title = view
        .device_panel
        .message
        .clone()
        .unwrap_or_else(|| format!("{} · devices", track.name()));
    let track_name_view = title_label(title, view.device_panel.message.is_some());
    let mut header = div()
        .id("device-panel-header")
        .debug_selector(|| "device-panel-header".into())
        .h(px(DEVICE_PANEL_HEADER_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(5.0))
        .text_xs()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .child(track_name_view);
    if let Some(label) = retained_source_path_label(track) {
        let element = retained_source_path_badge(label);
        header = header.child(element);
    }
    header = header.child(add_button);
    if effect_catalog.is_err() || instrument_catalog.is_err() {
        header = header.child(menu_message("device catalog unavailable"));
    }
    root = root.child(header).child(chain);
    if add_open {
        root = root.child(render_add_menu(
            cx,
            track_id,
            instrument.as_ref().map(|device| device.id()),
            effect_catalog,
            instrument_catalog,
        ));
    }
    root
}

#[derive(Clone)]
struct DeviceCardDropGeometry {
    track: TrackId,
    device: DeviceId,
    class: NativeDeviceClass,
    width: f32,
    next: Option<DeviceId>,
    first: Option<DeviceId>,
    visible_chain_bounds: std::rc::Rc<std::cell::RefCell<Option<Bounds<Pixels>>>>,
    /// Accepts browser sample drops that load or replace its sample.
    is_sampler: bool,
}

/// Keep only previews belonging to live effects of the matching registered type.
fn retire_transfer_previews(
    panel: &mut DevicePanelState,
    type_of: impl Fn(DeviceId) -> Option<hex_lattice_contract::DeviceTypeId>,
) {
    panel.delay_panels.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.delay"))
    });
    panel.saturator_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.saturator"))
    });
    panel.compressor_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.compressor"))
    });
    panel.overdrive_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.overdrive"))
    });
    panel.auto_pan_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.auto-pan"))
    });
    panel.reverb_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.reverb"))
    });
}

fn device_card_width(class: NativeDeviceClass, type_id: hex_lattice_contract::DeviceTypeId) -> f32 {
    let frame_width = match (class, type_id.as_str().ok()) {
        (NativeDeviceClass::Instrument, Some("hex.instrument.operator")) => {
            operator_panel::OPERATOR_FRAME_WIDTH
        }
        (NativeDeviceClass::Instrument, Some("hex.instrument.sampler")) => {
            crate::device_composition::SAMPLER_FRAME_WIDTH
        }
        (NativeDeviceClass::Effect, Some("hex.utility")) => UTILITY_FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.eq8" | "hex.eq")) => eq8_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.saturator")) => saturator_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.limiter")) => limiter_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.compressor")) => compressor_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.gate")) => gate_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.reverb")) => reverb_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.delay")) => delay_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.auto-pan")) => auto_pan_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.overdrive")) => overdrive_panel::FRAME_WIDTH,
        _ => DEVICE_FRAME_WIDTH,
    };
    frame_width + hex_widgets::DEVICE_FRAME_HEADER_WIDTH + device_meter::WIDTH
}

fn pointer_within_visible_device_drop_bounds(
    pointer: &gpui::Point<Pixels>,
    target: &Bounds<Pixels>,
    visible_chain: Option<&Bounds<Pixels>>,
) -> bool {
    target.contains(pointer) && visible_chain.is_some_and(|bounds| bounds.contains(pointer))
}

fn render_device_card_target(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    geometry: DeviceCardDropGeometry,
    rendered: Stateful<Div>,
) -> Stateful<Div> {
    let DeviceCardDropGeometry {
        track,
        device,
        class,
        width,
        next,
        first,
        visible_chain_bounds,
        is_sampler,
    } = geometry;
    let active = view
        .active_native_device_drag
        .as_ref()
        .and_then(|drag| drag.destination.get());
    let active_left = active.is_some_and(|target| {
        target.replace.is_none() && target.track == track && target.before == Some(device)
    });
    let active_right = active.is_some_and(|target| {
        target.replace.is_none() && target.track == track && target.before == next
    });
    let left_target = drag::NativeDeviceDropTarget {
        track,
        before: Some(device),
        replace: None,
    };
    let right_target = drag::NativeDeviceDropTarget {
        track,
        before: next,
        replace: None,
    };
    let front_target = drag::NativeDeviceDropTarget {
        track,
        before: first,
        replace: None,
    };
    let replacement_target = drag::NativeDeviceDropTarget {
        track,
        before: None,
        replace: Some(device),
    };
    let active_replace = active == Some(replacement_target);
    let selector = format!("device-drop-card-{}", device.value());
    let entity = cx.entity();
    let target_visible_bounds = visible_chain_bounds.clone();
    let mut wrapper = div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .relative()
        .w(px(width + DEVICE_CHAIN_PADDING))
        .pr(px(DEVICE_CHAIN_PADDING))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex_shrink_0()
        .drag_over::<drag::NativeDeviceDrag>(move |style, _, _, _| {
            style.bg(rgb(theme::color(theme::PaletteRole::Hover)))
        })
        .on_drag_move::<drag::NativeDeviceDrag>(move |event, _, cx| {
            let Some(payload) = event
                .dragged_item()
                .downcast_ref::<drag::NativeDeviceDrag>()
            else {
                return;
            };
            let pointer = f32::from(event.event.position.x);
            let left = f32::from(event.bounds.origin.x);
            let right = left + f32::from(event.bounds.size.width);
            let pointer_y = f32::from(event.event.position.y);
            let top = f32::from(event.bounds.origin.y);
            let bottom = top + f32::from(event.bounds.size.height);
            let target_bounds = event.bounds;
            let inside = pointer >= left
                && pointer <= right
                && pointer_y >= top
                && pointer_y <= bottom
                && pointer_within_visible_device_drop_bounds(
                    &event.event.position,
                    &target_bounds,
                    target_visible_bounds.borrow().as_ref(),
                );
            let target = match payload.source {
                drag::NativeDeviceDragSource::Browser {
                    class: NativeDeviceClass::Instrument,
                    ..
                } if class == NativeDeviceClass::Instrument => Some(replacement_target),
                drag::NativeDeviceDragSource::Browser {
                    class: NativeDeviceClass::Instrument,
                    ..
                } => Some(front_target),
                drag::NativeDeviceDragSource::PluginEffect(_)
                    if inside && class == NativeDeviceClass::Instrument =>
                {
                    Some(right_target)
                }
                _ if inside && pointer < left + (right - left) / 2.0 => Some(left_target),
                _ if inside => Some(right_target),
                _ => None,
            };
            let previous = payload.destination.get();
            if let Some(target) = target.filter(|_| inside)
                && previous != Some(target)
            {
                payload.destination.set(Some(target));
                entity.update(cx, |_, cx| cx.notify());
            }
        })
        .on_drop(
            cx.listener(move |view, payload: &drag::NativeDeviceDrag, window, cx| {
                let fallback = if class == NativeDeviceClass::Instrument
                    && matches!(
                        payload.source,
                        drag::NativeDeviceDragSource::PluginEffect(_)
                    ) {
                    right_target
                } else {
                    left_target
                };
                let target = payload.destination.get().unwrap_or(fallback);
                view.drop_native_device(payload, target, window, cx);
            }),
        )
        .child(rendered)
        .child(insertion_gap(cx, device));
    if is_sampler {
        wrapper = wrapper
            .drag_over::<drag::BrowserAudioDrag>(|style, _, _, _| {
                style
                    .border_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Secondary)))
            })
            .on_drop(
                cx.listener(move |view, payload: &drag::BrowserAudioDrag, _, cx| {
                    view.drop_browser_sample_on_sampler(payload, track, device, cx);
                }),
            );
    }
    if active_left || view.device_panel.insertion_is(Some(device)) {
        wrapper = wrapper.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(px(2.0))
                .h(px(DEVICE_FRAME_HEIGHT))
                .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
        );
    }
    if active_right {
        wrapper = wrapper.child(
            div()
                .absolute()
                .right_0()
                .top_0()
                .w(px(2.0))
                .h(px(DEVICE_FRAME_HEIGHT))
                .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
        );
    }
    if active_replace {
        wrapper = wrapper.child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top_0()
                .bottom_0()
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Secondary))),
        );
    }
    wrapper
}

/// Click target over the chain gap left of `before`'s card. It selects that
/// gap for paste and returns keys to the neutral rail.
fn insertion_gap(cx: &Context<EditorView>, before: DeviceId) -> Stateful<Div> {
    div()
        .id(ElementId::Name(
            format!("device-insert-{}", before.value()).into(),
        ))
        .debug_selector(move || format!("device-insert-{}", before.value()))
        .absolute()
        .top_0()
        .left(px(-DEVICE_CHAIN_PADDING))
        .w(px(DEVICE_CHAIN_PADDING))
        .h(px(DEVICE_FRAME_HEIGHT))
        .cursor(gpui::CursorStyle::IBeam)
        .on_mouse_down(MouseButton::Left, select_insertion(cx, Some(before)))
}

fn select_insertion(
    cx: &Context<EditorView>,
    before: Option<DeviceId>,
) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |view, _: &gpui::MouseDownEvent, window, cx| {
        view.device_panel.select_insertion(before);
        view.device_panel
            .chain_window
            .selection_focus(cx)
            .focus(window);
        cx.notify();
    })
}

fn render_device_end_target(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    target: drag::NativeDeviceDropTarget,
    first: Option<DeviceId>,
    visible_chain_bounds: std::rc::Rc<std::cell::RefCell<Option<Bounds<Pixels>>>>,
) -> Stateful<Div> {
    let instrument_target = drag::NativeDeviceDropTarget {
        track: target.track,
        before: first,
        replace: None,
    };
    let active = view.active_native_device_drag.as_ref().is_some_and(|drag| {
        let expected = match drag.source {
            drag::NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Instrument,
                ..
            } => instrument_target,
            _ => target,
        };
        drag.destination.get() == Some(expected)
    });
    let entity = cx.entity();
    let target_visible_bounds = visible_chain_bounds.clone();
    let selector = format!("device-drop-end-{}", target.track.value());
    let mut receiver = div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .relative()
        .flex_1()
        .min_w(px(40.0))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, select_insertion(cx, None))
        .bg(rgb(if active {
            theme::color(theme::PaletteRole::Hover)
        } else {
            theme::color(theme::PaletteRole::Surface)
        }))
        .drag_over::<drag::NativeDeviceDrag>(move |style, _, _, _| {
            style.bg(rgb(theme::color(theme::PaletteRole::Hover)))
        })
        .on_drag_move::<drag::NativeDeviceDrag>(move |event, _, cx| {
            let Some(payload) = event
                .dragged_item()
                .downcast_ref::<drag::NativeDeviceDrag>()
            else {
                return;
            };
            let pointer = f32::from(event.event.position.x);
            let left = f32::from(event.bounds.origin.x);
            let right = left + f32::from(event.bounds.size.width);
            let pointer_y = f32::from(event.event.position.y);
            let top = f32::from(event.bounds.origin.y);
            let bottom = top + f32::from(event.bounds.size.height);
            let target_bounds = event.bounds;
            let inside = pointer >= left
                && pointer <= right
                && pointer_y >= top
                && pointer_y <= bottom
                && pointer_within_visible_device_drop_bounds(
                    &event.event.position,
                    &target_bounds,
                    target_visible_bounds.borrow().as_ref(),
                );
            let destination = match payload.source {
                drag::NativeDeviceDragSource::Browser {
                    class: NativeDeviceClass::Instrument,
                    ..
                } => Some(instrument_target),
                _ if inside => Some(target),
                _ => None,
            };
            let previous = payload.destination.get();
            if inside && previous != destination {
                payload.destination.set(destination);
                entity.update(cx, |_, cx| cx.notify());
            }
        })
        .on_drop(
            cx.listener(move |view, payload: &drag::NativeDeviceDrag, window, cx| {
                let target = payload.destination.get().unwrap_or(target);
                view.drop_native_device(payload, target, window, cx);
            }),
        );
    if active || view.device_panel.insertion_is(None) {
        receiver = receiver.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(px(2.0))
                .h(px(DEVICE_FRAME_HEIGHT))
                .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
        );
    }
    receiver
}

fn device_drag_title(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    device: &hex_project::Device,
    class: NativeDeviceClass,
    title: gpui::SharedString,
) -> Stateful<Div> {
    let id = device.id();
    let generation = view.project_generation;
    let payload = drag::NativeDeviceDrag {
        project_generation: view.project_generation,
        revision: view.state.as_ref().map_or(0, |state| state.revision),
        source: drag::NativeDeviceDragSource::Existing {
            device: id,
            class,
            type_id: device.state().type_id(),
        },
        destination: std::rc::Rc::new(std::cell::Cell::new(None)),
        return_destination: std::rc::Rc::new(std::cell::Cell::new(None)),
    };
    let entity = cx.entity();
    let help = theme::text(format!("{title} · drag to reorder device"));
    div()
        .id(("device-drag-title", id.value()))
        .debug_selector(move || format!("device-drag-title-{}", id.value()))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .truncate()
        .cursor(gpui::CursorStyle::OpenHand)
        .tooltip(move |_, cx| {
            let help = help.clone();
            cx.new(|_| super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Help,
                description: help,
            })
            .into()
        })
        .child(hex_widgets::vertical_device_name(title))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, _, window, cx| {
                if view.project_generation == generation
                    && view.device_panel.selected_device == Some(id)
                {
                    view.device_panel.chain_window.focus(id, cx).focus(window);
                }
            }),
        )
        .on_drag(payload, move |payload, _, window, cx| {
            cx.set_active_drag_cursor_style(
                if window.modifiers().alt {
                    gpui::CursorStyle::DragCopy
                } else {
                    gpui::CursorStyle::ClosedHand
                },
                window,
            );
            entity.update(cx, |view, cx| {
                view.begin_native_device_drag(payload);
                cx.notify();
            });
            cx.new(|_| gpui::Empty)
        })
}

fn chain_ids(project: &hex_project::Project, owner: DevicePanelOwner) -> Vec<DeviceId> {
    match owner {
        DevicePanelOwner::Track(id) => project
            .track(id)
            .map(|track| {
                track
                    .ordered_device_chain()
                    .map(|node| match node {
                        hex_project::TrackDeviceRef::Instrument(item)
                        | hex_project::TrackDeviceRef::Effect(item) => item.id(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        DevicePanelOwner::Main => project
            .main_devices()
            .iter()
            .map(hex_project::Device::id)
            .collect(),
        DevicePanelOwner::Return(id) => project
            .return_tracks()
            .iter()
            .find(|track| track.id() == id)
            .map(|track| {
                track
                    .devices()
                    .iter()
                    .map(hex_project::Device::id)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn project_owner(owner: DevicePanelOwner) -> hex_project::DeviceChainOwner {
    match owner {
        DevicePanelOwner::Track(id) => hex_project::DeviceChainOwner::Track(id),
        DevicePanelOwner::Main => hex_project::DeviceChainOwner::Main,
        DevicePanelOwner::Return(id) => hex_project::DeviceChainOwner::Return(id),
    }
}

// Capture selection before nested controls consume their pointer event. Selection
// is local view state and never finalizes or dispatches a parameter edit.
fn device_selection_listener(
    device: DeviceId,
    generation: u64,
    owner: Option<DevicePanelOwner>,
    cx: &Context<EditorView>,
) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
        if event.button != MouseButton::Left
            || view.project_generation != generation
            || view.device_panel.owner != owner
        {
            return;
        }
        let Some(state) = view.state.as_ref() else {
            return;
        };
        let Some(owner) = owner else {
            return;
        };
        let order = chain_ids(&state.project, owner);
        let members: Vec<_> = if view.device_panel.is_selected(device) {
            order
                .iter()
                .copied()
                .filter(|id| view.device_panel.is_selected(*id))
                .collect()
        } else {
            vec![device]
        };
        view.device_panel.drag_copy =
            hex_project::DeviceSelectionIds::new(&members)
                .ok()
                .map(|selected| DeviceDragCopy {
                    generation,
                    revision: state.revision,
                    owner,
                    device,
                    selected,
                });
        if event.modifiers.alt && view.device_panel.is_selected(device) {
            if let Some(previous) = view.device_panel.selected_device {
                view.device_panel.selected_others.insert(previous);
            }
            view.device_panel.selected_others.remove(&device);
            view.device_panel.selected_device = Some(device);
        } else {
            view.device_panel
                .select_click(device, &order, event.modifiers);
        }
        cx.notify();
    })
}

fn vst3_status_body(
    view: &mut EditorView,
    device: &hex_project::Device,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let device_id = device.id();
    if view.degraded_plugins.contains(device_id.value()) {
        return vst3_degraded_body(device_id);
    }
    let foreign = view
        .audible_playback
        .as_ref()
        .filter(|audible| audible.project_generation == view.project_generation)
        .filter(|audible| {
            audible
                .state
                .project
                .device(device_id)
                .is_some_and(|current| current.state().processing_eq(device.state()))
        })
        .and_then(|audible| audible.observation.as_ref())
        .and_then(|observation| {
            observation.foreign_processor(device_id.value(), device.state().type_id())
        });
    let (message, status) = match foreign {
        Some(true) => {
            let body = vst3_parameters::body(view, device_id, cx);
            let body = div()
                .debug_selector(move || format!("device-vst3-active-{}", device_id.value()))
                .child(body);
            return div()
                .id(("device-body", device_id.value()))
                .debug_selector(move || format!("device-body-{}", device_id.value()))
                .child(body);
        }
        Some(false) => (
            theme::text("vst3 unavailable · saved state preserved"),
            "unavailable",
        ),
        None => (theme::text("vst3 · awaiting audio status"), "pending"),
    };
    let content = crate::device_composition::unsupported_body(message)
        .debug_selector(move || format!("device-vst3-{status}-{}", device_id.value()));
    div()
        .child(content)
        .id(ElementId::Name(
            format!("device-body-{}", device_id.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device_id.value()))
}

/// A vst3 endpoint that faulted renders as an aligned pass-through while the
/// other devices keep processing (#3977). Shows the shared unsupported-body
/// display with an explanatory tooltip.
fn vst3_degraded_body(device_id: DeviceId) -> Stateful<Div> {
    let selector = format!("device-vst3-degraded-{}", device_id.value());
    let content = crate::device_composition::unsupported_body(theme::text(
        "vst3 stopped responding · audio passes through",
    ))
    .id(ElementId::Name(selector.clone().into()))
    .debug_selector(move || selector.clone())
    .tooltip(|_, cx| {
        cx.new(|_| super::transport::MiniTooltip {
            kind: crate::tooltips::Kind::Help,
            description: theme::text(
                "the plugin missed its audio deadline · it is bypassed with its latency kept, and other devices keep playing",
            ),
        })
        .into()
    });
    div()
        .child(content)
        .id(ElementId::Name(
            format!("device-body-{}", device_id.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device_id.value()))
}

fn render_effect(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    track: TrackId,
    device: &hex_project::Device,
) -> Stateful<Div> {
    let device_id = device.id();
    let selected = view.device_panel.is_selected(device_id);
    let device_view = view
        .device_panel
        .views
        .remove(&device_id)
        .filter(|(class, cached, _)| {
            *class == NativeDeviceClass::Effect && cached == device.state()
        })
        .map(|(_, _, result)| result)
        .unwrap_or_else(|| {
            hex_engine::native_device_view_for_class(NativeDeviceClass::Effect, device.state())
                .map_err(|error| {
                    hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
                })
        });
    let leading = bypass_action(view, device, NativeDeviceClass::Effect, cx);
    let header_actions = device_actions(view, device, selected, device_view.is_ok(), cx);
    let rendered = match &device_view {
        _ if device.state().type_id().as_str() == Ok("hex.vst3") => {
            let body = vst3_status_body(view, device, cx);
            device_frame(
                device_drag_title(
                    view,
                    cx,
                    device,
                    NativeDeviceClass::Effect,
                    view.vst3_device_title(device),
                ),
                theme::device_frame_style(selected),
                leading,
                header_actions,
                body,
            )
        }
        Ok(device_view) => {
            let body = if device_view.type_id.as_str() == Ok("hex.utility") {
                utility_panel::render_body(view, cx, device_id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.limiter") {
                limiter_panel::render_body(view, cx, device, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.compressor") {
                compressor_panel::render_body(view, cx, device_id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.saturator") {
                saturator_panel::render_body(view, cx, device_id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.reverb") {
                reverb_panel::render_body(view, cx, device_id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.delay") {
                delay_panel::render_body(view, cx, device_id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.overdrive") {
                overdrive_panel::render_body(view, cx, device_id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.gate") {
                gate_panel::render_body(view, cx, device_id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.auto-pan") {
                auto_pan_panel::render_body(view, cx, device_id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.eq") {
                let sample_rate = view
                    .state
                    .as_ref()
                    .map_or(48000, |state| state.project.sample_rate());
                eq_panel::render_body(
                    view,
                    cx,
                    device_id,
                    hex_engine::ProcessorChainOwner::Track(track.value()),
                    device.state(),
                    device_view,
                    sample_rate,
                )
            } else if device_view.type_id.as_str() == Ok("hex.eq8") {
                let sample_rate = view
                    .state
                    .as_ref()
                    .map_or(48_000, |state| state.project.sample_rate());
                eq8_panel::render_body(
                    view,
                    cx,
                    device_id,
                    hex_engine::ProcessorChainOwner::Track(track.value()),
                    device.state(),
                    device_view,
                    sample_rate,
                )
            } else {
                parameter_rows::body(
                    view,
                    window,
                    cx,
                    device_id,
                    DeviceControlIdentity {
                        class: NativeDeviceClass::Effect,
                        type_id: device_view.type_id,
                    },
                    device_view,
                )
            };
            device_frame(
                device_drag_title(
                    view,
                    cx,
                    device,
                    NativeDeviceClass::Effect,
                    theme::text(device_view.name.clone()),
                ),
                theme::device_frame_style(selected),
                leading,
                header_actions,
                body,
            )
        }
        Err(error) => {
            let body = crate::device_composition::unsupported_body(theme::text(format!(
                "unsupported device state · {error}"
            )))
            .id(ElementId::Name(
                format!("device-body-{}", device_id.value()).into(),
            ))
            .debug_selector(move || format!("device-body-{}", device_id.value()));
            let title = theme::text(
                device
                    .state()
                    .type_id()
                    .as_str()
                    .unwrap_or("unknown device"),
            );
            device_frame(
                device_drag_title(view, cx, device, NativeDeviceClass::Effect, title),
                theme::device_frame_style(selected),
                leading,
                header_actions,
                body,
            )
        }
    };
    let rendered = rendered
        .h_full()
        .debug_selector(move || format!("device-shell-{}", device_id.value()));
    view.device_panel.views.insert(
        device_id,
        (
            NativeDeviceClass::Effect,
            device.state().clone(),
            device_view,
        ),
    );
    let selector = format!("effect-device-{}", device_id.value());
    let card_width = device_card_width(NativeDeviceClass::Effect, device.state().type_id());
    let rendered = div()
        .w(px(card_width - device_meter::WIDTH))
        .flex_shrink_0()
        .child(rendered);
    div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .w(px(card_width))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex()
        .flex_shrink_0()
        .capture_any_mouse_down(device_selection_listener(
            device_id,
            view.project_generation,
            view.device_panel.owner,
            cx,
        ))
        .child(rendered)
        .child(device_meter::render(view, device, cx))
}

impl EditorView {
    pub(super) fn duplicate_device_drag_request(
        &self,
        drag: &drag::NativeDeviceDrag,
        destination: hex_project::DeviceChainOwner,
        before: Option<DeviceId>,
    ) -> Result<Request, &'static str> {
        let drag::NativeDeviceDragSource::Existing { device, .. } = drag.source else {
            return Err("device copy canceled · selection changed");
        };
        let snapshot = self
            .device_panel
            .drag_copy
            .as_ref()
            .ok_or("device copy canceled · selection changed")?;
        if snapshot.device != device
            || snapshot.generation != drag.project_generation
            || snapshot.revision != drag.revision
        {
            return Err("device copy canceled · selection changed");
        }
        let project = &self
            .state
            .as_ref()
            .ok_or("device copy canceled · project changed")?
            .project;
        let copies_instrument = match snapshot.owner {
            DevicePanelOwner::Track(id) => project
                .track(id)
                .and_then(|track| track.instrument())
                .is_some_and(|device| snapshot.selected.ids().contains(&device.id())),
            _ => false,
        };
        if copies_instrument {
            match destination {
                hex_project::DeviceChainOwner::Track(id)
                    if project
                        .track(id)
                        .is_some_and(|track| !track.is_group() && track.instrument().is_none()) => {
                }
                _ => return Err("destination cannot accept another instrument"),
            }
        }
        Ok(Request::DeviceClipboard {
            project_generation: drag.project_generation,
            revision: drag.revision,
            action: hex_session::service::DeviceClipboardAction::Duplicate {
                source: project_owner(snapshot.owner),
                selected: snapshot.selected.clone(),
                destination,
                before,
            },
        })
    }

    fn drop_native_device_to_effect_bus(
        &mut self,
        drag: &drag::NativeDeviceDrag,
        owner: EffectBusOwner,
        before: Option<DeviceId>,
        window: &mut Window,
        cx: &mut Context<EditorView>,
    ) {
        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
        let owns_drag = self
            .active_native_device_drag
            .as_ref()
            .is_some_and(|active| std::rc::Rc::ptr_eq(&active.destination, &drag.destination));
        let current = owns_drag
            && self.native_device_drag_is_current(drag)
            && self.state.as_ref().is_some_and(|state| {
                state.revision == drag.revision
                    && owner.devices(&state.project).is_some_and(|devices| {
                        before
                            .is_none_or(|anchor| devices.iter().any(|device| device.id() == anchor))
                    })
            });
        if !current {
            self.error = Some("device drag canceled · project or source changed".into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if self.export_pending
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
            || self.device_gesture.snapshot().is_some()
            || self.device_panel.has_pending_draft(cx)
        {
            let _ = finalize_controls(self, cx);
            self.error = Some("device edit is still applying; drag again".into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if !finalize_controls(self, cx) {
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if window.modifiers().alt
            && matches!(drag.source, drag::NativeDeviceDragSource::Existing { .. })
        {
            let destination = match owner {
                EffectBusOwner::Main => hex_project::DeviceChainOwner::Main,
                EffectBusOwner::Return(id) => hex_project::DeviceChainOwner::Return(id),
            };
            let request = self.duplicate_device_drag_request(drag, destination, before);
            self.clear_native_device_drag(drag);
            match request {
                Ok(request) => {
                    if self.enqueue_device_barrier(request) {
                        self.error = None;
                        self.status = "copying devices".into();
                    }
                }
                Err(reason) => self.error = Some(reason.into()),
            }
            cx.notify();
            return;
        }
        let request = match drag.source {
            drag::NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Effect,
                type_id,
            } => Ok(owner.insert_request(drag.project_generation, drag.revision, before, type_id)),
            drag::NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Instrument,
                ..
            } => Err("this effect chain accepts effects only".to_owned()),
            drag::NativeDeviceDragSource::PluginEffect(ref selection) => {
                let destination = match owner {
                    EffectBusOwner::Main => hex_session::service::PluginEffectDestination::Main,
                    EffectBusOwner::Return(id) => {
                        hex_session::service::PluginEffectDestination::Return(id)
                    }
                };
                self.clear_native_device_drag(drag);
                self.insert_plugin_effect(selection, destination, before, cx);
                cx.notify();
                return;
            }
            drag::NativeDeviceDragSource::Existing { device, .. } => {
                let devices = self
                    .state
                    .as_ref()
                    .and_then(|state| owner.devices(&state.project));
                let owns_device = devices
                    .is_some_and(|devices| devices.iter().any(|effect| effect.id() == device));
                if owns_device {
                    let unchanged = devices.is_some_and(|devices| {
                        before == Some(device)
                            || devices
                                .iter()
                                .position(|entry| entry.id() == device)
                                .is_some_and(|index| {
                                    devices.get(index + 1).map(|next| next.id()) == before
                                })
                    });
                    if unchanged {
                        self.status = "device order unchanged".into();
                        self.clear_native_device_drag(drag);
                        cx.notify();
                        return;
                    }
                    Ok(Request::Edit(owner.reorder(device, before)))
                } else {
                    Err("devices can only be reordered within their current owner".to_owned())
                }
            }
        };
        self.clear_native_device_drag(drag);
        match request {
            Ok(request) => {
                if self.enqueue_device_barrier(request) {
                    self.error = None;
                    self.device_panel.message = None;
                    self.status = "updating effects".into();
                }
            }
            Err(reason) => {
                self.error = Some(reason.clone().into());
                self.device_panel.message = Some(reason);
            }
        }
        cx.notify();
    }

    pub(super) fn drop_native_device(
        &mut self,
        drag: &drag::NativeDeviceDrag,
        drop_target: drag::NativeDeviceDropTarget,
        window: &mut Window,
        cx: &mut Context<EditorView>,
    ) {
        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
        let owns_drag = self
            .active_native_device_drag
            .as_ref()
            .is_some_and(|active| {
                active.project_generation == drag.project_generation
                    && active.revision == drag.revision
                    && active.source == drag.source
                    && std::rc::Rc::ptr_eq(&active.destination, &drag.destination)
            });
        let stale = !owns_drag || !self.native_device_drag_is_current(drag);
        if stale {
            self.error = Some("device drag canceled · project or source changed".into());
            self.device_panel.message =
                Some("device drag canceled · project or source changed".into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }

        let pending_edit = self.opening_project
            || self.close_prompt_open
            || self.export_pending
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
            || self.device_gesture.snapshot().is_some()
            || self.device_panel.has_pending_draft(cx);
        if pending_edit {
            let _ = finalize_controls(self, cx);
            let message = "device edit is still applying; drag again";
            self.error = Some(message.into());
            self.device_panel.message = Some(message.into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if !finalize_controls(self, cx) {
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }

        let still_current = self.native_device_drag_is_current(drag)
            && self.state.as_ref().is_some_and(|state| {
                state.revision == drag.revision && state.project.track(drop_target.track).is_some()
            });
        if !still_current {
            self.error = Some("device drag canceled · project or source changed".into());
            self.device_panel.message =
                Some("device drag canceled · project or source changed".into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }

        let Some(track) = self.selected_track else {
            self.error = Some("select a track before dropping a device".into());
            self.device_panel.message = self.error.as_ref().map(ToString::to_string);
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        };
        let Some(state) = self.state.as_ref() else {
            self.clear_native_device_drag(drag);
            return;
        };
        let Some(target_track) = state.project.track(track) else {
            self.error = Some("device drop target is no longer available".into());
            self.device_panel.message = self.error.as_ref().map(ToString::to_string);
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        };
        if let Some(anchor) = drop_target.before
            && !target_track.ordered_device_chain().any(|node| match node {
                hex_project::TrackDeviceRef::Instrument(device)
                | hex_project::TrackDeviceRef::Effect(device) => device.id() == anchor,
            })
        {
            self.error = Some("device drop target changed; drag again".into());
            self.device_panel.message = self.error.as_ref().map(ToString::to_string);
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }

        let before =
            match drag.source {
                drag::NativeDeviceDragSource::Browser {
                    class: NativeDeviceClass::Instrument,
                    ..
                } if drop_target.replace.is_none() => target_track
                    .ordered_device_chain()
                    .next()
                    .map(|node| match node {
                        hex_project::TrackDeviceRef::Instrument(device)
                        | hex_project::TrackDeviceRef::Effect(device) => device.id(),
                    }),
                _ => drop_target.before,
            };
        if let drag::NativeDeviceDragSource::Browser {
            class: NativeDeviceClass::Instrument,
            ..
        } = drag.source
            && target_track.instrument().is_some()
            && drop_target.replace.is_none()
        {
            self.error = Some("instrument slot is already occupied".into());
            self.device_panel.message = self.error.as_ref().map(ToString::to_string);
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if let Some(replacement) = drop_target.replace {
            let source_is_browser_instrument = matches!(
                drag.source,
                drag::NativeDeviceDragSource::Browser {
                    class: NativeDeviceClass::Instrument,
                    ..
                }
            );
            let target_is_current_instrument = target_track
                .instrument()
                .is_some_and(|instrument| instrument.id() == replacement);
            if !source_is_browser_instrument
                || !target_is_current_instrument
                || drop_target.before.is_some()
            {
                self.error = Some("instrument replacement target changed; drag again".into());
                self.device_panel.message = self.error.as_ref().map(ToString::to_string);
                self.clear_native_device_drag(drag);
                cx.notify();
                return;
            }
        }
        if window.modifiers().alt
            && matches!(drag.source, drag::NativeDeviceDragSource::Existing { .. })
        {
            let request = self.duplicate_device_drag_request(
                drag,
                hex_project::DeviceChainOwner::Track(track),
                before,
            );
            self.clear_native_device_drag(drag);
            match request {
                Ok(request) => {
                    if self.enqueue_device_barrier(request) {
                        self.error = None;
                        self.status = "copying devices".into();
                    }
                }
                Err(reason) => self.error = Some(reason.into()),
            }
            cx.notify();
            return;
        }
        if let drag::NativeDeviceDragSource::Existing { device, .. } = drag.source
            && device_move_would_be_noop(&state.project, device, track, before)
        {
            self.status = "device order unchanged".into();
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }

        let source = match drag.source {
            drag::NativeDeviceDragSource::Browser { class, type_id } => {
                NativeDeviceDropSource::Browser { class, type_id }
            }
            drag::NativeDeviceDragSource::PluginEffect(ref selection) => {
                self.clear_native_device_drag(drag);
                self.insert_plugin_effect(
                    selection,
                    hex_session::service::PluginEffectDestination::Track(drop_target.track),
                    before,
                    cx,
                );
                cx.notify();
                return;
            }
            drag::NativeDeviceDragSource::Existing {
                device,
                class,
                type_id,
            } => NativeDeviceDropSource::Existing {
                device,
                class,
                type_id,
            },
        };
        let request = Request::DropNativeDevice {
            project_generation: drag.project_generation,
            revision: drag.revision,
            source,
            track,
            before,
            replace: drop_target.replace,
        };
        self.clear_native_device_drag(drag);
        if self.enqueue_device_barrier(request) {
            self.error = None;
            self.device_panel.message = None;
            self.status = match source {
                NativeDeviceDropSource::Browser { .. } if drop_target.replace.is_some() => {
                    "replacing instrument".into()
                }
                NativeDeviceDropSource::Browser { .. } => "adding device".into(),
                NativeDeviceDropSource::Existing { .. } => "moving device".into(),
            };
        }
        cx.notify();
    }
}

fn device_move_would_be_noop(
    project: &hex_project::Project,
    device: DeviceId,
    target_track: TrackId,
    before: Option<DeviceId>,
) -> bool {
    let Some(source_track) = project.tracks().iter().find(|track| {
        track.ordered_device_chain().any(|node| match node {
            hex_project::TrackDeviceRef::Instrument(candidate)
            | hex_project::TrackDeviceRef::Effect(candidate) => candidate.id() == device,
        })
    }) else {
        return false;
    };
    if source_track.id() != target_track {
        return false;
    }
    if before == Some(device) {
        return true;
    }
    let current: Vec<_> = source_track
        .ordered_device_chain()
        .map(|node| match node {
            hex_project::TrackDeviceRef::Instrument(candidate)
            | hex_project::TrackDeviceRef::Effect(candidate) => candidate.id(),
        })
        .collect();
    let mut moved: Vec<_> = current
        .iter()
        .copied()
        .filter(|candidate| *candidate != device)
        .collect();
    let destination = before
        .and_then(|anchor| moved.iter().position(|candidate| *candidate == anchor))
        .unwrap_or(moved.len());
    moved.insert(destination, device);
    moved == current
}

fn render_instrument(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    track: TrackId,
    device: &hex_project::Device,
) -> Stateful<Div> {
    let device_id = device.id();
    let selected = view.device_panel.is_selected(device_id);
    let cached = view
        .device_panel
        .views
        .remove(&device_id)
        .filter(|(class, state, _)| {
            *class == NativeDeviceClass::Instrument && state == device.state()
        })
        .map(|(_, _, result)| result);
    let device_view = cached.unwrap_or_else(|| {
        hex_engine::native_device_view_for_class(NativeDeviceClass::Instrument, device.state())
            .map_err(|error| {
                hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
            })
    });
    let is_operator = device.state().type_id().as_str() == Ok("hex.instrument.operator");
    let is_sampler = device.state().type_id().as_str() == Ok("hex.instrument.sampler");
    let leading = bypass_action(view, device, NativeDeviceClass::Instrument, cx);
    let header_actions = instrument_actions(view, track, device, selected, device_view.is_ok(), cx);
    let rendered = if device.state().type_id().as_str() == Ok("hex.vst3") {
        device_frame(
            device_drag_title(
                view,
                cx,
                device,
                NativeDeviceClass::Instrument,
                view.vst3_device_title(device),
            ),
            theme::device_frame_style(selected),
            leading,
            header_actions,
            vst3_status_body(view, device, cx),
        )
    } else if hex_engine::is_empty_sampler_state_on_control(device.state()) {
        device_frame(
            device_drag_title(
                view,
                cx,
                device,
                NativeDeviceClass::Instrument,
                theme::text("sampler"),
            ),
            theme::device_frame_style(selected),
            leading,
            header_actions,
            crate::device_composition::body()
                .p(px(8.0))
                .child(theme::text(
                    "empty sampler · choose a wav in the sample browser and load into sampler",
                )),
        )
    } else if is_sampler {
        let name = view
            .state
            .as_ref()
            .and_then(|s| {
                let reference = device.state().asset_references().first()?;
                s.project
                    .assets()
                    .find(|a| a.id().value() == reference.asset.value())?
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "missing sample identity".into());
        let readiness = if view.sampler_parameter_pending() {
            "preparing sampler edit · waiting for audio"
        } else if view.first_sampler_current() {
            "audio ready"
        } else {
            "audio not ready · offline export supported"
        };
        let gain =
            view.sampler_parameter_control(track, device, hex_engine::SamplerParameter::Gain, cx);
        let pitch = view.sampler_parameter_control(
            track,
            device,
            hex_engine::SamplerParameter::Transpose,
            cx,
        );
        let start = view.sampler_parameter_control(
            track,
            device,
            hex_engine::SamplerParameter::StartFrame,
            cx,
        );
        let end = view.sampler_parameter_control(
            track,
            device,
            hex_engine::SamplerParameter::EndFrame,
            cx,
        );
        let mut fields = vec![gain, pitch, start, end];
        let loop_error = match hex_engine::sampler_parameter_value_on_control(
            hex_engine::SamplerParameter::LoopEnabled,
            device.state(),
        ) {
            Ok(_) => {
                for parameter in [
                    hex_engine::SamplerParameter::LoopEnabled,
                    hex_engine::SamplerParameter::LoopStartFrame,
                    hex_engine::SamplerParameter::LoopEndFrame,
                    hex_engine::SamplerParameter::LoopCrossfadeMs,
                ] {
                    fields.push(view.sampler_parameter_control(track, device, parameter, cx));
                }
                None
            }
            Err(error) => Some(error.to_string()),
        };
        for parameter in [
            hex_engine::SamplerParameter::AttackMs,
            hex_engine::SamplerParameter::DecayMs,
            hex_engine::SamplerParameter::SustainPercent,
            hex_engine::SamplerParameter::ReleaseMs,
        ] {
            fields.push(view.sampler_parameter_control(track, device, parameter, cx));
        }
        let controls = div()
            .grid()
            // One dense row: twelve classic controls at the sampler's inline
            // value width (see `SAMPLER_INLINE_VALUE_WIDTH`) fit the 1024 px
            // device body, keeping the waveform overview's full height.
            .grid_cols(if loop_error.is_none() { 12 } else { 8 })
            .gap(px(2.0))
            .w_full()
            .flex_shrink_0()
            .debug_selector(|| "sampler-controls".into())
            .children(fields);
        let details = div()
            .w_full()
            .flex_shrink_0()
            .text_size(px(9.0))
            .line_height(px(12.0))
            .overflow_hidden()
            .child(theme::text(format!(
                "{readiness} · * release inactive in trigger mode{}",
                loop_error.map_or_else(String::new, |error| format!(" · {error}"))
            )));
        let overview = sampler_overview::render(view, track, device, &name, cx);
        let body = crate::device_composition::body()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .p(px(4.0))
            .text_xs()
            .child(overview)
            .child(controls)
            .child(details);
        device_frame(
            device_drag_title(
                view,
                cx,
                device,
                NativeDeviceClass::Instrument,
                theme::text("sampler"),
            ),
            theme::device_frame_style(selected),
            leading,
            header_actions,
            body,
        )
    } else {
        match &device_view {
            Ok(device_view) => {
                if is_operator && device_view.type_id.as_str() == Ok("hex.instrument.operator") {
                    let body = operator_panel::render_body(view, cx, device_id, device_view);
                    device_frame(
                        device_drag_title(
                            view,
                            cx,
                            device,
                            NativeDeviceClass::Instrument,
                            theme::text(device_view.name.clone()),
                        ),
                        theme::device_frame_style(selected),
                        leading,
                        header_actions,
                        body,
                    )
                } else {
                    let body = parameter_rows::body(
                        view,
                        window,
                        cx,
                        device_id,
                        DeviceControlIdentity {
                            class: NativeDeviceClass::Instrument,
                            type_id: device_view.type_id,
                        },
                        device_view,
                    );

                    device_frame(
                        device_drag_title(
                            view,
                            cx,
                            device,
                            NativeDeviceClass::Instrument,
                            theme::text(device_view.name.clone()),
                        ),
                        theme::device_frame_style(selected),
                        leading,
                        header_actions,
                        body,
                    )
                }
            }
            Err(error) => {
                let body = crate::device_composition::unsupported_body(theme::text(format!(
                    "unsupported instrument state · {error}"
                )))
                .id(ElementId::Name(
                    format!("device-body-{}", device_id.value()).into(),
                ))
                .debug_selector(move || format!("device-body-{}", device_id.value()));
                device_frame(
                    device_drag_title(
                        view,
                        cx,
                        device,
                        NativeDeviceClass::Instrument,
                        theme::text(
                            device
                                .state()
                                .type_id()
                                .as_str()
                                .unwrap_or("unknown instrument"),
                        ),
                    ),
                    theme::device_frame_style(selected),
                    leading,
                    header_actions,
                    body,
                )
            }
        }
    };
    let rendered = rendered
        .h_full()
        .debug_selector(move || format!("device-shell-{}", device_id.value()));
    view.device_panel.views.insert(
        device_id,
        (
            NativeDeviceClass::Instrument,
            device.state().clone(),
            device_view,
        ),
    );
    let selector = format!("instrument-device-{}", device_id.value());
    let card_width = device_card_width(NativeDeviceClass::Instrument, device.state().type_id());
    let rendered = div()
        .w(px(card_width - device_meter::WIDTH))
        .flex_shrink_0()
        .child(rendered);
    div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .w(px(card_width))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex()
        .flex_shrink_0()
        .capture_any_mouse_down(device_selection_listener(
            device_id,
            view.project_generation,
            view.device_panel.owner,
            cx,
        ))
        .child(rendered)
        .child(device_meter::render(view, device, cx))
}

fn instrument_actions(
    view: &EditorView,
    track: TrackId,
    device: &hex_project::Device,
    selected: bool,
    presets_available: bool,
    cx: &mut Context<EditorView>,
) -> Div {
    let id = device.id();
    let actions = div().flex().flex_col().items_center().gap(px(2.0));
    let actions = if device.state().type_id().as_str() == Ok("hex.vst3")
        && !view.degraded_plugins.contains(id.value())
    {
        actions.child(vst3_editor_button(id, selected, cx))
    } else {
        actions
    };
    let actions = actions.child(
        action_button(
            format!("instrument-replace-{}", id.value()),
            "↻",
            "replace instrument",
            theme::DeviceActionRole::Normal,
            selected,
        )
        .on_click(cx.listener(|view, _, _, cx| {
            view.device_panel.add_menu_open = true;
            cx.notify();
        })),
    );
    let actions = if presets_available {
        actions
            .child(super::device_preset::actions(
                id,
                NativeDeviceClass::Instrument,
                selected,
                cx,
            ))
            .child(super::device_comparison::actions(
                view,
                device,
                NativeDeviceClass::Instrument,
                selected,
                cx,
            ))
    } else {
        actions
    };
    actions.child(
        action_button(
            format!("instrument-remove-{}", id.value()),
            "×",
            "remove instrument",
            theme::DeviceActionRole::Destructive,
            selected,
        )
        .on_click(cx.listener(move |view, _, window, cx| {
            if finalize_controls(view, cx)
                && view.enqueue_device_barrier(Request::Edit(Command::RemoveInstrument { track }))
            {
                keyboard::after_pointer_removal(view, id, window, cx);
            }
            cx.notify();
        })),
    )
}

fn render_add_menu(
    cx: &mut Context<EditorView>,
    track: TrackId,
    instrument: Option<DeviceId>,
    effects: Result<Vec<hex_engine::NativeDeviceCatalogEntry>, String>,
    instruments: Result<Vec<hex_engine::NativeDeviceCatalogEntry>, String>,
) -> impl IntoElement {
    let mut menu = div()
        .id("device-add-menu")
        .debug_selector(|| "device-add-menu".into())
        .absolute()
        .top(px(DEVICE_PANEL_HEADER_HEIGHT))
        .left(px(4.0))
        .right(px(4.0))
        .max_h(px(crate::device_composition::BODY_HEIGHT))
        .overflow_y_scroll()
        .occlude()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(3.0))
        .px(px(4.0))
        .py(px(2.0))
        .bg(rgb(theme::color(theme::PaletteRole::Raised)));
    let mut any_entry = false;
    for (class, entries) in [
        (NativeDeviceClass::Instrument, instruments),
        (NativeDeviceClass::Effect, effects),
    ] {
        match entries {
            Ok(entries) => {
                for entry in entries {
                    any_entry = true;
                    let type_id = entry.type_id;
                    let type_label = type_id.as_str().unwrap_or("invalid device").to_owned();
                    let class_label = match class {
                        NativeDeviceClass::Effect => "effect",
                        NativeDeviceClass::Instrument => "instrument",
                    };
                    let action_id = format!("add-native-{class_label}-{type_label}");
                    let label = if class == NativeDeviceClass::Instrument && instrument.is_some() {
                        format!("replace · {}", entry.name)
                    } else {
                        format!("{class_label} · {}", entry.name)
                    };
                    menu = menu.child(
                        div()
                            .id(ElementId::Name(action_id.clone().into()))
                            .debug_selector(move || action_id.clone())
                            .h(px(18.0))
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .px(px(5.0))
                            .flex()
                            .items_center()
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
                            .cursor(gpui::CursorStyle::PointingHand)
                            .child(theme::text(label))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                let request = match (class, instrument) {
                                    (NativeDeviceClass::Instrument, Some(device)) => {
                                        Request::ReplaceNativeDevice {
                                            device,
                                            class,
                                            type_id: Box::new(type_id),
                                        }
                                    }
                                    _ => Request::AddNativeDevice {
                                        track,
                                        class,
                                        type_id: Box::new(type_id),
                                        before: None,
                                    },
                                };
                                if finalize_controls(view, cx)
                                    && view.enqueue_device_barrier(request)
                                {
                                    view.device_panel.add_menu_open = false;
                                    view.device_panel.message = None;
                                }
                                cx.notify();
                            })),
                    );
                }
            }
            Err(error) => {
                menu = menu.child(menu_message(&format!("device catalog error · {error}")));
            }
        }
    }
    if !any_entry {
        menu = menu.child(menu_message("no native devices available"));
    }
    menu
}

fn bypass_action(
    view: &EditorView,
    device: &hex_project::Device,
    class: NativeDeviceClass,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let id = device.id();
    let bypassed = view.device_panel.bypass.displayed(id, device.bypassed());
    let (on_color, off_color) = theme::device_enable_colors();
    let tooltip = if class == NativeDeviceClass::Instrument {
        "toggle instrument bypass"
    } else {
        "toggle device bypass"
    };
    hex_widgets::device_state_indicator(
        format!("device-bypass-{}", id.value()).into(),
        !bypassed,
        on_color,
        off_color,
    )
    .tooltip(move |_, cx| {
        cx.new(|_| super::transport::MiniTooltip {
            kind: crate::tooltips::Kind::Identification,
            description: tooltip.into(),
        })
        .into()
    })
    .on_click(cx.listener(move |view, _, _, cx| {
        if finalize_controls(view, cx) {
            view.toggle_device_bypass(id);
        }
        cx.notify();
    }))
}

fn device_actions(
    view: &EditorView,
    device: &hex_project::Device,
    selected: bool,
    presets_available: bool,
    cx: &mut Context<EditorView>,
) -> Div {
    let id = device.id();
    let mut actions = div().flex().flex_col().items_center().gap(px(2.0));
    if device.state().type_id().as_str() == Ok("hex.vst3")
        && !view.degraded_plugins.contains(id.value())
    {
        actions = actions.child(vst3_editor_button(id, selected, cx));
    }
    if presets_available {
        actions = actions
            .child(super::device_preset::actions(
                id,
                NativeDeviceClass::Effect,
                selected,
                cx,
            ))
            .child(super::device_comparison::actions(
                view,
                device,
                NativeDeviceClass::Effect,
                selected,
                cx,
            ));
    }
    actions.child(
        action_button(
            format!("device-remove-{}", id.value()),
            "×",
            "remove device",
            theme::DeviceActionRole::Destructive,
            selected,
        )
        .on_click(cx.listener(move |view, _, window, cx| {
            if finalize_controls(view, cx)
                && view.enqueue_device_barrier(Request::Edit(Command::RemoveDevice(id)))
            {
                keyboard::after_pointer_removal(view, id, window, cx);
            }
            cx.notify();
        })),
    )
}

fn vst3_editor_button(id: DeviceId, selected: bool, cx: &mut Context<EditorView>) -> Stateful<Div> {
    action_button(
        format!("device-vst3-editor-{}", id.value()),
        "↗",
        "show or hide plugin editor",
        theme::DeviceActionRole::Normal,
        selected,
    )
    .on_click(cx.listener(move |view, _, _, cx| {
        view.enqueue(Request::TogglePluginEditor {
            project_generation: view.project_generation,
            device: id,
        });
        cx.notify();
    }))
}

pub(super) fn action_button(
    id: String,
    icon: &'static str,
    tooltip: &'static str,
    role: theme::DeviceActionRole,
    selected: bool,
) -> Stateful<Div> {
    button_details(
        hex_widgets::device_action(
            theme::text(icon),
            theme::device_header_action_style(selected, role),
        ),
        id,
        tooltip,
    )
}

/// Latched header/toolbar settings retain segment artwork, distinct from commands.
pub(super) fn segment_button(
    id: String,
    label: &'static str,
    tooltip: &'static str,
    role: theme::DeviceActionRole,
    selected: bool,
) -> Stateful<Div> {
    button_details(
        hex_widgets::device_segment(
            theme::text(label),
            theme::device_header_action_style(selected, role),
        ),
        id,
        tooltip,
    )
}

fn button_details(surface: Div, id: String, tooltip: &'static str) -> Stateful<Div> {
    surface
        .id(ElementId::Name(id.clone().into()))
        .debug_selector(move || id.clone())
        .cursor(gpui::CursorStyle::PointingHand)
        .tooltip(move |_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: tooltip.into(),
            })
            .into()
        })
}

// Host-specific composition policy stays outside the shared bindings.
fn binding_policy(
    view: &EditorView,
    key: ParameterKey,
    identity: DeviceControlIdentity,
    descriptor: ParameterDescriptor,
    presentation: ControlPresentation,
    display: ControlText,
) -> bindings::BindingPolicy {
    let tab_navigation = identity.type_id.as_str() == Ok("hex.instrument.operator");
    let numeric_begin = if identity.class == NativeDeviceClass::Instrument
        && identity.type_id.as_str() == Ok("hex.instrument.operator")
    {
        operator_panel::shell_operator_for_numeric_parameter(key.parameter).and_then(|operator| {
            operator_panel::context::Owner::capture(view, key.device).map(|owner| {
                Box::new(
                    move |view: &mut EditorView, cx: &mut Context<EditorView>, key| {
                        operator_panel::context::select_oscillator(
                            view,
                            cx,
                            owner,
                            operator,
                            &[key],
                        );
                    },
                ) as bindings::NumericBeginHook
            })
        })
    } else {
        None
    };
    let toggle_appearance = if presentation == ControlPresentation::CompactButton {
        match display.label {
            Some("") => ToggleAppearance::MicroSquare,
            Some("bass mono audition") => ToggleAppearance::CompactIconButton,
            _ => ToggleAppearance::CompactButton,
        }
    } else {
        ToggleAppearance::Standard
    };
    let choice_order = match descriptor.kind {
        ParameterKind::Choice { labels } => Some(operator_panel::choice_order(
            identity,
            descriptor.id,
            labels.len(),
        )),
        _ => None,
    };
    bindings::BindingPolicy {
        tab_navigation,
        toggle_appearance,
        choice_order,
        numeric_begin,
    }
}

/// Bind the active Arrangement trim on the persistent device toolbar.
fn render_automation_trim(view: &mut EditorView, cx: &mut Context<EditorView>) -> Option<Div> {
    use hex_lattice_contract::DeviceAutomationTarget;
    let target = view.selected_automation_trim()?;
    let descriptor = match target.kind {
        DeviceAutomationTarget::HostInputGain => hex_engine::DEVICE_INPUT_GAIN,
        DeviceAutomationTarget::HostOutputGain => hex_engine::DEVICE_OUTPUT_GAIN,
        _ => return None,
    };
    let state = view.state.as_ref()?;
    let device = target.current(&state.project, view.project_generation)?;
    let identity = DeviceControlIdentity {
        class: target.class,
        type_id: device.state().type_id(),
    };
    let value =
        hex_engine::native_device_parameter_for_class(target.class, device.state(), descriptor.id)
            .ok()?;
    let control = bindings::render_parameter(
        view,
        cx,
        target.device,
        identity,
        &NativeDeviceParameterView {
            descriptor,
            semantics: None,
            value,
            enabled: true,
        },
        bindings::BindingPresentation {
            kind: ControlPresentation::InlineValue,
            text: ControlText::default(),
            policy: binding_policy,
            compact_toggle_width: 0.0,
        },
    );
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(3.0))
            .flex_shrink_0()
            .child(theme::text(descriptor.label))
            .child(control),
    )
}

fn parameter_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameter: &NativeDeviceParameterView,
) -> Div {
    parameter_control_with_presentation(
        view,
        cx,
        device,
        identity,
        parameter,
        ControlPresentation::Standard,
        ControlText::default(),
    )
}

fn parameter_control_with_presentation(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameter: &NativeDeviceParameterView,
    presentation: ControlPresentation,
    display: ControlText,
) -> Div {
    let compact_toggle_width = match display.label {
        Some("bass mono" | "mono" | "maximize" | "auto") => 64.0,
        Some("bass mono audition" | "r" | "q") => 14.0,
        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "on" | "fix" | "") => 18.0,
        Some("ø l" | "ø r" | "mute" | "dc") => 31.0,
        _ => 40.0,
    };
    bindings::render_parameter(
        view,
        cx,
        device,
        identity,
        parameter,
        bindings::BindingPresentation {
            kind: presentation,
            text: display,
            policy: binding_policy,
            compact_toggle_width,
        },
    )
}

fn menu_message(message: &str) -> Div {
    div()
        .h(px(18.0))
        .flex()
        .items_center()
        .px(px(4.0))
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(message))
}

fn empty_panel(message: &str) -> Stateful<Div> {
    div()
        .id("device-panel-empty")
        .debug_selector(|| "device-panel-empty".into())
        .h(px(DEVICE_PANEL_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .px(px(8.0))
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(message))
}

mod return_view;

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) mod tests;

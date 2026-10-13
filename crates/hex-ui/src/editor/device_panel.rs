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
    ControlStyle, DialLayout, DiscreteControlAppearance, NumericControl, NumericControlAppearance,
    NumericControlConfig, NumericControlEvent, NumericControlKind, NumericEventPhase, NumericScale,
    NumericSpec, Selector, SelectorChanged, SelectorConfig, Toggle, ToggleChanged, ToggleConfig,
    compact_device_frame, device_frame,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

mod auto_pan_panel;
mod chain_window;
mod compressor_panel;
mod eq8_panel;
mod gate_panel;
mod numeric_display;
mod operator_panel;
pub(super) use operator_panel::bind_keys as bind_operator_keys;
mod saturator_panel;
mod send_controls;
mod utility_panel;
pub(super) use send_controls::return_send_numeric_control;
pub(super) use send_controls::{
    commit_owned_numeric_edit, dispatch_numeric_gesture, finalize_send_controls_for_track,
};

const DEVICE_PANEL_HEIGHT: f32 = 213.0;
const DEVICE_PANEL_BORDER_WIDTH: f32 = 1.0;
const DEVICE_PANEL_HEADER_HEIGHT: f32 = 22.0;
const DEVICE_CHAIN_PADDING: f32 = 3.0;
const DEVICE_FRAME_WIDTH: f32 = 286.0;
pub(super) const UTILITY_FRAME_WIDTH: f32 = 144.0;
const DEVICE_FRAME_HEIGHT: f32 = DEVICE_PANEL_HEIGHT
    - DEVICE_PANEL_BORDER_WIDTH
    - DEVICE_PANEL_HEADER_HEIGHT
    - (DEVICE_CHAIN_PADDING * 2.0);
const DEVICE_FRAME_BODY_HEIGHT: f32 = DEVICE_FRAME_HEIGHT
    - hex_widgets::DEVICE_FRAME_HEADER_HEIGHT
    - (hex_widgets::DEVICE_FRAME_BORDER_WIDTH * 2.0);
const UTILITY_FRAME_BODY_HEIGHT: f32 = DEVICE_FRAME_HEIGHT
    - hex_widgets::COMPACT_DEVICE_FRAME_HEADER_HEIGHT
    - (hex_widgets::DEVICE_FRAME_BORDER_WIDTH * 2.0);

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

enum ParameterControl {
    Float {
        entity: Entity<NumericControl>,
        _subscription: Subscription,
        class: NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        descriptor: &'static ParameterDescriptor,
    },
    Bool {
        entity: Entity<Toggle>,
        _subscription: Subscription,
        class: NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        descriptor: &'static ParameterDescriptor,
    },
    Choice {
        entity: Entity<Selector>,
        _subscription: Subscription,
        class: NativeDeviceClass,
        type_id: hex_lattice_contract::DeviceTypeId,
        descriptor: &'static ParameterDescriptor,
    },
}

fn parameter_control_matches_identity(
    control: &ParameterControl,
    identity: DeviceControlIdentity,
) -> bool {
    match control {
        ParameterControl::Float { class, type_id, .. }
        | ParameterControl::Bool { class, type_id, .. }
        | ParameterControl::Choice { class, type_id, .. } => {
            *class == identity.class && *type_id == identity.type_id
        }
    }
}

#[derive(Default)]
pub(super) struct DevicePanelState {
    pub(super) owner: Option<DevicePanelOwner>,
    chain_window: chain_window::ChainWindow,
    generation: u64,
    controls: HashMap<ParameterKey, ParameterControl>,
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
    operator_context_focus: HashMap<DeviceId, HashMap<gpui::EntityId, Subscription>>,
    operator_previews: HashMap<DeviceId, operator_panel::preview::State>,
    operator_preview_inventory: Option<OperatorPreviewInventory>,
    operator_envelope_graphs:
        HashMap<operator_panel::envelope::GraphKey, operator_panel::envelope::GraphControl>,
    operator_envelope_gestures:
        HashMap<operator_panel::envelope::GraphKey, operator_panel::envelope::GraphGesture>,
    eq8_panels: HashMap<DeviceId, eq8_panel::Eq8PanelState>,
    eq8_graphs: HashMap<DeviceId, eq8_panel::GraphControl>,
    eq8_responses: HashMap<DeviceId, eq8_panel::ResponseCache>,
    saturator_previews: HashMap<DeviceId, saturator_panel::PreviewCache>,
    compressor_previews: HashMap<DeviceId, compressor_panel::PreviewCache>,
    eq8_graph_gestures: HashMap<DeviceId, eq8_panel::GraphGestureState>,
    eq8_audition_toggles: HashMap<DeviceId, Eq8AuditionToggle>,
    pub(super) spectrum_result: Option<hex_audio::spectrum_worker::PublishedSpectrum>,
    add_menu_open: bool,
    message: Option<String>,
}

struct OperatorPreviewInventory {
    generation: u64,
    revision: u64,
    devices: Arc<HashSet<DeviceId>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DevicePanelOwner {
    Track(TrackId),
    Return(hex_project::ReturnId),
}

impl DevicePanelState {
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

pub(super) fn control_style() -> ControlStyle {
    ControlStyle {
        background: theme::color(theme::PaletteRole::Background),
        surface: theme::color(theme::PaletteRole::Surface),
        raised: theme::color(theme::PaletteRole::Raised),
        rail: theme::color(theme::PaletteRole::Border),
        border: theme::color(theme::PaletteRole::Border),
        text: theme::readable_text(
            theme::color(theme::PaletteRole::Surface),
            theme::color(theme::PaletteRole::Text),
        ),
        muted: theme::color(theme::PaletteRole::Muted),
        accent: theme::color(theme::PaletteRole::Secondary),
        focus_marker: 0xaaaaaa,
        dial_rail: 0x000000,
        hover: theme::color(theme::PaletteRole::Hover),
        font_size: 9.0,
    }
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
        if let ParameterControl::Float { entity, .. } = control {
            controls.push((
                GestureTarget::DeviceParameter {
                    device: key.device,
                    parameter: key.parameter,
                },
                entity.clone(),
                Some(*key),
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
            let active_gesture_matches = view
                .device_gesture
                .snapshot()
                .is_some_and(|snapshot| snapshot.target == target)
                && !view.device_gesture.has_pending_terminal();
            if finalized.previous != finalized.value || active_gesture_matches {
                if let Err(error) = commit_owned_numeric_edit(view, target, &finalized) {
                    first_error.get_or_insert_with(|| error.to_owned());
                    continue;
                }
                if let Some(key) = parameter_key {
                    view.device_panel
                        .pending_values
                        .insert(key, ParameterValue::Float(finalized.value as f32));
                }
            }
            entity.update(cx, |control, cx| {
                control.accept_finish_edit_for_owner(finalized, cx)
            });
        }
    }
    if let Some(error) = first_error {
        view.error = Some(format!("finish the device value edit: {error}").to_lowercase());
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
        view.device_panel.operator_context_focus.clear();
        operator_panel::preview::cancel_all(view, cx);
        view.device_panel.operator_preview_inventory = None;
        operator_panel::envelope::cancel_all(view, cx);
        view.device_panel.eq8_panels.clear();
        view.device_panel.eq8_audition_toggles.clear();
        view.device_panel.eq8_graphs.clear();
        view.device_panel.eq8_responses.clear();
        view.device_panel.saturator_previews.clear();
        view.device_panel.compressor_previews.clear();
        view.device_panel.eq8_graph_gestures.clear();
        view.device_panel.spectrum_result = None;
        if let Some(audio) = view.audio.as_mut() {
            let _ = audio.control.spectrum_target.clear();
            let _ = audio.control.processor_audition.cancel_immediately();
        }
        view.device_panel.chain_window = Default::default();
        view.device_panel.owner = None;
        view.device_panel.generation = view.project_generation;
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
    let desired_owner = view
        .focused_return
        .filter(|id| {
            state
                .project
                .return_tracks()
                .iter()
                .any(|track| track.id() == *id)
        })
        .map(DevicePanelOwner::Return)
        .or_else(|| view.selected_track.map(DevicePanelOwner::Track));
    if desired_owner != view.device_panel.owner {
        if let Some(audio) = view.audio.as_mut() {
            let _ = audio.control.spectrum_target.clear();
            let _ = audio.control.processor_audition.cancel_immediately();
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
        view.device_panel.controls.clear();
        view.device_panel.views.clear();
        view.device_panel.pending_values.clear();
        view.device_panel.operator_pages.clear();
        view.device_panel.operator_context_focus.clear();
        operator_panel::preview::cancel_interactions(view, cx);
        operator_panel::envelope::cancel_all(view, cx);
        view.device_panel.eq8_panels.clear();
        view.device_panel.eq8_audition_toggles.clear();
        view.device_panel.eq8_graphs.clear();
        view.device_panel.eq8_responses.clear();
        view.device_panel.saturator_previews.clear();
        view.device_panel.compressor_previews.clear();
        view.device_panel.eq8_graph_gestures.clear();
        view.device_panel.spectrum_result = None;
        view.device_panel.chain_window = Default::default();
        view.device_panel.owner = desired_owner;
        view.device_panel.message = None;
    }
    match view.device_panel.owner {
        Some(DevicePanelOwner::Track(track)) => render_track(view, window, cx, &state, track),
        Some(DevicePanelOwner::Return(return_track)) => {
            return_view::render(view, window, cx, &state, return_track)
        }
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
        DevicePanelOwner::Track(track) => state.project.track(track).is_some(),
        DevicePanelOwner::Return(return_track) => state
            .project
            .return_tracks()
            .iter()
            .any(|candidate| candidate.id() == return_track),
    }
}

fn render_owner(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    state: &ViewState,
    owner: DevicePanelOwner,
) -> Stateful<Div> {
    match owner {
        DevicePanelOwner::Track(track) => render_track(view, window, cx, state, track),
        DevicePanelOwner::Return(return_track) => {
            return_view::render(view, window, cx, state, return_track)
        }
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
    view.device_panel.views.clear();
    view.device_panel.pending_values.clear();
    retire_removed_eq8_graphs(view, cx, &HashSet::new());
}

pub(super) fn retire_removed_eq8_graphs(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    alive: &HashSet<DeviceId>,
) {
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

fn clear_removed_spectrum_target(
    view: &mut EditorView,
    owner: hex_engine::ProcessorChainOwner,
    identities: &HashMap<DeviceId, DeviceControlIdentity>,
) {
    let stale = view.audio.as_ref().and_then(|audio| {
        audio.control.spectrum_target.desired().map(|target| {
            target.identity.owner == owner
                && !identities.iter().any(|(device, identity)| {
                    device.value() == target.identity.device_id
                        && identity.class == NativeDeviceClass::Effect
                        && identity.type_id == target.identity.device_type
                        && identity.type_id.as_str() == Ok("hex.eq8")
                })
        })
    }) == Some(true);
    if stale && let Some(audio) = view.audio.as_mut() {
        let _ = audio.control.spectrum_target.clear();
        view.device_panel.spectrum_result = None;
    }
}

fn clear_removed_processor_audition_target(
    view: &mut EditorView,
    owner: hex_engine::ProcessorChainOwner,
    identities: &HashMap<DeviceId, DeviceControlIdentity>,
) {
    let stale = view.audio.as_ref().and_then(|audio| {
        audio.control.processor_audition.desired().map(|target| {
            target.identity.owner == owner
                && !identities.iter().any(|(device, identity)| {
                    device.value() == target.identity.device_id
                        && identity.class == NativeDeviceClass::Effect
                        && identity.type_id == target.identity.device_type
                        && identity.type_id.as_str() == Ok("hex.eq8")
                })
        })
    }) == Some(true);
    if stale && let Some(audio) = view.audio.as_mut() {
        let _ = audio.control.processor_audition.cancel_immediately();
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
    let devices = track.devices().to_vec();
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
    clear_removed_spectrum_target(
        view,
        hex_engine::ProcessorChainOwner::Track(track_id.value()),
        &identities,
    );
    clear_removed_processor_audition_target(
        view,
        hex_engine::ProcessorChainOwner::Track(track_id.value()),
        &identities,
    );
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
    let stale_target_has_work = active_target.is_some_and(|target| match target {
        GestureTarget::DeviceParameter { device, .. } => !alive.contains(&device),
        GestureTarget::DeviceParameterPair { device, .. } => !alive.contains(&device),
        GestureTarget::ReturnSend { .. } => false,
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
    let alive_eq8: HashSet<_> = identities
        .iter()
        .filter_map(|(device, identity)| {
            (identity.class == NativeDeviceClass::Effect
                && identity.type_id.as_str().ok() == Some("hex.eq8"))
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

    if view.device_panel.effect_catalog.is_none() {
        view.device_panel.effect_catalog = Some(
            hex_engine::native_device_catalog_for_class(NativeDeviceClass::Effect)
                .map_err(|error| error.to_string().to_lowercase()),
        );
    }
    if view.device_panel.instrument_catalog.is_none() {
        view.device_panel.instrument_catalog = Some(
            hex_engine::native_device_catalog_for_class(NativeDeviceClass::Instrument)
                .map_err(|error| error.to_string().to_lowercase()),
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
        .gap(px(DEVICE_CHAIN_PADDING))
        .p(px(DEVICE_CHAIN_PADDING))
        .overflow_x_scroll()
        .overflow_y_hidden();
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
    let first_device = device_ids.first().copied();
    let mut effect_index: usize = 0;
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
            );
            chain = chain.child(card);
            if class == NativeDeviceClass::Effect {
                effect_index = effect_index.saturating_add(1);
            }
            continue;
        }
        let rendered = match class {
            NativeDeviceClass::Instrument => render_instrument(view, cx, track_id, device),
            NativeDeviceClass::Effect => {
                let rendered = render_effect(view, cx, track_id, &devices, effect_index, device);
                effect_index = effect_index.saturating_add(1);
                rendered
            }
        };
        let focus = view.device_panel.chain_window.focus(device.id(), cx);
        chain = chain.child(
            render_device_card_target(
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
                },
                rendered,
            )
            .track_focus(&focus),
        );
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
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(if view.device_panel.message.is_some() {
                    theme::color(theme::PaletteRole::Primary)
                } else {
                    theme::color(theme::PaletteRole::Muted)
                }))
                .child(theme::text(title)),
        )
        .child(add_button);
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
}

/// Keep only previews belonging to live effects of the matching registered type.
fn retire_transfer_previews(
    panel: &mut DevicePanelState,
    type_of: impl Fn(DeviceId) -> Option<hex_lattice_contract::DeviceTypeId>,
) {
    panel.saturator_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.saturator"))
    });
    panel.compressor_previews.retain(|device, _| {
        type_of(*device).is_some_and(|type_id| type_id.as_str() == Ok("hex.compressor"))
    });
}

fn device_card_width(class: NativeDeviceClass, type_id: hex_lattice_contract::DeviceTypeId) -> f32 {
    match (class, type_id.as_str().ok()) {
        (NativeDeviceClass::Instrument, Some("hex.instrument.operator")) => {
            operator_panel::OPERATOR_FRAME_WIDTH
        }
        (NativeDeviceClass::Effect, Some("hex.utility")) => UTILITY_FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.eq8")) => eq8_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.saturator")) => saturator_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.compressor")) => compressor_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.gate")) => gate_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.auto-pan")) => auto_pan_panel::FRAME_WIDTH,
        _ => DEVICE_FRAME_WIDTH,
    }
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
        .w(px(width))
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
                _ if inside && pointer < left + (right - left) / 2.0 => Some(left_target),
                _ if inside => Some(right_target),
                _ => None,
            };
            let previous = payload.destination.get();
            if let Some(target) = target.filter(|_| inside) {
                if previous != Some(target) {
                    payload.destination.set(Some(target));
                    entity.update(cx, |_, cx| cx.notify());
                }
            }
        })
        .on_drop(
            cx.listener(move |view, payload: &drag::NativeDeviceDrag, window, cx| {
                let target = payload.destination.get().unwrap_or(left_target);
                view.drop_native_device(payload, target, window, cx);
            }),
        )
        .child(rendered);
    if active_left {
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
    if active {
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

fn device_drag_grip(
    device: &hex_project::Device,
    class: NativeDeviceClass,
    project_generation: u64,
    revision: u64,
    compact: bool,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let id = device.id();
    let payload = drag::NativeDeviceDrag {
        project_generation,
        revision,
        source: drag::NativeDeviceDragSource::Existing {
            device: id,
            class,
            type_id: device.state().type_id(),
        },
        destination: std::rc::Rc::new(std::cell::Cell::new(None)),
    };
    let entity = cx.entity();
    let release_entity = entity.clone();
    div()
        .id(("device-drag-grip", id.value()))
        .debug_selector(move || format!("device-drag-grip-{}", id.value()))
        .w(px(if compact { 12.0 } else { 14.0 }))
        .when(compact, |style| style.h(px(12.0)))
        .when(!compact, |style| style.h_full())
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .cursor_move()
        .text_size(px(if compact { 9.0 } else { 12.0 }))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .hover(|style| style.text_color(rgb(theme::color(theme::PaletteRole::Text))))
        .tooltip(|_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                description: "drag to reorder device".into(),
            })
            .into()
        })
        .child(theme::text("⠿"))
        .on_drag(payload.clone(), move |payload, _, window, cx| {
            cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
            entity.update(cx, |view, cx| {
                view.begin_native_device_drag(payload);
                cx.notify();
            });
            cx.new(|_| NativeDeviceDragGhost)
        })
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
            release_entity.update(cx, |view, cx| {
                view.clear_native_device_drag(&payload);
                cx.notify();
            });
        })
}

struct NativeDeviceDragGhost;

impl gpui::Render for NativeDeviceDragGhost {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut Context<'_, Self>,
    ) -> impl gpui::IntoElement {
        div().w(px(0.0)).h(px(0.0))
    }
}

fn render_effect(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    track: TrackId,
    effects: &[hex_project::Device],
    index: usize,
    device: &hex_project::Device,
) -> Stateful<Div> {
    let device_id = device.id();
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
                .map_err(|error| error.to_string().to_lowercase())
        });
    let mut header_actions = device_actions(track, effects, index, cx);
    header_actions = header_actions.child(device_drag_grip(
        device,
        NativeDeviceClass::Effect,
        view.project_generation,
        view.state.as_ref().map_or(0, |state| state.revision),
        matches!(
            device.state().type_id().as_str(),
            Ok("hex.utility" | "hex.saturator" | "hex.compressor" | "hex.gate" | "hex.auto-pan")
        ),
        cx,
    ));
    let rendered = match &device_view {
        Ok(device_view) => {
            let body = if device_view.type_id.as_str() == Ok("hex.utility") {
                utility_panel::render_body(view, cx, device_id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.compressor") {
                compressor_panel::render_body(view, cx, device_id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.saturator") {
                saturator_panel::render_body(view, cx, device_id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.gate") {
                gate_panel::render_body(view, cx, device_id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.auto-pan") {
                auto_pan_panel::render_body(view, cx, device_id, device_view)
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
                let mut body = div()
                    .id(ElementId::Name(
                        format!("device-body-{}", device_id.value()).into(),
                    ))
                    .debug_selector(move || format!("device-body-{}", device_id.value()))
                    .w_full()
                    .h(px(DEVICE_FRAME_BODY_HEIGHT))
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_wrap()
                    .content_start()
                    .gap(px(2.0))
                    .p(px(3.0));
                for parameter in &device_view.parameters {
                    body = body.child(parameter_control(
                        view,
                        cx,
                        device_id,
                        DeviceControlIdentity {
                            class: NativeDeviceClass::Effect,
                            type_id: device_view.type_id,
                        },
                        parameter,
                    ));
                }
                body
            };
            if matches!(
                device_view.type_id.as_str(),
                Ok("hex.utility"
                    | "hex.eq8"
                    | "hex.saturator"
                    | "hex.compressor"
                    | "hex.gate"
                    | "hex.auto-pan")
            ) {
                compact_device_frame(
                    theme::text(device_view.name.clone()),
                    control_style(),
                    header_actions,
                    body,
                )
            } else {
                device_frame(
                    theme::text(device_view.name.clone()),
                    control_style(),
                    header_actions,
                    body,
                )
            }
        }
        Err(error) => {
            let compact = matches!(
                device.state().type_id().as_str(),
                Ok("hex.utility"
                    | "hex.eq8"
                    | "hex.saturator"
                    | "hex.compressor"
                    | "hex.gate"
                    | "hex.auto-pan")
            );
            let body = div()
                .id(ElementId::Name(
                    format!("device-body-{}", device_id.value()).into(),
                ))
                .debug_selector(move || format!("device-body-{}", device_id.value()))
                .w_full()
                .h(px(if compact {
                    UTILITY_FRAME_BODY_HEIGHT
                } else {
                    DEVICE_FRAME_BODY_HEIGHT
                }))
                .min_h_0()
                .overflow_y_scroll()
                .p(px(5.0))
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                .child(theme::text(format!("unsupported device state · {error}")));
            let title = theme::text(
                device
                    .state()
                    .type_id()
                    .as_str()
                    .unwrap_or("unknown device"),
            );
            if compact {
                compact_device_frame(title, control_style(), header_actions, body)
            } else {
                device_frame(title, control_style(), header_actions, body)
            }
        }
    };
    view.device_panel.views.insert(
        device_id,
        (
            NativeDeviceClass::Effect,
            device.state().clone(),
            device_view,
        ),
    );
    let selector = format!("effect-device-{}", device_id.value());
    let frame_width = match device.state().type_id().as_str() {
        Ok("hex.utility") => UTILITY_FRAME_WIDTH,
        Ok("hex.eq8") => eq8_panel::FRAME_WIDTH,
        Ok("hex.saturator") => saturator_panel::FRAME_WIDTH,
        Ok("hex.compressor") => compressor_panel::FRAME_WIDTH,
        Ok("hex.gate") => gate_panel::FRAME_WIDTH,
        Ok("hex.auto-pan") => auto_pan_panel::FRAME_WIDTH,
        _ => DEVICE_FRAME_WIDTH,
    };
    div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .w(px(frame_width))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex_shrink_0()
        .child(rendered)
}

impl EditorView {
    fn drop_native_device_to_return(
        &mut self,
        drag: &drag::NativeDeviceDrag,
        return_track: hex_project::ReturnId,
        window: &mut Window,
        cx: &mut Context<EditorView>,
    ) {
        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
        let current = self.native_device_drag_is_current(drag)
            && self.state.as_ref().is_some_and(|state| {
                state.revision == drag.revision
                    && state
                        .project
                        .return_tracks()
                        .iter()
                        .any(|candidate| candidate.id() == return_track)
            });
        if !current {
            self.error = Some("device drag canceled · project or source changed".into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
            || self.device_gesture.snapshot().is_some()
            || !finalize_controls(self, cx)
        {
            self.error = Some("device edit is still applying; drag again".into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        let command = match drag.source {
            drag::NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Effect,
                type_id,
            } => hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Effect,
                type_id,
            )
            .map(|state| Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            })
            .map_err(|error| error.to_string().to_lowercase()),
            drag::NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Instrument,
                ..
            } => Err("return tracks accept effects only".to_owned()),
            drag::NativeDeviceDragSource::Existing { device, .. } => {
                let owns_device = self.state.as_ref().is_some_and(|state| {
                    state.project.return_tracks().iter().any(|candidate| {
                        candidate.id() == return_track
                            && candidate
                                .devices()
                                .iter()
                                .any(|effect| effect.id() == device)
                    })
                });
                if owns_device {
                    Ok(Command::MoveReturnDevice {
                        device,
                        before: None,
                    })
                } else {
                    Err("devices can only be reordered within their current owner".to_owned())
                }
            }
        };
        self.clear_native_device_drag(drag);
        match command {
            Ok(command) => {
                if self.enqueue_device_barrier(Request::Edit(command)) {
                    self.error = None;
                    self.device_panel.message = None;
                    self.status = "updating return effects".into();
                }
            }
            Err(reason) => {
                self.error = Some(reason.clone());
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
            self.device_panel.message = self.error.clone();
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
            self.device_panel.message = self.error.clone();
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
            self.device_panel.message = self.error.clone();
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
            self.device_panel.message = self.error.clone();
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
                self.device_panel.message = self.error.clone();
                self.clear_native_device_drag(drag);
                cx.notify();
                return;
            }
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
    cx: &mut Context<EditorView>,
    track: TrackId,
    device: &hex_project::Device,
) -> Stateful<Div> {
    let device_id = device.id();
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
            .map_err(|error| error.to_string().to_lowercase())
    });
    let is_operator = device.state().type_id().as_str() == Ok("hex.instrument.operator");
    let header_actions = instrument_actions(track, device, cx).child(device_drag_grip(
        device,
        NativeDeviceClass::Instrument,
        view.project_generation,
        view.state.as_ref().map_or(0, |state| state.revision),
        is_operator,
        cx,
    ));
    let rendered = match &device_view {
        Ok(device_view) => {
            if is_operator && device_view.type_id.as_str() == Ok("hex.instrument.operator") {
                let body = operator_panel::render_body(view, cx, device_id, device_view);
                compact_device_frame(
                    theme::text(device_view.name.clone()),
                    operator_panel::frame_style(),
                    header_actions,
                    body,
                )
            } else {
                let mut body = div()
                    .id(ElementId::Name(
                        format!("device-body-{}", device_id.value()).into(),
                    ))
                    .debug_selector(move || format!("device-body-{}", device_id.value()))
                    .w_full()
                    .h(px(DEVICE_FRAME_BODY_HEIGHT))
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_wrap()
                    .content_start()
                    .gap(px(2.0))
                    .p(px(3.0));
                if device_view.parameters.is_empty() {
                    body = body.child(menu_message("no parameters"));
                }
                for parameter in &device_view.parameters {
                    body = body.child(parameter_control(
                        view,
                        cx,
                        device_id,
                        DeviceControlIdentity {
                            class: NativeDeviceClass::Instrument,
                            type_id: device_view.type_id,
                        },
                        parameter,
                    ));
                }
                device_frame(
                    theme::text(device_view.name.clone()),
                    control_style(),
                    header_actions,
                    body,
                )
            }
        }
        Err(error) => {
            let body = div()
                .id(ElementId::Name(
                    format!("device-body-{}", device_id.value()).into(),
                ))
                .debug_selector(move || format!("device-body-{}", device_id.value()))
                .w_full()
                .h(px(DEVICE_FRAME_BODY_HEIGHT))
                .min_h_0()
                .overflow_y_scroll()
                .p(px(5.0))
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                .child(theme::text(format!(
                    "unsupported instrument state · {error}"
                )));
            device_frame(
                theme::text(
                    device
                        .state()
                        .type_id()
                        .as_str()
                        .unwrap_or("unknown instrument"),
                ),
                control_style(),
                header_actions,
                body,
            )
        }
    };
    view.device_panel.views.insert(
        device_id,
        (
            NativeDeviceClass::Instrument,
            device.state().clone(),
            device_view,
        ),
    );
    let selector = format!("instrument-device-{}", device_id.value());
    let frame_width = if is_operator {
        operator_panel::OPERATOR_FRAME_WIDTH
    } else {
        DEVICE_FRAME_WIDTH
    };
    div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .w(px(frame_width))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex_shrink_0()
        .child(rendered)
}

fn instrument_actions(
    track: TrackId,
    device: &hex_project::Device,
    cx: &mut Context<EditorView>,
) -> Div {
    let id = device.id();
    let bypassed = device.bypassed();
    let mut actions = div().flex().items_center().gap(px(2.0));
    actions = actions.child(
        action_button(
            format!("device-bypass-{}", id.value()),
            if bypassed { "⊘" } else { "●" },
            "toggle instrument bypass",
            if bypassed {
                theme::color(theme::PaletteRole::Muted)
            } else {
                theme::color(theme::PaletteRole::SecondaryText)
            },
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if finalize_controls(view, cx) {
                view.enqueue_device_barrier(Request::Edit(Command::SetDeviceBypass {
                    device: id,
                    bypassed: !bypassed,
                }));
            }
            cx.notify();
        })),
    );
    actions = actions.child(
        action_button(
            format!("instrument-replace-{}", id.value()),
            "↻",
            "replace instrument",
            theme::color(theme::PaletteRole::Text),
        )
        .on_click(cx.listener(|view, _, _, cx| {
            view.device_panel.add_menu_open = true;
            cx.notify();
        })),
    );
    actions.child(
        action_button(
            format!("instrument-remove-{}", id.value()),
            "×",
            "remove instrument",
            theme::color(theme::PaletteRole::Primary),
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if finalize_controls(view, cx) {
                view.enqueue_device_barrier(Request::Edit(Command::RemoveInstrument { track }));
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
        .right(px(4.0))
        .h(px(23.0))
        .flex()
        .items_center()
        .gap(px(3.0))
        .px(px(4.0))
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

fn device_actions(
    track: TrackId,
    devices: &[hex_project::Device],
    index: usize,
    cx: &mut Context<EditorView>,
) -> Div {
    let device = &devices[index];
    let id = device.id();
    let action_size = if matches!(
        device.state().type_id().as_str(),
        Ok("hex.utility" | "hex.saturator" | "hex.compressor" | "hex.gate" | "hex.auto-pan")
    ) {
        12.0
    } else {
        14.0
    };
    let bypassed = device.bypassed();
    let before_up = index.checked_sub(1).map(|previous| devices[previous].id());
    let after_down = devices.get(index + 2).map(|device| device.id());
    let has_down = index + 1 < devices.len();
    let mut actions = div().flex().items_center().gap(px(2.0));
    actions = actions.child(
        action_button_sized(
            format!("device-bypass-{}", id.value()),
            if bypassed { "⊘" } else { "●" },
            "toggle device bypass",
            if bypassed {
                theme::color(theme::PaletteRole::Muted)
            } else {
                theme::color(theme::PaletteRole::SecondaryText)
            },
            action_size,
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if finalize_controls(view, cx) {
                view.enqueue_device_barrier(Request::Edit(Command::SetDeviceBypass {
                    device: id,
                    bypassed: !bypassed,
                }));
            }
            cx.notify();
        })),
    );
    actions = actions.child(
        action_button_sized(
            format!("device-up-{}", id.value()),
            "‹",
            "move device left",
            theme::color(theme::PaletteRole::Text),
            action_size,
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if let Some(before) = before_up
                && finalize_controls(view, cx)
            {
                view.enqueue_device_barrier(Request::Edit(Command::MoveDevice {
                    device: id,
                    track,
                    before: Some(before),
                }));
            }
            cx.notify();
        })),
    );
    actions = actions.child(
        action_button_sized(
            format!("device-down-{}", id.value()),
            "›",
            "move device right",
            theme::color(theme::PaletteRole::Text),
            action_size,
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if has_down && finalize_controls(view, cx) {
                view.enqueue_device_barrier(Request::Edit(Command::MoveDevice {
                    device: id,
                    track,
                    before: after_down,
                }));
            }
            cx.notify();
        })),
    );
    actions.child(
        action_button_sized(
            format!("device-remove-{}", id.value()),
            "×",
            "remove device",
            theme::color(theme::PaletteRole::Primary),
            action_size,
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if finalize_controls(view, cx) {
                view.enqueue_device_barrier(Request::Edit(Command::RemoveDevice(id)));
            }
            cx.notify();
        })),
    )
}

fn action_button(
    id: String,
    icon: &'static str,
    tooltip: &'static str,
    color: u32,
) -> Stateful<Div> {
    action_button_sized(id, icon, tooltip, color, 14.0)
}

fn action_button_sized(
    id: String,
    icon: &'static str,
    tooltip: &'static str,
    color: u32,
    size: f32,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.clone().into()))
        .debug_selector(move || id.clone())
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(if size <= 12.0 { 8.0 } else { 10.0 }))
        .text_color(rgb(color))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
        .cursor(gpui::CursorStyle::PointingHand)
        .tooltip(move |_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                description: tooltip.into(),
            })
            .into()
        })
        .child(theme::text(icon))
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlPresentation {
    Standard,
    CompactDial,
    MiniDial,
    MicroDial,
    InlineValue,
    CompactButton,
    CompactSelector,
    CompactWideSelector,
}

#[derive(Clone, Copy, Default)]
struct ControlText {
    label: Option<&'static str>,
    codec: Option<numeric_display::NumericDisplayCodec>,
}

pub(super) fn effective_parameter_value(
    view: &mut EditorView,
    key: ParameterKey,
    canonical_value: ParameterValue,
) -> ParameterValue {
    let gesture_value = view
        .device_gesture
        .snapshot()
        .and_then(|snapshot| match snapshot.target {
            GestureTarget::DeviceParameter { device, parameter }
                if device == key.device && parameter == key.parameter =>
            {
                Some(snapshot.value)
            }
            GestureTarget::DeviceParameterPair { device, first, .. }
                if device == key.device && first == key.parameter =>
            {
                Some(snapshot.value)
            }
            GestureTarget::DeviceParameterPair { device, second, .. }
                if device == key.device && second == key.parameter =>
            {
                snapshot.secondary_value
            }
            _ => None,
        });
    let pending_value = view.device_panel.pending_values.get(&key).copied();
    if pending_value.is_some()
        && view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.device_gesture_barriers.is_empty()
        && view.device_gesture.snapshot().is_none()
    {
        view.device_panel.pending_values.remove(&key);
    }
    gesture_value.unwrap_or_else(|| {
        view.device_panel
            .pending_values
            .get(&key)
            .copied()
            .unwrap_or(canonical_value)
    })
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
    let style = if identity.type_id.as_str() == Ok("hex.instrument.operator")
        && presentation == ControlPresentation::InlineValue
    {
        let mut style = operator_panel::display_style();
        style.text = style.accent;
        style
    } else {
        control_style()
    };
    let display_label = display.label;
    let key = ParameterKey {
        device,
        parameter: parameter.descriptor.id,
    };
    let descriptor = parameter.descriptor;
    let value = effective_parameter_value(view, key, parameter.value);
    let enabled = parameter.enabled;
    let compatible = matches!(
        (descriptor.kind, value),
        (ParameterKind::Float { .. }, ParameterValue::Float(_))
            | (ParameterKind::Bool, ParameterValue::Bool(_))
            | (ParameterKind::Choice { .. }, ParameterValue::Choice(_))
    );
    if !compatible {
        if let Some(mut control) = view.device_panel.controls.remove(&key) {
            cancel_control(view, cx, key, &mut control);
        }
        return unsupported_parameter(descriptor.label);
    }
    let matching = match view.device_panel.controls.get(&key) {
        Some(ParameterControl::Float {
            class: old_class,
            type_id: old_type,
            descriptor: old_descriptor,
            ..
        }) => {
            *old_class == identity.class
                && *old_type == identity.type_id
                && *old_descriptor == descriptor
                && matches!(descriptor.kind, ParameterKind::Float { .. })
        }
        Some(ParameterControl::Bool {
            class: old_class,
            type_id: old_type,
            descriptor: old_descriptor,
            ..
        }) => {
            *old_class == identity.class
                && *old_type == identity.type_id
                && *old_descriptor == descriptor
                && matches!(descriptor.kind, ParameterKind::Bool)
        }
        Some(ParameterControl::Choice {
            class: old_class,
            type_id: old_type,
            descriptor: old_descriptor,
            ..
        }) => {
            *old_class == identity.class
                && *old_type == identity.type_id
                && *old_descriptor == descriptor
                && matches!(descriptor.kind, ParameterKind::Choice { .. })
        }
        None => false,
    };
    if !matching {
        if let Some(mut old) = view.device_panel.controls.remove(&key) {
            cancel_control(view, cx, key, &mut old);
        }
        if create_parameter_control(view, cx, key, identity, parameter, presentation, display)
            .is_none()
        {
            return unsupported_parameter(descriptor.label);
        }
    }
    match view.device_panel.controls.get(&key) {
        Some(ParameterControl::Float { entity, .. }) => {
            if let ParameterValue::Float(value) = value {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    control.set_enabled(enabled, cx);
                    let _ = control.set_external_value(f64::from(value), cx);
                });
            } else {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    control.set_enabled(enabled, cx);
                });
            }
            let (width, height) = match presentation {
                ControlPresentation::CompactDial => (64.0, 52.0),
                ControlPresentation::MiniDial | ControlPresentation::MicroDial => (
                    hex_widgets::MICRO_DIAL_WIDTH,
                    hex_widgets::MICRO_DIAL_HEIGHT,
                ),
                ControlPresentation::InlineValue => (46.0, 14.0),
                _ => (136.0, 26.0),
            };
            div().w(px(width)).h(px(height)).child(entity.clone())
        }
        Some(ParameterControl::Bool { entity, .. }) => {
            if let ParameterValue::Bool(value) = value {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    control.set_enabled(enabled, cx);
                    control.set_checked(value, cx);
                });
            } else {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    control.set_enabled(enabled, cx);
                });
            }
            let (width, height) = if presentation == ControlPresentation::CompactButton {
                let width = match display_label {
                    Some("bass mono" | "mono") => 64.0,
                    Some("bass mono audition") => 14.0,
                    Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8") => 18.0,
                    Some("on" | "fix" | "") => 18.0,
                    Some("ø l" | "ø r" | "mute" | "dc") => 31.0,
                    _ => 40.0,
                };
                (width, 14.0)
            } else {
                (136.0, 22.0)
            };
            div().w(px(width)).h(px(height)).child(entity.clone())
        }
        Some(ParameterControl::Choice { entity, .. }) => {
            if let ParameterValue::Choice(value) = value {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    control.set_enabled(enabled, cx);
                    let _ = control.set_selected(value as usize, cx);
                });
            } else {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    control.set_enabled(enabled, cx);
                });
            }
            let (width, height) = match presentation {
                ControlPresentation::CompactSelector => (64.0, 14.0),
                ControlPresentation::CompactWideSelector => (120.0, 14.0),
                _ => (136.0, 22.0),
            };
            div().w(px(width)).h(px(height)).child(entity.clone())
        }
        None => unsupported_parameter(descriptor.label),
    }
}

fn create_parameter_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    key: ParameterKey,
    identity: DeviceControlIdentity,
    parameter: &NativeDeviceParameterView,
    presentation: ControlPresentation,
    display: ControlText,
) -> Option<()> {
    let descriptor = parameter.descriptor;
    let value = parameter.value;
    let operator_context_owner = operator_panel::context::Owner::capture(view, key.device);
    let style = if identity.type_id.as_str() == Ok("hex.instrument.operator")
        && presentation == ControlPresentation::InlineValue
    {
        let mut style = operator_panel::display_style();
        style.text = style.accent;
        style
    } else {
        control_style()
    };
    let ControlText {
        label: display_label,
        codec: display_codec,
    } = display;
    let id = format!(
        "device-{}-parameter-{}",
        key.device.value(),
        key.parameter.value()
    );
    match (descriptor.kind, value) {
        (ParameterKind::Float { min, max, unit }, ParameterValue::Float(value)) if min < max => {
            let default = match descriptor.default {
                ParameterValue::Float(default) => f64::from(default),
                _ => return None,
            };
            let scale = match descriptor.presentation {
                ParameterPresentation::LinearAmplitudeDecibels => {
                    NumericScale::LinearAmplitudeDecibels
                }
                ParameterPresentation::Logarithmic => NumericScale::Logarithmic,
                ParameterPresentation::Linear if min < 0.0 && max > 0.0 => NumericScale::Bipolar,
                ParameterPresentation::Linear => NumericScale::Linear,
            };
            let mut spec = NumericSpec::new(
                f64::from(min),
                f64::from(max),
                default,
                scale,
                2,
                unit.map(|unit| std::sync::Arc::<str>::from(theme::text(unit).as_ref())),
            )
            .ok()?;
            if let Some(codec) = display_codec {
                spec = codec.apply(spec);
            }
            let kind = match presentation {
                ControlPresentation::CompactDial
                | ControlPresentation::MiniDial
                | ControlPresentation::MicroDial => NumericControlKind::Knob,
                _ => NumericControlKind::HorizontalSlider,
            };
            let appearance = match presentation {
                ControlPresentation::CompactDial => NumericControlAppearance::CompactDial,
                ControlPresentation::MiniDial => NumericControlAppearance::MiniDial,
                ControlPresentation::MicroDial => NumericControlAppearance::MicroDial,
                ControlPresentation::InlineValue => NumericControlAppearance::InlineValue,
                _ => NumericControlAppearance::Standard,
            };
            let config = NumericControlConfig::new(
                id,
                theme::text(display_label.unwrap_or(descriptor.label)),
                kind,
                spec,
                f64::from(value),
                style,
            )
            .ok()?
            .with_appearance(appearance)
            .with_dial_layout(
                if display_codec == Some(numeric_display::NumericDisplayCodec::Balance) {
                    DialLayout::Pan
                } else {
                    DialLayout::ReadoutGap
                },
            );
            let entity = cx.new(|control_cx| NumericControl::from_config(control_cx, config));
            let target = GestureTarget::DeviceParameter {
                device: key.device,
                parameter: key.parameter,
            };
            let subscription =
                cx.subscribe(&entity, move |view, _, event: &NumericControlEvent, cx| {
                    if event.phase == NumericEventPhase::Begin
                        && identity.class == NativeDeviceClass::Instrument
                        && identity.type_id.as_str() == Ok("hex.instrument.operator")
                        && let Some(operator) =
                            operator_panel::shell_operator_for_numeric_parameter(key.parameter)
                    {
                        if let Some(owner) = operator_context_owner {
                            operator_panel::context::select_oscillator(
                                view,
                                cx,
                                owner,
                                operator,
                                &[key],
                            );
                        }
                    }
                    let value = ParameterValue::Float(event.value as f32);
                    if let Err(error) = dispatch_numeric_gesture(view, target, event.phase, value)
                        && event.phase != NumericEventPhase::Cancel
                    {
                        view.error = Some(error.to_owned());
                    }
                    cx.notify();
                });
            view.device_panel.controls.insert(
                key,
                ParameterControl::Float {
                    entity,
                    _subscription: subscription,
                    class: identity.class,
                    type_id: identity.type_id,
                    descriptor,
                },
            );
            Some(())
        }
        (ParameterKind::Bool, ParameterValue::Bool(value)) => {
            let appearance = if presentation == ControlPresentation::CompactButton {
                if display_label == Some("") {
                    DiscreteControlAppearance::MicroSquare
                } else if display_label == Some("bass mono audition") {
                    DiscreteControlAppearance::CompactIconButton
                } else {
                    DiscreteControlAppearance::CompactButton
                }
            } else {
                DiscreteControlAppearance::Standard
            };
            let config = ToggleConfig::new(
                id,
                theme::text(display_label.unwrap_or(descriptor.label)),
                value,
                style,
            )
            .ok()?
            .with_appearance(appearance);
            let entity = cx.new(|control_cx| Toggle::from_config(control_cx, config));
            let target = key;
            let subscription = cx.subscribe(&entity, move |view, _, event: &ToggleChanged, cx| {
                // A discrete control may hide a numeric field (for example,
                // switching Operator tuning to fixed frequency). Do not let
                // that transition strand an invalid draft offscreen.
                if !finalize_controls(view, cx) {
                    let previous = view
                        .device_panel
                        .pending_values
                        .get(&target)
                        .copied()
                        .or_else(|| {
                            view.device_panel
                                .views
                                .get(&target.device)
                                .and_then(|(_, _, device_view)| device_view.as_ref().ok())
                                .and_then(|device_view| {
                                    device_view.parameters.iter().find(|parameter| {
                                        parameter.descriptor.id == target.parameter
                                    })
                                })
                                .map(|parameter| parameter.value)
                        })
                        .unwrap_or(ParameterValue::Bool(!event.checked));
                    if let (
                        ParameterValue::Bool(previous),
                        Some(ParameterControl::Bool { entity, .. }),
                    ) = (previous, view.device_panel.controls.get(&target))
                    {
                        let entity = entity.clone();
                        entity.update(cx, |control, cx| control.set_checked(previous, cx));
                    }
                    return;
                }
                let value = ParameterValue::Bool(event.checked);
                view.device_panel.pending_values.insert(target, value);
                if !view.enqueue_device_barrier(Request::SetDeviceParameter {
                    device: target.device,
                    parameter: target.parameter,
                    value,
                }) {
                    view.device_panel.pending_values.remove(&target);
                }
                cx.notify();
            });
            view.device_panel.controls.insert(
                key,
                ParameterControl::Bool {
                    entity,
                    _subscription: subscription,
                    class: identity.class,
                    type_id: identity.type_id,
                    descriptor,
                },
            );
            Some(())
        }
        (ParameterKind::Choice { labels }, ParameterValue::Choice(value)) => {
            let choices = labels.iter().copied().map(theme::text);
            let appearance = if matches!(
                presentation,
                ControlPresentation::CompactSelector | ControlPresentation::CompactWideSelector
            ) {
                DiscreteControlAppearance::CompactSelector
            } else {
                DiscreteControlAppearance::Standard
            };
            let config = SelectorConfig::new(id, choices, value as usize, style)
                .ok()?
                .with_appearance(appearance);
            let entity = cx.new(|control_cx| Selector::from_config(control_cx, config));
            let target = key;
            let subscription =
                cx.subscribe(&entity, move |view, _, event: &SelectorChanged, cx| {
                    let Ok(value) = u32::try_from(event.index) else {
                        return;
                    };
                    if !finalize_controls(view, cx) {
                        return;
                    }
                    let value = ParameterValue::Choice(value);
                    view.device_panel.pending_values.insert(target, value);
                    if !view.enqueue_device_barrier(Request::SetDeviceParameter {
                        device: target.device,
                        parameter: target.parameter,
                        value,
                    }) {
                        view.device_panel.pending_values.remove(&target);
                    }
                    cx.notify();
                });
            view.device_panel.controls.insert(
                key,
                ParameterControl::Choice {
                    entity,
                    _subscription: subscription,
                    class: identity.class,
                    type_id: identity.type_id,
                    descriptor,
                },
            );
            Some(())
        }
        _ => None,
    }
}

fn cancel_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    key: ParameterKey,
    control: &mut ParameterControl,
) {
    if let ParameterControl::Float { entity, .. } = control {
        entity.update(cx, |control, cx| control.cancel(cx));
        if view.device_gesture.snapshot().is_some_and(|snapshot| {
            snapshot.target
                == (GestureTarget::DeviceParameter {
                    device: key.device,
                    parameter: key.parameter,
                })
        }) {
            let _ = view.cancel_device_gesture();
        }
    }
}

fn unsupported_parameter(label: &str) -> Div {
    div()
        .w(px(136.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(format!("{label} · unsupported")))
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
        .h(px(228.0))
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
mod tests;

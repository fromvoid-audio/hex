//! View-local panels for exact hosted VST3 controller sources. Persisted rows
//! supply names and order only. Native Perform receipts can advance the knob's
//! display. Fresh controller pages authorize new edits. A held pointer uses
//! the admitted numeric gesture and Session key through its terminal.
use super::*;
use gpui::{Entity, Subscription, UniformListScrollHandle, uniform_list};
use hex_audio::prepared::AppliedAudioSource;
use hex_engine::AudioSourceContext;
use hex_session::device_gesture::{GestureKey, PluginParameterOrigin, PluginParameterValue};
use hex_session::service::{
    PluginParameterConversion, PluginParameterConversionQuery, PluginParameterConversionResult,
    PluginParameterQuery, PluginParameterResult, PluginParameterRow, PluginParameterText,
};
use hex_widgets::{
    ControlAvailability, NumericControl, NumericControlConfig, NumericControlEvent,
    NumericControlPresentation, NumericEventPhase, NumericGestureId, NumericScale, NumericSpec,
    NumericTextRequest, NumericTextRequestId, TextInput, Toggle, ToggleAppearance, ToggleChanged,
    ToggleConfig,
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

const PAGE_REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const MAX_AUTOMATIC_RETRIES: u8 = 3;
const MAX_CACHED_STEPS: u32 = 64;
const MAX_FORMATTED_STEP_ENTRIES: usize = 512;
const MAX_UNSUPPORTED_FORMAT_ENTRIES: usize = 512;

pub(super) fn is_toggle_parameter(steps: u32) -> bool {
    steps == 1
}

pub(super) fn toggle_checked(normalized: f64) -> bool {
    normalized >= 0.5
}

pub(super) fn toggle_label(checked: bool, display: Option<&str>) -> &str {
    match display {
        Some(text) if !text.is_empty() => text,
        _ if checked => "on",
        _ => "off",
    }
}

pub(super) fn step_index_from_normalized(steps: u32, normalized: f64) -> u32 {
    if steps == 0 {
        return 0;
    }
    let max = f64::from(steps);
    (normalized.clamp(0.0, 1.0) * max).round().clamp(0.0, max) as u32
}

pub(super) fn control_value_from_normalized(steps: u32, normalized: f64) -> f64 {
    if steps > 1 {
        f64::from(step_index_from_normalized(steps, normalized))
    } else {
        normalized.clamp(0.0, 1.0)
    }
}

pub(super) fn normalized_from_control_value(steps: u32, value: f64) -> f64 {
    if steps > 1 {
        let max = f64::from(steps);
        (value.round().clamp(0.0, max) / max).clamp(0.0, 1.0)
    } else {
        value.clamp(0.0, 1.0)
    }
}

#[derive(Clone)]
pub(super) enum ParameterControl {
    Numeric(Entity<NumericControl>),
    Toggle(Entity<Toggle>),
}

impl ParameterControl {
    pub(super) fn entity_id(&self) -> gpui::EntityId {
        match self {
            Self::Numeric(control) => control.entity_id(),
            Self::Toggle(toggle) => toggle.entity_id(),
        }
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn as_numeric(&self) -> Option<&Entity<NumericControl>> {
        match self {
            Self::Numeric(control) => Some(control),
            Self::Toggle(_) => None,
        }
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn as_toggle(&self) -> Option<&Entity<Toggle>> {
        match self {
            Self::Toggle(toggle) => Some(toggle),
            Self::Numeric(_) => None,
        }
    }

    pub(super) fn is_interacting(&self, cx: &App) -> bool {
        match self {
            Self::Numeric(control) => control.read(cx).is_interacting(),
            Self::Toggle(_) => false,
        }
    }

    pub(super) fn is_editing(&self, cx: &App) -> bool {
        match self {
            Self::Numeric(control) => control.read(cx).is_editing(),
            Self::Toggle(_) => false,
        }
    }

    #[cfg(feature = "ui-test-support")]
    pub(super) fn normalized_value(&self, steps: u32, cx: &App) -> f64 {
        match self {
            Self::Numeric(control) => {
                normalized_from_control_value(steps, control.read(cx).value())
            }
            Self::Toggle(toggle) => {
                if toggle.read(cx).checked() {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    pub(super) fn set_external_state(
        &self,
        steps: u32,
        normalized: f64,
        display: Option<&str>,
        cx: &mut Context<EditorView>,
    ) {
        match self {
            Self::Numeric(control) => {
                let value = control_value_from_normalized(steps, normalized);
                control.update(cx, |control, cx| {
                    let _ = control.set_external_value(value, cx);
                    if let Some(display) = display
                        && !display.is_empty()
                    {
                        let _ = control.set_external_display(value, display, cx);
                    }
                });
            }
            Self::Toggle(toggle) => {
                let checked = toggle_checked(normalized);
                let label = toggle_label(checked, display);
                toggle.update(cx, |toggle, cx| {
                    toggle.set_checked(checked, cx);
                    toggle.set_label(theme::text(hex_session::service::text::verbatim(label)), cx);
                });
            }
        }
    }

    pub(super) fn set_external_display_for_current(
        &self,
        display: &str,
        cx: &mut Context<EditorView>,
    ) {
        if display.is_empty() {
            return;
        }
        match self {
            Self::Numeric(control) => {
                control.update(cx, |control, cx| {
                    let value = control.value();
                    let _ = control.set_external_display(value, display, cx);
                });
            }
            Self::Toggle(toggle) => {
                toggle.update(cx, |toggle, cx| {
                    toggle.set_label(
                        theme::text(hex_session::service::text::verbatim(display)),
                        cx,
                    );
                });
            }
        }
    }

    pub(super) fn set_availability(
        &self,
        availability: ControlAvailability,
        cx: &mut Context<EditorView>,
    ) {
        match self {
            Self::Numeric(control) => {
                control.update(cx, |control, cx| control.set_availability(availability, cx));
            }
            Self::Toggle(toggle) => {
                toggle.update(cx, |toggle, cx| toggle.set_availability(availability, cx));
            }
        }
    }

    pub(super) fn set_refresh_availability(
        &self,
        row: &PluginParameterRow,
        cx: &mut Context<EditorView>,
    ) {
        match self {
            Self::Numeric(control) => {
                control.update(cx, |control, cx| set_refresh_availability(control, row, cx));
            }
            Self::Toggle(toggle) => {
                toggle.update(cx, |toggle, cx| {
                    if row.metadata_revision == 0 || row.editable {
                        toggle.set_availability(ControlAvailability::Busy, cx);
                    } else {
                        toggle.set_enabled(false, cx);
                    }
                });
            }
        }
    }

    pub(super) fn set_external_style(
        &self,
        style: hex_widgets::ControlStyle,
        cx: &mut Context<EditorView>,
    ) {
        match self {
            Self::Numeric(control) => {
                control.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                });
            }
            Self::Toggle(toggle) => {
                toggle.update(cx, |toggle, cx| {
                    let _ = toggle.set_external_style(style, cx);
                });
            }
        }
    }

    pub(super) fn into_slot_element(self) -> gpui::AnyElement {
        match self {
            Self::Numeric(control) => control.into_any_element(),
            Self::Toggle(toggle) => div()
                .w(px(hex_widgets::MICRO_DIAL_WIDTH))
                .h(px(hex_widgets::MICRO_DIAL_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .px(px(2.0))
                .child(toggle)
                .into_any_element(),
        }
    }
}

mod cache;
mod row_projection;
mod scrollbar;
use row_projection::{
    PAGE_SIZE, TouchObservation, display_only, fresh_touch_row, is_refresh_yield, matching_page,
    matching_rows, refresh_page,
};

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    reconciled: Option<(AudioSourceContext, Option<u64>)>,
    panels: HashMap<DeviceId, Panel>,
    in_flight: Option<(PluginParameterQuery, AppliedAudioSource)>,
    deferred_result: Option<(PluginParameterResult, u64)>,
    conversion: Option<PendingConversion>,
    live_format: Option<PendingFormat>,
    queued_format: Option<QueuedFormat>,
    conversion_result: Option<(PluginParameterConversionResult, u64)>,
    conversion_error: Option<String>,
    last_touched_device: Option<DeviceId>,
    reported_status: Option<ReportedStatus>,
}

impl State {
    pub(super) fn has_pending_draft(&self, cx: &App) -> bool {
        self.conversion.is_some()
            || self
                .panels
                .values()
                .any(|panel| panel.has_pending_draft(cx))
    }

    pub(super) fn has_pending_text_edit(&self, cx: &App) -> bool {
        self.conversion.is_some()
            || self
                .panels
                .values()
                .any(|panel| panel.has_pending_text_edit(cx))
    }

    pub(super) fn has_active_control(&self, device: DeviceId, cx: &App) -> bool {
        self.conversion
            .as_ref()
            .is_some_and(|pending| pending.origin.device == device)
            || self
                .panels
                .get(&device)
                .is_some_and(|panel| panel.has_pending_draft(cx))
    }
}
struct PendingConversion {
    query: PluginParameterConversionQuery,
    source: AppliedAudioSource,
    origin: PluginParameterOrigin,
    control: Entity<NumericControl>,
    draft: NumericTextRequestId,
}
struct PendingFormat {
    query: PluginParameterConversionQuery,
    source: AppliedAudioSource,
    steps: u32,
}
#[derive(Clone)]
struct QueuedFormat {
    device: DeviceId,
    source: hex_lattice_contract::ForeignProcessorBinding,
    output_identity: u64,
    context: AudioSourceContext,
    parameter: u32,
    steps: u32,
    normalized: f64,
}
struct DragPreview {
    source: hex_lattice_contract::ForeignProcessorBinding,
    parameter: u32,
    normalized: f64,
    display: String,
}
#[derive(Clone, PartialEq, Eq)]
struct ReportedStatus {
    device: u64,
    message: String,
    error: Option<String>,
}
struct LiveTouchValue {
    touch: TouchObservation,
    value: PluginParameterValue,
    sequence: u64,
    formatted: String,
}

struct Panel {
    owner: super::DevicePanelOwner,
    key: String,
    cache_path: PathBuf,
    cache_loading: bool,
    cache_write_pending: bool,
    cache_needs_save: bool,
    cache_error: Option<String>,
    query: PluginParameterQuery,
    source: Option<hex_lattice_contract::ForeignProcessorBinding>,
    descriptor_signature: Option<[u8; 32]>,
    total: Option<u16>,
    result: Option<Arc<Vec<PluginParameterRow>>>,
    matched: Arc<Vec<usize>>,
    filter: String,
    search: Entity<TextInput>,
    _observe_search: Subscription,
    scroll: UniformListScrollHandle,
    scrollbar: scrollbar::State,
    visible_pages: BTreeSet<u16>,
    priority_page: Option<u16>,
    queued_touch: Option<TouchObservation>,
    live_values: HashMap<u32, LiveTouchValue>,
    formatted_steps: HashMap<(u32, u32), String>,
    format_unsupported: BTreeSet<u32>,
    drag_preview: Option<DragPreview>,
    pending_touch: Option<TouchObservation>,
    last_touched: Option<u32>,
    touch_unavailable: bool,
    follow_knob: Option<(u32, Knob)>,
    placeholder_knob: Option<Entity<NumericControl>>,
    pending_touch_request: Option<u64>,
    next_page: u16,
    knobs: HashMap<u32, Knob>,
    row_placeholders: HashMap<u32, RowPlaceholder>,
    next_query_at: Instant,
    retry_count: u8,
}

impl Panel {
    fn has_pending_draft(&self, cx: &App) -> bool {
        self.knobs
            .values()
            .chain(self.follow_knob.iter().map(|(_, knob)| knob))
            .any(|knob| knob.control.is_interacting(cx) || knob.control.is_editing(cx))
    }

    fn has_pending_text_edit(&self, cx: &App) -> bool {
        self.knobs
            .values()
            .chain(self.follow_knob.iter().map(|(_, knob)| knob))
            .any(|knob| knob.control.is_editing(cx))
    }
}
struct Knob {
    source: hex_lattice_contract::ForeignProcessorBinding,
    output: u64,
    name: String,
    default: f64,
    steps: u32,
    control: ParameterControl,
    origin: Rc<Cell<PluginParameterOrigin>>,
    drag: Rc<KnobDrag>,
    _subscription: Subscription,
    _text_observation: Option<Subscription>,
}
impl Knob {
    fn matches_descriptor(&self, row: &PluginParameterRow) -> bool {
        self.name == row.name && self.default == row.default_normalized && self.steps == row.steps
    }

    fn matches_source(
        &self,
        source: hex_lattice_contract::ForeignProcessorBinding,
        query: PluginParameterQuery,
        row: &PluginParameterRow,
    ) -> bool {
        self.source == source
            && self.output == query.output_identity
            && self.matches_descriptor(row)
    }

    fn owns(&self, key: GestureKey) -> bool {
        self.drag
            .borrow()
            .as_ref()
            .is_some_and(|(_, owner)| *owner == key)
    }

    fn pending_value(
        &self,
        active: Option<hex_session::device_gesture::GestureSnapshot>,
    ) -> Option<f64> {
        active
            .filter(|gesture| self.owns(gesture.key))
            .and_then(|gesture| gesture.plugin_value)
            .map(PluginParameterValue::normalized)
    }

    fn renew(
        &self,
        query: PluginParameterQuery,
        row: &PluginParameterRow,
        presentation: (f64, Option<&str>),
        enabled: bool,
        active: Option<hex_session::device_gesture::GestureSnapshot>,
        cx: &mut Context<EditorView>,
    ) -> Option<ParameterControl> {
        let (value, display) = presentation;
        self.origin.set(parameter_origin(query, row)?);
        let value = self.pending_value(active).unwrap_or(value);
        let busy = active.is_some_and(|gesture| !self.owns(gesture.key));
        self.control
            .set_external_state(row.steps, value, display, cx);
        self.control
            .set_availability(ControlAvailability::for_edit(enabled, busy), cx);
        Some(self.control.clone())
    }
}

struct RowPlaceholder {
    name: String,
    default: f64,
    steps: u32,
    control: ParameterControl,
}

/// Metadata reads keep the control poll active without claiming edit custody.
pub(in crate::editor) fn query_pending(view: &EditorView) -> bool {
    view.device_panel.vst3_parameters.in_flight.is_some()
        || view.device_panel.vst3_parameters.deferred_result.is_some()
        || view.device_panel.vst3_parameters.conversion.is_some()
        || view.device_panel.vst3_parameters.live_format.is_some()
        || view
            .device_panel
            .vst3_parameters
            .conversion_result
            .is_some()
}

impl Panel {
    fn rows(&self) -> Option<&[PluginParameterRow]> {
        self.result.as_ref().map(|rows| rows.as_slice())
    }

    fn observe_value(
        &mut self,
        touch: TouchObservation,
        sequence: u64,
        value: Option<PluginParameterValue>,
    ) {
        // Bound this map by the admitted row inventory. Unknown targets still
        // follow the ordinary controller lookup before displaying a value.
        if let Some(value) = value
            && self.source == Some(touch.binding)
            && self.query.context == touch.source.context
            && self.query.output_identity == touch.source.output_identity
            && let Some(row) = self.result.as_ref().and_then(|rows| {
                rows.iter()
                    .find(|row| row.id == touch.parameter && row.metadata_revision != 0)
            })
            && sequence > row.editor_sequence
        {
            let live = self
                .live_values
                .entry(touch.parameter)
                .or_insert_with(|| LiveTouchValue {
                    touch,
                    value,
                    sequence,
                    formatted: row.value.clone(),
                });
            if sequence > live.sequence {
                live.sequence = sequence;
                live.value = value;
            }
        }
    }

    fn live_value(&self, row: &PluginParameterRow) -> Option<&LiveTouchValue> {
        self.live_values.get(&row.id).filter(|live| {
            self.source == Some(live.touch.binding)
                && self.query.context == live.touch.source.context
                && self.query.output_identity == live.touch.source.output_identity
                && row.metadata_revision != 0
        })
    }

    fn display_value(&self, row: &PluginParameterRow) -> f64 {
        self.live_value(row)
            .map_or(row.normalized, |live| live.value.normalized())
    }

    fn clear_format_caches(&mut self) {
        self.formatted_steps.clear();
        self.format_unsupported.clear();
        self.drag_preview = None;
    }

    fn cache_step_label(&mut self, parameter: u32, steps: u32, normalized: f64, display: &str) {
        if steps == 0 || steps > MAX_CACHED_STEPS || display.is_empty() {
            return;
        }
        let key = (parameter, step_index_from_normalized(steps, normalized));
        if self.formatted_steps.len() >= MAX_FORMATTED_STEP_ENTRIES
            && !self.formatted_steps.contains_key(&key)
        {
            self.formatted_steps.clear();
        }
        self.formatted_steps.insert(key, display.to_owned());
    }

    fn record_row_displays(&mut self, rows: &[PluginParameterRow]) {
        for row in rows {
            if row.metadata_revision != 0 {
                self.cache_step_label(row.id, row.steps, row.normalized, &row.value);
            }
        }
    }

    fn cached_step_label(&self, parameter: u32, steps: u32, normalized: f64) -> Option<&str> {
        if steps == 0 || steps > MAX_CACHED_STEPS {
            return None;
        }
        let key = (parameter, step_index_from_normalized(steps, normalized));
        self.formatted_steps.get(&key).map(String::as_str)
    }

    fn active_drag_preview(&self, parameter: u32) -> Option<&DragPreview> {
        self.drag_preview
            .as_ref()
            .filter(|preview| preview.parameter == parameter && self.source == Some(preview.source))
    }

    fn current_drag_or_row_display(&self, parameter: u32) -> Option<&str> {
        if let Some(preview) = self
            .active_drag_preview(parameter)
            .filter(|preview| !preview.display.is_empty())
        {
            return Some(preview.display.as_str());
        }
        self.rows()?
            .iter()
            .find(|row| row.id == parameter)
            .map(|row| self.display_text(row))
            .filter(|text| !text.is_empty())
    }

    fn display_for_value<'a>(
        &'a self,
        row: &'a PluginParameterRow,
        normalized: f64,
    ) -> Option<&'a str> {
        if let Some(preview) = self
            .active_drag_preview(row.id)
            .filter(|preview| !preview.display.is_empty())
        {
            return Some(preview.display.as_str());
        }
        if let Some(cached) = self.cached_step_label(row.id, row.steps, normalized) {
            return Some(cached);
        }
        if normalized == row.normalized {
            let text = self.display_text(row);
            if !text.is_empty() {
                return Some(text);
            }
        }
        if is_toggle_parameter(row.steps) {
            return Some(toggle_label(toggle_checked(normalized), None));
        }
        None
    }

    fn painted_value(&self, view: &EditorView, device: DeviceId, row: &PluginParameterRow) -> f64 {
        if let Some(preview) = self.active_drag_preview(row.id) {
            return preview.normalized;
        }
        if let Some(live) = self.live_value(row) {
            return live.value.normalized();
        }
        if self.source.is_some()
            && row.metadata_revision != 0
            && let Some(class) = row.automation_class
            && let Some(value) = view
                .device_panel
                .playback_values
                .vst3(device, row.id, class, row.steps)
        {
            return value;
        }
        row.normalized
    }

    fn display_text<'a>(&'a self, row: &'a PluginParameterRow) -> &'a str {
        // Only the SDK page or read-only Format conversion formats values.
        if let Some(preview) = self
            .active_drag_preview(row.id)
            .filter(|preview| !preview.display.is_empty())
        {
            return preview.display.as_str();
        }
        self.live_value(row)
            .map_or(row.value.as_str(), |live| live.formatted.as_str())
    }

    fn reconcile_live_values(
        &mut self,
        source: hex_lattice_contract::ForeignProcessorBinding,
        rows: &[PluginParameterRow],
    ) {
        self.live_values.retain(|&id, live| {
            live.touch.binding == source
                && !rows
                    .iter()
                    .any(|row| row.id == id && row.editor_sequence >= live.sequence)
        });
    }

    fn refilter(&mut self, reset_scroll: bool) {
        let Some(matched) = self.rows().map(|rows| matching_rows(rows, &self.filter)) else {
            return;
        };
        let changed = *self.matched != matched;
        if reset_scroll || changed {
            self.scrollbar.invalidate();
        }
        if changed {
            self.matched = Arc::new(matched);
        }
        if reset_scroll {
            self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        }
    }

    fn mark_values_stale(&mut self) {
        self.mark_values_stale_with_commit(None);
    }

    fn mark_values_stale_with_commit(&mut self, committed: Option<(u32, f64)>) {
        let total = self.rows().and_then(|rows| u16::try_from(rows.len()).ok());
        let committed_display = committed.and_then(|(id, value)| {
            self.active_drag_preview(id)
                .filter(|preview| !preview.display.is_empty())
                .map(|preview| preview.display.clone())
                .or_else(|| {
                    let row = self.rows()?.iter().find(|row| row.id == id)?;
                    self.cached_step_label(row.id, row.steps, value)
                        .map(str::to_owned)
                        .or_else(|| {
                            is_toggle_parameter(row.steps)
                                .then(|| toggle_label(toggle_checked(value), None).to_owned())
                        })
                })
        });
        if let Some(rows) = &self.result {
            self.result = Some(Arc::new(
                rows.iter()
                    .map(|row| {
                        let mut pending = display_only(row);
                        // Keep the last observed presentation while the source renews.
                        // Capabilities stay revoked; this value never authorizes input
                        // and the disk cache still stores descriptors only.
                        pending.normalized = self.display_value(row);
                        pending.value = self.display_text(row).to_owned();
                        if let Some((id, value)) = committed
                            && row.id == id
                        {
                            pending.normalized = value;
                            if let Some(display) = committed_display.as_ref() {
                                pending.value = display.clone();
                            }
                        }
                        pending
                    })
                    .collect(),
            ));
        }
        self.source = None;
        self.total = total;
        // Retain presentation identity. Source and metadata revocation above
        // prevent these controls from admitting input until a fresh page arrives.
        self.priority_page = None;
        self.queued_touch = None;
        self.live_values.clear();
        self.drag_preview = None;
        self.pending_touch = None;
        self.pending_touch_request = None;
        // The selected parameter is view intent, not edit authority. Keep its
        // id across a canonical refresh; display-only rows cannot enable edits.
        self.touch_unavailable = false;
        self.next_query_at = Instant::now();
    }
}

mod knobs;
mod lifecycle;
mod mapping;
mod query;
mod render;
mod touch;

use knobs::*;
pub(in crate::editor) use lifecycle::tick;
use lifecycle::*;
use mapping::*;
pub(super) use mapping::{
    MappingTarget, has_mapping_target, mapping_label, mapping_target, mapping_target_is_current,
};
use query::*;
pub(in crate::editor) use query::{install_conversion_result, install_result};
pub(super) use render::{body, pending_body};
pub(in crate::editor) use render::{refresh, refresh_after_gesture};
#[cfg(feature = "ui-test-support")]
pub(in crate::editor) use render::{test_state, test_touch_state};
pub(in crate::editor) use touch::touched;
use touch::*;

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

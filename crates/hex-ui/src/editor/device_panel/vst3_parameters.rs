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
    NumericTextRequest, NumericTextRequestId, TextInput,
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
            .any(|knob| {
                knob.control.read_with(cx, |control, _| {
                    control.is_interacting() || control.is_editing()
                })
            })
    }

    fn has_pending_text_edit(&self, cx: &App) -> bool {
        self.knobs
            .values()
            .chain(self.follow_knob.iter().map(|(_, knob)| knob))
            .any(|knob| {
                knob.control
                    .read_with(cx, |control, _| control.is_editing())
            })
    }
}
struct Knob {
    source: hex_lattice_contract::ForeignProcessorBinding,
    output: u64,
    name: String,
    default: f64,
    steps: u32,
    control: Entity<NumericControl>,
    origin: Rc<Cell<PluginParameterOrigin>>,
    drag: Rc<KnobDrag>,
    _subscription: Subscription,
    _text_observation: Subscription,
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
        presentation: (f64, &str),
        enabled: bool,
        active: Option<hex_session::device_gesture::GestureSnapshot>,
        cx: &mut Context<EditorView>,
    ) -> Option<Entity<NumericControl>> {
        let (value, display) = presentation;
        self.origin.set(parameter_origin(query, row)?);
        let value = self.pending_value(active).unwrap_or(value);
        let busy = active.is_some_and(|gesture| !self.owns(gesture.key));
        self.control.update(cx, |control, cx| {
            let _ = control.set_external_value(value, cx);
            if value == row.normalized {
                let _ = control.set_external_display(value, display, cx);
            }
            control.set_availability(ControlAvailability::for_edit(enabled, busy), cx);
        });
        Some(self.control.clone())
    }
}

struct RowPlaceholder {
    name: String,
    default: f64,
    control: Entity<NumericControl>,
}

/// Metadata reads keep the control poll active without claiming edit custody.
pub(in crate::editor) fn query_pending(view: &EditorView) -> bool {
    view.device_panel.vst3_parameters.in_flight.is_some()
        || view.device_panel.vst3_parameters.deferred_result.is_some()
        || view.device_panel.vst3_parameters.conversion.is_some()
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

    fn painted_value(&self, view: &EditorView, device: DeviceId, row: &PluginParameterRow) -> f64 {
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
        // Only the SDK page formats values. Hold its last text while the
        // immediate knob preview is ahead of the controller observation.
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
                            // The plug-in must supply its own formatted text.
                            pending.value = "updating".into();
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

//! View-local panels for exact hosted VST3 controller sources. Persisted rows
//! supply names and order only. Native Perform receipts can advance the knob's
//! display. Fresh controller pages still authorize edits.
use super::*;
use gpui::{Entity, Subscription, UniformListScrollHandle, uniform_list};
use hex_audio::prepared::AppliedAudioSource;
use hex_engine::AudioSourceContext;
use hex_session::device_gesture::{GestureKey, PluginParameterOrigin, PluginParameterValue};
use hex_session::service::{PluginParameterQuery, PluginParameterResult, PluginParameterRow};
use hex_widgets::{
    ControlAvailability, NumericControl, NumericControlConfig, NumericControlEvent,
    NumericControlPresentation, NumericEventPhase, NumericGestureId, NumericScale, NumericSpec,
    TextInput,
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

const PAGE_SIZE: usize = 4;
const PAGE_REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const MAX_AUTOMATIC_RETRIES: u8 = 3;
const REFRESH_YIELDED_TO_CONTROL: &str = "vst3 parameter refresh yielded to control";

mod cache;
mod scrollbar;

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    reconciled: Option<(AudioSourceContext, Option<u64>)>,
    panels: HashMap<DeviceId, Panel>,
    in_flight: Option<(DeviceId, PluginParameterQuery)>,
    deferred_result: Option<(PluginParameterResult, u64)>,
    last_touched_device: Option<DeviceId>,
    reported_status: Option<ReportedStatus>,
}
#[derive(Clone, PartialEq, Eq)]
struct ReportedStatus {
    device: u64,
    message: String,
    error: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TouchObservation {
    source: AppliedAudioSource,
    binding: hex_lattice_contract::ForeignProcessorBinding,
    parameter: u32,
}
#[derive(Clone, Copy)]
struct LiveTouchValue {
    touch: TouchObservation,
    value: PluginParameterValue,
    after_request: u64,
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

    fn renew(
        &self,
        query: PluginParameterQuery,
        row: &PluginParameterRow,
        value: f64,
        enabled: bool,
        active: Option<GestureKey>,
        cx: &mut Context<EditorView>,
    ) -> Option<Entity<NumericControl>> {
        self.origin.set(parameter_origin(query, row)?);
        let busy = active.is_some_and(|key| !self.owns(key));
        self.control.update(cx, |control, cx| {
            let _ = control.set_external_value(value, cx);
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
}

impl Panel {
    fn rows(&self) -> Option<&[PluginParameterRow]> {
        self.result.as_ref().map(|rows| rows.as_slice())
    }

    fn observe_value(&mut self, touch: TouchObservation, value: Option<PluginParameterValue>) {
        // Bound this map by the admitted row inventory. Unknown targets still
        // follow the ordinary controller lookup before displaying a value.
        if let Some(value) = value
            && self.source == Some(touch.binding)
            && self.query.context == touch.source.context
            && self.query.output_identity == touch.source.output_identity
            && self.rows().is_some_and(|rows| {
                rows.iter()
                    .any(|row| row.id == touch.parameter && row.metadata_revision != 0)
            })
        {
            self.live_values.insert(
                touch.parameter,
                LiveTouchValue {
                    touch,
                    value,
                    after_request: self.query.request_id,
                },
            );
        }
    }

    fn display_value(&self, row: &PluginParameterRow) -> f64 {
        self.live_values
            .get(&row.id)
            .filter(|live| {
                self.source == Some(live.touch.binding)
                    && self.query.context == live.touch.source.context
                    && self.query.output_identity == live.touch.source.output_identity
                    && row.metadata_revision != 0
            })
            .map_or(row.normalized, |live| live.value.normalized())
    }

    fn reconcile_live_values(
        &mut self,
        query: PluginParameterQuery,
        source: hex_lattice_contract::ForeignProcessorBinding,
        rows: &[PluginParameterRow],
    ) {
        self.live_values.retain(|&id, live| {
            live.touch.binding == source
                && (query.request_id <= live.after_request || !rows.iter().any(|row| row.id == id))
        });
    }

    fn refilter(&mut self, reset_scroll: bool) {
        let Some(matched) = self.rows().map(|rows| matching_rows(rows, &self.filter)) else {
            return;
        };
        if reset_scroll || *self.matched != matched {
            self.scrollbar.invalidate();
        }
        self.matched = Arc::new(matched);
        if reset_scroll {
            self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        }
    }

    fn mark_values_stale(&mut self) {
        self.scrollbar.invalidate();
        let total = self.rows().and_then(|rows| u16::try_from(rows.len()).ok());
        if let Some(rows) = &self.result {
            self.result = Some(Arc::new(
                rows.iter()
                    .map(|row| {
                        let mut pending = display_only(row);
                        // Keep the last observed presentation while the source renews.
                        // Capabilities stay revoked; this value never authorizes input
                        // and the disk cache still stores descriptors only.
                        pending.normalized = row.normalized;
                        pending.value = row.value.clone();
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

fn display_only(row: &PluginParameterRow) -> PluginParameterRow {
    PluginParameterRow {
        value: String::new(),
        normalized: row.default_normalized,
        editable: false,
        automation_class: None,
        metadata_revision: 0,
        ..row.clone()
    }
}

fn matching_rows(rows: &[PluginParameterRow], filter: &str) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(index, row)| {
            (filter.is_empty() || row.name.to_lowercase().contains(filter)).then_some(index)
        })
        .collect()
}

fn merge_page(
    cached: &[PluginParameterRow],
    cached_signature: Option<[u8; 32]>,
    cached_total: Option<u16>,
    total: u16,
    first: u16,
    live_signature: [u8; 32],
    rows: &[PluginParameterRow],
) -> Option<Vec<PluginParameterRow>> {
    let first = usize::from(first);
    let total_usize = usize::from(total);
    let page_len = total_usize.saturating_sub(first).min(PAGE_SIZE);
    if cached_signature != Some(live_signature)
        || first % PAGE_SIZE != 0
        || total_usize != cached.len()
        || cached_total != Some(total)
        || rows.len() != page_len
        || cached
            .get(first..first.saturating_add(rows.len()))
            .is_none_or(|prior| prior.iter().zip(rows).any(|(old, new)| old.id != new.id))
    {
        return None;
    }
    let mut merged = cached.to_vec();
    for (offset, row) in rows.iter().cloned().enumerate() {
        merged[first + offset] = row;
    }
    Some(merged)
}

fn refresh_page(
    has_rows: bool,
    source_validated: bool,
    priority_page: Option<u16>,
    visible_pages: &BTreeSet<u16>,
    next_page: u16,
    rows_empty: bool,
) -> Option<Option<u16>> {
    if !has_rows {
        return Some(None);
    }
    if let Some(page) = priority_page {
        return Some(Some(page));
    }
    if !source_validated {
        return Some(Some(0));
    }
    if visible_pages.is_empty() {
        return rows_empty.then_some(Some(0));
    }
    visible_pages
        .range(next_page..)
        .next()
        .or_else(|| visible_pages.first())
        .copied()
        .map(Some)
}

fn is_refresh_yield(error: &str) -> bool {
    error == REFRESH_YIELDED_TO_CONTROL
}

fn current_context(view: &EditorView) -> Option<AudioSourceContext> {
    Some(AudioSourceContext {
        project_generation: view.project_generation,
        editor_revision: view.state.as_ref()?.revision,
    })
}

fn current_applied_source(view: &EditorView) -> Option<AppliedAudioSource> {
    view.audio
        .as_ref()
        .and_then(|audio| audio.status())
        .and_then(|status| status.native_effect_source())
}

fn touch_is_current(view: &EditorView, device: DeviceId, touch: TouchObservation) -> bool {
    if !touch.binding.is_valid() {
        return false;
    }
    let Some(panel) = view.device_panel.vst3_parameters.panels.get(&device) else {
        return false;
    };
    let Some(context) = current_context(view) else {
        return false;
    };
    touch.source.context.project_generation == context.project_generation
        && touch.source.context.editor_revision >= context.editor_revision
        && view
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            == Some(touch.source.output_identity)
        && current_applied_source(view) == Some(touch.source)
        && panel.query.context.project_generation == touch.source.context.project_generation
        && panel.query.output_identity == touch.source.output_identity
        && view
            .state
            .as_ref()
            .is_some_and(|state| super::chain_ids(&state.project, panel.owner).contains(&device))
}

pub(in crate::editor) fn touched(
    view: &mut EditorView,
    source: AppliedAudioSource,
    binding: hex_lattice_contract::ForeignProcessorBinding,
    device_id: u64,
    parameter: u32,
    value: Option<PluginParameterValue>,
) {
    let Some(device) = view
        .device_panel
        .vst3_parameters
        .panels
        .keys()
        .copied()
        .find(|device| device.value() == device_id)
    else {
        return;
    };
    let touch = TouchObservation {
        source,
        binding,
        parameter,
    };
    if !touch_is_current(view, device, touch) {
        return;
    }
    view.device_panel.vst3_parameters.last_touched_device = Some(device);
    if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
        panel.observe_value(touch, value);
        queue_native_touch(panel, touch);
    }
}

// Called only after the current applied source and binding have been checked.
// Repeated Perform notifications are refresh hints, not new touch identities.
fn queue_native_touch(panel: &mut Panel, touch: TouchObservation) {
    if panel.queued_touch == Some(touch)
        || panel.queued_touch.is_none() && panel.pending_touch == Some(touch)
    {
        // An earlier request can return a useful intermediate value while the
        // pointer is still moving. Do not orphan that request on every Perform.
        return;
    }
    let page = (panel.query.context == touch.source.context
        && panel.source == Some(touch.binding)
        && panel.last_touched == Some(touch.parameter))
    .then(|| {
        panel.rows()?.iter().find_map(|row| {
            (row.id == touch.parameter && row.metadata_revision != 0).then_some(row.page_first)
        })
    })
    .flatten();
    if let Some(page) = page {
        panel.queued_touch = None;
        panel.pending_touch = None;
        panel.pending_touch_request = None;
        // Prioritize the known target at the next scheduled read. Repeated
        // Perform callbacks must not bypass the refresh cadence.
        panel.priority_page = Some(page);
    } else {
        panel.queued_touch = Some(touch);
        panel.pending_touch_request = None;
    }
}

fn queue_host_touch(
    view: &mut EditorView,
    device: DeviceId,
    parameter: u32,
    cx: &mut Context<EditorView>,
) {
    let Some(source) = current_applied_source(view) else {
        return;
    };
    let Some(binding) = view
        .device_panel
        .vst3_parameters
        .panels
        .get(&device)
        .and_then(|panel| panel.source)
    else {
        return;
    };
    let touch = TouchObservation {
        source,
        binding,
        parameter,
    };
    if !touch_is_current(view, device, touch) {
        return;
    }
    let preview = view
        .device_panel
        .vst3_parameters
        .panels
        .get_mut(&device)
        .and_then(|panel| {
            let row = panel
                .rows()
                .and_then(|rows| rows.iter().find(|row| row.id == parameter))?
                .clone();
            if row.metadata_revision == 0 {
                return None;
            }
            panel.last_touched = Some(parameter);
            panel.touch_unavailable = !row.editable;
            panel.pending_touch = None;
            panel.queued_touch = None;
            panel.pending_touch_request = None;
            Some((panel.owner, row))
        });
    if let Some((owner, row)) = preview {
        view.device_panel.vst3_parameters.last_touched_device = Some(device);
        view.remember_touched_vst3(owner, device, &row, cx);
    }
}

fn apply_touch(
    view: &mut EditorView,
    device: DeviceId,
    touch: TouchObservation,
    cx: &mut Context<EditorView>,
) -> bool {
    if !touch_is_current(view, device, touch) {
        if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
            if panel.queued_touch == Some(touch) {
                panel.queued_touch = None;
            }
            if panel.pending_touch == Some(touch) {
                panel.pending_touch = None;
            }
        }
        return false;
    }
    if current_context(view) != Some(touch.source.context) {
        if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
            panel.queued_touch = None;
            panel.pending_touch = Some(touch);
            panel.pending_touch_request = None;
        }
        return false;
    }
    let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) else {
        return false;
    };
    let row = panel
        .rows()
        .and_then(|rows| rows.iter().find(|row| row.id == touch.parameter))
        .cloned();
    queue_touch_refresh(panel, touch, row.as_ref());
    cx.notify();
    false
}

fn queue_touch_refresh(
    panel: &mut Panel,
    touch: TouchObservation,
    row: Option<&PluginParameterRow>,
) {
    let page = row.map(|row| row.page_first);
    // Keep the last validated presentation until the new target resolves.
    // A pending touch suspends follow-control input without erasing its label.
    panel.touch_unavailable = false;
    panel.queued_touch = None;
    panel.pending_touch = Some(touch);
    panel.pending_touch_request = None;
    if panel.source != Some(touch.binding) {
        panel.mark_values_stale();
        panel.pending_touch = Some(touch);
        panel.priority_page = page;
    } else if let Some(page) = page {
        panel.priority_page = Some(page);
    } else {
        // Unknown IDs require a full inventory query, not removal of the
        // current inventory. A plug-in can report non-exposed edit targets.
        panel.priority_page = None;
    }
    panel.next_query_at = Instant::now();
}

fn fresh_touch_row(
    touch: TouchObservation,
    source: hex_lattice_contract::ForeignProcessorBinding,
    rows: &[PluginParameterRow],
    page_first: Option<u16>,
) -> Option<PluginParameterRow> {
    if touch.binding != source {
        return None;
    }
    rows.iter()
        .find(|row| {
            row.id == touch.parameter
                && row.metadata_revision != 0
                && page_first.is_none_or(|first| row.page_first == first)
        })
        .cloned()
}

fn resolve_pending_touch(
    panel: &mut Panel,
    source: hex_lattice_contract::ForeignProcessorBinding,
    fresh_rows: &[PluginParameterRow],
    page_first: Option<u16>,
    is_touch_query: bool,
) -> Option<(super::DevicePanelOwner, PluginParameterRow)> {
    if !is_touch_query {
        return None;
    }
    let touch = panel.pending_touch?;
    panel.pending_touch_request = None;
    if touch.binding != source {
        panel.pending_touch = None;
        panel.last_touched = None;
        panel.touch_unavailable = false;
        return None;
    }
    if let Some(row) = fresh_touch_row(touch, source, fresh_rows, page_first) {
        panel.last_touched = Some(touch.parameter);
        panel.touch_unavailable = !row.editable;
        panel.pending_touch = None;
        return Some((panel.owner, row));
    }
    panel.last_touched = Some(touch.parameter);
    panel.touch_unavailable = true;
    panel.pending_touch = None;
    None
}

fn parameter_origin(
    query: PluginParameterQuery,
    row: &PluginParameterRow,
) -> Option<PluginParameterOrigin> {
    Some(PluginParameterOrigin {
        context: query.context,
        device: query.device,
        parameter: row.id,
        page_first: row.page_first,
        metadata_revision: row.metadata_revision,
        initial: PluginParameterValue::new(row.normalized).ok()?,
    })
}

fn cache_location(view: &EditorView, device: DeviceId) -> Option<(String, PathBuf)> {
    let state = view.state.as_ref()?.project.device(device)?.state();
    let key = hex_session::service::vst3_parameter_cache_key(state)?;
    let home = std::env::var_os("HOME")?;
    let root = hex_preferences::browser_places_path(std::path::Path::new(&home))
        .with_file_name("vst3-parameters");
    Some((key.clone(), root.join(format!("{key}.json"))))
}

fn ensure_panel(view: &mut EditorView, device: DeviceId, cx: &mut Context<EditorView>) {
    if view.device_gesture.snapshot().is_some() {
        return;
    }
    let Some(owner) = view.device_panel.owner else {
        return;
    };
    let Some(context) = current_context(view) else {
        return;
    };
    let output_identity = view
        .audio
        .as_ref()
        .and_then(|audio| audio.control.output_identity())
        .unwrap_or(0);
    let (key, path) = cache_location(view, device).unwrap_or_default();
    let cache_enabled = !key.is_empty();
    if view
        .device_panel
        .vst3_parameters
        .panels
        .contains_key(&device)
    {
        let same = view
            .device_panel
            .vst3_parameters
            .panels
            .get(&device)
            .is_some_and(|panel| panel.owner == owner && panel.key == key);
        if same {
            let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) else {
                return;
            };
            if panel.query.context.project_generation != context.project_generation
                || panel.query.context.editor_revision != context.editor_revision
                || panel.query.output_identity != output_identity
            {
                panel.query.context = context;
                panel.query.output_identity = output_identity;
                panel.mark_values_stale();
            }
            return;
        }
        view.device_panel.vst3_parameters.panels.remove(&device);
    }
    let query = PluginParameterQuery {
        context,
        device,
        output_identity,
        request_id: 0,
        page_first: None,
    };
    let search = cx.new(super::super::browser_input::plugin_parameter_search);
    let placeholder_knob = placeholder_knob(device, cx);
    let search_id = search.entity_id();
    let observe_search = cx.observe(&search, move |view, _, cx| {
        if let Some(panel) = view
            .device_panel
            .vst3_parameters
            .panels
            .values_mut()
            .find(|panel| panel.search.entity_id() == search_id)
        {
            panel.filter = panel.search.read(cx).text().trim().to_lowercase();
            panel.refilter(true);
        }
        cx.notify();
    });
    view.device_panel.vst3_parameters.panels.insert(
        device,
        Panel {
            owner,
            key: key.clone(),
            cache_path: path.clone(),
            cache_loading: cache_enabled,
            cache_write_pending: false,
            cache_needs_save: false,
            cache_error: None,
            query,
            source: None,
            descriptor_signature: None,
            total: None,
            result: None,
            matched: Arc::default(),
            filter: String::new(),
            search,
            _observe_search: observe_search,
            scroll: UniformListScrollHandle::new(),
            scrollbar: scrollbar::State::default(),
            visible_pages: BTreeSet::new(),
            priority_page: None,
            queued_touch: None,
            live_values: HashMap::new(),
            pending_touch: None,
            pending_touch_request: None,
            last_touched: None,
            touch_unavailable: false,
            follow_knob: None,
            placeholder_knob,
            next_page: 0,
            knobs: HashMap::new(),
            row_placeholders: HashMap::new(),
            next_query_at: Instant::now(),
            retry_count: 0,
        },
    );
    if !cache_enabled {
        return;
    }
    let weak = cx.entity().downgrade();
    let load_key = key.clone();
    let load_path = path.clone();
    let completion_key = key;
    cx.spawn(async move |_, cx| {
        let cache = cx
            .background_executor()
            .spawn(async move { cache::load(&load_path, &load_key) })
            .await;
        let _ = weak.update(cx, |view, cx| {
            let Some(panel) = view
                .device_panel
                .vst3_parameters
                .panels
                .get_mut(&device)
                .filter(|panel| {
                    panel.key == completion_key
                        && panel.query.context.project_generation
                            == query.context.project_generation
                })
            else {
                return;
            };
            panel.cache_loading = false;
            match cache {
                Ok(Some(cached)) => {
                    panel.total = u16::try_from(cached.rows.len()).ok();
                    panel.descriptor_signature = Some(cached.descriptor_signature);
                    panel.result = Some(Arc::new(cached.rows));
                    panel.refilter(false);
                }
                Ok(None) => {}
                Err(error) => panel.cache_error = Some(error),
            }
            panel.next_query_at = Instant::now();
            cx.notify();
        });
    })
    .detach();
}

pub(in crate::editor) fn install_result(
    view: &mut EditorView,
    result: PluginParameterResult,
    revision: u64,
) {
    let Some((_, pending)) = view.device_panel.vst3_parameters.in_flight else {
        return;
    };
    if pending != result.query {
        return;
    }
    view.device_panel.vst3_parameters.deferred_result = Some((result, revision));
}

fn apply_result(
    view: &mut EditorView,
    result: PluginParameterResult,
    revision: u64,
    cx: &mut Context<EditorView>,
) {
    let Some((device, pending)) = view.device_panel.vst3_parameters.in_flight else {
        return;
    };
    if pending != result.query {
        return;
    }
    if interactions_active(view, cx) {
        view.device_panel.vst3_parameters.deferred_result = Some((result, revision));
        return;
    }
    view.device_panel.vst3_parameters.in_flight = None;
    let context = current_context(view);
    let output = view
        .audio
        .as_ref()
        .and_then(|audio| audio.control.output_identity());
    if result.query.context.project_generation != view.project_generation
        || result.query.context.editor_revision != revision
        || Some(result.query.context) != context
        || output != Some(result.query.output_identity)
    {
        return;
    }
    let Some(panel) = view
        .device_panel
        .vst3_parameters
        .panels
        .get_mut(&device)
        .filter(|panel| panel.query.device == result.query.device)
    else {
        return;
    };
    let touch_query = panel.pending_touch_request == Some(result.query.request_id);
    panel.next_query_at = Instant::now() + PAGE_REFRESH_INTERVAL;
    let rows = match result.rows {
        Ok(rows) => rows,
        Err(error) => {
            record_refresh_failure(panel, error);
            return;
        }
    };
    let Some(source) = result.source else {
        record_error(panel, "vst3 parameter source changed".into());
        panel.mark_values_stale();
        return;
    };
    panel.reconcile_live_values(result.query, source, &rows);
    if panel.descriptor_signature != result.descriptor_signature {
        panel.live_values.clear();
    }
    let Some(descriptor_signature) = result.descriptor_signature else {
        panel.result = None;
        panel.source = None;
        panel.descriptor_signature = None;
        panel.total = None;
        panel.knobs.clear();
        record_error(
            panel,
            "vst3 parameter descriptor signature unavailable".into(),
        );
        return;
    };
    if panel.source.is_some_and(|current| current != source) {
        let pending_touch = panel
            .pending_touch
            .or(panel.queued_touch)
            .filter(|touch| touch.binding == source);
        let priority_page = pending_touch.and_then(|touch| {
            panel
                .rows()
                .and_then(|rows| rows.iter().find(|row| row.id == touch.parameter))
                .map(|row| row.page_first)
        });
        panel.cache_error = None;
        panel.mark_values_stale();
        panel.pending_touch = pending_touch;
        panel.priority_page = priority_page;
        panel.next_query_at = Instant::now();
        return;
    }
    if result.query.page_first.is_none() {
        if usize::from(result.total) != rows.len() {
            panel.result = None;
            panel.source = None;
            panel.descriptor_signature = None;
            panel.total = None;
            panel.knobs.clear();
            record_error(panel, "vst3 parameter inventory differs".into());
            return;
        }
        panel.scrollbar.invalidate();
        panel.source = Some(source);
        panel.descriptor_signature = Some(descriptor_signature);
        panel.total = Some(result.total);
        panel.cache_error = None;
        panel.retry_count = 0;
        panel
            .knobs
            .retain(|id, _| rows.iter().any(|row| row.id == *id));
        panel
            .row_placeholders
            .retain(|id, _| rows.iter().any(|row| row.id == *id));
        if panel
            .follow_knob
            .as_ref()
            .is_some_and(|(id, _)| !rows.iter().any(|row| row.id == *id))
        {
            panel.follow_knob = None;
        }
        if panel.pending_touch.is_none() || touch_query {
            panel.priority_page = None;
        }
        if panel
            .pending_touch
            .is_some_and(|touch| touch.binding != source)
        {
            panel.pending_touch = None;
            panel.last_touched = None;
            panel.touch_unavailable = false;
        }
        if panel
            .queued_touch
            .is_some_and(|touch| touch.binding != source)
        {
            panel.queued_touch = None;
        }
        let preview = panel.last_touched.and_then(|id| {
            rows.iter()
                .find(|row| row.id == id && row.automation_class.is_some())
                .cloned()
                .map(|row| (panel.owner, row))
        });
        let touched_preview = resolve_pending_touch(panel, source, &rows, None, touch_query);
        panel.result = Some(Arc::new(rows));
        panel.refilter(false);
        panel.cache_needs_save = true;
        if let Some((owner, row)) = touched_preview.or(preview) {
            view.remember_touched_vst3(owner, device, &row, cx);
        }
        return;
    }
    let Some(cached) = panel.rows().map(|rows| rows.to_vec()) else {
        record_error(panel, "vst3 parameter cache is unavailable".into());
        panel.mark_values_stale();
        return;
    };
    let Some(first) = result.query.page_first else {
        return;
    };
    let Some(merged) = merge_page(
        &cached,
        panel.descriptor_signature,
        panel.total,
        result.total,
        first,
        descriptor_signature,
        &rows,
    ) else {
        panel.cache_error = None;
        panel.result = None;
        panel.source = None;
        panel.descriptor_signature = None;
        panel.total = None;
        panel.knobs.clear();
        panel.follow_knob = None;
        panel.priority_page = None;
        panel.pending_touch_request = None;
        panel.next_query_at = Instant::now();
        return;
    };
    for (offset, row) in rows.iter().enumerate() {
        let index = usize::from(first) + offset;
        if &cached[index] != row
            && let Some(origin) = parameter_origin(result.query, row)
        {
            let value = panel.display_value(row);
            if let Some(knob) = panel.knobs.get(&row.id) {
                knob.origin.set(origin);
                knob.control.update(cx, |control, cx| {
                    let _ = control.set_external_value(value, cx);
                });
            }
            if let Some((id, knob)) = panel.follow_knob.as_ref()
                && *id == row.id
            {
                knob.origin.set(origin);
                knob.control.update(cx, |control, cx| {
                    let _ = control.set_external_value(value, cx);
                });
            }
        }
    }
    let preview = panel.last_touched.and_then(|id| {
        merged
            .iter()
            .find(|row| row.id == id && row.automation_class.is_some())
            .cloned()
            .map(|row| (panel.owner, row))
    });
    if panel.priority_page == Some(first) && (panel.pending_touch.is_none() || touch_query) {
        panel.priority_page = None;
    }
    if panel
        .pending_touch
        .is_some_and(|touch| touch.binding != source)
    {
        panel.pending_touch = None;
        panel.last_touched = None;
        panel.touch_unavailable = false;
    }
    if panel
        .queued_touch
        .is_some_and(|touch| touch.binding != source)
    {
        panel.queued_touch = None;
    }
    panel.source = Some(source);
    panel.descriptor_signature = Some(descriptor_signature);
    panel.result = Some(Arc::new(merged));
    panel.cache_error = None;
    panel.retry_count = 0;
    panel.refilter(false);
    let touched_preview = resolve_pending_touch(panel, source, &rows, Some(first), touch_query);
    if let Some((owner, row)) = touched_preview.or(preview) {
        view.remember_touched_vst3(owner, device, &row, cx);
    }
}

// Called on the editor poll cadence. One query is outstanding globally because
// the native controller mailbox is shared by all hosted devices.
fn interactions_active(view: &EditorView, cx: &Context<EditorView>) -> bool {
    view.device_gesture.snapshot().is_some()
        || view
            .device_panel
            .vst3_parameters
            .panels
            .values()
            .any(|panel| {
                panel
                    .knobs
                    .values()
                    .any(|knob| knob.control.read(cx).is_interacting())
                    || panel
                        .follow_knob
                        .as_ref()
                        .is_some_and(|(_, knob)| knob.control.read(cx).is_interacting())
            })
}

fn record_refresh_failure(panel: &mut Panel, error: String) {
    if is_refresh_yield(&error) {
        panel.cache_error = None;
        panel.retry_count = 0;
        panel.next_query_at = Instant::now();
    } else if error != "vst3 controller is busy · try again" {
        record_error(panel, error);
    }
    // A controller operation owns the mailbox briefly. Keep the last
    // presentation and retry at the normal page cadence.
}

fn record_error(panel: &mut Panel, error: String) {
    panel.retry_count = panel.retry_count.saturating_add(1);
    panel.cache_error = Some(error);
    let exponent = u32::from(panel.retry_count.saturating_sub(1).min(5));
    panel.next_query_at = Instant::now() + Duration::from_secs(1_u64 << exponent);
}

fn report_panel_state(view: &mut EditorView) -> bool {
    let active_owner = view
        .device_panel_active
        .then_some(view.device_panel.owner)
        .flatten();
    let panels = active_owner
        .and_then(|owner| {
            let state = view.state.as_ref()?;
            Some(
                super::chain_ids(&state.project, owner)
                    .into_iter()
                    .filter_map(|device| {
                        let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
                        (panel.owner == owner).then_some((device, panel))
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or_default();

    // Surface a real failure from any visible card before informational state.
    let error_state = panels.iter().find_map(|(device, panel)| {
        panel.cache_error.as_ref().map(|error| {
            (
                device.value(),
                format!(
                    "vst3 parameter refresh failed · {}",
                    hex_session::service::text::verbatim(error)
                ),
                Some(error.clone()),
            )
        })
    });
    let next = error_state.or_else(|| {
        let last_device = view.device_panel.vst3_parameters.last_touched_device;
        if let Some(device) = last_device {
            let Some((device, panel)) = panels.iter().find(|(candidate, _)| *candidate == device)
            else {
                // A touch can belong to another retained card or be between
                // native observation and fresh metadata; keep the last report.
                return None;
            };
            if panel.pending_touch.is_some() || panel.queued_touch.is_some() {
                return None;
            }
            if panel.touch_unavailable {
                return Some((
                    device.value(),
                    "last touched vst3 parameter unavailable".into(),
                    None,
                ));
            }
            let row = panel.last_touched.and_then(|id| {
                panel
                    .rows()
                    .and_then(|rows| rows.iter().find(|row| row.id == id))
            })?;
            if row.metadata_revision == 0 || panel.source.is_none() {
                return None;
            }
            return Some((
                device.value(),
                if row.editable {
                    "vst3 parameter touched".into()
                } else {
                    "last touched vst3 parameter is read only".into()
                },
                None,
            ));
        }

        // Only say there has been no touch when no card has ever supplied a
        // validated touch in this panel set.
        panels
            .first()
            .map(|(device, _)| (device.value(), "no vst parameter touched".into(), None))
    });
    let previous = view.device_panel.vst3_parameters.reported_status.clone();
    if previous.as_ref().is_some_and(|previous| {
        next.as_ref().is_some_and(|(device, message, error)| {
            previous.device == *device && previous.message == *message && previous.error == *error
        })
    }) || (previous.is_none() && next.is_none())
    {
        return false;
    }
    if let Some((device, message, error)) = next {
        let previous_error = previous
            .as_ref()
            .and_then(|previous| previous.error.as_deref());
        view.device_panel.vst3_parameters.reported_status = Some(ReportedStatus {
            device,
            message: message.clone(),
            error: error.clone(),
        });
        view.status = message;
        if let Some(error) = error {
            if view.error.is_none()
                || previous_error.is_some_and(|previous| view.error.as_deref() == Some(previous))
            {
                view.error = Some(error.into());
            }
        } else if previous_error.is_some_and(|error| view.error.as_deref() == Some(error)) {
            view.error = None;
        }
        true
    } else if active_owner.is_none()
        || previous.as_ref().is_some_and(|previous| {
            !panels
                .iter()
                .any(|(device, _)| device.value() == previous.device)
        })
        || previous.as_ref().is_some_and(|previous| {
            view.device_panel
                .vst3_parameters
                .last_touched_device
                .is_some_and(|device| device.value() == previous.device)
                && view.audio.as_ref().is_some_and(|audio| {
                    audio.control.output_identity().is_none()
                        && audio
                            .status()
                            .and_then(|status| status.native_effect_source())
                            .is_none()
                })
        })
    {
        let Some(previous) = previous else {
            return false;
        };
        view.device_panel.vst3_parameters.reported_status = None;
        if view.status == previous.message {
            view.status.clear();
        }
        if previous
            .error
            .as_deref()
            .is_some_and(|error| view.error.as_deref() == Some(error))
        {
            view.error = None;
        }
        true
    } else {
        false
    }
}

pub(in crate::editor) fn tick(view: &mut EditorView, cx: &mut Context<EditorView>) {
    reconcile_panels(view, cx);
    if report_panel_state(view) {
        cx.notify();
    }
    let generation = view.project_generation;
    if let Some(device) = view
        .device_panel
        .vst3_parameters
        .panels
        .iter()
        .find_map(|(device, panel)| panel.cache_needs_save.then_some(*device))
        && let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device)
    {
        panel.cache_needs_save = false;
        save_cache(panel, device, generation, cx);
        return;
    }
    if view.stopped_playback_restore.is_some() || interactions_active(view, cx) {
        return;
    }
    if let Some((result, revision)) = view.device_panel.vst3_parameters.deferred_result.take() {
        apply_result(view, result, revision, cx);
        cx.notify();
        return;
    }
    let queued = view
        .device_panel
        .vst3_parameters
        .panels
        .iter_mut()
        .find_map(|(device, panel)| panel.queued_touch.take().map(|touch| (*device, touch)));
    if let Some((device, touch)) = queued
        && apply_touch(view, device, touch, cx)
    {
        return;
    }
    if view.device_panel.vst3_parameters.in_flight.is_some() {
        return;
    }
    let now = Instant::now();
    let pending_touch =
        view.device_panel
            .vst3_parameters
            .panels
            .iter()
            .find_map(|(device, panel)| {
                (panel
                    .pending_touch
                    .is_some_and(|touch| current_context(view) == Some(touch.source.context))
                    && !panel.cache_loading
                    && panel.retry_count <= MAX_AUTOMATIC_RETRIES
                    && now >= panel.next_query_at)
                    .then_some((*device, panel.priority_page))
            });
    if let Some((device, page)) = pending_touch {
        submit_query(view, device, page, cx);
        return;
    }
    let Some(owner) = view.device_panel.owner.filter(|_| view.device_panel_active) else {
        return;
    };
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let active = super::chain_ids(&state.project, owner);
    let next = active.into_iter().find_map(|device| {
        let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
        if panel.owner != owner || panel.cache_loading || now < panel.next_query_at {
            return None;
        }
        if panel.retry_count > MAX_AUTOMATIC_RETRIES {
            return None;
        }
        let mut visible_pages = panel.visible_pages.clone();
        if let Some(id) = panel.last_touched
            && let Some(row) = panel
                .rows()
                .and_then(|rows| rows.iter().find(|row| row.id == id))
        {
            visible_pages.insert(row.page_first);
        }
        let page = refresh_page(
            panel.rows().is_some(),
            panel.source.is_some(),
            panel.priority_page,
            &visible_pages,
            panel.next_page,
            panel.rows().is_some_and(<[_]>::is_empty),
        )?;
        Some((device, page))
    });
    if let Some((device, page)) = next {
        if let Some(first) = page
            && let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device)
        {
            panel.next_page = first.saturating_add(PAGE_SIZE as u16);
        }
        submit_query(view, device, page, cx);
    }
}

fn reconcile_panels(view: &mut EditorView, cx: &Context<EditorView>) {
    let Some(context) = current_context(view) else {
        return;
    };
    let output_identity = view
        .audio
        .as_ref()
        .and_then(|audio| audio.control.output_identity());
    if view.device_panel.vst3_parameters.reconciled == Some((context, output_identity)) {
        return;
    }
    let stale = view
        .device_panel
        .vst3_parameters
        .panels
        .iter()
        .filter_map(|(device, panel)| {
            let state = view.state.as_ref()?;
            let present = state.project.device(*device).is_some()
                && super::chain_ids(&state.project, panel.owner).contains(device);
            let key = cache_location(view, *device)
                .map(|(key, _)| key)
                .unwrap_or_default();
            (!present
                || panel.query.context.project_generation != context.project_generation
                || key != panel.key)
                .then_some(*device)
        })
        .collect::<Vec<_>>();
    for device in &stale {
        view.device_panel.vst3_parameters.panels.remove(device);
    }
    if view
        .device_panel
        .vst3_parameters
        .last_touched_device
        .is_some_and(|device| stale.contains(&device))
    {
        view.device_panel.vst3_parameters.last_touched_device = None;
    }
    if interactions_active(view, cx) {
        return;
    }
    for panel in view.device_panel.vst3_parameters.panels.values_mut() {
        if panel.query.context != context || output_identity != Some(panel.query.output_identity) {
            let pending_touch = panel.pending_touch.or(panel.queued_touch).filter(|touch| {
                touch.source.context.project_generation == context.project_generation
                    && touch.source.context.editor_revision >= context.editor_revision
                    && Some(touch.source.output_identity) == output_identity
            });
            let touch_page = pending_touch.and_then(|touch| {
                panel
                    .rows()
                    .and_then(|rows| rows.iter().find(|row| row.id == touch.parameter))
                    .map(|row| row.page_first)
            });
            panel.query.context = context;
            if let Some(output_identity) = output_identity {
                panel.query.output_identity = output_identity;
            }
            panel.mark_values_stale();
            panel.pending_touch = pending_touch;
            panel.priority_page = touch_page;
        }
    }
    view.device_panel.vst3_parameters.reconciled = Some((context, output_identity));
}

fn submit_query(
    view: &mut EditorView,
    device: DeviceId,
    page_first: Option<u16>,
    cx: &mut Context<EditorView>,
) {
    let Some(context) = current_context(view) else {
        return;
    };
    let Some(output_identity) = view
        .audio
        .as_ref()
        .and_then(|audio| audio.control.output_identity())
    else {
        return;
    };
    let Some(request_id) = view.device_panel.vst3_parameters.serial.checked_add(1) else {
        return;
    };
    if !view
        .device_panel
        .vst3_parameters
        .panels
        .contains_key(&device)
    {
        return;
    }
    let query = PluginParameterQuery {
        context,
        device,
        output_identity,
        request_id,
        page_first,
    };
    if !view.enqueue(Request::QueryPluginParameters(query)) {
        return;
    }
    view.device_panel.vst3_parameters.serial = request_id;
    view.device_panel.vst3_parameters.in_flight = Some((device, query));
    let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) else {
        return;
    };
    panel.query = query;
    if panel.pending_touch.is_some() && panel.priority_page == page_first {
        panel.pending_touch_request = Some(request_id);
    }
    panel.next_query_at = Instant::now() + PAGE_REFRESH_INTERVAL;
    panel.cache_error = None;
    cx.notify();
}

fn save_cache(panel: &mut Panel, device: DeviceId, generation: u64, cx: &mut Context<EditorView>) {
    if panel.cache_write_pending || panel.key.is_empty() {
        return;
    }
    let Some(rows) = panel.rows().map(|rows| rows.to_vec()) else {
        return;
    };
    let Some(descriptor_signature) = panel.descriptor_signature else {
        return;
    };
    let key = panel.key.clone();
    let completion_key = key.clone();
    let path = panel.cache_path.clone();
    panel.cache_write_pending = true;
    let weak = cx.entity().downgrade();
    cx.spawn(async move |_, cx| {
        let result = cx
            .background_executor()
            .spawn(async move { cache::save(&path, &key, &descriptor_signature, &rows) })
            .await;
        let _ = weak.update(cx, |view, cx| {
            if view.project_generation == generation
                && let Some(panel) = view
                    .device_panel
                    .vst3_parameters
                    .panels
                    .get_mut(&device)
                    .filter(|panel| panel.key == completion_key)
            {
                panel.cache_write_pending = false;
                if let Err(error) = result {
                    panel.cache_error = Some(error);
                }
            }
            cx.notify();
        });
    })
    .detach();
}

pub(super) fn body(view: &mut EditorView, device: DeviceId, cx: &mut Context<EditorView>) -> Div {
    ensure_panel(view, device, cx);
    let body = crate::device_composition::body()
        .w(px(crate::device_composition::GENERIC_FRAME_WIDTH))
        .flex_col()
        .min_w_0();
    let restoring = view.stopped_playback_restore.is_some();
    if !view
        .device_panel
        .vst3_parameters
        .panels
        .contains_key(&device)
    {
        return body.child(
            div()
                .h(px(56.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .w_full()
                .child(div().flex_1().min_w_0().h(px(32.0)))
                .child(
                    div()
                        .w(px(80.0 + 8.0 + hex_widgets::MICRO_DIAL_WIDTH))
                        .h(px(40.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(80.0)).h(px(40.0)))
                        .child(
                            div()
                                .w(px(hex_widgets::MICRO_DIAL_WIDTH))
                                .h(px(hex_widgets::MICRO_DIAL_HEIGHT)),
                        ),
                ),
        );
    }
    let (follow, follow_row, _) = if restoring {
        (None, None, None)
    } else {
        follow_display(view, device, cx)
    };
    let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) else {
        return body.h(px(56.0)).child(div().h(px(56.0)).flex_shrink_0());
    };
    let row = follow_row.as_ref();
    if follow.is_none()
        && let (Some(knob), Some(row)) = (&panel.placeholder_knob, row)
    {
        let value = row.normalized;
        knob.update(cx, |control, cx| {
            let _ = control.set_external_value(value, cx);
        });
    }
    let name = row.map(|row| row.name.as_str());
    let value = row.map(|row| row.value.as_str()).unwrap_or("");
    let retry = panel.cache_error.is_some() || panel.retry_count > MAX_AUTOMATIC_RETRIES;
    let field = if retry {
        hex_widgets::action(theme::text("retry"), theme::action_style())
            .id(("vst3-parameter-retry", device.value()))
            .debug_selector(|| "vst3-parameter-retry".to_string())
            .w(px(80.0))
            .h(px(20.0))
            .on_click(cx.listener(move |view, _, _, cx| {
                if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
                    panel.retry_count = 0;
                    panel.cache_error = None;
                    panel.next_query_at = Instant::now();
                }
                cx.notify();
            }))
            .into_any_element()
    } else {
        div()
            .debug_selector(|| "vst3-last-touch-value".to_string())
            .w(px(80.0))
            .h(px(20.0))
            .min_w_0()
            .truncate()
            .text_right()
            .child(theme::text(hex_session::service::text::verbatim(value)))
            .into_any_element()
    };
    let details = div()
        .w(px(80.0))
        .h(px(40.0))
        .min_w_0()
        .flex_col()
        .justify_center()
        .gap_0()
        .child(
            div()
                .w(px(80.0))
                .h(px(20.0))
                .min_w_0()
                .truncate()
                .when_some(name, |label, name| {
                    label
                        .debug_selector(|| "vst3-last-touch-label".to_string())
                        .child(theme::text(hex_session::service::text::verbatim(name)))
                }),
        )
        .child(field);
    let control = follow.or_else(|| panel.placeholder_knob.clone());
    let control = control.map_or_else(
        || {
            div()
                .w(px(hex_widgets::MICRO_DIAL_WIDTH))
                .h(px(hex_widgets::MICRO_DIAL_HEIGHT))
                .flex_shrink_0()
        },
        |control| {
            div()
                .debug_selector(|| "vst3-last-touched-knob".to_string())
                .w(px(hex_widgets::MICRO_DIAL_WIDTH))
                .h(px(hex_widgets::MICRO_DIAL_HEIGHT))
                .flex_shrink_0()
                .child(control)
        },
    );
    let controls = div()
        .debug_selector(|| "vst3-last-touch-row".to_string())
        .w(px(80.0 + 8.0 + hex_widgets::MICRO_DIAL_WIDTH))
        .h(px(hex_widgets::MICRO_DIAL_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .child(details)
        .child(control);
    let toolbar = div()
        .debug_selector(|| "vst3-search-row".to_string())
        .h(px(56.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .w_full()
        .child(
            div()
                .debug_selector(|| "vst3-parameter-search-input".to_string())
                .flex_1()
                .min_w_0()
                .child(panel.search.clone()),
        )
        .child(controls);
    let body = body.child(toolbar);
    if restoring {
        return body.child(
            div()
                .debug_selector(|| "vst3-parameter-list-area".to_string())
                .flex_1()
                .min_h_0()
                .w_full(),
        );
    }
    match &panel.result {
        None => body.child(
            div()
                .debug_selector(|| "vst3-parameter-list-area".to_string())
                .flex_1()
                .min_h_0()
                .w_full(),
        ),
        Some(rows) if rows.is_empty() => body.child(
            div()
                .debug_selector(|| "vst3-parameter-list-area".to_string())
                .flex_1()
                .min_h_0()
                .w_full()
                .child(theme::text("no parameters reported")),
        ),
        Some(_) if panel.matched.is_empty() => body.child(
            div()
                .debug_selector(|| "vst3-parameter-list-area".to_string())
                .flex_1()
                .min_h_0()
                .w_full()
                .child(theme::text("no matching parameters")),
        ),
        Some(rows) => {
            let rows = Arc::clone(rows);
            let matched = Arc::clone(&panel.matched);
            let query = panel.query;
            let scroll = panel.scroll.clone();
            let editor = cx.weak_entity();
            let scrollbar = scrollbar::element(panel, device, cx);
            body.child(
                div()
                    .debug_selector(|| "vst3-parameter-list-area".to_string())
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(div().flex().flex_col().flex_1().min_w_0().h_full().child(
                        uniform_list(
                            ("vst3-parameter-list", device.value()),
                            matched.len(),
                            move |range, _, app| {
                                let visible: Vec<_> = range.clone().collect();
                                let firsts: BTreeSet<_> = visible
                                    .iter()
                                    .map(|index| ((matched[*index] / PAGE_SIZE) * PAGE_SIZE) as u16)
                                    .collect();
                                let _ = editor.update(app, |view, cx| {
                                    if let Some(panel) =
                                        view.device_panel.vst3_parameters.panels.get_mut(&device)
                                        && panel.visible_pages != firsts
                                    {
                                        panel.visible_pages = firsts;
                                        panel.next_page =
                                            panel.visible_pages.first().copied().unwrap_or(0);
                                        cx.notify();
                                    }
                                });
                                range
                                    .filter_map(|index| {
                                        let row_index = *matched.get(index)?;
                                        let row = rows.get(row_index)?;
                                        let knob = if row.editable {
                                            editor
                                                .update(app, |view, cx| {
                                                    if let Some(panel) = view
                                                        .device_panel
                                                        .vst3_parameters
                                                        .panels
                                                        .get_mut(&device)
                                                    {
                                                        panel.row_placeholders.remove(&row.id);
                                                    }
                                                    knob(view, device, query, row, cx)
                                                })
                                                .ok()
                                                .flatten()
                                        } else {
                                            editor
                                                .update(app, |view, cx| {
                                                    row_placeholder(view, device, row, cx)
                                                })
                                                .ok()
                                                .flatten()
                                        };
                                        let row_id = row.id;
                                        let row_editor = editor.clone();
                                        Some(
                                            div()
                                                .h(px(hex_widgets::MICRO_DIAL_HEIGHT + 8.0))
                                                .w_full()
                                                .child(
                                                    hex_widgets::device_section(
                                                        theme::device_style(
                                                            crate::theme::DeviceStyleRole::Layout,
                                                        ),
                                                        hex_widgets::DeviceSectionSurface::Rail,
                                                    )
                                                    .id(("vst3-parameter", row.id))
                                                    .debug_selector(move || {
                                                        format!("vst3-parameter-row-{row_id}")
                                                    })
                                                    .h(px(hex_widgets::MICRO_DIAL_HEIGHT + 4.0))
                                                    .w_full()
                                                    .min_w_0()
                                                    .py(px(2.0))
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .text_xs()
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .flex()
                                                            .items_center()
                                                            .gap_2()
                                                            .child(div().flex_1().min_w_0().truncate().child(theme::text(
                                                                hex_session::service::text::verbatim(&row.name),
                                                            ))),
                                                    )
                                                    .children(knob)
                                                    .when(row.automation_class.is_some(), |item| {
                                                        item.on_mouse_down(
                                                            gpui::MouseButton::Right,
                                                            move |event, window, app| {
                                                                let _ = row_editor.update(
                                                                    app,
                                                                    |view, cx| {
                                                                        if show_automation_menu(
                                                                            view,
                                                                            device,
                                                                            query,
                                                                            row_id,
                                                                            event.position,
                                                                            window,
                                                                            cx,
                                                                        ) {
                                                                            cx.stop_propagation();
                                                                        }
                                                                    },
                                                                );
                                                            },
                                                        )
                                                    }),
                                                ),
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            },
                        )
                        .track_scroll(scroll)
                        .flex_1()
                        .min_h_0()
                        .w_full(),
                    ))
                    .children([scrollbar]),
            )
        }
    }
}

fn follow_display(
    view: &mut EditorView,
    device: DeviceId,
    cx: &mut Context<EditorView>,
) -> (
    Option<Entity<NumericControl>>,
    Option<PluginParameterRow>,
    Option<&'static str>,
) {
    let Some(panel) = view.device_panel.vst3_parameters.panels.get(&device) else {
        return (None, None, Some("no vst parameter touched"));
    };
    let active = view.device_gesture.snapshot().map(|gesture| gesture.key);
    if let Some((id, knob)) = panel.follow_knob.as_ref()
        && knob.control.read(cx).is_interacting()
        && active.is_none_or(|key| knob.owns(key))
    {
        let row = panel
            .rows()
            .and_then(|rows| rows.iter().find(|row| row.id == *id))
            .cloned();
        return (
            Some(knob.control.clone()),
            row,
            Some("refreshing last touched parameter"),
        );
    }
    let refreshing = panel.pending_touch.is_some() || panel.queued_touch.is_some();
    let Some(id) = panel.last_touched else {
        return (None, None, Some("no vst parameter touched"));
    };
    let Some(row) = panel
        .rows()
        .and_then(|rows| rows.iter().find(|row| row.id == id))
        .cloned()
    else {
        return (
            None,
            None,
            Some(if panel.touch_unavailable {
                "last touched parameter unavailable"
            } else {
                "refreshing last touched parameter"
            }),
        );
    };
    if !row.editable || row.metadata_revision == 0 || panel.source.is_none() {
        let control = panel
            .follow_knob
            .as_ref()
            .filter(|(current, knob)| *current == id && knob.matches_descriptor(&row))
            .map(|(_, knob)| knob.control.clone());
        if let Some(control) = &control {
            control.update(cx, |control, cx| {
                let _ = control.set_external_value(row.normalized, cx);
                control.set_enabled(false, cx);
            });
        }
        let status = if row.metadata_revision == 0 || panel.source.is_none() {
            "refreshing last touched parameter"
        } else {
            "last touched parameter is read only"
        };
        return (control, Some(row), Some(status));
    }
    let query = panel.query;
    let status = if refreshing {
        Some("refreshing last touched parameter")
    } else {
        row.automation_class
            .is_none()
            .then_some("automation unavailable")
    };
    (
        follow_knob(view, device, query, &row, cx),
        Some(row),
        status,
    )
}

fn show_automation_menu(
    view: &mut EditorView,
    device: DeviceId,
    query: PluginParameterQuery,
    id: u32,
    position: gpui::Point<gpui::Pixels>,
    window: &mut gpui::Window,
    cx: &mut Context<EditorView>,
) -> bool {
    if current_context(view) != Some(query.context)
        || view
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            != Some(query.output_identity)
    {
        return false;
    }
    let row = view
        .device_panel
        .vst3_parameters
        .panels
        .get(&device)
        .filter(|panel| {
            panel.query.context == query.context
                && panel.query.output_identity == query.output_identity
        })
        .and_then(Panel::rows)
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.id == id && row.automation_class.is_some())
        })
        .cloned();
    row.is_some_and(|row| view.open_vst3_automation_menu(position, query.device, &row, window, cx))
}

pub(in crate::editor) fn refresh(view: &mut EditorView) {
    for panel in view.device_panel.vst3_parameters.panels.values_mut() {
        panel.mark_values_stale();
    }
}

#[cfg(feature = "ui-test-support")]
pub(in crate::editor) fn test_state(view: &EditorView) -> (Vec<(u64, u32, gpui::EntityId)>, bool) {
    let state = &view.device_panel.vst3_parameters;
    let mut controls = state
        .panels
        .iter()
        .flat_map(|(device, panel)| {
            panel
                .knobs
                .iter()
                .map(|(parameter, knob)| (device.value(), *parameter, knob.control.entity_id()))
        })
        .collect::<Vec<_>>();
    controls.sort_by_key(|(device, parameter, _)| (*device, *parameter));
    (
        controls,
        state.in_flight.is_some() || state.deferred_result.is_some(),
    )
}

#[cfg(feature = "ui-test-support")]
pub(in crate::editor) fn test_touch_state(
    view: &EditorView,
    device_id: u64,
    cx: &App,
) -> (Option<(u32, f64)>, Option<gpui::EntityId>, bool) {
    let Some((device, panel)) = view
        .device_panel
        .vst3_parameters
        .panels
        .iter()
        .find(|(device, _)| device.value() == device_id)
    else {
        return (None, None, false);
    };
    let touched = panel.last_touched.and_then(|parameter| {
        panel
            .rows()
            .and_then(|rows| rows.iter().find(|row| row.id == parameter))
            .filter(|row| row.metadata_revision != 0)
            .map(|row| {
                let displayed = panel
                    .follow_knob
                    .as_ref()
                    .filter(|(id, _)| *id == parameter)
                    .map_or(row.normalized, |(_, knob)| knob.control.read(cx).value());
                (parameter, displayed)
            })
    });
    let follow = panel
        .follow_knob
        .as_ref()
        .map(|(_, knob)| knob.control.entity_id());
    let lane_matches = touched
        .is_some_and(|(parameter, _)| view.test_touched_vst3_lane(panel.owner, *device, parameter));
    (touched, follow, lane_matches)
}

fn placeholder_knob(
    device: DeviceId,
    cx: &mut Context<EditorView>,
) -> Option<Entity<NumericControl>> {
    let spec = NumericSpec::new(0.0, 1.0, 0.5, NumericScale::Linear, 3, None::<&str>).ok()?;
    let config = NumericControlConfig::new(
        format!("vst3-follow-placeholder-{}", device.value()),
        theme::text(""),
        NumericControlPresentation::MicroDial,
        spec,
        0.5,
        theme::control_style(),
    )
    .ok()?;
    let control = cx.new(|cx| NumericControl::from_config(cx, config));
    control.update(cx, |control, cx| control.set_enabled(false, cx));
    Some(control)
}

// A bounded set of display-only controls supports virtualization without giving
// cached descriptors the authority to dispatch parameter gestures.
fn row_placeholder(
    view: &mut EditorView,
    device: DeviceId,
    row: &PluginParameterRow,
    cx: &mut Context<EditorView>,
) -> Option<Entity<NumericControl>> {
    let panel = view.device_panel.vst3_parameters.panels.get_mut(&device)?;
    if let Some(knob) = panel.knobs.get(&row.id)
        && knob.matches_descriptor(row)
    {
        let control = knob.control.clone();
        control.update(cx, |control, cx| {
            let _ = control.set_external_value(row.normalized, cx);
            control.set_enabled(false, cx);
        });
        return Some(control);
    }
    if let Some(placeholder) = panel.row_placeholders.get(&row.id)
        && placeholder.name == row.name
        && placeholder.default == row.default_normalized
    {
        let control = placeholder.control.clone();
        control.update(cx, |control, cx| {
            let _ = control.set_external_value(row.normalized, cx);
        });
        return Some(control);
    }
    // The list also renders an item for measurement, so pruning by each
    // requested range would evict controls that are still on screen.
    if panel.row_placeholders.len() >= 128 {
        panel.row_placeholders.clear();
    }
    let spec = NumericSpec::new(
        0.0,
        1.0,
        row.default_normalized,
        NumericScale::Linear,
        3,
        None::<&str>,
    )
    .ok()?;
    let config = NumericControlConfig::new(
        format!("vst3-placeholder-{}-{}", device.value(), row.id),
        theme::text(hex_session::service::text::verbatim(&row.name)),
        NumericControlPresentation::MicroDial,
        spec,
        row.normalized,
        theme::control_style(),
    )
    .ok()?;
    let control = cx.new(|cx| NumericControl::from_config(cx, config));
    control.update(cx, |control, cx| control.set_enabled(false, cx));
    panel.row_placeholders.insert(
        row.id,
        RowPlaceholder {
            name: row.name.clone(),
            default: row.default_normalized,
            control: control.clone(),
        },
    );
    Some(control)
}

fn knob(
    view: &mut EditorView,
    device: DeviceId,
    query: PluginParameterQuery,
    row: &PluginParameterRow,
    cx: &mut Context<EditorView>,
) -> Option<Entity<NumericControl>> {
    let active = view.device_gesture.snapshot().map(|gesture| gesture.key);
    let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
    if let Some(knob) = panel.knobs.get(&row.id)
        && knob.matches_source(panel.source?, query, row)
    {
        return knob.renew(query, row, panel.display_value(row), true, active, cx);
    }
    let knob = create_knob(view, device, query, row, "row", true, cx)?;
    let control = knob.control.clone();
    view.device_panel
        .vst3_parameters
        .panels
        .get_mut(&device)?
        .knobs
        .insert(row.id, knob);
    Some(control)
}

fn follow_knob(
    view: &mut EditorView,
    device: DeviceId,
    query: PluginParameterQuery,
    row: &PluginParameterRow,
    cx: &mut Context<EditorView>,
) -> Option<Entity<NumericControl>> {
    let active = view.device_gesture.snapshot().map(|gesture| gesture.key);
    let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
    let enabled = panel.pending_touch.is_none() && panel.queued_touch.is_none();
    if let Some((id, knob)) = panel.follow_knob.as_ref()
        && *id == row.id
        && knob.matches_source(panel.source?, query, row)
    {
        return knob.renew(query, row, panel.display_value(row), enabled, active, cx);
    }
    let knob = create_knob(view, device, query, row, "follow", enabled, cx)?;
    let control = knob.control.clone();
    view.device_panel
        .vst3_parameters
        .panels
        .get_mut(&device)?
        .follow_knob = Some((row.id, knob));
    Some(control)
}

fn create_knob(
    view: &mut EditorView,
    device: DeviceId,
    query: PluginParameterQuery,
    row: &PluginParameterRow,
    role: &str,
    enabled: bool,
    cx: &mut Context<EditorView>,
) -> Option<Knob> {
    let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
    let source = panel.source?;
    let origin = parameter_origin(query, row)?;
    let spec = NumericSpec::new(
        0.0,
        1.0,
        row.default_normalized,
        NumericScale::Linear,
        3,
        None::<&str>,
    )
    .ok()?;
    let control_id = if role == "row" {
        format!("vst3-knob-{}-{}", query.device.value(), row.id)
    } else {
        format!("vst3-follow-knob-{}-{}", query.device.value(), row.id)
    };
    let config = NumericControlConfig::new(
        control_id,
        theme::text(hex_session::service::text::verbatim(&row.name)),
        NumericControlPresentation::MicroDial,
        spec,
        panel.display_value(row),
        theme::control_style(),
    )
    .ok()?;
    let availability =
        ControlAvailability::for_edit(enabled, view.device_gesture.snapshot().is_some());
    let control = cx.new(|cx| {
        let mut control = NumericControl::from_config(cx, config);
        control.set_availability(availability, cx);
        control
    });
    let origin = Rc::new(Cell::new(origin));
    let event_origin = Rc::clone(&origin);
    let drag = Rc::new(RefCell::new(None));
    let event_drag = Rc::clone(&drag);
    let steps = row.steps;
    let is_follow = role == "follow";
    let subscription =
        cx.subscribe(
            &control,
            move |view, emitter, event: &NumericControlEvent, cx| {
                let origin = event_origin.get();
                // Pending metadata suspends new follow gestures, but an admitted
                // pointer gesture must still reach its existing terminal owner.
                let continuing_pointer =
                    event.phase != NumericEventPhase::Begin && event.gesture.is_some();
                if !continuing_pointer && view.device_gesture.snapshot().is_some() {
                    return;
                }
                let current =
                    view.device_panel
                        .vst3_parameters
                        .panels
                        .get(&device)
                        .filter(|panel| {
                            (!is_follow
                                || continuing_pointer
                                || (panel.pending_touch.is_none() && panel.queued_touch.is_none()))
                                && panel.source == Some(source)
                                && panel.query.context == origin.context
                                && panel.query.output_identity == query.output_identity
                        })
                        .and_then(Panel::rows)
                        .and_then(|rows| rows.iter().find(|row| row.id == origin.parameter))
                        .is_some_and(|row| {
                            row.editable
                                && row.metadata_revision != 0
                                && row.metadata_revision == origin.metadata_revision
                                && row.normalized == origin.initial.normalized()
                        })
                        && view
                            .device_panel
                            .vst3_parameters
                            .panels
                            .get(&device)
                            .is_some_and(|panel| {
                                panel.knobs.get(&origin.parameter).is_some_and(|knob| {
                                    knob.control.entity_id() == emitter.entity_id()
                                }) || panel.follow_knob.as_ref().is_some_and(|(id, knob)| {
                                    *id == origin.parameter
                                        && knob.control.entity_id() == emitter.entity_id()
                                })
                            });
                if !current || current_context(view) != Some(origin.context) {
                    return;
                }
                if let Err(reason) = knob_event(view, origin, event, steps, &event_drag) {
                    view.error = Some(reason.into());
                }
                if event.phase == NumericEventPhase::Begin {
                    queue_host_touch(view, device, origin.parameter, cx);
                }
                view.pump_device_gesture();
                cx.notify();
            },
        );
    Some(Knob {
        source,
        output: query.output_identity,
        name: row.name.clone(),
        default: row.default_normalized,
        steps: row.steps,
        control,
        origin,
        drag,
        _subscription: subscription,
    })
}

type KnobDrag = RefCell<Option<(Option<NumericGestureId>, GestureKey)>>;

fn knob_event(
    view: &mut EditorView,
    origin: PluginParameterOrigin,
    event: &NumericControlEvent,
    steps: u32,
    drag: &KnobDrag,
) -> Result<(), &'static str> {
    let normalized = if steps == 0 {
        event.value
    } else {
        let steps = f64::from(steps);
        (event.value * steps).round() / steps
    };
    let value = PluginParameterValue::new(normalized)?;
    let mut drag = drag.borrow_mut();
    if view.automation_close_preflight.frozen()
        && (event.phase == NumericEventPhase::Begin
            || (event.phase == NumericEventPhase::Commit && event.gesture.is_none()))
    {
        return Err("close is pending · automation recording is unavailable");
    }
    if event.phase == NumericEventPhase::Begin {
        let key = view.begin_vst3_parameter_gesture(origin)?;
        *drag = Some((event.gesture.clone(), key));
        return Ok(());
    }
    if event.phase == NumericEventPhase::Commit
        && event.gesture.is_none()
        && view.device_gesture.snapshot().is_none()
    {
        view.begin_vst3_parameter_gesture(origin)?;
        return view.device_gesture.finish_plugin(value);
    }
    let Some((pointer, key)) = drag.as_ref() else {
        return Ok(());
    };
    if *pointer != event.gesture
        || view
            .device_gesture
            .snapshot()
            .is_none_or(|active| active.key != *key)
    {
        return Ok(());
    }
    match event.phase {
        NumericEventPhase::Change => view.device_gesture.change_plugin(value).map(|_| ()),
        NumericEventPhase::Commit => {
            *drag = None;
            view.device_gesture.finish_plugin(value)
        }
        NumericEventPhase::Cancel => {
            *drag = None;
            view.device_gesture.cancel()
        }
        NumericEventPhase::Begin => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_rows_are_display_only_until_a_live_page_arrives() {
        let cached = PluginParameterRow {
            id: 7,
            name: "gain".into(),
            value: String::new(),
            normalized: 0.25,
            default_normalized: 0.25,
            steps: 0,
            editable: false,
            automation_class: None,
            page_first: 0,
            metadata_revision: 0,
        };
        let stale = display_only(&PluginParameterRow {
            value: "live".into(),
            normalized: 0.8,
            editable: true,
            automation_class: Some([1; 16]),
            metadata_revision: 7,
            ..cached.clone()
        });
        assert!(stale.value.is_empty());
        assert_eq!(stale.normalized, stale.default_normalized);
        assert!(!stale.editable);
        assert_eq!(stale.automation_class, None);
        assert_eq!(stale.metadata_revision, 0);
    }

    #[test]
    fn matching_rows_keeps_cache_indices_for_live_page_requests() {
        let rows = ["oscillator", "filter cutoff", "filter resonance", "level"]
            .into_iter()
            .enumerate()
            .map(|(i, name)| PluginParameterRow {
                id: 100 + 3 * i as u32,
                name: name.into(),
                value: String::new(),
                normalized: 0.0,
                default_normalized: 0.0,
                steps: 0,
                editable: false,
                automation_class: None,
                page_first: 0,
                metadata_revision: 0,
            })
            .collect::<Vec<_>>();
        let found = matching_rows(&rows, "filter");
        assert_eq!(found, [1, 2]);
        assert_eq!(
            found
                .iter()
                .map(|&index| (index / PAGE_SIZE) * PAGE_SIZE)
                .collect::<Vec<_>>(),
            [0, 0]
        );
    }

    #[test]
    fn live_page_merge_requires_the_cached_total_and_ordered_ids() {
        let rows = (0_u32..6)
            .map(|index| PluginParameterRow {
                id: 20 + index,
                name: format!("parameter {index}"),
                value: String::new(),
                normalized: 0.0,
                default_normalized: 0.0,
                steps: 0,
                editable: false,
                automation_class: None,
                page_first: ((index / 4) * 4) as u16,
                metadata_revision: 0,
            })
            .collect::<Vec<_>>();
        let mut second_page = rows[4..].to_vec();
        second_page[0].value = "live".into();
        let merged =
            merge_page(&rows, Some([8; 32]), Some(6), 6, 4, [8; 32], &second_page).unwrap();
        assert_eq!(merged[4].value, "live");
        assert!(merge_page(&rows, Some([8; 32]), Some(5), 6, 4, [8; 32], &second_page).is_none());
        assert!(merge_page(&rows, Some([8; 32]), Some(6), 6, 1, [8; 32], &rows[1..5]).is_none());
        let mut reordered = second_page;
        reordered.swap(0, 1);
        assert!(merge_page(&rows, Some([8; 32]), Some(6), 6, 4, [8; 32], &reordered).is_none());
        let unchanged_ids_with_changed_signature = rows[4..].to_vec();
        assert!(
            merge_page(
                &rows,
                Some([8; 32]),
                Some(6),
                6,
                4,
                [9; 32],
                &unchanged_ids_with_changed_signature,
            )
            .is_none()
        );
        let value_only_update = rows[4..]
            .iter()
            .cloned()
            .map(|mut row| {
                row.value = "changed value".into();
                row
            })
            .collect::<Vec<_>>();
        assert!(
            merge_page(
                &rows,
                Some([8; 32]),
                Some(6),
                6,
                4,
                [8; 32],
                &value_only_update
            )
            .is_some()
        );
    }

    #[test]
    fn unvalidated_cache_requests_page_zero_without_visible_matches() {
        let no_visible_pages = BTreeSet::new();
        assert_eq!(
            refresh_page(true, false, None, &no_visible_pages, 0, false),
            Some(Some(0))
        );
        assert_eq!(
            refresh_page(true, true, None, &no_visible_pages, 0, false),
            None
        );
        assert_eq!(
            refresh_page(true, true, Some(12), &no_visible_pages, 0, false),
            Some(Some(12))
        );
    }

    #[test]
    fn native_touch_resolves_only_from_a_fresh_page_for_the_exact_binding() {
        let binding = hex_lattice_contract::ForeignProcessorBinding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let other_binding = hex_lattice_contract::ForeignProcessorBinding {
            epoch: 5,
            ..binding
        };
        let touch = TouchObservation {
            source: AppliedAudioSource {
                context: AudioSourceContext {
                    project_generation: 8,
                    editor_revision: 9,
                },
                audio_revision: 10,
                output_identity: 11,
            },
            binding,
            parameter: 7,
        };
        let cached = PluginParameterRow {
            id: 7,
            name: "gain".into(),
            value: String::new(),
            normalized: 0.2,
            default_normalized: 0.2,
            steps: 0,
            editable: false,
            automation_class: None,
            page_first: 4,
            metadata_revision: 0,
        };
        let live = PluginParameterRow {
            value: "0.75".into(),
            normalized: 0.75,
            editable: true,
            automation_class: Some([6; 16]),
            metadata_revision: 3,
            ..cached.clone()
        };
        assert_eq!(
            fresh_touch_row(touch, binding, std::slice::from_ref(&live), Some(4)),
            Some(live.clone())
        );
        assert_eq!(
            fresh_touch_row(touch, other_binding, std::slice::from_ref(&cached), Some(4)),
            None
        );
        assert_eq!(fresh_touch_row(touch, binding, &[cached], Some(4)), None);
        assert_eq!(fresh_touch_row(touch, binding, &[live], Some(0)), None);
    }

    #[test]
    fn control_yield_is_a_quiet_refresh_retry() {
        assert!(is_refresh_yield(REFRESH_YIELDED_TO_CONTROL));
        assert!(!is_refresh_yield("vst3 parameter source changed"));
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn vst3_header_and_list_bounds_stay_fixed_across_touch_states(cx: &mut gpui::TestAppContext) {
        use crate::editor::{clip_drag_ui_test, device_panel::DevicePanelOwner};
        use gpui::VisualTestContext;
        let (project, track, device) = parameter_project();
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
            view.device_panel_active = true;
            view.clip_view_open = true;
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
                layout_row(0.2, true, 1),
                PluginParameterRow {
                    id: 8,
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
            assert!(view.device_gesture.snapshot().is_none());
        });
        visual.simulate_mouse_up(down, gpui::MouseButton::Left, modifiers);
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
            let active = view.device_gesture.begin_plugin(origin).unwrap();
            view.device_gesture.finish_plugin(origin.initial).unwrap();
            cx.notify();
            (active, other)
        });
        visual.run_until_parked();
        let other_selector =
            Box::leak(format!("vst3-knob-{}-8-knob", device.value()).into_boxed_str());
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
            assert_eq!(view.device_gesture.snapshot().unwrap().key, active);
            assert!(view.device_gesture.has_pending_terminal());
        });
        visual.simulate_mouse_up(other_dial, gpui::MouseButton::Left, modifiers);
        editor.update(&mut visual, |view, cx| {
            use hex_session::device_gesture::{GestureAck, GesturePhase};
            for phase in [GesturePhase::Begin, GesturePhase::Commit] {
                let request = view.device_gesture.next_request().unwrap();
                assert_eq!((request.key(), request.phase()), (active, phase));
                view.device_gesture.mark_submitted();
                assert!(view.device_gesture.acknowledge(&GestureAck {
                    preview_retained: false,
                    source_lost: false,
                    key: active,
                    phase,
                    sequence: request.sequence(),
                    accepted: true,
                    reason: None,
                }));
            }
            assert!(view.device_gesture.snapshot().is_none());
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
            assert!(view.device_gesture.snapshot().is_none());
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
            assert!(view.device_gesture.snapshot().is_none());
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
        let dial =
            Box::leak(format!("vst3-placeholder-{}-124-knob", device.value()).into_boxed_str());
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
            assert!(view.device_gesture.snapshot().is_none());
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
        editor.read_with(&visual, |view, _| {
            assert!(
                view.device_panel.vst3_parameters.panels[&device]
                    .scrollbar
                    .test_dragging()
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
            assert!(view.device_gesture.snapshot().is_none());
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
            assert!(view.device_gesture.snapshot().is_none());
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
            // A native receipt can paint ahead of an already submitted page.
            // It must never change the edit origin or be overwritten by that page.
            panel.query.request_id = 12;
            panel.observe_value(touch, Some(PluginParameterValue::new(0.7).unwrap()));
            let row = panel.rows().unwrap()[0].clone();
            assert_eq!(panel.display_value(&row), 0.7);
            assert_eq!(
                parameter_origin(panel.query, &row)
                    .unwrap()
                    .initial
                    .normalized(),
                0.4
            );
            panel.reconcile_live_values(panel.query, binding, std::slice::from_ref(&row));
            assert_eq!(panel.display_value(&row), 0.7);
            let newer = PluginParameterQuery {
                request_id: 13,
                ..panel.query
            };
            panel.reconcile_live_values(newer, binding, std::slice::from_ref(&row));
            assert_eq!(panel.display_value(&row), 0.4);
            panel.observe_value(touch, Some(PluginParameterValue::new(0.8).unwrap()));
            panel.source = Some(hex_lattice_contract::ForeignProcessorBinding {
                epoch: 5,
                ..binding
            });
            assert_eq!(panel.display_value(&row), 0.4);
            panel.source = Some(binding);
            panel.live_values.clear();

            let query = panel.query;
            panel.observe_value(touch, Some(PluginParameterValue::new(0.9).unwrap()));
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
        let state =
            NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, payload).unwrap();
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
        }
    }
}

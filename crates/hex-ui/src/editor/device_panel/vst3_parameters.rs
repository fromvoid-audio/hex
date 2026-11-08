//! View-local panels for exact hosted VST3 controller sources. Persisted rows
//! supply names and order only; every displayed parameter value is refreshed
//! from the active source before it can be edited.
use super::*;
use gpui::{Entity, Subscription, UniformListScrollHandle, uniform_list};
use hex_engine::AudioSourceContext;
use hex_session::device_gesture::{GestureKey, PluginParameterOrigin, PluginParameterValue};
use hex_session::service::{PluginParameterQuery, PluginParameterResult, PluginParameterRow};
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlPresentation,
    NumericEventPhase, NumericGestureId, NumericScale, NumericSpec, TextInput,
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

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    panels: HashMap<DeviceId, Panel>,
    in_flight: Option<(DeviceId, PluginParameterQuery)>,
    deferred_result: Option<(PluginParameterResult, u64)>,
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
    visible_pages: BTreeSet<u16>,
    next_page: u16,
    knobs: HashMap<u32, Knob>,
    next_query_at: Instant,
    retry_count: u8,
}
struct Knob {
    control: Entity<NumericControl>,
    origin: Rc<Cell<PluginParameterOrigin>>,
    _subscription: Subscription,
}

impl Panel {
    fn rows(&self) -> Option<&[PluginParameterRow]> {
        self.result.as_ref().map(|rows| rows.as_slice())
    }

    fn refilter(&mut self, reset_scroll: bool) {
        let Some(matched) = self.rows().map(|rows| matching_rows(rows, &self.filter)) else {
            return;
        };
        self.matched = Arc::new(matched);
        if reset_scroll {
            self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        }
    }

    fn mark_values_stale(&mut self) {
        let total = self.rows().and_then(|rows| u16::try_from(rows.len()).ok());
        if let Some(rows) = &self.result {
            self.result = Some(Arc::new(rows.iter().map(display_only).collect()));
        }
        self.source = None;
        self.total = total;
        self.knobs.clear();
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
    visible_pages: &BTreeSet<u16>,
    next_page: u16,
    rows_empty: bool,
) -> Option<Option<u16>> {
    if !has_rows {
        return Some(None);
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
    if view.stopped_playback_restore.is_some() || view.device_gesture.snapshot().is_some() {
        return;
    }
    let Some(owner) = view.device_panel.owner else {
        return;
    };
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
            visible_pages: BTreeSet::new(),
            next_page: 0,
            knobs: HashMap::new(),
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
                        && panel.query.output_identity == query.output_identity
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
    panel.next_query_at = Instant::now() + PAGE_REFRESH_INTERVAL;
    let rows = match result.rows {
        Ok(rows) => rows,
        Err(error) if is_refresh_yield(&error) => {
            panel.cache_error = None;
            panel.retry_count = 0;
            panel.next_query_at = Instant::now();
            return;
        }
        Err(error) => {
            record_error(panel, error);
            return;
        }
    };
    let Some(source) = result.source else {
        record_error(panel, "vst3 parameter source changed".into());
        panel.mark_values_stale();
        return;
    };
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
        panel.cache_error = None;
        panel.result = None;
        panel.source = None;
        panel.descriptor_signature = None;
        panel.total = None;
        panel.knobs.clear();
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
        panel.source = Some(source);
        panel.descriptor_signature = Some(descriptor_signature);
        panel.total = Some(result.total);
        panel.cache_error = None;
        panel.retry_count = 0;
        panel.knobs.clear();
        panel.result = Some(Arc::new(rows));
        panel.refilter(false);
        panel.cache_needs_save = true;
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
        panel.next_query_at = Instant::now();
        return;
    };
    for (offset, row) in rows.iter().enumerate() {
        let index = usize::from(first) + offset;
        if &cached[index] != row
            && let Some(knob) = panel.knobs.get(&row.id)
            && let Some(origin) = parameter_origin(result.query, row)
        {
            knob.origin.set(origin);
            let value = row.normalized;
            knob.control.update(cx, |control, cx| {
                let _ = control.set_external_value(value, cx);
            });
        }
    }
    panel.source = Some(source);
    panel.descriptor_signature = Some(descriptor_signature);
    panel.result = Some(Arc::new(merged));
    panel.cache_error = None;
    panel.retry_count = 0;
    panel.refilter(false);
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
            })
}

fn record_error(panel: &mut Panel, error: String) {
    panel.retry_count = panel.retry_count.saturating_add(1);
    panel.cache_error = Some(error);
    let exponent = u32::from(panel.retry_count.saturating_sub(1).min(5));
    panel.next_query_at = Instant::now() + Duration::from_secs(1_u64 << exponent);
}

pub(in crate::editor) fn tick(view: &mut EditorView, cx: &mut Context<EditorView>) {
    reconcile_panels(view, cx);
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
    if view.device_panel.vst3_parameters.in_flight.is_some() {
        return;
    }
    let Some(owner) = view.device_panel.owner.filter(|_| view.device_panel_active) else {
        return;
    };
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let active = super::chain_ids(&state.project, owner);
    let now = Instant::now();
    let next = active.into_iter().find_map(|device| {
        let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
        if panel.owner != owner || panel.cache_loading || now < panel.next_query_at {
            return None;
        }
        if panel.retry_count > MAX_AUTOMATIC_RETRIES {
            return None;
        }
        let page = refresh_page(
            panel.rows().is_some(),
            panel.source.is_some(),
            &panel.visible_pages,
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
            (!present || key != panel.key).then_some(*device)
        })
        .collect::<Vec<_>>();
    for device in stale {
        view.device_panel.vst3_parameters.panels.remove(&device);
    }
    if interactions_active(view, cx) {
        return;
    }
    for panel in view.device_panel.vst3_parameters.panels.values_mut() {
        if panel.query.context != context || output_identity != Some(panel.query.output_identity) {
            panel.query.context = context;
            if let Some(output_identity) = output_identity {
                panel.query.output_identity = output_identity;
            }
            panel.mark_values_stale();
        }
    }
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
    let panel = view.device_panel.vst3_parameters.panels.get(&device);
    let body = crate::device_composition::body()
        .w(px(crate::device_composition::GENERIC_FRAME_WIDTH))
        .flex_col()
        .min_w_0();
    if view.stopped_playback_restore.is_some() {
        return body
            .debug_selector(|| "vst3-controls-restoring".to_string())
            .child(theme::text("restoring vst3…"));
    }
    let Some(panel) = panel else {
        return body.child(theme::text("reading vst3 parameters…"));
    };
    let mut body = body.child(panel.search.clone());
    if let Some(error) = &panel.cache_error {
        body = body.child(theme::text(error));
    }
    if panel.retry_count > MAX_AUTOMATIC_RETRIES {
        body = body.child(
            hex_widgets::action(theme::text("retry"), theme::action_style())
                .id(("vst3-parameter-retry", device.value()))
                .on_click(cx.listener(move |view, _, _, cx| {
                    if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
                        panel.retry_count = 0;
                        panel.cache_error = None;
                        panel.next_query_at = Instant::now();
                    }
                    cx.notify();
                })),
        );
    }
    match &panel.result {
        None => body.child(theme::text("reading vst3 parameters…")),
        Some(rows) if rows.is_empty() => body.child(theme::text("no parameters reported")),
        Some(_) if panel.matched.is_empty() => body.child(theme::text("no matching parameters")),
        Some(rows) => {
            let rows = Arc::clone(rows);
            let matched = Arc::clone(&panel.matched);
            let query = panel.query;
            let scroll = panel.scroll.clone();
            let editor = cx.weak_entity();
            body.child(
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
                                panel.next_page = panel.visible_pages.first().copied().unwrap_or(0);
                                cx.notify();
                            }
                        });
                        range
                            .filter_map(|index| {
                                let row_index = *matched.get(index)?;
                                let row = rows.get(row_index)?;
                                let knob = if row.editable {
                                    editor
                                        .update(app, |view, cx| knob(view, device, query, row, cx))
                                        .ok()
                                        .flatten()
                                } else {
                                    None
                                };
                                let row_id = row.id;
                                let row_editor = editor.clone();
                                Some(
                                    div()
                                        .id(("vst3-parameter", row.id))
                                        .debug_selector(move || {
                                            format!("vst3-parameter-row-{row_id}")
                                        })
                                        .h(px(28.0))
                                        .w_full()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .text_xs()
                                        .child(div().flex_1().min_w_0().truncate().child(
                                            theme::text(hex_session::service::text::verbatim(
                                                &row.name,
                                            )),
                                        ))
                                        .children(knob)
                                        .when(row.automation_class.is_some(), |item| {
                                            item.on_mouse_down(
                                                gpui::MouseButton::Right,
                                                move |event, window, app| {
                                                    let _ = row_editor.update(app, |view, cx| {
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
                                                    });
                                                },
                                            )
                                        })
                                        .when(!row.editable, |item| {
                                            item.child(div().max_w(px(92.0)).truncate().child(
                                                theme::text(hex_session::service::text::verbatim(
                                                    &row.value,
                                                )),
                                            ))
                                        }),
                                )
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .track_scroll(scroll)
                .flex_1()
                .min_h_0()
                .w_full(),
            )
        }
    }
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

fn knob(
    view: &mut EditorView,
    device: DeviceId,
    query: PluginParameterQuery,
    row: &PluginParameterRow,
    cx: &mut Context<EditorView>,
) -> Option<Entity<NumericControl>> {
    let panel = view.device_panel.vst3_parameters.panels.get_mut(&device)?;
    if let Some(knob) = panel.knobs.get(&row.id) {
        return Some(knob.control.clone());
    }
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
    let config = NumericControlConfig::new(
        format!("vst3-knob-{}-{}", query.device.value(), row.id),
        theme::text(hex_session::service::text::verbatim(&row.name)),
        NumericControlPresentation::MicroDial,
        spec,
        row.normalized,
        theme::control_style(),
    )
    .ok()?;
    let control = cx.new(|cx| NumericControl::from_config(cx, config));
    let origin = Rc::new(Cell::new(origin));
    let event_origin = Rc::clone(&origin);
    let drag = Rc::new(RefCell::new(None));
    let steps = row.steps;
    let subscription = cx.subscribe(
        &control,
        move |view, emitter, event: &NumericControlEvent, cx| {
            let origin = event_origin.get();
            let current = view
                .device_panel
                .vst3_parameters
                .panels
                .get(&device)
                .filter(|panel| {
                    panel.source == Some(source)
                        && panel.query.context == origin.context
                        && panel.query.output_identity == query.output_identity
                })
                .and_then(Panel::rows)
                .and_then(|rows| rows.iter().find(|row| row.id == origin.parameter))
                .is_some_and(|row| {
                    row.metadata_revision == origin.metadata_revision
                        && row.normalized == origin.initial.normalized()
                })
                && view
                    .device_panel
                    .vst3_parameters
                    .panels
                    .get(&device)
                    .and_then(|panel| panel.knobs.get(&origin.parameter))
                    .is_some_and(|knob| knob.control.entity_id() == emitter.entity_id());
            if !current || current_context(view) != Some(origin.context) {
                return;
            }
            if let Err(reason) = knob_event(view, origin, event, steps, &drag) {
                view.error = Some(reason.into());
            }
            view.pump_device_gesture();
            cx.notify();
        },
    );
    panel.knobs.insert(
        row.id,
        Knob {
            control: control.clone(),
            origin,
            _subscription: subscription,
        },
    );
    Some(control)
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
    if event.phase == NumericEventPhase::Begin {
        let key = view.device_gesture.begin_plugin(origin)?;
        *drag = Some((event.gesture.clone(), key));
        return Ok(());
    }
    if event.phase == NumericEventPhase::Commit
        && event.gesture.is_none()
        && view.device_gesture.snapshot().is_none()
    {
        view.device_gesture.begin_plugin(origin)?;
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
            refresh_page(true, false, &no_visible_pages, 0, false),
            Some(Some(0))
        );
        assert_eq!(refresh_page(true, true, &no_visible_pages, 0, false), None);
    }

    #[test]
    fn control_yield_is_a_quiet_refresh_retry() {
        assert!(is_refresh_yield(REFRESH_YIELDED_TO_CONTROL));
        assert!(!is_refresh_yield("vst3 parameter source changed"));
    }
}

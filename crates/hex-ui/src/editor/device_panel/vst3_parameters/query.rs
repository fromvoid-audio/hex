//! Hosted VST3 parameter query and conversion result installation.
use super::*;

pub(in crate::editor) fn ensure_panel(
    view: &mut EditorView,
    device: DeviceId,
    cx: &mut Context<EditorView>,
) {
    if view.device_panel.gesture.snapshot().is_some() {
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
        retire_panel(view, device);
    }
    let query = PluginParameterQuery {
        context,
        device,
        output_identity,
        request_id: 0,
        page_first: None,
    };
    let search = cx.new(super::super::super::browser_input::plugin_parameter_search);
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
            formatted_steps: HashMap::new(),
            format_unsupported: BTreeSet::new(),
            drag_preview: None,
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
    let Some((pending, _)) = view.device_panel.vst3_parameters.in_flight else {
        return;
    };
    if pending != result.query {
        return;
    }
    view.device_panel.vst3_parameters.deferred_result = Some((result, revision));
}

pub(in crate::editor) fn install_conversion_result(
    view: &mut EditorView,
    result: PluginParameterConversionResult,
    revision: u64,
) {
    let state = &view.device_panel.vst3_parameters;
    let matches_parse = state
        .conversion
        .as_ref()
        .is_some_and(|pending| pending.query == result.query);
    let matches_format = state
        .live_format
        .as_ref()
        .is_some_and(|pending| pending.query == result.query);
    if matches_parse || matches_format {
        view.device_panel.vst3_parameters.conversion_result = Some((result, revision));
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::editor) fn request_live_format(
    view: &mut EditorView,
    device: DeviceId,
    source: hex_lattice_contract::ForeignProcessorBinding,
    output_identity: u64,
    context: AudioSourceContext,
    parameter: u32,
    steps: u32,
    normalized: f64,
    cx: &mut Context<EditorView>,
) {
    view.device_panel.vst3_parameters.queued_format = Some(QueuedFormat {
        device,
        source,
        output_identity,
        context,
        parameter,
        steps,
        normalized,
    });
    pump_live_format(view, cx);
}

pub(in crate::editor) fn pump_live_format(view: &mut EditorView, cx: &mut Context<EditorView>) {
    if view.device_panel.vst3_parameters.conversion.is_some()
        || view.device_panel.vst3_parameters.live_format.is_some()
    {
        return;
    }
    let Some(queued) = view.device_panel.vst3_parameters.queued_format.take() else {
        return;
    };
    let Some(applied_source) = refresh_source(view) else {
        return;
    };
    if current_context(view) != Some(queued.context)
        || applied_source.output_identity != queued.output_identity
        || !refreshable_device(view, queued.device, applied_source)
    {
        return;
    }
    let still_current = view
        .device_panel
        .vst3_parameters
        .panels
        .get(&queued.device)
        .is_some_and(|panel| {
            panel.source == Some(queued.source)
                && panel.query.context == queued.context
                && panel.query.output_identity == queued.output_identity
                && !panel.format_unsupported.contains(&queued.parameter)
        });
    if !still_current {
        return;
    }
    let Some(request_id) = view.device_panel.vst3_parameters.serial.checked_add(1) else {
        return;
    };
    let query = PluginParameterConversionQuery {
        context: queued.context,
        device: queued.device,
        output_identity: queued.output_identity,
        request_id,
        source: queued.source,
        parameter: queued.parameter,
        operation: PluginParameterConversion::Format(queued.normalized),
    };
    if !view.enqueue(Request::QueryPluginParameterConversion(query.clone())) {
        // Keep queued for the next tick if the service mailbox is momentarily full.
        view.device_panel.vst3_parameters.queued_format = Some(queued);
        return;
    }
    view.device_panel.vst3_parameters.serial = request_id;
    view.device_panel.vst3_parameters.live_format = Some(PendingFormat {
        query,
        source: applied_source,
        steps: queued.steps,
    });
    cx.notify();
}

pub(in crate::editor) fn conversion_target_current(
    view: &EditorView,
    pending: &PendingConversion,
) -> bool {
    let query = &pending.query;
    if current_context(view) != Some(query.context)
        || refresh_source(view) != Some(pending.source)
        || !refreshable_device(view, query.device, pending.source)
        || view.device_panel.gesture.snapshot().is_some()
        || view.worker_requests_pending != 0
        || !view.pending_requests.is_empty()
        || !view.device_panel.active
    {
        return false;
    }
    let Some(panel) = view.device_panel.vst3_parameters.panels.get(&query.device) else {
        return false;
    };
    if view.device_panel.owner != Some(panel.owner)
        || panel.source != Some(query.source)
        || panel.query.context != query.context
        || panel.query.output_identity != query.output_identity
    {
        return false;
    }
    let same_control = panel
        .knobs
        .get(&query.parameter)
        .is_some_and(|knob| knob.control.entity_id() == pending.control.entity_id())
        || panel.follow_knob.as_ref().is_some_and(|(id, knob)| {
            *id == query.parameter
                && knob.control.entity_id() == pending.control.entity_id()
                && panel.pending_touch.is_none()
                && panel.queued_touch.is_none()
        });
    same_control
        && panel
            .rows()
            .and_then(|rows| rows.iter().find(|row| row.id == query.parameter))
            .is_some_and(|row| {
                row.editable
                    && row.metadata_revision != 0
                    && row.metadata_revision == pending.origin.metadata_revision
                    && row.normalized == pending.origin.initial.normalized()
            })
}

pub(in crate::editor) fn refuse_conversion(
    view: &mut EditorView,
    control: &Entity<NumericControl>,
    draft: &NumericTextRequestId,
    reason: &str,
    cx: &mut Context<EditorView>,
) {
    if control
        .update(cx, |control, cx| {
            control.resolve_deferred_text(draft, None, None, cx)
        })
        .unwrap_or(false)
    {
        let state = &mut view.device_panel.vst3_parameters;
        if view.error.is_none() || view.error.as_deref() == state.conversion_error.as_deref() {
            state.conversion_error = Some(reason.to_owned());
            view.error = Some(reason.to_owned().into());
        }
        cx.notify();
    }
}

pub(in crate::editor) fn submit_conversion(
    view: &mut EditorView,
    control: Entity<NumericControl>,
    origin: PluginParameterOrigin,
    binding: hex_lattice_contract::ForeignProcessorBinding,
    output_identity: u64,
    request: NumericTextRequest,
    cx: &mut Context<EditorView>,
) {
    let reject = |view: &mut EditorView, reason, cx: &mut Context<EditorView>| {
        refuse_conversion(view, &control, &request.id, reason, cx);
    };
    if view.device_panel.vst3_parameters.conversion.is_some()
        || view.device_panel.vst3_parameters.live_format.is_some()
    {
        reject(view, "vst3 controller is busy · try again", cx);
        return;
    }
    view.device_panel.vst3_parameters.queued_format = None;
    let Ok(text) = PluginParameterText::new(&request.text) else {
        reject(view, "vst3 parameter text is too long or invalid", cx);
        return;
    };
    let Some(source) = refresh_source(view) else {
        reject(view, "vst3 parameter source unavailable", cx);
        return;
    };
    let Some(request_id) = view.device_panel.vst3_parameters.serial.checked_add(1) else {
        reject(view, "vst3 conversion request limit reached", cx);
        return;
    };
    let query = PluginParameterConversionQuery {
        context: origin.context,
        device: origin.device,
        output_identity,
        request_id,
        source: binding,
        parameter: origin.parameter,
        operation: PluginParameterConversion::Parse(text),
    };
    let pending = PendingConversion {
        query: query.clone(),
        source,
        origin,
        control: control.clone(),
        draft: request.id.clone(),
    };
    if !conversion_target_current(view, &pending) {
        reject(view, "vst3 parameter target changed", cx);
        return;
    }
    if !view.enqueue(Request::QueryPluginParameterConversion(query)) {
        reject(view, "vst3 controller is busy · try again", cx);
        return;
    }
    let state = &mut view.device_panel.vst3_parameters;
    if state
        .conversion_error
        .as_deref()
        .is_some_and(|error| view.error.as_deref() == Some(error))
    {
        view.error = None;
    }
    state.conversion_error = None;
    view.device_panel.vst3_parameters.serial = request_id;
    view.device_panel.vst3_parameters.conversion = Some(pending);
    cx.notify();
}

pub(in crate::editor) fn apply_conversion_result(
    view: &mut EditorView,
    result: PluginParameterConversionResult,
    revision: u64,
    cx: &mut Context<EditorView>,
) {
    if view
        .device_panel
        .vst3_parameters
        .live_format
        .as_ref()
        .is_some_and(|pending| pending.query == result.query)
    {
        let Some(pending) = view.device_panel.vst3_parameters.live_format.take() else {
            return;
        };
        apply_live_format_result(view, pending, result, revision, cx);
        pump_live_format(view, cx);
        return;
    }
    let Some(pending) = view.device_panel.vst3_parameters.conversion.take() else {
        return;
    };
    if pending.query != result.query {
        view.device_panel.vst3_parameters.conversion = Some(pending);
        return;
    }
    if revision != pending.query.context.editor_revision
        || !conversion_target_current(view, &pending)
    {
        refuse_conversion(
            view,
            &pending.control,
            &pending.draft,
            "vst3 parameter target changed",
            cx,
        );
        return;
    }
    match result.value {
        Ok(value) => {
            let steps = view
                .device_panel
                .vst3_parameters
                .panels
                .get(&pending.query.device)
                .and_then(|panel| {
                    panel
                        .rows()?
                        .iter()
                        .find(|row| row.id == pending.query.parameter)
                        .map(|row| row.steps)
                })
                .unwrap_or(0);
            let control_value = control_value_from_normalized(steps, value.normalized);
            if let Some(display) = value.display.as_deref()
                && !display.is_empty()
                && let Some(panel) = view
                    .device_panel
                    .vst3_parameters
                    .panels
                    .get_mut(&pending.query.device)
            {
                panel.cache_step_label(pending.query.parameter, steps, value.normalized, display);
                panel.drag_preview = Some(DragPreview {
                    source: pending.query.source,
                    parameter: pending.query.parameter,
                    normalized: value.normalized,
                    display: display.to_owned(),
                });
            }
            if let Err(error) = pending.control.update(cx, |control, cx| {
                control.resolve_deferred_text(
                    &pending.draft,
                    Some(control_value),
                    value.display.as_deref(),
                    cx,
                )
            }) && view.error.is_none()
            {
                let reason = error.to_string().to_lowercase();
                view.device_panel.vst3_parameters.conversion_error = Some(reason.clone());
                view.error = Some(reason.into());
            }
        }
        Err(reason) => refuse_conversion(view, &pending.control, &pending.draft, &reason, cx),
    }
}

fn apply_live_format_result(
    view: &mut EditorView,
    pending: PendingFormat,
    result: PluginParameterConversionResult,
    revision: u64,
    cx: &mut Context<EditorView>,
) {
    let query = &pending.query;
    if revision != query.context.editor_revision
        || current_context(view) != Some(query.context)
        || refresh_source(view) != Some(pending.source)
    {
        return;
    }
    match result.value {
        Ok(converted) => {
            let Some(display) = converted.display.filter(|text| !text.is_empty()) else {
                return;
            };
            let (row_control, follow_control) = {
                let Some(panel) = view
                    .device_panel
                    .vst3_parameters
                    .panels
                    .get_mut(&query.device)
                    .filter(|panel| {
                        panel.source == Some(query.source)
                            && panel.query.context == query.context
                            && panel.query.output_identity == query.output_identity
                    })
                else {
                    return;
                };
                panel.cache_step_label(
                    query.parameter,
                    pending.steps,
                    converted.normalized,
                    &display,
                );
                let normalized = panel
                    .active_drag_preview(query.parameter)
                    .map_or(converted.normalized, |preview| preview.normalized);
                if panel.active_drag_preview(query.parameter).is_some() {
                    panel.drag_preview = Some(DragPreview {
                        source: query.source,
                        parameter: query.parameter,
                        normalized,
                        display: display.clone(),
                    });
                }
                let row_control = panel
                    .knobs
                    .get(&query.parameter)
                    .map(|knob| knob.control.clone());
                let follow_control = panel
                    .follow_knob
                    .as_ref()
                    .filter(|(id, _)| *id == query.parameter)
                    .map(|(_, knob)| knob.control.clone());
                (row_control, follow_control)
            };
            if let Some(control) = row_control {
                control.set_external_display_for_current(&display, cx);
            }
            if let Some(control) = follow_control {
                control.set_external_display_for_current(&display, cx);
            }
            cx.notify();
        }
        Err(reason) if reason == "vst3 parameter formatting unsupported" => {
            if let Some(panel) = view
                .device_panel
                .vst3_parameters
                .panels
                .get_mut(&query.device)
                .filter(|panel| panel.source == Some(query.source))
            {
                if panel.format_unsupported.len() >= MAX_UNSUPPORTED_FORMAT_ENTRIES {
                    panel.format_unsupported.clear();
                }
                panel.format_unsupported.insert(query.parameter);
            }
        }
        Err(_) => {}
    }
}

pub(in crate::editor) fn apply_result(
    view: &mut EditorView,
    result: PluginParameterResult,
    revision: u64,
    current_source: Option<AppliedAudioSource>,
    cx: &mut Context<EditorView>,
) {
    let Some((pending, source)) = view.device_panel.vst3_parameters.in_flight else {
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
    let device = pending.device;
    if current_source != Some(source)
        || view.device_panel.degraded_plugins.contains(device.value())
        || result.query.context.editor_revision != revision
        || Some(result.query.context) != current_context(view)
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
    panel.reconcile_live_values(source, &rows);
    if panel.descriptor_signature != result.descriptor_signature {
        panel.live_values.clear();
        panel.clear_format_caches();
    }
    let Some(descriptor_signature) = result.descriptor_signature else {
        panel.result = None;
        panel.source = None;
        panel.descriptor_signature = None;
        panel.total = None;
        panel.knobs.clear();
        panel.clear_format_caches();
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
        panel.clear_format_caches();
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
        panel.record_row_displays(&rows);
        let touched_preview = resolve_pending_touch(panel, source, &rows, None, touch_query);
        panel.result = Some(Arc::new(rows));
        panel.refilter(false);
        panel.cache_needs_save = true;
        if let Some((owner, row)) = touched_preview.or(preview) {
            view.remember_touched_vst3(owner, device, &row, cx);
        }
        return;
    }
    let Some(cached) = panel.rows() else {
        record_error(panel, "vst3 parameter cache is unavailable".into());
        panel.mark_values_stale();
        return;
    };
    let Some(first) = result.query.page_first else {
        return;
    };
    let Some(previous_page) = matching_page(
        cached,
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
    let page_changed = previous_page != rows.as_slice();
    let filter_changed = !panel.filter.is_empty()
        && previous_page
            .iter()
            .zip(&rows)
            .any(|(old, new)| old.name != new.name);
    let mut control_updates = Vec::new();
    for (previous, row) in previous_page.iter().zip(&rows) {
        if previous != row
            && let Some(origin) = parameter_origin(result.query, row)
        {
            let value = panel.display_value(row);
            let display = panel.display_for_value(row, value).map(str::to_owned);
            if let Some(knob) = panel.knobs.get(&row.id) {
                knob.origin.set(origin);
                control_updates.push((knob.control.clone(), row.steps, value, display.clone()));
            }
            if let Some((id, knob)) = panel.follow_knob.as_ref()
                && *id == row.id
            {
                knob.origin.set(origin);
                control_updates.push((knob.control.clone(), row.steps, value, display));
            }
        }
    }
    panel.record_row_displays(&rows);
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
    panel.cache_error = None;
    panel.retry_count = 0;
    if page_changed && let Some(cached) = panel.result.as_mut() {
        let first = usize::from(first);
        Arc::make_mut(cached)[first..first + rows.len()].clone_from_slice(&rows);
    }
    if filter_changed {
        panel.refilter(false);
    }
    let preview = panel.last_touched.and_then(|id| {
        panel
            .rows()?
            .iter()
            .find(|row| row.id == id && row.automation_class.is_some())
            .cloned()
            .map(|row| (panel.owner, row))
    });
    let touched_preview = resolve_pending_touch(panel, source, &rows, Some(first), touch_query);
    for (control, steps, value, display) in control_updates {
        control.set_external_state(steps, value, display.as_deref(), cx);
    }
    if let Some((owner, row)) = touched_preview.or(preview) {
        view.remember_touched_vst3(owner, device, &row, cx);
    }
}

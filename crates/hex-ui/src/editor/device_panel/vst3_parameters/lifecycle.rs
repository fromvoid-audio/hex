//! Hosted VST3 panel polling, reconciliation, and cache persistence.
use super::*;

// Called on the editor poll cadence. One query is outstanding globally because
// the native controller mailbox is shared by all hosted devices.
pub(in crate::editor) fn interactions_active(view: &EditorView, cx: &Context<EditorView>) -> bool {
    view.device_panel.gesture.snapshot().is_some()
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

pub(in crate::editor) fn record_refresh_failure(panel: &mut Panel, error: String) {
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

pub(in crate::editor) fn record_error(panel: &mut Panel, error: String) {
    panel.retry_count = panel.retry_count.saturating_add(1);
    panel.cache_error = Some(error);
    let exponent = u32::from(panel.retry_count.saturating_sub(1).min(5));
    panel.next_query_at = Instant::now() + Duration::from_secs(1_u64 << exponent);
}

pub(in crate::editor) fn report_panel_state(view: &mut EditorView) -> bool {
    let active_owner = view
        .device_panel
        .active
        .then_some(view.device_panel.owner)
        .flatten();
    let panels = active_owner
        .and_then(|owner| {
            let state = view.state.as_ref()?;
            Some(
                super::super::chain_ids(&state.project, owner)
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
    if let Some((result, revision)) = view.device_panel.vst3_parameters.conversion_result.take() {
        apply_conversion_result(view, result, revision, cx);
    }
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
    if view.audio_pub.stopped_playback_restore.is_some() || interactions_active(view, cx) {
        return;
    }
    let source = refresh_source(view);
    if let Some((result, revision)) = view.device_panel.vst3_parameters.deferred_result.take() {
        apply_result(view, result, revision, source, cx);
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
    let Some(source) = source else {
        return;
    };
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
                    && now >= panel.next_query_at
                    && refreshable_device(view, *device, source))
                .then_some((*device, panel.priority_page))
            });
    if let Some((device, page)) = pending_touch {
        submit_query(view, device, page, source, cx);
        return;
    }
    let Some(owner) = view.device_panel.owner.filter(|_| view.device_panel.active) else {
        return;
    };
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let active = super::super::chain_ids(&state.project, owner);
    let next = active.into_iter().find_map(|device| {
        let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
        if panel.owner != owner || panel.cache_loading || now < panel.next_query_at {
            return None;
        }
        if panel.retry_count > MAX_AUTOMATIC_RETRIES || !refreshable_device(view, device, source) {
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
        submit_query(view, device, page, source, cx);
    }
}

/// Dropping a cached control also drops its event subscription. Cancel only
/// the exact pointer edit it owns before relying on GPUI to release capture.
pub(in crate::editor) fn retire_panel(view: &mut EditorView, device: DeviceId) {
    let Some(panel) = view.device_panel.vst3_parameters.panels.remove(&device) else {
        return;
    };
    if !view.device_panel.gesture.has_pending_terminal()
        && let Some(active) = view.device_panel.gesture.snapshot()
        && panel
            .knobs
            .values()
            .chain(panel.follow_knob.iter().map(|(_, knob)| knob))
            .any(|knob| knob.owns(active.key))
    {
        let _ = view.cancel_device_gesture();
        view.pump_device_gesture();
    }
}

pub(in crate::editor) fn reconcile_panels(view: &mut EditorView, cx: &Context<EditorView>) {
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
                && super::super::chain_ids(&state.project, panel.owner).contains(device);
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
        retire_panel(view, *device);
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
            panel.mark_values_stale();
            panel.query.context = context;
            if let Some(output_identity) = output_identity {
                panel.query.output_identity = output_identity;
            }
            panel.pending_touch = pending_touch;
            panel.priority_page = touch_page;
        }
    }
    view.device_panel.vst3_parameters.reconciled = Some((context, output_identity));
}

pub(in crate::editor) fn submit_query(
    view: &mut EditorView,
    device: DeviceId,
    page_first: Option<u16>,
    source: AppliedAudioSource,
    cx: &mut Context<EditorView>,
) {
    let Some(context) = current_context(view) else {
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
        output_identity: source.output_identity,
        request_id,
        page_first,
    };
    if !view.enqueue(Request::QueryPluginParameters(query)) {
        return;
    }
    view.device_panel.vst3_parameters.serial = request_id;
    view.device_panel.vst3_parameters.in_flight = Some((query, source));
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

pub(in crate::editor) fn save_cache(
    panel: &mut Panel,
    device: DeviceId,
    generation: u64,
    cx: &mut Context<EditorView>,
) {
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

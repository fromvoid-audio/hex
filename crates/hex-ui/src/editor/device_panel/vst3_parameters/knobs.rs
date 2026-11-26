//! Hosted VST3 parameter knob construction and gesture event routing.
use super::*;

pub(in crate::editor) fn placeholder_knob(
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

pub(in crate::editor) fn set_refresh_availability(
    control: &mut NumericControl,
    row: &PluginParameterRow,
    cx: &mut Context<NumericControl>,
) {
    if row.metadata_revision == 0 || row.editable {
        control.suspend_input(cx);
    } else {
        control.set_enabled(false, cx);
    }
}

// A bounded set of display-only controls supports virtualization without giving
// cached descriptors the authority to dispatch parameter gestures.
pub(in crate::editor) fn row_placeholder(
    view: &mut EditorView,
    device: DeviceId,
    row: &PluginParameterRow,
    cx: &mut Context<EditorView>,
) -> Option<Entity<NumericControl>> {
    let active = view.device_panel.gesture.snapshot();
    let panel = view.device_panel.vst3_parameters.panels.get_mut(&device)?;
    if let Some(knob) = panel.knobs.get(&row.id)
        && knob.matches_descriptor(row)
    {
        let control = knob.control.clone();
        let value = if panel.source.is_none_or(|source| source == knob.source)
            && panel.query.output_identity == knob.output
        {
            knob.pending_value(active).unwrap_or(row.normalized)
        } else {
            row.normalized
        };
        control.update(cx, |control, cx| {
            let _ = control.set_external_value(value, cx);
            set_refresh_availability(control, row, cx);
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

pub(in crate::editor) fn knob(
    view: &mut EditorView,
    device: DeviceId,
    query: PluginParameterQuery,
    row: &PluginParameterRow,
    cx: &mut Context<EditorView>,
) -> Option<Entity<NumericControl>> {
    let enabled = !dense_target_owns_parameter(view, device, row.id);
    let active = view.device_panel.gesture.snapshot();
    let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
    if let Some(knob) = panel.knobs.get(&row.id)
        && knob.matches_source(panel.source?, query, row)
    {
        return knob.renew(
            query,
            row,
            (
                panel.painted_value(view, device, row),
                panel.display_text(row),
            ),
            enabled,
            active,
            cx,
        );
    }
    let knob = create_knob(view, device, query, row, "row", enabled, cx)?;
    let control = knob.control.clone();
    view.device_panel
        .vst3_parameters
        .panels
        .get_mut(&device)?
        .knobs
        .insert(row.id, knob);
    Some(control)
}

pub(in crate::editor) fn follow_knob(
    view: &mut EditorView,
    device: DeviceId,
    query: PluginParameterQuery,
    row: &PluginParameterRow,
    cx: &mut Context<EditorView>,
) -> Option<Entity<NumericControl>> {
    let active = view.device_panel.gesture.snapshot();
    let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
    let enabled = panel.pending_touch.is_none()
        && panel.queued_touch.is_none()
        && !dense_target_owns_parameter(view, device, row.id);
    if let Some((id, knob)) = panel.follow_knob.as_ref()
        && *id == row.id
        && knob.matches_source(panel.source?, query, row)
    {
        return knob.renew(
            query,
            row,
            (
                panel.painted_value(view, device, row),
                panel.display_text(row),
            ),
            enabled,
            active,
            cx,
        );
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

pub(in crate::editor) fn create_knob(
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
    let initial_value = panel.painted_value(view, device, row);
    let config = NumericControlConfig::new(
        control_id,
        theme::text(hex_session::service::text::verbatim(&row.name)),
        NumericControlPresentation::MicroDial,
        spec,
        initial_value,
        theme::control_style(),
    )
    .ok()?;
    let availability =
        ControlAvailability::for_edit(enabled, view.device_panel.gesture.snapshot().is_some());
    let initial_display = panel.display_text(row).to_owned();
    let has_current_display = initial_value == row.normalized;
    let control = cx.new(|cx| {
        let mut control = NumericControl::from_config(cx, config);
        control.set_deferred_text(true, cx);
        if has_current_display {
            let _ = control.set_external_display(initial_value, &initial_display, cx);
        }
        control.set_availability(availability, cx);
        control
    });
    let origin = Rc::new(Cell::new(origin));
    let event_origin = Rc::clone(&origin);
    let drag = Rc::new(RefCell::new(None));
    let event_drag = Rc::clone(&drag);
    let steps = row.steps;
    let is_follow = role == "follow";
    let text_origin = Rc::clone(&origin);
    let text_observation = cx.observe(&control, move |view, emitter, cx| {
        if let Some(request) = emitter.update(cx, |control, _| control.take_deferred_text_request())
        {
            submit_conversion(
                view,
                emitter,
                text_origin.get(),
                source,
                query.output_identity,
                request,
                cx,
            );
        }
    });
    let subscription = cx.subscribe(
        &control,
        move |view, emitter, event: &NumericControlEvent, cx| {
            let origin = event_origin.get();
            // Both pointer and typed edits must finish the exact gesture they
            // began, even if a redraw has refreshed the original page. A
            // different control or gesture cannot claim this continuation.
            let continuing_owned = event.phase != NumericEventPhase::Begin
                && event_drag.borrow().as_ref().is_some_and(|(gesture, key)| {
                    *gesture == event.gesture
                        && view
                            .device_panel
                            .gesture
                            .snapshot()
                            .is_some_and(|active| active.key == *key)
                });
            if !continuing_owned {
                if view.audio_pub.stopped_playback_restore.is_some()
                    || view.device_panel.gesture.snapshot().is_some()
                    || current_context(view) != Some(origin.context)
                {
                    return;
                }
                let Some(panel) = view.device_panel.vst3_parameters.panels.get(&device) else {
                    return;
                };
                let valid_row = panel
                    .rows()
                    .and_then(|rows| rows.iter().find(|row| row.id == origin.parameter))
                    .is_some_and(|row| {
                        row.editable
                            && row.metadata_revision != 0
                            && row.metadata_revision == origin.metadata_revision
                            && row.normalized == origin.initial.normalized()
                    });
                let current_control = panel
                    .knobs
                    .get(&origin.parameter)
                    .is_some_and(|knob| knob.control.entity_id() == emitter.entity_id())
                    || panel.follow_knob.as_ref().is_some_and(|(id, knob)| {
                        *id == origin.parameter && knob.control.entity_id() == emitter.entity_id()
                    });
                if panel.source != Some(source)
                    || panel.query.context != origin.context
                    || panel.query.output_identity != query.output_identity
                    || (is_follow
                        && (panel.pending_touch.is_some() || panel.queued_touch.is_some()))
                    || !valid_row
                    || !current_control
                {
                    return;
                }
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
        _text_observation: text_observation,
    })
}

pub(in crate::editor) type KnobDrag = RefCell<Option<(Option<NumericGestureId>, GestureKey)>>;

pub(in crate::editor) fn knob_event(
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
    if view.persistence.automation_close_preflight.frozen()
        && (event.phase == NumericEventPhase::Begin
            || (event.phase == NumericEventPhase::Commit && event.gesture.is_none()))
    {
        return Err("close is pending · automation recording is unavailable");
    }
    if event.phase == NumericEventPhase::Begin {
        let key = view.begin_vst3_parameter_gesture(origin)?;
        *drag = Some((event.gesture.clone(), key));
        view.pump_device_gesture();
        return Ok(());
    }
    if event.phase == NumericEventPhase::Commit
        && event.gesture.is_none()
        && view.device_panel.gesture.snapshot().is_none()
    {
        let key = view.begin_vst3_parameter_gesture(origin)?;
        *drag = Some((None, key));
        view.device_panel.gesture.finish_plugin(value)?;
        view.pump_device_gesture();
        return Ok(());
    }
    let Some((pointer, key)) = drag.as_ref() else {
        return Ok(());
    };
    if *pointer != event.gesture
        || view
            .device_panel
            .gesture
            .snapshot()
            .is_none_or(|active| active.key != *key)
    {
        return Ok(());
    }
    let result = match event.phase {
        NumericEventPhase::Change => view.device_panel.gesture.change_plugin(value).map(|_| ()),
        NumericEventPhase::Commit => view.device_panel.gesture.finish_plugin(value),
        NumericEventPhase::Cancel => view.device_panel.gesture.cancel(),
        NumericEventPhase::Begin => Ok(()),
    };
    if result.is_ok() {
        view.pump_device_gesture();
    }
    result
}

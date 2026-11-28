//! Hosted VST3 parameter control construction and gesture event routing.
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

pub(in crate::editor) fn parameter_spec(row: &PluginParameterRow) -> Option<NumericSpec> {
    if row.steps > 1 {
        let max = f64::from(row.steps);
        let default = control_value_from_normalized(row.steps, row.default_normalized);
        NumericSpec::new(0.0, max, default, NumericScale::Linear, 0, None::<&str>)
            .ok()?
            .with_integer_values()
            .ok()
    } else {
        NumericSpec::new(
            0.0,
            1.0,
            row.default_normalized,
            NumericScale::Linear,
            3,
            None::<&str>,
        )
        .ok()
    }
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
) -> Option<ParameterControl> {
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
        let display = panel.display_for_value(row, value).map(str::to_owned);
        control.set_external_state(row.steps, value, display.as_deref(), cx);
        control.set_refresh_availability(row, cx);
        return Some(control);
    }
    if let Some(placeholder) = panel.row_placeholders.get(&row.id)
        && placeholder.name == row.name
        && placeholder.default == row.default_normalized
        && placeholder.steps == row.steps
    {
        let control = placeholder.control.clone();
        let display = panel
            .display_for_value(row, row.normalized)
            .map(str::to_owned);
        control.set_external_state(row.steps, row.normalized, display.as_deref(), cx);
        return Some(control);
    }
    // The list also renders an item for measurement, so pruning by each
    // requested range would evict controls that are still on screen.
    if panel.row_placeholders.len() >= 128 {
        panel.row_placeholders.clear();
    }
    let display = panel
        .display_for_value(row, row.normalized)
        .map(str::to_owned);
    let control = if is_toggle_parameter(row.steps) {
        let checked = toggle_checked(row.normalized);
        let label = theme::text(hex_session::service::text::verbatim(toggle_label(
            checked,
            display.as_deref(),
        )));
        let config = ToggleConfig::new(
            format!("vst3-placeholder-{}-{}", device.value(), row.id),
            label,
            checked,
            theme::control_style(),
        )
        .ok()?
        .with_appearance(ToggleAppearance::CompactButton);
        let toggle = cx.new(|cx| Toggle::from_config(cx, config));
        toggle.update(cx, |toggle, cx| toggle.set_enabled(false, cx));
        ParameterControl::Toggle(toggle)
    } else {
        let spec = parameter_spec(row)?;
        let initial_value = control_value_from_normalized(row.steps, row.normalized);
        let config = NumericControlConfig::new(
            format!("vst3-placeholder-{}-{}", device.value(), row.id),
            theme::text(hex_session::service::text::verbatim(&row.name)),
            NumericControlPresentation::MicroDial,
            spec,
            initial_value,
            theme::control_style(),
        )
        .ok()?;
        let control = cx.new(|cx| {
            let mut control = NumericControl::from_config(cx, config);
            control.set_deferred_text(true, cx);
            if let Some(display) = display.as_deref()
                && !display.is_empty()
            {
                let _ = control.set_external_display(initial_value, display, cx);
            }
            control.set_enabled(false, cx);
            control
        });
        ParameterControl::Numeric(control)
    };
    panel.row_placeholders.insert(
        row.id,
        RowPlaceholder {
            name: row.name.clone(),
            default: row.default_normalized,
            steps: row.steps,
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
) -> Option<ParameterControl> {
    let enabled = !dense_target_owns_parameter(view, device, row.id);
    let active = view.device_panel.gesture.snapshot();
    let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
    if let Some(knob) = panel.knobs.get(&row.id)
        && knob.matches_source(panel.source?, query, row)
    {
        let painted = panel.painted_value(view, device, row);
        let display = panel.display_for_value(row, painted).map(str::to_owned);
        return knob.renew(
            query,
            row,
            (painted, display.as_deref()),
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
) -> Option<ParameterControl> {
    let active = view.device_panel.gesture.snapshot();
    let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
    let enabled = panel.pending_touch.is_none()
        && panel.queued_touch.is_none()
        && !dense_target_owns_parameter(view, device, row.id);
    if let Some((id, knob)) = panel.follow_knob.as_ref()
        && *id == row.id
        && knob.matches_source(panel.source?, query, row)
    {
        let painted = panel.painted_value(view, device, row);
        let display = panel.display_for_value(row, painted).map(str::to_owned);
        return knob.renew(
            query,
            row,
            (painted, display.as_deref()),
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

fn can_admit_control_gesture(
    view: &EditorView,
    device: DeviceId,
    origin: PluginParameterOrigin,
    source: hex_lattice_contract::ForeignProcessorBinding,
    output_identity: u64,
    emitter_id: gpui::EntityId,
    is_follow: bool,
) -> bool {
    if view.audio_pub.stopped_playback_restore.is_some()
        || view.device_panel.gesture.snapshot().is_some()
        || current_context(view) != Some(origin.context)
    {
        return false;
    }
    let Some(panel) = view.device_panel.vst3_parameters.panels.get(&device) else {
        return false;
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
        .is_some_and(|knob| knob.control.entity_id() == emitter_id)
        || panel.follow_knob.as_ref().is_some_and(|(id, knob)| {
            *id == origin.parameter && knob.control.entity_id() == emitter_id
        });
    panel.source == Some(source)
        && panel.query.context == origin.context
        && panel.query.output_identity == output_identity
        && (!is_follow || (panel.pending_touch.is_none() && panel.queued_touch.is_none()))
        && valid_row
        && current_control
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
    if is_toggle_parameter(row.steps) {
        create_toggle(view, device, query, row, role, enabled, cx)
    } else {
        create_numeric_knob(view, device, query, row, role, enabled, cx)
    }
}

fn create_toggle(
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
    let control_id = if role == "row" {
        format!("vst3-toggle-{}-{}", query.device.value(), row.id)
    } else {
        format!("vst3-follow-toggle-{}-{}", query.device.value(), row.id)
    };
    let initial_value = panel.painted_value(view, device, row);
    let checked = toggle_checked(initial_value);
    let initial_display = panel
        .display_for_value(row, initial_value)
        .map(str::to_owned);
    let label = theme::text(hex_session::service::text::verbatim(toggle_label(
        checked,
        initial_display.as_deref(),
    )));
    let config = ToggleConfig::new(control_id, label, checked, theme::control_style())
        .ok()?
        .with_appearance(ToggleAppearance::CompactButton);
    let availability =
        ControlAvailability::for_edit(enabled, view.device_panel.gesture.snapshot().is_some());
    let toggle = cx.new(|cx| {
        let mut toggle = Toggle::from_config(cx, config);
        toggle.set_availability(availability, cx);
        toggle
    });
    let origin = Rc::new(Cell::new(origin));
    let event_origin = Rc::clone(&origin);
    let drag = Rc::new(RefCell::new(None));
    let event_drag = Rc::clone(&drag);
    let is_follow = role == "follow";
    let subscription = cx.subscribe(&toggle, move |view, emitter, event: &ToggleChanged, cx| {
        let origin = event_origin.get();
        if !can_admit_control_gesture(
            view,
            device,
            origin,
            source,
            query.output_identity,
            emitter.entity_id(),
            is_follow,
        ) {
            emitter.update(cx, |toggle, cx| toggle.set_checked(!event.checked, cx));
            return;
        }
        let normalized = if event.checked { 1.0 } else { 0.0 };
        let next_display = view
            .device_panel
            .vst3_parameters
            .panels
            .get(&device)
            .and_then(|panel| panel.cached_step_label(origin.parameter, 1, normalized))
            .unwrap_or_else(|| toggle_label(event.checked, None))
            .to_owned();
        if let Err(reason) = toggle_commit(view, origin, normalized, &event_drag) {
            emitter.update(cx, |toggle, cx| toggle.set_checked(!event.checked, cx));
            view.error = Some(reason.into());
            cx.notify();
            return;
        }
        emitter.update(cx, |toggle, cx| {
            toggle.set_label(
                theme::text(hex_session::service::text::verbatim(&next_display)),
                cx,
            );
        });
        if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
            panel.drag_preview = Some(DragPreview {
                source,
                parameter: origin.parameter,
                normalized,
                display: next_display,
            });
        }
        queue_host_touch(view, device, origin.parameter, cx);
        view.pump_device_gesture();
        cx.notify();
    });
    Some(Knob {
        source,
        output: query.output_identity,
        name: row.name.clone(),
        default: row.default_normalized,
        steps: row.steps,
        control: ParameterControl::Toggle(toggle),
        origin,
        drag,
        _subscription: subscription,
        _text_observation: None,
    })
}

fn create_numeric_knob(
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
    let spec = parameter_spec(row)?;
    let control_id = if role == "row" {
        format!("vst3-knob-{}-{}", query.device.value(), row.id)
    } else {
        format!("vst3-follow-knob-{}-{}", query.device.value(), row.id)
    };
    let initial_normalized = panel.painted_value(view, device, row);
    let initial_value = control_value_from_normalized(row.steps, initial_normalized);
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
    let initial_display = panel
        .display_for_value(row, initial_normalized)
        .map(str::to_owned);
    let control = cx.new(|cx| {
        let mut control = NumericControl::from_config(cx, config);
        control.set_deferred_text(true, cx);
        if let Some(display) = initial_display.as_deref()
            && !display.is_empty()
        {
            let _ = control.set_external_display(initial_value, display, cx);
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
            if !continuing_owned
                && !can_admit_control_gesture(
                    view,
                    device,
                    origin,
                    source,
                    query.output_identity,
                    emitter.entity_id(),
                    is_follow,
                )
            {
                return;
            }
            if let Err(reason) = knob_event(view, origin, event, steps, &event_drag) {
                view.error = Some(reason.into());
            } else {
                update_live_drag_display(
                    view,
                    device,
                    source,
                    query.output_identity,
                    origin,
                    steps,
                    emitter,
                    event,
                    cx,
                );
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
        control: ParameterControl::Numeric(control),
        origin,
        drag,
        _subscription: subscription,
        _text_observation: Some(text_observation),
    })
}

#[allow(clippy::too_many_arguments)]
fn update_live_drag_display(
    view: &mut EditorView,
    device: DeviceId,
    source: hex_lattice_contract::ForeignProcessorBinding,
    output_identity: u64,
    origin: PluginParameterOrigin,
    steps: u32,
    emitter: Entity<NumericControl>,
    event: &NumericControlEvent,
    cx: &mut Context<EditorView>,
) {
    let normalized = normalized_from_control_value(steps, event.value);
    match event.phase {
        NumericEventPhase::Begin => {
            if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
                let display = panel
                    .current_drag_or_row_display(origin.parameter)
                    .unwrap_or("")
                    .to_owned();
                if !display.is_empty() {
                    panel.drag_preview = Some(DragPreview {
                        source,
                        parameter: origin.parameter,
                        normalized: origin.initial.normalized(),
                        display,
                    });
                }
            }
        }
        NumericEventPhase::Change => {
            let (cached, hold_display, unsupported) = {
                let Some(panel) = view.device_panel.vst3_parameters.panels.get(&device) else {
                    return;
                };
                (
                    panel
                        .cached_step_label(origin.parameter, steps, normalized)
                        .map(str::to_owned),
                    panel
                        .current_drag_or_row_display(origin.parameter)
                        .map(str::to_owned),
                    panel.format_unsupported.contains(&origin.parameter),
                )
            };
            if let Some(display) = cached {
                emitter.update(cx, |control, cx| {
                    let _ = control.set_external_display(event.value, &display, cx);
                });
                if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
                    panel.drag_preview = Some(DragPreview {
                        source,
                        parameter: origin.parameter,
                        normalized,
                        display,
                    });
                }
            } else if !unsupported {
                // Hold the last plugin-formatted text while the read-only Format
                // query completes (~1–5ms) so raw normalized floats never flash.
                if let Some(display) = hold_display {
                    emitter.update(cx, |control, cx| {
                        let _ = control.set_external_display(event.value, &display, cx);
                    });
                    if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
                        panel.drag_preview = Some(DragPreview {
                            source,
                            parameter: origin.parameter,
                            normalized,
                            display,
                        });
                    }
                }
                request_live_format(
                    view,
                    device,
                    source,
                    output_identity,
                    origin.context,
                    origin.parameter,
                    steps,
                    normalized,
                    cx,
                );
            } else if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
                panel.drag_preview = None;
            }
        }
        NumericEventPhase::Commit => {
            if event.gesture.is_none()
                && let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device)
                && let Some(display) = panel
                    .cached_step_label(origin.parameter, steps, normalized)
                    .map(str::to_owned)
            {
                emitter.update(cx, |control, cx| {
                    let _ = control.set_external_display(event.value, &display, cx);
                });
                panel.drag_preview = Some(DragPreview {
                    source,
                    parameter: origin.parameter,
                    normalized,
                    display,
                });
            }
            if view
                .device_panel
                .vst3_parameters
                .queued_format
                .as_ref()
                .is_some_and(|queued| {
                    queued.device == device && queued.parameter == origin.parameter
                })
            {
                view.device_panel.vst3_parameters.queued_format = None;
            }
        }
        NumericEventPhase::Cancel => {
            if view
                .device_panel
                .vst3_parameters
                .queued_format
                .as_ref()
                .is_some_and(|queued| {
                    queued.device == device && queued.parameter == origin.parameter
                })
            {
                view.device_panel.vst3_parameters.queued_format = None;
            }
            let restored = view
                .device_panel
                .vst3_parameters
                .panels
                .get_mut(&device)
                .and_then(|panel| {
                    panel.drag_preview = None;
                    let row = panel
                        .rows()?
                        .iter()
                        .find(|row| row.id == origin.parameter)?;
                    Some((
                        control_value_from_normalized(steps, row.normalized),
                        row.value.clone(),
                    ))
                });
            if let Some((restored_value, restored_display)) = restored
                && !restored_display.is_empty()
            {
                emitter.update(cx, |control, cx| {
                    let _ = control.set_external_display(restored_value, &restored_display, cx);
                });
            }
        }
    }
}

pub(in crate::editor) type KnobDrag = RefCell<Option<(Option<NumericGestureId>, GestureKey)>>;

fn toggle_commit(
    view: &mut EditorView,
    origin: PluginParameterOrigin,
    normalized: f64,
    drag: &KnobDrag,
) -> Result<(), &'static str> {
    let value = PluginParameterValue::new(normalized)?;
    if view.persistence.automation_close_preflight.frozen() {
        return Err("close is pending · automation recording is unavailable");
    }
    let key = view.begin_vst3_parameter_gesture(origin)?;
    *drag.borrow_mut() = Some((None, key));
    view.device_panel.gesture.finish_plugin(value)?;
    view.pump_device_gesture();
    Ok(())
}

pub(in crate::editor) fn knob_event(
    view: &mut EditorView,
    origin: PluginParameterOrigin,
    event: &NumericControlEvent,
    steps: u32,
    drag: &KnobDrag,
) -> Result<(), &'static str> {
    let normalized = normalized_from_control_value(steps, event.value);
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

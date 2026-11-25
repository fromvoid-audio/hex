//! Hosted VST3 parameter panel rendering and automation lane menus.
use super::*;

pub(in crate::editor) fn body(
    view: &mut EditorView,
    device: DeviceId,
    cx: &mut Context<EditorView>,
) -> Div {
    ensure_panel(view, device, cx);
    render_body(view, device, cx)
}

/// Present an existing inventory during an applied-source handoff.
pub(in crate::editor) fn pending_body(
    view: &mut EditorView,
    device: DeviceId,
    cx: &mut Context<EditorView>,
) -> Option<Div> {
    view.device_panel
        .vst3_parameters
        .panels
        .get(&device)?
        .rows()?;
    ensure_panel(view, device, cx);
    let panel = view.device_panel.vst3_parameters.panels.get_mut(&device)?;
    if panel.query.context.project_generation != view.project_generation
        || Some(panel.owner) != view.device_panel.owner
        || panel.rows().is_none()
    {
        return None;
    }
    if panel.source.is_some() {
        panel.mark_values_stale();
    }
    Some(render_body(view, device, cx))
}

pub(in crate::editor) fn row_automated(
    device: Option<&hex_project::Device>,
    row: &PluginParameterRow,
) -> bool {
    let Some(class) = row.automation_class else {
        return false;
    };
    let target = hex_lattice_contract::DeviceAutomationTarget::Vst3 { parameter: row.id };
    automation_tint::authored(device, target).is_some_and(|lane| lane.vst3_class == Some(class))
}

pub(in crate::editor) fn render_body(
    view: &mut EditorView,
    device: DeviceId,
    cx: &mut Context<EditorView>,
) -> Div {
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
    let project = view.state.as_ref().map(|state| state.project.clone());
    let (follow, follow_row, _) = follow_display(view, device, cx);
    let follow_automated = follow_row.as_ref().is_some_and(|row| {
        row_automated(
            project.as_ref().and_then(|project| project.device(device)),
            row,
        )
    });
    let follow_mapping = follow_row.as_ref().and_then(|row| {
        super::super::modulators::vst3_target_overlay(view, cx, device, row, "follow")
    });
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
    let value = row.map(|row| panel.display_text(row)).unwrap_or("");
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
    let follow_target = follow
        .as_ref()
        .and(row)
        .filter(|_| panel.pending_touch.is_none() && panel.queued_touch.is_none())
        .map(|row| (panel.query, row.id));
    let control = follow.or_else(|| panel.placeholder_knob.clone());
    let control = control.map_or_else(
        || {
            div()
                .w(px(hex_widgets::MICRO_DIAL_WIDTH))
                .h(px(hex_widgets::MICRO_DIAL_HEIGHT))
                .flex_shrink_0()
        },
        |control| {
            control.update(cx, |control, cx| {
                let _ = control.set_external_style(
                    automation_tint::style(theme::control_style(), follow_automated),
                    cx,
                );
            });
            div()
                .debug_selector(|| "vst3-last-touched-knob".to_string())
                .w(px(hex_widgets::MICRO_DIAL_WIDTH))
                .h(px(hex_widgets::MICRO_DIAL_HEIGHT))
                .flex_shrink_0()
                .child(control)
                .when(follow_automated, automation_tint::mark)
                .when_some(follow_target, |item, (query, parameter)| {
                    item.capture_any_mouse_down(cx.listener(
                        move |view, event: &gpui::MouseDownEvent, _, cx| {
                            if event.button == gpui::MouseButton::Left {
                                select_host_automation_lane(view, query, parameter, cx);
                            }
                        },
                    ))
                })
        },
    );
    let controls = div()
        .debug_selector(|| "vst3-last-touch-row".to_string())
        .relative()
        .w(px(80.0 + 8.0 + hex_widgets::MICRO_DIAL_WIDTH))
        .h(px(hex_widgets::MICRO_DIAL_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .child(details)
        .child(control)
        .children(follow_mapping);
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
                                let firsts: BTreeSet<_> = range
                                    .clone()
                                    .filter_map(|index| matched.get(index))
                                    .map(|row_index| ((row_index / PAGE_SIZE) * PAGE_SIZE) as u16)
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
                                let canonical = project.as_ref().and_then(|project| project.device(device));
                                range
                                    .filter_map(|index| {
                                        let row_index = *matched.get(index)?;
                                        let row = rows.get(row_index)?;
                                        let automated = row_automated(canonical, row);
                                        let (knob, mapping) = editor
                                            .update(app, |view, cx| {
                                                let knob = if row.editable && !restoring {
                                                    if let Some(panel) = view
                                                        .device_panel
                                                        .vst3_parameters
                                                        .panels
                                                        .get_mut(&device)
                                                    {
                                                        panel.row_placeholders.remove(&row.id);
                                                    }
                                                    knob(view, device, query, row, cx)
                                                } else {
                                                    row_placeholder(view, device, row, cx)
                                                };
                                                if let Some(knob) = &knob {
                                                    knob.update(cx, |control, cx| {
                                                        let _ = control.set_external_style(
                                                            automation_tint::style(theme::control_style(), automated),
                                                            cx,
                                                        );
                                                    });
                                                }
                                                let mapping = super::super::modulators::vst3_target_overlay(
                                                    view, cx, device, row, "row",
                                                );
                                                (knob, mapping)
                                            })
                                            .unwrap_or_default();
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
                                                    .relative()
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
                                                    .children(mapping)
                                                    .when(automated, automation_tint::mark)
                                                    .when(row.automation_class.is_some(), |item| {
                                                        let press_editor = row_editor.clone();
                                                        item.capture_any_mouse_down(move |event, _, app| {
                                                            if event.button == gpui::MouseButton::Left {
                                                                let _ = press_editor.update(app, |view, cx| {
                                                                    select_host_automation_lane(view, query, row_id, cx);
                                                                });
                                                            }
                                                        }).on_mouse_down(
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

pub(in crate::editor) fn follow_display(
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
    let active = view.device_panel.gesture.snapshot();
    if let Some((id, knob)) = panel.follow_knob.as_ref()
        && knob.control.read(cx).is_interacting()
        && active.is_none_or(|gesture| knob.owns(gesture.key))
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
    let restoring = view.stopped_playback_restore.is_some();
    if restoring || !row.editable || row.metadata_revision == 0 || panel.source.is_none() {
        let pending = panel.follow_knob.as_ref().and_then(|(current, knob)| {
            (*current == id
                && panel.source.is_none_or(|source| source == knob.source)
                && panel.query.output_identity == knob.output)
                .then(|| knob.pending_value(active))
                .flatten()
        });
        let control = panel
            .follow_knob
            .as_ref()
            .filter(|(current, knob)| *current == id && knob.matches_descriptor(&row))
            .map(|(_, knob)| knob.control.clone());
        if let Some(control) = &control {
            control.update(cx, |control, cx| {
                let _ = control.set_external_value(pending.unwrap_or(row.normalized), cx);
                set_refresh_availability(control, &row, cx);
            });
        }
        let status = if restoring || row.metadata_revision == 0 || panel.source.is_none() {
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

pub(in crate::editor) fn select_host_automation_lane(
    view: &mut EditorView,
    query: PluginParameterQuery,
    parameter: u32,
    cx: &mut Context<EditorView>,
) {
    if view.stopped_playback_restore.is_some() || current_context(view) != Some(query.context) {
        return;
    }
    let Some(panel) = view.device_panel.vst3_parameters.panels.get(&query.device) else {
        return;
    };
    if Some(panel.owner) != view.device_panel.owner
        || panel.query.context != query.context
        || panel.query.output_identity != query.output_identity
        || !panel.source.is_some_and(|source| source.is_valid())
    {
        return;
    }
    let Some(row) = panel.rows().and_then(|rows| {
        rows.iter().find(|row| {
            row.id == parameter
                && row.editable
                && row.metadata_revision != 0
                && row.automation_class.is_some()
        })
    }) else {
        return;
    };
    // A copied row can select the lane, while parameter edits still require
    // the active host gesture and applied source.
    let owner = panel.owner;
    let row = row.clone();
    view.select_vst3_automation_parameter(owner, query.device, &row, cx);
}

pub(in crate::editor) fn show_automation_menu(
    view: &mut EditorView,
    device: DeviceId,
    query: PluginParameterQuery,
    id: u32,
    position: gpui::Point<gpui::Pixels>,
    window: &mut gpui::Window,
    cx: &mut Context<EditorView>,
) -> bool {
    if view.stopped_playback_restore.is_some()
        || current_context(view) != Some(query.context)
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

/// Freeze only the exact accepted gesture's value as display-only while its
/// controller page is refreshed. The old source grants no new edit authority.
pub(in crate::editor) fn refresh_after_gesture(
    view: &mut EditorView,
    committed: Option<hex_session::device_gesture::GestureSnapshot>,
) {
    use hex_session::device_gesture::GestureTarget;
    for (device, panel) in &mut view.device_panel.vst3_parameters.panels {
        let preview = committed.and_then(|snapshot| {
            let GestureTarget::PluginParameter {
                device: target,
                parameter,
            } = snapshot.target
            else {
                return None;
            };
            if *device != target
                || panel.query.context.project_generation != snapshot.key.generation
            {
                return None;
            }
            let owns = |knob: &Knob| {
                panel.source.is_none_or(|source| source == knob.source)
                    && panel.query.output_identity == knob.output
                    && knob.owns(snapshot.key)
            };
            let current = panel.knobs.get(&parameter).is_some_and(owns)
                || panel
                    .follow_knob
                    .as_ref()
                    .is_some_and(|(id, knob)| *id == parameter && owns(knob));
            current.then_some((parameter, snapshot.plugin_value?.normalized()))
        });
        panel.mark_values_stale_with_commit(preview);
    }
}

#[cfg(feature = "ui-test-support")]
pub(in crate::editor) fn test_state(view: &EditorView) -> (Vec<(u64, u32, gpui::EntityId)>, bool) {
    let state = &view.device_panel.vst3_parameters;
    let mut controls = state
        .panels
        .iter()
        .flat_map(|(device, panel)| {
            panel.knobs.iter().filter_map(|(parameter, knob)| {
                let row = panel
                    .rows()?
                    .iter()
                    .find(|row| row.id == *parameter && row.editable)?;
                knob.matches_source(panel.source?, panel.query, row)
                    .then_some((device.value(), *parameter, knob.control.entity_id()))
            })
        })
        .collect::<Vec<_>>();
    controls.sort_by_key(|(device, parameter, _)| (*device, *parameter));
    (controls, query_pending(view))
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

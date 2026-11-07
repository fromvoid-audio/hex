//! One view-local inventory projection. Session and the desktop retain the
//! original source and all controller query custody.
use super::*;
use gpui::{Entity, Subscription, UniformListScrollHandle, uniform_list};
use hex_engine::AudioSourceContext;
use hex_session::device_gesture::{GestureKey, PluginParameterOrigin, PluginParameterValue};
use hex_session::service::{PluginParameterQuery, PluginParameterResult, PluginParameterRow};
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlPresentation,
    NumericEventPhase, NumericGestureId, NumericScale, NumericSpec, TextInput,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    panel: Option<Panel>,
    refresh: bool,
}
struct Panel {
    query: PluginParameterQuery,
    search: Entity<TextInput>,
    _observe_search: Subscription,
    scroll: UniformListScrollHandle,
    result: Option<Result<Arc<Vec<PluginParameterRow>>, String>>,
    matched: Arc<Vec<usize>>,
    filter: String,
    knobs: HashMap<u32, Knob>,
}
struct Knob {
    control: Entity<NumericControl>,
    _subscription: Subscription,
}

impl Panel {
    fn refilter(&mut self) {
        let Some(Ok(rows)) = &self.result else { return };
        self.matched = Arc::new(matching_rows(rows, &self.filter));
        self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
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

fn current_context(view: &EditorView) -> Option<AudioSourceContext> {
    Some(AudioSourceContext {
        project_generation: view.project_generation,
        editor_revision: view.state.as_ref()?.revision,
    })
}

fn open(view: &mut EditorView, device: DeviceId, cx: &mut Context<EditorView>) {
    if view.stopped_playback_restore.is_some() {
        return;
    }
    let Some(context) = current_context(view) else {
        return;
    };
    let Some(request_id) = view.device_panel.vst3_parameters.serial.checked_add(1) else {
        return;
    };
    let Some(output_identity) = view
        .audio
        .as_ref()
        .and_then(|audio| audio.control.output_identity())
    else {
        return;
    };
    let query = PluginParameterQuery {
        context,
        device,
        output_identity,
        request_id,
    };
    if !view.enqueue(Request::QueryPluginParameters(query)) {
        return;
    }
    view.device_panel.vst3_parameters.serial = request_id;
    if let Some(panel) = view.device_panel.vst3_parameters.panel.as_mut()
        && panel.query.device == device
        && panel.query.context.project_generation == context.project_generation
    {
        panel.query = query;
        panel.result = None;
        panel.knobs.clear();
        view.device_panel.vst3_parameters.refresh = false;
        return;
    }
    let search = cx.new(super::super::browser_input::plugin_parameter_search);
    let observe_search = cx.observe(&search, |view, _, cx| {
        if let Some(panel) = view.device_panel.vst3_parameters.panel.as_mut() {
            panel.filter = panel.search.read(cx).text().trim().to_lowercase();
            panel.refilter();
        }
        cx.notify();
    });
    view.device_panel.vst3_parameters.panel = Some(Panel {
        query,
        search,
        _observe_search: observe_search,
        scroll: UniformListScrollHandle::new(),
        result: None,
        matched: Arc::default(),
        filter: String::new(),
        knobs: HashMap::new(),
    });
}

pub(in crate::editor) fn install_result(
    view: &mut EditorView,
    result: PluginParameterResult,
    revision: u64,
) {
    if result.query.context.project_generation != view.project_generation
        || result.query.context.editor_revision != revision
        || view
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            != Some(result.query.output_identity)
    {
        return;
    }
    let Some(panel) = view
        .device_panel
        .vst3_parameters
        .panel
        .as_mut()
        .filter(|panel| panel.query == result.query)
    else {
        return;
    };
    panel.result = Some(result.rows.map(Arc::new));
    panel.refilter();
}

pub(super) fn body(view: &mut EditorView, device: DeviceId, cx: &mut Context<EditorView>) -> Div {
    let context = current_context(view);
    let output = view
        .audio
        .as_ref()
        .and_then(|audio| audio.control.output_identity());
    if view
        .device_panel
        .vst3_parameters
        .panel
        .as_ref()
        .is_some_and(|panel| {
            Some(panel.query.output_identity) != output
                || panel.query.context.project_generation != view.project_generation
        })
    {
        view.device_panel.vst3_parameters.panel = None;
    }
    if view
        .device_panel
        .vst3_parameters
        .panel
        .as_ref()
        .is_some_and(|panel| {
            panel.query.device == device
                && (Some(panel.query.context) != context
                    || view.device_panel.vst3_parameters.refresh)
        })
    {
        open(view, device, cx);
    }
    let state = &view.device_panel.vst3_parameters;
    let panel = state
        .panel
        .as_ref()
        .filter(|panel| panel.query.device == device);
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
        return body.child(
            div()
                .id(("vst3-parameters", device.value()))
                .debug_selector(move || format!("vst3-parameters-{}", device.value()))
                .cursor_pointer()
                .text_xs()
                .child(theme::text("parameters"))
                .on_click(cx.listener(move |view, _, _, cx| {
                    open(view, device, cx);
                    cx.notify();
                })),
        );
    };
    let body = body.child(panel.search.clone());
    match &panel.result {
        None => body.child(theme::text("reading vst3 parameters…")),
        Some(Err(error)) => body.child(theme::text(error)).child(
            div()
                .id(("retry-vst3-parameters", device.value()))
                .debug_selector(|| "vst3-controls-retry".to_string())
                .cursor_pointer()
                .child(theme::text("retry"))
                .on_click(cx.listener(move |view, _, _, cx| {
                    open(view, device, cx);
                    cx.notify();
                })),
        ),
        Some(Ok(rows)) if rows.is_empty() => body.child(theme::text("no parameters reported")),
        Some(Ok(_)) if panel.matched.is_empty() => {
            body.child(theme::text("no matching parameters"))
        }
        Some(Ok(rows)) => {
            let rows = Arc::clone(rows);
            let matched = Arc::clone(&panel.matched);
            let query = panel.query;
            let editor = cx.weak_entity();
            body.child(
                uniform_list(
                    ("vst3-parameter-list", device.value()),
                    matched.len(),
                    move |range, _, app| {
                        range
                            .filter_map(|index| {
                                let row = rows.get(*matched.get(index)?)?;
                                let knob = if row.editable {
                                    editor
                                        .update(app, |view, cx| knob(view, query, row, cx))
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
                                        .debug_selector({
                                            let id = row.id;
                                            move || format!("vst3-parameter-row-{id}")
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
                .track_scroll(panel.scroll.clone())
                .flex_1()
                .min_h_0()
                .w_full(),
            )
        }
    }
}

fn show_automation_menu(
    view: &mut EditorView,
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
        .panel
        .as_ref()
        .filter(|panel| panel.query == query)
        .and_then(|panel| panel.result.as_ref()?.as_ref().ok())
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.id == id && row.automation_class.is_some())
        })
        .cloned();
    row.is_some_and(|row| view.open_vst3_automation_menu(position, query.device, &row, window, cx))
}

pub(in crate::editor) fn refresh(view: &mut EditorView) {
    view.device_panel.vst3_parameters.refresh = true;
}

fn knob(
    view: &mut EditorView,
    query: PluginParameterQuery,
    row: &PluginParameterRow,
    cx: &mut Context<EditorView>,
) -> Option<Entity<NumericControl>> {
    let panel = view
        .device_panel
        .vst3_parameters
        .panel
        .as_mut()
        .filter(|panel| panel.query == query)?;
    if let Some(knob) = panel.knobs.get(&row.id) {
        return Some(knob.control.clone());
    }
    let origin = PluginParameterOrigin {
        context: query.context,
        device: query.device,
        parameter: row.id,
        page_first: row.page_first,
        metadata_revision: row.metadata_revision,
        initial: PluginParameterValue::new(row.normalized).ok()?,
    };
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
    let drag = Rc::new(RefCell::new(None));
    let steps = row.steps;
    let subscription = cx.subscribe(
        &control,
        move |view, emitter, event: &NumericControlEvent, cx| {
            let current = view
                .device_panel
                .vst3_parameters
                .panel
                .as_ref()
                .filter(|panel| panel.query == query)
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
    fn matches_named_rows_across_the_inventory_without_renumbering_ids() {
        let rows = ["oscillator", "filter cutoff", "filter resonance", "level"]
            .into_iter()
            .enumerate()
            .map(|(i, name)| PluginParameterRow {
                id: 100 + 3 * i as u32,
                name: name.into(),
                value: "25%".into(),
                normalized: 0.25,
                default_normalized: 0.25,
                steps: 0,
                editable: true,
                automation_class: None,
                page_first: 0,
                metadata_revision: 1,
            })
            .collect::<Vec<_>>();
        let found = matching_rows(&rows, "filter");
        assert_eq!(
            found.iter().map(|&i| rows[i].id).collect::<Vec<_>>(),
            [103, 106]
        );
        assert!(matching_rows(&rows, "delay").is_empty());
    }
}

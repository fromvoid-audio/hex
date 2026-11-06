//! One view-local inventory projection. Session and the desktop retain the
//! original source and all controller query custody.
use super::*;
use gpui::{Entity, Subscription, UniformListScrollHandle, uniform_list};
use hex_engine::AudioSourceContext;
use hex_session::service::{PluginParameterQuery, PluginParameterResult, PluginParameterRow};
use hex_widgets::TextInput;

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    panel: Option<Panel>,
}
struct Panel {
    query: PluginParameterQuery,
    search: Entity<TextInput>,
    _observe_search: Subscription,
    scroll: UniformListScrollHandle,
    result: Option<Result<Arc<Vec<PluginParameterRow>>, String>>,
    matched: Arc<Vec<usize>>,
    filter: String,
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
    let search = cx.new(super::super::browser_input::plugin_parameter_search);
    let observe_search = cx.observe(&search, |view, _, cx| {
        if let Some(panel) = view.device_panel.vst3_parameters.panel.as_mut() {
            panel.filter = panel.search.read(cx).text().trim().to_lowercase();
            panel.refilter();
        }
        cx.notify();
    });
    view.device_panel.vst3_parameters.serial = request_id;
    view.device_panel.vst3_parameters.panel = Some(Panel {
        query,
        search,
        _observe_search: observe_search,
        scroll: UniformListScrollHandle::new(),
        result: None,
        matched: Arc::default(),
        filter: String::new(),
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
    let state = &mut view.device_panel.vst3_parameters;
    if state.panel.as_ref().is_some_and(|panel| {
        Some(panel.query.context) != context || Some(panel.query.output_identity) != output
    }) {
        state.panel = None;
    }
    let panel = state
        .panel
        .as_ref()
        .filter(|panel| panel.query.device == device);
    let body = crate::device_composition::body()
        .w(px(crate::device_composition::GENERIC_FRAME_WIDTH))
        .flex_col()
        .min_w_0();
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
            body.child(
                uniform_list(
                    ("vst3-parameter-list", device.value()),
                    matched.len(),
                    move |range, _, _| {
                        range
                            .filter_map(|index| {
                                let row = rows.get(*matched.get(index)?)?;
                                Some(
                                    div()
                                        .id(("vst3-parameter", row.id))
                                        .debug_selector({
                                            let id = row.id;
                                            move || format!("vst3-parameter-row-{id}")
                                        })
                                        .h(px(22.0))
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
                                        .child(div().max_w(px(92.0)).truncate().child(
                                            theme::text(hex_session::service::text::verbatim(
                                                &row.value,
                                            )),
                                        )),
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

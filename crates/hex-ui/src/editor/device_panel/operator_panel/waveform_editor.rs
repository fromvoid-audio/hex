//! Optional native waveform workspace; canonical partial edits remain in harmonics.
use super::*;

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    operator: usize,
    parameters: &[NativeDeviceParameterView],
) -> Div {
    let (graph, waveform) = harmonics::graph(view, cx, device, operator, parameters);
    let harmonic_range_control = harmonics::ranges(view, cx, device, operator, parameters);
    let graph = graph.flex_1().min_w_0();
    hex_widgets::device_display(super::super::device_layout_style())
        .debug_selector(move || format!("operator-waveform-editor-{}", device.value()))
        .w(px(crate::device_composition::VECTOR_WAVEFORM_WIDTH))
        .h_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .p(px(2.0))
        .gap(px(2.0))
        .child(
            detail_caption(["a waveform", "b waveform", "c waveform", "d waveform"][operator])
                .h(px(12.0)),
        )
        .child(
            div()
                .id("operator-waveform-cycle")
                .debug_selector(|| "operator-waveform-cycle".into())
                .w_full()
                .h(px(48.0))
                .flex_shrink_0()
                .child(waveform),
        )
        .child(
            div()
                .w_full()
                .flex_1()
                .min_h_0()
                .flex()
                .gap(px(2.0))
                .child(graph)
                .child(harmonic_range_control),
        )
}

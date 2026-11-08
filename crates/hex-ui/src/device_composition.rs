//! Production device geometry shared by the editor and disposable native galleries.
//! Slots contain already-bound controls. This module owns no entities, editing,
//! devices, project state or audio work, and selects no device-specific colors.
use crate::theme::{self, DeviceStyleRole};
use gpui::{Div, IntoElement, SharedString, div, prelude::*, px, rgb};
use hex_widgets::{DeviceLayout, DeviceSectionSurface, device_compact_section, device_layout};

/// Every native device uses one 200 px host, including its left header and outline.
pub(crate) const FRAME_HEIGHT: f32 = 200.0;
pub(crate) const BODY_HEIGHT: f32 = FRAME_HEIGHT - 2.0 * hex_widgets::DEVICE_FRAME_BORDER_WIDTH;
pub(crate) const GENERIC_FRAME_WIDTH: f32 = 286.0;
pub(crate) const SAMPLER_FRAME_WIDTH: f32 = 1040.0;
pub(crate) const UTILITY_FRAME_WIDTH: f32 = 144.0;
pub(crate) const SATURATOR_FRAME_WIDTH: f32 = 230.0;
pub(crate) const EQ8_FRAME_WIDTH: f32 = 600.0;
pub(crate) const DELAY_FRAME_WIDTH: f32 = 500.0;
pub(crate) const LIMITER_FRAME_WIDTH: f32 = 300.0;
pub(crate) const COMPRESSOR_FRAME_WIDTH: f32 = 326.0;
pub(crate) const OVERDRIVE_FRAME_WIDTH: f32 = 174.0;
pub(crate) const GATE_FRAME_WIDTH: f32 = 142.0;
pub(crate) const REVERB_FRAME_WIDTH: f32 = 608.0;
pub(crate) const AUTO_PAN_FRAME_WIDTH: f32 = 144.0;
pub(crate) const VECTOR_SHELL_WIDTH: f32 = 174.0;
pub(crate) const VECTOR_DETAIL_WIDTH: f32 = 340.0;
pub(crate) const VECTOR_PREVIEW_WIDTH: f32 = 2.0 * hex_widgets::MICRO_DIAL_WIDTH + 148.0;
pub(crate) const VECTOR_WIDTH: f32 =
    VECTOR_SHELL_WIDTH + VECTOR_DETAIL_WIDTH + VECTOR_PREVIEW_WIDTH + 30.0;

pub(crate) fn body() -> Div {
    hex_widgets::device_body(theme::device_style(DeviceStyleRole::Layout))
        .w_full()
        .h(px(BODY_HEIGHT))
        .min_h_0()
}

/// Registered devices without a specialized arrangement use this same surface.
pub(crate) fn wrapped_body(controls: impl IntoIterator<Item = Div>) -> Div {
    body()
        .flex_wrap()
        .content_start()
        .gap(px(hex_widgets::DEVICE_ROW_GUTTER))
        .children(controls.into_iter().map(|control| {
            hex_widgets::device_dense_section(
                theme::device_style(DeviceStyleRole::Layout),
                DeviceSectionSurface::Rail,
            )
            .flex_shrink_0()
            .child(control)
        }))
}

/// Shared windowed generic body; the widget owns its surface/corner inset.
pub(crate) fn windowed_body(content: impl IntoElement) -> Div {
    hex_widgets::device_windowed_body(theme::device_style(DeviceStyleRole::Layout), content)
        .w_full()
        .h(px(BODY_HEIGHT))
        .min_h_0()
}

/// One unsupported-state presentation for instruments, Track and Return effects.
/// The outer caller supplies its device ID; the nested scroll ID is scoped by it.
pub(crate) fn unsupported_body(message: SharedString) -> Div {
    let style = theme::device_style(DeviceStyleRole::ErrorDisplay);
    body().child(
        device_compact_section(style, DeviceSectionSurface::Display)
            .id("device-unsupported-content")
            .w_full()
            .h_full()
            .overflow_y_scroll()
            .p(px(5.0))
            .text_xs()
            .text_color(rgb(style.text))
            .child(theme::text(message)),
    )
}

pub(crate) fn rail_column() -> Div {
    device_layout(DeviceLayout::Column).gap(px(hex_widgets::DEVICE_ROW_GUTTER))
}

pub(crate) fn compact_rail(content: impl IntoElement) -> Div {
    device_compact_section(
        theme::device_style(DeviceStyleRole::Layout),
        DeviceSectionSurface::Rail,
    )
    .h_full()
    .child(content)
}

pub(crate) fn dense_rail(content: impl IntoElement) -> Div {
    hex_widgets::device_dense_section(
        theme::device_style(DeviceStyleRole::Layout),
        DeviceSectionSurface::Rail,
    )
    .h_full()
    .child(content)
}

pub(crate) fn utility(input: impl IntoElement, output: impl IntoElement) -> Div {
    body()
        .relative()
        .w(px(140.0))
        .overflow_hidden()
        .gap(px(2.0))
        .py(px(2.0))
        .child(
            compact_rail(input)
                .debug_selector(|| "device-input-section".into())
                .w(px(64.0))
                .flex_shrink_0(),
        )
        .child(hex_widgets::device_divider(theme::device_style(
            DeviceStyleRole::Layout,
        )))
        .child(
            compact_rail(output)
                .debug_selector(|| "device-output-section".into())
                .w(px(64.0))
                .flex_shrink_0(),
        )
}

/// A graph receives one rounded boundary. Vertical insets preserve its width.
pub(crate) fn plot(content: impl IntoElement) -> Div {
    device_compact_section(
        theme::device_style(DeviceStyleRole::Layout),
        DeviceSectionSurface::Display,
    )
    .debug_selector(|| "device-plot-section".into())
    .w_full()
    .flex_1()
    .min_h_0()
    .flex()
    .flex_col()
    .child(content)
}

pub(crate) fn control_row() -> Div {
    hex_widgets::device_section(
        theme::device_style(DeviceStyleRole::Layout),
        DeviceSectionSurface::Rail,
    )
    .w_full()
    .flex()
    .items_center()
}

pub(crate) fn stacked_plot(
    chooser: impl IntoElement,
    graph: impl IntoElement,
    range: impl IntoElement,
    controls: impl IntoElement,
) -> Div {
    body()
        .flex_col()
        .gap(px(2.0))
        .py(px(3.0))
        .child(div().h(px(14.0)).flex_shrink_0().child(chooser))
        .child(plot(graph))
        .child(range)
        .child(controls)
}

pub(crate) fn split_body(rail: impl IntoElement, main: impl IntoElement) -> Div {
    body().gap(px(4.0)).py(px(3.0)).child(rail).child(main)
}

pub(crate) fn plot_column(
    toolbar: impl IntoElement,
    graph: impl IntoElement,
    footer: impl IntoElement,
) -> Div {
    rail_column()
        .w_full()
        .min_w_0()
        .h_full()
        .flex_1()
        .child(toolbar)
        .child(plot(graph))
        .child(footer)
}

pub(crate) fn centered_rails(rails: impl IntoElement) -> Div {
    body().justify_center().py(px(3.0)).child(rails)
}

pub(crate) fn paired_rails(first: impl IntoElement, second: impl IntoElement) -> Div {
    body()
        .justify_between()
        .gap(px(2.0))
        .py(px(3.0))
        .child(dense_rail(first))
        .child(dense_rail(second))
}

pub(crate) fn three_pane(
    shell: impl IntoElement,
    detail: impl IntoElement,
    preview: impl IntoElement,
) -> Div {
    body()
        .overflow_hidden()
        .child(shell)
        .child(detail)
        .child(preview)
}

/// Stable Vector inspector and tab slots; the owner retains focus and page state.
pub(crate) fn tabbed_detail(detail: impl IntoElement, tabs: impl IntoElement) -> Div {
    hex_widgets::device_display(theme::device_style(DeviceStyleRole::Layout))
        .flex_1()
        .w(px(VECTOR_DETAIL_WIDTH))
        .min_w_0()
        .h_full()
        .child(detail)
        .child(
            div()
                .absolute()
                .top(px(86.0))
                .left_0()
                .w_full()
                .px(px(4.0))
                .child(tabs),
        )
}

/// Shared graph-dominant EQ geometry. Caller supplies the real bound controls.
/// The catalog uses these same slots with explicitly view-local fixtures.
pub(crate) fn eq_sidebar(
    id: impl Into<gpui::SharedString>,
    content: impl IntoElement,
    controls: impl IntoElement,
) -> gpui::Stateful<Div> {
    let id = id.into();
    let scroll_id = gpui::SharedString::from(format!("{id}-scroll"));
    div()
        .id(id)
        .w(px(160.0))
        .h_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .p(px(2.0))
        .gap(px(2.0))
        .text_size(px(8.0))
        .line_height(px(10.0))
        .child(
            div()
                .id(scroll_id)
                .w_full()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(content),
        )
        .child(div().w_full().h(px(14.0)).flex_shrink_0().child(controls))
}

pub(crate) fn eq_graph_body(graph: impl IntoElement, sidebar: impl IntoElement) -> Div {
    body()
        .gap(px(2.0))
        .child(plot(graph).min_w_0())
        .child(sidebar)
}

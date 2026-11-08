use crate::{ControlStyle, DEVICE_CORNER_RADIUS, DeviceFrameStyle};
use gpui::{Div, IntoElement, SharedString, TextRun, canvas, div, point, prelude::*, px, rgb};

/// Canonical compact shell geometry, in logical pixels, for every device.
pub const DEVICE_FRAME_HEADER_WIDTH: f32 = 24.0;
/// Minimum length of the title slot within the left header.
pub const DEVICE_FRAME_HEADER_HEIGHT: f32 = 14.0;
pub const DEVICE_FRAME_HEADER_PADDING: f32 = 3.0;
pub const DEVICE_FRAME_HEADER_GAP: f32 = 2.0;
pub const DEVICE_FRAME_BORDER_WIDTH: f32 = 1.0;
pub const DEVICE_BODY_INSET: f32 = 3.0;
pub const DEVICE_LAYOUT_GUTTER: f32 = 4.0;
pub const DEVICE_SECTION_RADIUS: f32 = 3.0;
pub const DEVICE_SECTION_INSET: f32 = 4.0;
pub const DEVICE_ROW_GUTTER: f32 = 2.0;

/// Orthogonal compositions; owners supply sizes, children and musical meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceLayout {
    Row,
    Column,
    Wrapped,
    /// Equal-width columns. Zero is treated as one column.
    Grid(u16),
}

/// Compose controls without coupling appearance to a device type or parameter.
pub fn device_layout(layout: DeviceLayout) -> Div {
    let base = div().min_w_0().min_h_0().gap(px(DEVICE_LAYOUT_GUTTER));
    match layout {
        DeviceLayout::Row => base.flex(),
        DeviceLayout::Column => base.flex().flex_col(),
        DeviceLayout::Wrapped => base.flex().flex_wrap().content_start(),
        DeviceLayout::Grid(columns) => base.grid().grid_cols(columns.max(1)),
    }
}

/// Equal flexible split panes. Use a Row composition for unequal/fixed widths.
pub fn device_split(first: impl IntoElement, second: impl IntoElement) -> Div {
    device_layout(DeviceLayout::Row)
        .child(div().min_w_0().flex_1().child(first))
        .child(div().min_w_0().flex_1().child(second))
}

/// Shared surround body. Children are inset beyond the rounded lower corners.
/// Keep this inset when adding scrolling or custom layouts. GPUI's overflow mask
/// is rectangular: the inset, not `overflow_hidden` alone, protects the corners.
pub fn device_body(style: ControlStyle) -> Div {
    device_layout(DeviceLayout::Row)
        .w_full()
        .overflow_hidden()
        .rounded_bl(px(DEVICE_SECTION_RADIUS))
        .rounded_br(px(DEVICE_SECTION_RADIUS))
        .border_l(px(DEVICE_BODY_INSET))
        .border_r(px(DEVICE_BODY_INSET))
        .border_color(rgb(style.surface))
        .py(px(1.0))
        .bg(rgb(style.surface))
}

/// Scroll body whose content owns the inset, so row offsets and scroll extent
/// include it exactly once while preserving the shared surface and corners.
pub fn device_windowed_body(style: ControlStyle, content: impl IntoElement) -> Div {
    device_body(style)
        .flex_col()
        .border_l_0()
        .border_r_0()
        .py_0()
        .child(div().w_full().p(px(DEVICE_BODY_INSET)).child(content))
}

/// Semantic surfaces, independent of device identity and section dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceSectionSurface {
    Surround,
    Rail,
    Display,
}

/// Rounded short/tall section. Horizontal content inset is at least the radius,
/// so even full-height child artwork stays out of the rounded corner cutouts.
/// Children may choose row/column/grid layout and explicit height independently.
pub fn device_section(style: ControlStyle, surface: DeviceSectionSurface) -> Div {
    section(
        style,
        surface,
        DEVICE_SECTION_RADIUS,
        DEVICE_SECTION_INSET,
        false,
    )
}

/// Rounded section with vertical structural insets instead of horizontal ones.
/// Useful for narrow full-width controls: callers keep their width, and the
/// top/bottom borders keep descendant paint and hits out of rounded corners.
/// Use one section boundary; do not nest another inset around the same content.
pub fn device_compact_section(style: ControlStyle, surface: DeviceSectionSurface) -> Div {
    section(
        style,
        surface,
        DEVICE_SECTION_RADIUS,
        DEVICE_SECTION_INSET,
        true,
    )
}

/// Dense full-width sections use one-pixel corners/insets for tightly packed
/// columns. This preserves fourteen/fifty-two-pixel controls at fixed heights.
pub fn device_dense_section(style: ControlStyle, surface: DeviceSectionSurface) -> Div {
    section(style, surface, 1.0, 1.0, true)
}

fn section(
    style: ControlStyle,
    surface: DeviceSectionSurface,
    radius: f32,
    inset: f32,
    vertical: bool,
) -> Div {
    let color = match surface {
        DeviceSectionSurface::Surround => style.surface,
        DeviceSectionSurface::Rail => style.rail,
        DeviceSectionSurface::Display => style.background,
    };
    div()
        .min_w_0()
        .min_h_0()
        .relative()
        .rounded(px(radius))
        .overflow_hidden()
        .when(vertical, |section| {
            section.border_t(px(inset)).border_b(px(inset))
        })
        .when(!vertical, |section| {
            section.border_l(px(inset)).border_r(px(inset))
        })
        .border_color(rgb(color))
        .bg(rgb(color))
}

/// A short selectable row with one-pixel rounding and vertical containment.
/// Fourteen-pixel controls fit in a seventeen-pixel row without width changes.
pub fn device_compact_row(style: ControlStyle, active: bool) -> Div {
    section(
        style,
        if active {
            DeviceSectionSurface::Surround
        } else {
            DeviceSectionSurface::Rail
        },
        1.0,
        1.0,
        true,
    )
    .flex()
    .items_center()
    .gap(px(DEVICE_ROW_GUTTER))
}

/// A rail's shared surround and row gutters. Width/height belong to composition.
pub fn device_rail(style: ControlStyle) -> Div {
    device_layout(DeviceLayout::Column)
        .gap(px(DEVICE_ROW_GUTTER))
        .bg(rgb(style.surface))
}

/// Grouped parameter row with shared focus surface; no semantic focus ownership.
pub fn device_grouped_row(style: ControlStyle, active: bool) -> Div {
    device_section(
        style,
        if active {
            DeviceSectionSurface::Surround
        } else {
            DeviceSectionSurface::Rail
        },
    )
    .w_full()
    .flex()
    .flex_shrink_0()
    .items_center()
    .gap(px(DEVICE_ROW_GUTTER))
}

/// Recessed graph/inspector display. Owners retain graph and tab/focus state.
pub fn device_display(style: ControlStyle) -> Div {
    device_section(style, DeviceSectionSurface::Display)
        .flex()
        .flex_col()
        .gap(px(DEVICE_SECTION_RADIUS))
}

/// Canonical housing for all production and sandbox devices; bodies supply slots
/// instead of recreating the housing. Title/actions retain caller input handlers. The idle
/// header and surround use the same surface, without an intervening border.
/// The left header keeps its controls above a flexible vertical title slot.
/// The caller owns device commands and supplies a stable parent element identity.
/// Shared housing toggles own only transient macro/modulator placeholder visibility;
/// they never alter device state. Open panels share a scroll region with the body.
/// Use `device_body` for rounded body/content containment.
pub fn device_frame(
    title: impl IntoElement,
    style: impl Into<DeviceFrameStyle>,
    leading_control: impl IntoElement,
    header_actions: impl IntoElement,
    body: impl IntoElement,
) -> Div {
    let style = style.into();
    let radius = DEVICE_CORNER_RADIUS;
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .min_h_0()
        .border_1()
        .border_color(rgb(style.border))
        .bg(rgb(style.controls.surface))
        .rounded(px(radius))
        .overflow_hidden()
        .child(crate::device_housing::Housing {
            body: body.into_any_element(),
            title: title.into_any_element(),
            leading: leading_control.into_any_element(),
            actions: header_actions.into_any_element(),
            style,
        })
}

/// Canonical left header composition, also available to custom surrounds. Insets clip
/// oversized caller actions/title content to the safe rounded interior.
pub fn device_header(
    title: impl IntoElement,
    style: impl Into<DeviceFrameStyle>,
    leading_control: impl IntoElement,
    header_actions: impl IntoElement,
) -> Div {
    let style = style.into();
    div()
        .debug_selector(|| "device-frame-header".into())
        .flex()
        .flex_col()
        .flex_shrink_0()
        .w(px(DEVICE_FRAME_HEADER_WIDTH))
        .border_l(px(DEVICE_FRAME_HEADER_PADDING))
        .border_r(px(DEVICE_FRAME_HEADER_PADDING))
        .py(px(DEVICE_FRAME_HEADER_PADDING))
        .border_color(rgb(style.header_background))
        .overflow_hidden()
        .gap(px(DEVICE_FRAME_HEADER_GAP))
        .items_center()
        .bg(rgb(style.header_background))
        .rounded_tl(px(DEVICE_CORNER_RADIUS - DEVICE_FRAME_BORDER_WIDTH))
        .rounded_bl(px(DEVICE_CORNER_RADIUS - DEVICE_FRAME_BORDER_WIDTH))
        .child(div().flex_shrink_0().child(leading_control))
        .child(div().flex_shrink_0().child(header_actions))
        .child(
            div()
                .w_full()
                .min_h(px(DEVICE_FRAME_HEADER_HEIGHT))
                .flex_1()
                .flex()
                .items_center()
                .min_w_0()
                .overflow_hidden()
                .text_color(rgb(style.header_text))
                .text_size(px(style.controls.font_size))
                .child(title),
        )
}

/// Paint the caller's display text using the inherited font and color. Only the
/// glyphs rotate. The owner keeps the title's ordinary layout and drag target.
pub fn vertical_device_name(name: impl Into<SharedString>) -> impl IntoElement {
    let name = name.into();
    let name = if name.contains(['\n', '\r']) {
        SharedString::from(name.replace(['\n', '\r'], " "))
    } else {
        name
    };
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let available = bounds.size.height - px(4.0);
            if available <= px(0.0) || name.is_empty() {
                return;
            }
            let style = window.text_style();
            let font = style.font();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let text = window.text_system();
            let run = TextRun {
                len: name.len(),
                font: font.clone(),
                color: style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let mut line =
                text.shape_line(name.clone(), font_size, std::slice::from_ref(&run), None);
            if line.width > available {
                let mut runs = vec![run];
                let fitted = text.line_wrapper(font, font_size).truncate_line(
                    name.clone(),
                    available,
                    "…",
                    &mut runs,
                );
                line = text.shape_line(fitted, font_size, &runs, None);
            }
            let center = bounds.center();
            let baseline = point(
                center.x + (line.ascent - line.descent) / 2.0,
                center.y + line.width / 2.0,
            );
            for run in &line.runs {
                for glyph in &run.glyphs {
                    let origin = baseline + point(glyph.position.y, -glyph.position.x);
                    if glyph.is_emoji {
                        // Keep the glyph in the name even when it has no
                        // monochrome variant in the inherited font.
                        let _ = window.paint_emoji(origin, run.font_id, glyph.id, font_size);
                    } else {
                        let _ = window.paint_vertical_glyph(
                            origin,
                            run.font_id,
                            glyph.id,
                            font_size,
                            style.color,
                        );
                    }
                }
            }
        },
    )
    .size_full()
}

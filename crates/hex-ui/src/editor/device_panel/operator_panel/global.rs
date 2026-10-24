//! Canonical controller assignments and global instrument settings.
use super::*;

pub(super) fn controls(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
) -> Div {
    let mut row = div()
        .w_full()
        .h(px(29.0))
        .flex_shrink_0()
        .flex()
        .gap(px(2.0));
    for (parameter, label, presentation) in [
        (601, "voices", ControlPresentation::CompactSelector),
        (602, "rtg", ControlPresentation::CompactButton),
        (603, "interpol.", ControlPresentation::CompactButton),
        (604, "antialias", ControlPresentation::CompactButton),
        (605, "pan", ControlPresentation::InlineValue),
        (606, "pan<key", ControlPresentation::InlineValue),
        (607, "pan<rnd", ControlPresentation::InlineValue),
    ] {
        let control = compact_control_with_codec(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            presentation,
            Some(""),
            (parameter == 605).then_some(super::super::numeric_display::NumericDisplayCodec::Pan),
        );
        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(detail_caption(label))
                .child(control.w_full()),
        );
    }
    row
}

pub(super) fn expression_routing(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
) -> Div {
    const WIDTHS: [f32; 5] = [70.0, 78.0, 46.0, 78.0, 46.0];
    let behavior = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(608),
        ControlPresentation::CompactSelector,
        Some(""),
    )
    .id(ElementId::Name(
        format!("operator-behavior-{}", device.value()).into(),
    ))
    .tooltip(|_, cx| {
        cx.new(|_| crate::editor::transport::MiniTooltip {
            kind: crate::tooltips::Kind::Identification,
            description: theme::text(
                "legacy preserves saved playback; choose operator to enable global controls",
            ),
        })
        .into()
    });
    let mut header = div().h(px(14.0)).flex().gap(px(2.0)).child(
        div()
            .w(px(WIDTHS[0]))
            .flex()
            .items_center()
            .child(behavior.flex_1().min_w_0()),
    );
    for (column, label) in ["", "conn. a", "amt. a", "conn. b", "amt. b"]
        .into_iter()
        .enumerate()
        .skip(1)
    {
        header = header.child(
            detail_caption(label)
                .w(px(WIDTHS[column]))
                .debug_selector(move || format!("operator-routing-column-{column}")),
        );
    }
    let mut content = div()
        .w_full()
        .h(px(99.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .child(header);
    for (row, source) in ["velocity", "key", "aftertouch", "pitch bend", "mod wheel"]
        .into_iter()
        .enumerate()
    {
        let mut line = div()
            .w_full()
            .h(px(17.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(detail_caption(source).w(px(WIDTHS[0])));
        for column in 0..4 {
            let parameter = 620 + row as u32 * 4 + column;
            let field = if parameter == 626 || parameter == 634 {
                // Fixed assignments are descriptive labels, not one-item menus.
                div()
                    .w(px(WIDTHS[column as usize + 1]))
                    .child(section_title(if parameter == 626 {
                        "time"
                    } else {
                        "pitch"
                    }))
            } else {
                compact_control(
                    view,
                    cx,
                    device,
                    identity,
                    parameters,
                    id(parameter),
                    if column % 2 == 0 {
                        ControlPresentation::CompactSelector
                    } else {
                        ControlPresentation::InlineValue
                    },
                    Some(""),
                )
                .w(px(WIDTHS[column as usize + 1]))
            };
            line = line.child(field);
        }
        content = content.child(line);
    }
    content
}

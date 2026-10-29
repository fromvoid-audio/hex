//! Shared device-housing display state. Panels are explicit placeholders and
//! never edit mappings, devices or audio. State lasts while the caller's stable
//! device identity is continuously rendered. Both header and body use that scope.
use crate::{
    ActionStyle, ControlStyle, DEVICE_HEADER_ACTION_SIZE, DeviceFrameStyle, toggle_action,
};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, IntoElement, MouseButton, Render, RenderOnce,
    Window, div, prelude::*, px, rgb,
};

struct Panels {
    visible: [bool; 2],
    focus: [FocusHandle; 2],
}
fn state(window: &mut Window, cx: &mut App) -> Entity<Panels> {
    window.use_keyed_state("device-housing-panels", cx, |_, cx| Panels {
        visible: [false; 2],
        focus: [
            cx.focus_handle().tab_stop(true).tab_index(0),
            cx.focus_handle().tab_stop(true).tab_index(0),
        ],
    })
}

#[derive(IntoElement)]
pub(super) struct Housing {
    pub body: AnyElement,
    pub title: AnyElement,
    pub leading: AnyElement,
    pub actions: AnyElement,
    pub style: DeviceFrameStyle,
}
impl RenderOnce for Housing {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = state(window, cx);
        let style = self.style.controls;
        let mut controls = div()
            .flex()
            .items_center()
            .gap(px(crate::DEVICE_FRAME_HEADER_GAP))
            .child(self.leading);
        for (index, (id, glyph, label)) in [
            ("device-macro-toggle", "m", "macro edit panel · placeholder"),
            (
                "device-modulator-toggle",
                "~",
                "modulator panel · placeholder",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let checked = state.read(cx).visible[index];
            let focus = state.read(cx).focus[index].clone();
            let click_state = state.clone();
            let key_state = state.clone();
            let background = if checked { style.accent } else { style.raised };
            let text = if checked {
                style.accent_text
            } else {
                style.text
            };
            controls = controls.child(
                toggle_action(
                    glyph.into(),
                    ActionStyle {
                        background,
                        text,
                        hover_background: background,
                        hover_text: Some(text),
                    },
                )
                .id(id)
                .debug_selector(move || id.into())
                .size(px(DEVICE_HEADER_ACTION_SIZE))
                .flex_shrink_0()
                .text_size(px(8.0))
                .track_focus(&focus)
                .border_1()
                .border_color(rgb(background))
                .focus(move |button| button.border_color(rgb(style.focus_marker)))
                .tooltip(move |_, cx| cx.new(|_| PanelTooltip(label, style)).into())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    click_state.update(cx, |panels, cx| {
                        panels.focus[index].focus(window);
                        panels.visible[index] = !panels.visible[index];
                        cx.notify();
                    });
                })
                .on_key_down(move |event, window, cx| {
                    if event.keystroke.key == "tab"
                        && !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.alt
                        && !event.keystroke.modifiers.platform
                    {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev();
                        } else {
                            window.focus_next();
                        }
                        cx.stop_propagation();
                    }
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        if !event.is_held {
                            key_state.update(cx, |panels, cx| {
                                panels.visible[index] = !panels.visible[index];
                                cx.notify();
                            });
                        }
                    }
                }),
            );
        }
        let visible = state.read(cx).visible;
        let body = panel_body(self.body, style, visible);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(crate::device_header(
                self.title,
                self.style,
                controls,
                self.actions,
            ))
            .child(body)
    }
}
fn panel_body(body: AnyElement, style: ControlStyle, visible: [bool; 2]) -> AnyElement {
    let mut content = div()
        .id("device-housing-content")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll();
    for (index, (id, title, detail)) in [
        (
            "device-macro-panel",
            "macro edit · placeholder",
            "macro mappings are not implemented",
        ),
        (
            "device-modulator-panel",
            "modulators · placeholder",
            "modulation routing is not implemented",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        if visible[index] {
            content = content.child(
                div()
                    .debug_selector(move || id.into())
                    .w_full()
                    .flex_shrink_0()
                    .p(px(4.0))
                    .border_b_1()
                    .border_color(rgb(style.border))
                    .bg(rgb(style.rail))
                    .text_color(rgb(style.text))
                    .text_size(px(style.font_size))
                    .child(div().child(style.text_case.present(title)))
                    .child(div().child(style.text_case.present(detail))),
            );
        }
    }
    content
        .child(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .min_h_full()
                .child(body),
        )
        .into_any_element()
}
struct PanelTooltip(&'static str, ControlStyle);
impl Render for PanelTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(px(4.0))
            .bg(rgb(self.1.raised))
            .text_color(rgb(self.1.text))
            .text_size(px(self.1.font_size))
            .child(self.1.text_case.present(self.0))
    }
}

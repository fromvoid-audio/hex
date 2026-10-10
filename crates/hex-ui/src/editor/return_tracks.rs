//! Compact pinned return/mix controls shared by Arrangement and track headers.
use super::EditorView;
use super::tracks::TRACK_ROW_HEIGHT;
use crate::theme;
use gpui::{Context, Div, ElementId, div, prelude::*, px, rgb};
use hex_project::{Command, Track};

const RETURN_ROW_HEIGHT: f32 = 23.0;
const RETURN_SECTION_HEADER_HEIGHT: f32 = 20.0;
const RETURN_SECTION_MAX_HEIGHT: f32 = 67.0;

impl EditorView {
    pub(super) fn return_tracks_view(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let Some(state) = self.state.as_ref() else {
            return div()
                .id("pinned-return-tracks-empty")
                .h(px(0.0))
                .flex_shrink_0();
        };
        let returns = state.project.return_tracks();
        let desired = RETURN_SECTION_HEADER_HEIGHT + returns.len() as f32 * RETURN_ROW_HEIGHT + 1.0;
        let height = desired.min(RETURN_SECTION_MAX_HEIGHT);
        let mut rows = div()
            .id("pinned-return-tracks")
            .debug_selector(|| "pinned-return-tracks".to_owned())
            .h(px(height))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)));
        let add = div()
            .id("add-return-track")
            .debug_selector(|| "add-return-track".to_owned())
            .size(px(17.0))
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .cursor(gpui::CursorStyle::PointingHand)
            .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
            .tooltip(|_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    description: "add return track".into(),
                })
                .into()
            })
            .child(theme::text("+"))
            .on_click(cx.listener(|view, _, _, cx| {
                let count = view
                    .state
                    .as_ref()
                    .map_or(0, |state| state.project.return_tracks().len());
                super::device_panel::enqueue_return_edit(
                    view,
                    Command::AddReturnTrack {
                        name: format!("return {}", count + 1),
                    },
                    cx,
                );
                cx.notify();
            }));
        rows = rows.child(
            div()
                .id("return-track-section-header")
                .debug_selector(|| "return-track-section-header".to_owned())
                .h(px(RETURN_SECTION_HEADER_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(4.0))
                .px(px(5.0))
                .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                .border_b_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text("returns"))
                .child(div().flex_1())
                .child(add),
        );
        for track in returns {
            let id = track.id();
            let selected = self.focused_return == Some(id);
            let name = track.name().to_owned();
            let gain = track.gain();
            let muted = track.muted();
            let row = div()
                .id(ElementId::Name(
                    format!("return-track-{}", id.value()).into(),
                ))
                .debug_selector(move || format!("return-track-row-{}", id.value()))
                .h(px(RETURN_ROW_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(3.0))
                .px(px(5.0))
                .border_b_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                .bg(rgb(if selected {
                    theme::color(theme::PaletteRole::Hover)
                } else {
                    theme::color(theme::PaletteRole::Surface)
                }))
                .child(
                    div()
                        .id(ElementId::Name(
                            format!("return-track-focus-{}", id.value()).into(),
                        ))
                        .debug_selector(move || format!("return-track-focus-{}", id.value()))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .cursor(gpui::CursorStyle::PointingHand)
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                        .child(theme::text(name))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            if view.focus_return_details(id, window, cx) {
                                cx.notify();
                            }
                        })),
                )
                .child(
                    return_button(
                        format!("return-gain-down-{}", id.value()),
                        "−",
                        "decrease return gain",
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        super::device_panel::enqueue_return_edit(
                            view,
                            Command::ReturnGain {
                                return_track: id,
                                gain: (gain - 0.1).clamp(0.0, 2.0),
                            },
                            cx,
                        );
                        cx.notify();
                    })),
                )
                .child(
                    div()
                        .w(px(32.0))
                        .text_xs()
                        .text_right()
                        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
                        .child(theme::text(format!("{:.0}%", gain * 100.0))),
                )
                .child(
                    return_button(
                        format!("return-gain-up-{}", id.value()),
                        "+",
                        "increase return gain",
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        super::device_panel::enqueue_return_edit(
                            view,
                            Command::ReturnGain {
                                return_track: id,
                                gain: (gain + 0.1).clamp(0.0, 2.0),
                            },
                            cx,
                        );
                        cx.notify();
                    })),
                )
                .child(
                    return_button(
                        format!("return-mute-{}", id.value()),
                        if muted { "○" } else { "●" },
                        "toggle return mute",
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        super::device_panel::enqueue_return_edit(
                            view,
                            Command::ReturnMute {
                                return_track: id,
                                muted: !muted,
                            },
                            cx,
                        );
                        cx.notify();
                    })),
                )
                .child(
                    return_button(
                        format!("return-remove-{}", id.value()),
                        "×",
                        "remove return track",
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        super::device_panel::enqueue_return_edit(
                            view,
                            Command::RemoveReturnTrack(id),
                            cx,
                        );
                        cx.notify();
                    })),
                );
            rows = rows.child(row);
        }
        rows
    }

    pub(super) fn return_send_menu(
        &self,
        track: &Track,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let track_id = track.id();
        let mut menu = div()
            .id(("return-send-menu", track_id.value()))
            .debug_selector(|| format!("return-send-menu-{}", track_id.value()))
            .absolute()
            .right_0()
            .bottom(px(TRACK_ROW_HEIGHT))
            .w(px(190.0))
            .max_h(px(120.0))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .p(px(3.0))
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .shadow_lg();
        let Some(state) = self.state.as_ref() else {
            return menu;
        };
        let returns = state.project.return_tracks();
        if returns.is_empty() {
            return menu.child(
                div()
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .px(px(4.0))
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("no return tracks")),
            );
        }
        for return_track in returns {
            let return_id = return_track.id();
            let name = return_track.name().to_owned();
            let gain = track
                .sends()
                .iter()
                .find(|send| send.return_track() == return_id)
                .map_or(0.0, hex_project::SendLevel::gain);
            let decrease = stepped_send_gain(gain, false);
            let increase = stepped_send_gain(gain, true);
            let mut row = div()
                .id(ElementId::Name(
                    format!("return-send-row-{}-{}", track_id.value(), return_id.value()).into(),
                ))
                .debug_selector(|| {
                    format!("return-send-row-{}-{}", track_id.value(), return_id.value())
                })
                .h(px(21.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(3.0))
                .px(px(3.0))
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                .child(div().flex_1().min_w_0().truncate().child(theme::text(name)))
                .child(
                    send_adjust_button(
                        format!("send-down-{}-{}", track_id.value(), return_id.value()),
                        "−",
                        "decrease send level",
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        super::device_panel::enqueue_return_edit(
                            view,
                            Command::SetReturnSend {
                                track: track_id,
                                return_track: return_id,
                                gain: decrease,
                            },
                            cx,
                        );
                        cx.notify();
                    })),
                )
                .child(
                    div()
                        .id(ElementId::Name(
                            format!(
                                "return-send-value-{}-{}",
                                track_id.value(),
                                return_id.value()
                            )
                            .into(),
                        ))
                        .debug_selector(move || {
                            format!(
                                "return-send-value-{}-{}",
                                track_id.value(),
                                return_id.value()
                            )
                        })
                        .w(px(34.0))
                        .text_right()
                        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
                        .child(theme::text(if gain == 0.0 {
                            "off".to_owned()
                        } else {
                            format!("{gain:.3}")
                        })),
                )
                .child(
                    send_adjust_button(
                        format!("send-up-{}-{}", track_id.value(), return_id.value()),
                        "+",
                        "increase send level",
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        super::device_panel::enqueue_return_edit(
                            view,
                            Command::SetReturnSend {
                                track: track_id,
                                return_track: return_id,
                                gain: increase,
                            },
                            cx,
                        );
                        cx.notify();
                    })),
                );
            if gain > 0.0 {
                row = row.bg(rgb(theme::color(theme::PaletteRole::Surface)));
            }
            menu = menu.child(row);
        }
        menu
    }
}

fn return_button(id: String, icon: &'static str, tooltip: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(ElementId::Name(id.clone().into()))
        .debug_selector(move || id.clone())
        .size(px(15.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
        .cursor(gpui::CursorStyle::PointingHand)
        .tooltip(move |_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                description: tooltip.into(),
            })
            .into()
        })
        .child(theme::text(icon))
}

fn send_adjust_button(
    id: String,
    icon: &'static str,
    tooltip: &'static str,
) -> gpui::Stateful<Div> {
    div()
        .id(ElementId::Name(id.clone().into()))
        .debug_selector(move || id.clone())
        .size(px(15.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
        .cursor(gpui::CursorStyle::PointingHand)
        .tooltip(move |_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                description: tooltip.into(),
            })
            .into()
        })
        .child(theme::text(icon))
}

/// Change one return send in bounded steps while keeping roundoff from leaving
/// a nonzero sparse send at either endpoint. Interior stored values are not
/// quantized.
pub(super) fn stepped_send_gain(gain: f32, increase: bool) -> f32 {
    const MAX_GAIN: f32 = 2.0;
    const STEP: f32 = 0.1;
    const ENDPOINT_EPSILON: f32 = 1.0e-6;

    let gain = if gain.is_finite() {
        gain.clamp(0.0, MAX_GAIN)
    } else {
        0.0
    };
    let delta = if increase { STEP } else { -STEP };
    let stepped = (gain + delta).clamp(0.0, MAX_GAIN);
    if stepped <= ENDPOINT_EPSILON {
        0.0
    } else if MAX_GAIN - stepped <= ENDPOINT_EPSILON {
        MAX_GAIN
    } else {
        stepped
    }
}

#[cfg(test)]
mod tests {
    use super::stepped_send_gain;

    #[test]
    fn three_send_steps_up_then_down_remove_the_sparse_send_exactly() {
        let mut gain = 0.0;
        for _ in 0..3 {
            gain = stepped_send_gain(gain, true);
        }
        assert!(gain > 0.0);
        for _ in 0..3 {
            gain = stepped_send_gain(gain, false);
        }
        assert_eq!(gain, 0.0);
    }

    #[test]
    fn send_steps_preserve_interior_values_and_clamp_finite_endpoints() {
        let stepped = stepped_send_gain(0.375, false);
        assert!((stepped - 0.275).abs() < 1.0e-6);
        assert!((stepped - 0.3).abs() > 1.0e-3);
        assert_eq!(stepped_send_gain(0.0, false), 0.0);
        assert_eq!(stepped_send_gain(2.0, true), 2.0);
        assert_eq!(stepped_send_gain(1.999_999_9, true), 2.0);
        assert!(stepped_send_gain(f32::NAN, true).is_finite());
    }
}

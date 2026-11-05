//! Compact canonical mixer controls. All musical changes stay on the actor.
use super::{
    EditorView,
    waveform::{button, toggle_button_with_style},
};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_project::Command;
use hex_session::service::Request;

impl EditorView {
    pub(super) fn mixer_strip(&mut self, cx: &mut Context<Self>, show_main: bool) -> Div {
        if show_main {
            super::track_gain::retire_stale(self, cx);
        }
        let mut strip = div()
            .h(px(26.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .text_xs();
        let Some(state) = self.state.clone() else {
            return strip;
        };
        if let Some(track) = self.selected_track.and_then(|id| state.project.track(id)) {
            let id = track.id();
            strip = strip
                .child(
                    div()
                        .max_w(px(140.0))
                        .truncate()
                        .child(theme::text(track.name())),
                )
                .child(
                    button("◀", "balance left", ("balance-left", id.value())).on_click(
                        cx.listener(move |view, _, _, cx| {
                            view.enqueue(Request::Edit(Command::BalanceBy {
                                track: id,
                                amount: -0.1,
                            }));
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    button(
                        "•",
                        "reset balance to center",
                        ("balance-reset", id.value()),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.enqueue(Request::Edit(Command::Balance {
                            track: id,
                            balance: 0.0,
                        }));
                        cx.notify();
                    })),
                )
                .child(
                    button("▶", "balance right", ("balance-right", id.value())).on_click(
                        cx.listener(move |view, _, _, cx| {
                            view.enqueue(Request::Edit(Command::BalanceBy {
                                track: id,
                                amount: 0.1,
                            }));
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    div()
                        .w(px(100.0))
                        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
                        .child(theme::text(balance_label(track.balance()))),
                );
        }
        if show_main {
            if let Some(track) = self.selected_track.and_then(|id| state.project.track(id)) {
                let id = track.id();
                let (background, text) = super::track_gain::activator_colors(!track.muted());
                let (solo_background, solo_text) =
                    super::track_gain::activator_colors(self.displayed_solo(id, track.soloed()));
                strip = strip
                    .child(
                        toggle_button_with_style(
                            "on",
                            if track.muted() {
                                "activate track"
                            } else {
                                "deactivate track"
                            },
                            ("console-track-mute", id.value()),
                            theme::action_style().with_state_colors(background, text),
                        )
                        .debug_selector(|| "console-track-mute".into())
                        .on_click(cx.listener(move |view, _, _, cx| {
                            cx.stop_propagation();
                            view.toggle_mute(id);
                            cx.notify();
                        })),
                    )
                    .child(
                        toggle_button_with_style(
                            "s",
                            "solo track · s · cmd+click additive",
                            ("console-track-solo", id.value()),
                            theme::action_style().with_state_colors(solo_background, solo_text),
                        )
                        .debug_selector(|| "console-track-solo".into())
                        .on_click(cx.listener(
                            move |view, event: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                view.toggle_solo(id, !event.modifiers().platform);
                                cx.notify();
                            },
                        )),
                    );
            }
            use hex_session::device_gesture::GestureTarget;
            let active_gain = self.device_gesture.snapshot().and_then(|snapshot| {
                if snapshot.key.generation != self.project_generation {
                    return None;
                }
                match snapshot.target {
                    GestureTarget::TrackGain { track } => Some(track),
                    _ => None,
                }
            });
            let owner = active_gain
                .or_else(|| {
                    self.track_gain_controls
                        .pending_track_gain(self.project_generation, cx)
                })
                .and_then(|id| state.project.track(id))
                .or_else(|| self.selected_track.and_then(|id| state.project.track(id)));
            if let Some(track) = owner {
                let id = track.id();
                if self.selected_track != Some(id) {
                    strip = strip.child(
                        div()
                            .max_w(px(140.0))
                            .truncate()
                            .debug_selector(|| "console-track-gain-owner".into())
                            .child(theme::text(format!("editing {}", track.name()))),
                    );
                }
                let gain = self.track_volume(track, cx);
                strip = strip.child(
                    div()
                        .w(px(110.0))
                        .flex_shrink_0()
                        .debug_selector(move || format!("console-track-gain-{}", id.value()))
                        .child(gain),
                );
            }
            if let Some(track) = self.selected_track.and_then(|id| state.project.track(id)) {
                let id = track.id();
                let peak = self.track_meters.level(id);
                strip = strip.child(
                    div()
                        .flex_shrink_0()
                        .debug_selector(move || format!("console-track-meter-{}", id.value()))
                        .child(super::track_meters::render(id, peak, 26.0)),
                );
            }
        }
        if let Some(fades) = self.selected_fade_controls(&state, cx) {
            strip = strip.children([
                div()
                    .w(px(1.0))
                    .h(px(16.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Border))),
                fades,
            ]);
        }
        strip = strip.child(
            button(
                "routing",
                "show track audio and midi routing",
                ("routing-toggle", 0),
            )
            .on_click(cx.listener(|view, _, _, cx| {
                view.routing_open = !view.routing_open;
                view.routing_page = 0;
                cx.notify();
            })),
        );
        if show_main {
            strip = strip
                .child(div().flex_1())
                .child(
                    div()
                        .id("console-main-header")
                        .debug_selector(|| "console-main-header".into())
                        .on_click(cx.listener(|view, _, window, cx| {
                            super::device_panel::select_main(view, window, cx)
                        }))
                        .child(theme::text("main")),
                )
                .children([self.main_track_controls(cx)]);
        }
        strip
    }

    pub(super) fn main_track_controls(&self, cx: &mut Context<Self>) -> Div {
        let Some(state) = self.state.as_ref() else {
            return div();
        };
        div()
            .debug_selector(|| "main-track-controls".to_owned())
            .flex()
            .items_center()
            .gap_1()
            .child(
                button("−", "decrease main gain", ("master-down", 0)).on_click(cx.listener(
                    |view, _, _, cx| {
                        view.enqueue(Request::Edit(Command::MasterGainBy { amount: -0.1 }));
                        cx.notify();
                    },
                )),
            )
            .child(
                button("•", "reset main gain to unity", ("master-reset", 0)).on_click(cx.listener(
                    |view, _, _, cx| {
                        view.enqueue(Request::Edit(Command::MasterGain { gain: 1.0 }));
                        cx.notify();
                    },
                )),
            )
            .child(
                button("+", "increase main gain", ("master-up", 0))
                    .debug_selector(|| "main-gain-up".to_owned())
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.enqueue(Request::Edit(Command::MasterGainBy { amount: 0.1 }));
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w(px(42.0))
                    .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                    .child(theme::text(format!(
                        "{:.0}%",
                        state.project.master_gain() * 100.0
                    ))),
            )
    }
}

fn balance_label(balance: f32) -> String {
    let percent = (balance.abs() * 100.0).round() as u32;
    if percent == 0 {
        "balance c".into()
    } else {
        format!("balance {}{percent}", if balance < 0.0 { "l" } else { "r" })
    }
}

//! Compact canonical mixer controls. All musical changes stay on the actor.
use super::{EditorView, waveform::button};
use crate::theme::{self, Palette};
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_project::Command;
use hex_session::service::Request;

impl EditorView {
    pub(super) fn mixer_strip(&self, cx: &mut Context<Self>, show_main: bool) -> Div {
        let mut strip = div()
            .h(px(26.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .border_t_1()
            .border_color(rgb(Palette::BORDER))
            .bg(rgb(Palette::SURFACE))
            .text_xs();
        let Some(state) = self.state.as_ref() else {
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
                        .text_color(rgb(Palette::CYAN))
                        .child(theme::text(balance_label(track.balance()))),
                );
        }
        if let Some(fades) = self.selected_fade_controls(state, cx) {
            strip = strip.children([div().w(px(1.0)).h(px(16.0)).bg(rgb(Palette::BORDER)), fades]);
        }
        if show_main {
            strip = strip
                .child(div().flex_1())
                .child(theme::text("main"))
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
                    .text_color(rgb(Palette::TEXT))
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

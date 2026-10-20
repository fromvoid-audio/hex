//! Compact canonical mixer controls. All musical changes stay on the actor.
use super::{EditorView, waveform::button};
use crate::theme;
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
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .text_xs();
        let Some(state) = self.state.as_ref() else {
            return strip;
        };
        let selected = match state.read_with(|snapshot| {
            self.selected_track
                .and_then(|id| snapshot.model().track(id))
                .map(|track| (track.id(), track.name().to_owned(), track.balance()))
        }) {
            Ok(selected) => selected,
            Err(_) => return strip,
        };
        if let Some((id, name, balance)) = selected {
            strip = strip
                .child(div().max_w(px(140.0)).truncate().child(theme::text(name)))
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
                        .child(theme::text(balance_label(balance))),
                );
        }
        if let Some(fades) = self.selected_fade_controls(cx) {
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
        let Some(gain) = self.state.as_ref().and_then(|state| {
            state
                .read_with(|snapshot| snapshot.model().master_gain())
                .ok()
        }) else {
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
                    .child(theme::text(format!("{:.0}%", gain * 100.0))),
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

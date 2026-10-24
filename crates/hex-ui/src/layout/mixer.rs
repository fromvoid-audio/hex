use crate::app::{Daw, INSPECTOR_HEIGHT, MIXER_HEIGHT, TRACK_COLORS};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_engine::live::{Command, NAMES, TRACKS};
use std::sync::atomic::Ordering::Relaxed;

impl Daw {
    pub(crate) fn mixer(&self, cx: &mut Context<Self>) -> Div {
        let mut strips = div().flex().flex_1().min_w_0();
        for track in 0..TRACKS {
            strips = strips.child(self.mixer_strip(track, cx));
        }
        div()
            .h(px(MIXER_HEIGHT))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .child(
                div()
                    .h(px(22.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("mixer"))
                    .child(theme::text("track level · peak")),
            )
            .child(strips)
    }

    pub(crate) fn mixer_strip(&self, track: usize, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let peak = self
            .audio
            .as_ref()
            .map_or(0.0, |audio| audio.telemetry.peak(track));
        let colors = theme::meter_colors();
        let control = hex_widgets::level_meter(
            format!("demo-track-meter-{track}").into(),
            peak,
            peak.is_finite() && peak > 1.0,
            hex_widgets::LevelMeterStyle {
                background: theme::color(theme::PaletteRole::WaveBackground),
                fill: colors.color(hex_widgets::MeterLevel::from_amplitude(peak, false)),
                above: colors.clipping,
            },
        )
        .h(px(42.0))
        .w(px(6.0));
        div()
            .id(("mixer-strip", track))
            .w(px(94.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .border_r_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(if self.selected == track {
                theme::color(theme::PaletteRole::Raised)
            } else {
                theme::color(theme::PaletteRole::Surface)
            }))
            .child(
                div()
                    .h(px(17.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(rgb(TRACK_COLORS[track]))
                    .child(theme::text(NAMES[track]))
                    .child(theme::text(format!("{:02}", track + 1))),
            )
            .child(
                div()
                    .id(("mixer-select", track))
                    .h(px(48.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_row_reverse()
                    .items_end()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = track;
                        cx.notify();
                    }))
                    .child(control)
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                            .child(theme::text("0")),
                    ),
            )
            .child(
                div()
                    .h(px(22.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .child(
                        Self::with_tooltip(
                            Self::button("−", false).id(("gain-down", track)),
                            "decrease track gain",
                        )
                        .h(px(22.0))
                        .min_w(px(22.0))
                        .flex_shrink_0()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let gain = (this.gains[track] - 0.1).max(0.0);
                            if this.send(Command::Gain { track, gain }) {
                                this.gains[track] = gain;
                            }
                            cx.notify();
                        })),
                    )
                    .child(theme::text(format!("{:.0}%", self.gains[track] * 100.0)))
                    .child(
                        Self::with_tooltip(
                            Self::button("+", false).id(("gain-up", track)),
                            "increase track gain",
                        )
                        .h(px(22.0))
                        .min_w(px(22.0))
                        .flex_shrink_0()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let gain = (this.gains[track] + 0.1).min(2.0);
                            if this.send(Command::Gain { track, gain }) {
                                this.gains[track] = gain;
                            }
                            cx.notify();
                        })),
                    ),
            )
            .child(
                Self::with_tooltip(
                    Self::toggle_button("m", self.muted[track]).id(("mute", track)),
                    "toggle track mute",
                )
                .h(px(22.0))
                .min_w(px(22.0))
                .flex_shrink_0()
                .on_click(cx.listener(move |this, _, _, cx| {
                    let muted = !this.muted[track];
                    if this.send(Command::Mute { track, muted }) {
                        this.muted[track] = muted;
                    }
                    cx.notify();
                })),
            )
    }

    pub(crate) fn inspector(&self) -> Div {
        let name = NAMES[self.selected];
        div()
            .h(px(INSPECTOR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .px_2()
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .child(
                div()
                    .w(px(3.0))
                    .h(px(42.0))
                    .bg(rgb(TRACK_COLORS[self.selected])),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                            .truncate()
                            .child(theme::text(format!("{} · source inspector", name))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                            .truncate()
                            .child(theme::text("generated stereo · 4 beats · source 120 bpm")),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .max_w(px(132.0))
                    .min_w_0()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .truncate()
                    .child(theme::text("tempo changes pitch")),
            )
    }

    pub(crate) fn status_bar(&self, fault: bool) -> Div {
        let label = self
            .audio
            .as_ref()
            .map_or("no output device".to_owned(), |audio| audio.label.clone());
        let metrics = self.audio.as_ref().map_or(String::new(), |audio| {
            format!(
                "{:.2} ms · {} late",
                audio.telemetry.callback_max_ns.load(Relaxed) as f64 / 1e6,
                audio.telemetry.deadline_misses.load(Relaxed)
            )
        });
        div()
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .text_xs()
            .text_color(rgb(if fault {
                theme::color(theme::PaletteRole::Primary)
            } else {
                theme::color(theme::PaletteRole::Muted)
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .truncate()
                    .child(theme::text(if fault {
                        "audio device failed · restart hex to reconnect".to_owned()
                    } else {
                        self.status.clone()
                    })),
            )
            .child(
                div()
                    .max_w(px(220.0))
                    .min_w_0()
                    .text_xs()
                    .truncate()
                    .child(theme::text(label)),
            )
            .child(
                div()
                    .max_w(px(112.0))
                    .min_w_0()
                    .text_xs()
                    .truncate()
                    .child(theme::text(metrics)),
            )
    }
}

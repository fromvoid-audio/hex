use crate::app::{Daw, HEADER_HEIGHT, TRACK_COLORS};
use crate::theme;
use gpui::{Context, Div, Window, div, prelude::*, px, rgb};
use hex_engine::live::{Command, Mode, NAMES, TRACKS};

impl Daw {
    pub(crate) fn button(label: &'static str, active: bool) -> Div {
        hex_widgets::action(theme::text(label), theme::action_style())
            .min_w(px(28.0))
            .h(px(26.0))
            .px_2()
            .text_sm()
            .bg(rgb(if active {
                theme::color(theme::PaletteRole::PrimaryDark)
            } else {
                theme::color(theme::PaletteRole::Raised)
            }))
            .text_color(rgb(if active {
                theme::color(theme::PaletteRole::Text)
            } else {
                theme::color(theme::PaletteRole::Muted)
            }))
    }

    pub(crate) fn toggle_button(label: &'static str, active: bool) -> Div {
        Self::button(label, active).rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
    }

    pub(crate) fn with_tooltip(
        button: gpui::Stateful<Div>,
        description: &'static str,
    ) -> gpui::Stateful<Div> {
        button.tooltip(move |_, cx| cx.new(|_| SmallTooltip { description }).into())
    }

    pub(crate) fn transport(&self, cx: &mut Context<Self>, beat: f64) -> Div {
        let playing = self.playing();
        div()
            .h(px(HEADER_HEIGHT))
            .flex()
            .items_center()
            .gap_1()
            .pl(px(theme::TITLEBAR_LEADING_INSET))
            .pr_2()
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .child(
                div()
                    .px_2()
                    .text_sm()
                    .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                    .child(theme::text("hex")),
            )
            .child(
                Self::with_tooltip(
                    Self::toggle_button("◀", false).id("browser-toggle"),
                    "toggle browser",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.browser_open = !this.browser_open;
                    cx.notify();
                })),
            )
            .child(
                Self::with_tooltip(
                    Self::toggle_button(if playing { "Ⅱ" } else { "▶" }, playing).id("play"),
                    if playing {
                        "pause playback"
                    } else {
                        "start playback · space"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle();
                    cx.notify();
                })),
            )
            .child(
                Self::with_tooltip(
                    Self::toggle_button("■", false).id("stop"),
                    "stop and return to start · escape",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.stop();
                    cx.notify();
                })),
            )
            .child(div().w(px(8.0)))
            .child(
                Self::with_tooltip(Self::button("−", false).id("tempo-down"), "decrease tempo")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let bpm = (this.bpm - 1.0).max(20.0);
                        if this.send(Command::Tempo(bpm)) {
                            this.bpm = bpm;
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w(px(64.0))
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                    .child(theme::text(format!("{:.0} bpm", self.bpm))),
            )
            .child(
                Self::with_tooltip(Self::button("+", false).id("tempo-up"), "increase tempo")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let bpm = (this.bpm + 1.0).min(300.0);
                        if this.send(Command::Tempo(bpm)) {
                            this.bpm = bpm;
                        }
                        cx.notify();
                    })),
            )
            .child(div().w(px(8.0)))
            .child(
                div()
                    .w(px(72.0))
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                    .child(theme::text(format!(
                        "{}.{}.{:02}",
                        (beat / 4.0) as usize + 1,
                        beat as usize % 4 + 1,
                        (beat.fract() * 100.0) as usize
                    ))),
            )
            .child(div().flex_1())
            .child(
                Self::with_tooltip(
                    Self::toggle_button("arrangement", self.mode == Mode::Arrangement)
                        .id("arrangement"),
                    "show arrangement",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.send(Command::Mode(Mode::Arrangement)) {
                        this.mode = Mode::Arrangement;
                    }
                    cx.notify();
                })),
            )
            .child(
                Self::with_tooltip(
                    Self::toggle_button("session", self.mode == Mode::Session).id("session"),
                    "show session",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.send(Command::Mode(Mode::Session)) {
                        this.mode = Mode::Session;
                    }
                    cx.notify();
                })),
            )
            .child(div().w(px(8.0)))
            .child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("first light · demo")),
            )
            .child(crate::build_info::label())
    }

    pub(crate) fn browser(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let mut browser = div()
            .id("browser")
            .w(px(188.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)));
        if !self.browser_open {
            return browser.w(px(30.0)).child(
                Self::with_tooltip(
                    Self::toggle_button(">", false).id("browser-expand"),
                    "show browser",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.browser_open = true;
                    cx.notify();
                })),
            );
        }

        browser = browser
            .child(
                div()
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .text_xs()
                    .child(theme::text("demo sources"))
                    .child(
                        div()
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                            .child(theme::text("⌕")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("all"))
                    .child(theme::text("·"))
                    .child(theme::text("audio"))
                    .child(theme::text("·"))
                    .child(theme::text("tracks")),
            );
        for track in 0..TRACKS {
            let selected = self.selected == track;
            browser = browser.child(
                div()
                    .id(("source", track))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .cursor_pointer()
                    .bg(rgb(if selected {
                        theme::color(theme::PaletteRole::Raised)
                    } else {
                        theme::color(theme::PaletteRole::Surface)
                    }))
                    .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
                    .text_xs()
                    .text_color(rgb(if selected {
                        theme::color(theme::PaletteRole::Text)
                    } else {
                        theme::color(theme::PaletteRole::Muted)
                    }))
                    .child(div().w(px(3.0)).h(px(14.0)).bg(rgb(TRACK_COLORS[track])))
                    .child(theme::text(NAMES[track]))
                    .child(div().flex_1())
                    .child(theme::text("×2"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = track;
                        cx.notify();
                    })),
            );
        }
        browser.child(div().flex_1()).child(
            div()
                .h(px(24.0))
                .px_2()
                .py_1()
                .border_t_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text("generated · 4 stereo sources")),
        )
    }
}

struct SmallTooltip {
    description: &'static str,
}

impl Render for SmallTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::tooltips::view(
            &crate::tooltips::Kind::Identification,
            self.description,
            _cx,
        )
    }
}

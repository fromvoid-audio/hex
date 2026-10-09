use crate::app::{Daw, TRACK_COLORS};
use crate::theme::{self, Palette};
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_engine::live::{Command, Mode, NAMES, TRACKS};

impl Daw {
    pub(crate) fn arrangement(&self, cx: &mut Context<Self>, beat: f64) -> Div {
        let mut rows = div().flex().flex_col();
        for track in 0..TRACKS {
            let mut clips = div()
                .relative()
                .flex_1()
                .flex()
                .gap_1()
                .px_1()
                .items_center()
                .border_r_1()
                .border_color(rgb(Palette::GRID));
            for scene in 0..2 {
                let label = if scene == 0 { "a" } else { "b" };
                clips = clips.child(
                    div()
                        .id(("arrangement-clip", track * 2 + scene))
                        .relative()
                        .flex_1()
                        .h(px(36.0))
                        .px_2()
                        .py_1()
                        .cursor_pointer()
                        .border_l_2()
                        .border_color(rgb(TRACK_COLORS[track]))
                        .bg(rgb(Palette::CLIP))
                        .hover(|style| style.bg(rgb(Palette::HOVER)))
                        .text_xs()
                        .text_color(rgb(Palette::TEXT))
                        .child(theme::text(format!("{} · {}", NAMES[track], label)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = track;
                            cx.notify();
                        })),
                );
            }
            let position = (beat / 8.0).rem_euclid(1.0) as f32;
            clips = clips.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui::relative(position))
                    .w(px(1.0))
                    .bg(rgb(Palette::CORAL)),
            );
            rows = rows.child(
                div()
                    .h(px(44.0))
                    .flex_shrink_0()
                    .flex()
                    .border_b_1()
                    .border_color(rgb(Palette::BORDER))
                    .child(clips)
                    .child(self.track(self.selected == track, track, cx)),
            );
        }
        let main = Self::ruler();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(main)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(rows)
                    .child(Self::empty_grid(true)),
            )
    }

    pub(crate) fn ruler() -> Div {
        let mut ruler = div()
            .h(px(24.0))
            .flex()
            .border_b_1()
            .border_color(rgb(Palette::BORDER))
            .bg(rgb(Palette::SURFACE));
        for beat in 0..8 {
            ruler = ruler.child(
                div()
                    .flex_1()
                    .border_l_1()
                    .border_color(rgb(Palette::GRID))
                    .px_1()
                    .text_xs()
                    .text_color(rgb(Palette::MUTED))
                    .child(theme::text(format!("{}.{}", beat / 4 + 1, beat % 4 + 1))),
            );
        }
        ruler = ruler.child(div().w(px(100.0)).flex_shrink_0().px_2());
        ruler
    }

    fn empty_grid(trailing_header: bool) -> Div {
        let mut grid = div()
            .relative()
            .flex_1()
            .min_h_0()
            .bg(rgb(Palette::BACKGROUND));
        if trailing_header {
            grid = grid.mr(px(100.0));
        }
        for beat in 1..8 {
            grid = grid.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui::relative(beat as f32 / 8.0))
                    .w(px(1.0))
                    .bg(rgb(Palette::GRID)),
            );
        }
        for row in 1..6 {
            grid = grid.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(gpui::relative(row as f32 / 6.0))
                    .h(px(1.0))
                    .bg(rgb(Palette::GRID)),
            );
        }
        grid
    }

    pub(crate) fn track(
        &self,
        selected: bool,
        track: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id(("track-header", track))
            .w(px(100.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .cursor_pointer()
            .bg(rgb(if selected {
                Palette::RAISED
            } else {
                Palette::SURFACE
            }))
            .border_l_2()
            .border_color(rgb(TRACK_COLORS[track]))
            .text_xs()
            .text_color(rgb(if selected {
                Palette::TEXT
            } else {
                Palette::MUTED
            }))
            .child(theme::text(NAMES[track]))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = track;
                cx.notify();
            }))
    }

    pub(crate) fn session(&self, cx: &mut Context<Self>, active_scene: usize) -> Div {
        let mut rows = div().flex().flex_col();
        let mut track_labels = div().flex().flex_1();
        for track in 0..TRACKS {
            track_labels = track_labels.child(
                div()
                    .flex_1()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(TRACK_COLORS[track]))
                    .child(theme::text(NAMES[track])),
            );
        }
        // Scene rows use their own compact header and track lanes, independent of beat ruler labels.
        rows = rows.child(
            div()
                .h(px(24.0))
                .flex_shrink_0()
                .flex()
                .border_b_1()
                .border_color(rgb(Palette::BORDER))
                .child(
                    div()
                        .w(px(100.0))
                        .flex_shrink_0()
                        .px_2()
                        .text_xs()
                        .text_color(rgb(Palette::MUTED))
                        .child(theme::text("scene")),
                )
                .child(track_labels),
        );
        for scene in 0..2 {
            let mut row = div()
                .h(px(40.0))
                .flex_shrink_0()
                .flex()
                .border_b_1()
                .border_color(rgb(Palette::BORDER));
            row = row.child(
                div()
                    .w(px(100.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(if active_scene == scene {
                        Palette::CORAL
                    } else {
                        Palette::MUTED
                    }))
                    .child(theme::text(format!(
                        "scene {}",
                        if scene == 0 { "a" } else { "b" }
                    )))
                    .child(
                        Self::with_tooltip(
                            Self::button("▶", active_scene == scene).id(("scene-launch", scene)),
                            if scene == 0 {
                                "launch scene a · bar quantized"
                            } else {
                                "launch scene b · bar quantized"
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.send(Command::LaunchScene(scene)) {
                                this.mode = Mode::Session;
                            }
                            cx.notify();
                        })),
                    ),
            );
            for track in 0..TRACKS {
                row = row.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .px_1()
                        .border_l_1()
                        .border_color(rgb(Palette::GRID))
                        .child(
                            div()
                                .w_full()
                                .h(px(32.0))
                                .flex()
                                .items_center()
                                .px_2()
                                .border_l_2()
                                .border_color(rgb(TRACK_COLORS[track]))
                                .bg(rgb(Palette::CLIP))
                                .text_xs()
                                .text_color(rgb(Palette::TEXT))
                                .child(theme::text(format!(
                                    "{} · {}",
                                    NAMES[track],
                                    if scene == 0 { "a" } else { "b" }
                                ))),
                        ),
                );
            }
            rows = rows.child(row);
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(rows)
            .child(Self::empty_grid(false))
    }
}

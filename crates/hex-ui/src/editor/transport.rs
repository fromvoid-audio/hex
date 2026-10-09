use super::{
    AddMidiClip, AddTrack, EditorMode, EditorView, ImportAudio, Redo, RemoveTrack, SaveProject,
    SplitClip, Stop, ToggleBrowser, TogglePlay, Undo,
};
use crate::theme::{self, Palette};
use gpui::{Context, Div, Window, div, prelude::*, px, rgb};

impl EditorView {
    pub(super) fn transport(&self, cx: &mut Context<Self>) -> Div {
        let playing = self
            .audio_status()
            .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Relaxed));
        let can_undo = self.state().is_some_and(|state| state.can_undo);
        let can_redo = self.state().is_some_and(|state| state.can_redo);
        let output_label = self.output_label();
        let mut row = div()
            .h(px(38.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            // The transparent macOS titlebar places its traffic lights over this
            // row. Reserve their leading region without widening other panels.
            .pl(px(theme::TITLEBAR_LEADING_INSET))
            .pr_2()
            .border_b_1()
            .border_color(rgb(Palette::BORDER))
            .bg(rgb(Palette::SURFACE))
            .child(
                div()
                    .w(px(38.0))
                    .text_sm()
                    .text_color(rgb(Palette::CORAL))
                    .child(theme::text("hex")),
            )
            .child(self.icon("↓", "import audio · cmd+i", ImportAudio, cx))
            .child(self.icon("▤", "toggle browser · cmd+alt+b", ToggleBrowser, cx))
            .child(self.control_icon(
                "↑",
                "export wav · cmd+shift+e",
                |view, _, cx| view.export_dialog(cx),
                cx,
            ))
            .child(self.icon(
                "▣",
                "save project · cmd+s · save as · cmd+shift+s",
                SaveProject,
                cx,
            ))
            .child(self.icon(
                if playing { "Ⅱ" } else { "▶" },
                "play or pause · space",
                TogglePlay,
                cx,
            ))
            .child(self.icon("■", "stop · escape", Stop, cx))
            .child(
                self.control_icon(
                    "↻",
                    "toggle loop · l · cmd+l sets selected clip",
                    |view, _, _| view.toggle_loop(),
                    cx,
                )
                .text_color(rgb(
                    if self
                        .state()
                        .and_then(|state| state.project.loop_region())
                        .is_some_and(|region| region.enabled())
                    {
                        Palette::CORAL
                    } else {
                        Palette::MUTED
                    },
                )),
            )
            .child(self.icon("✂", "split at playback cursor · cmd+e", SplitClip, cx))
            .child(
                self.icon("↶", "undo · cmd+z", Undo, cx)
                    .opacity(if can_undo { 1.0 } else { 0.45 }),
            )
            .child(
                self.icon("↷", "redo · cmd+shift+z", Redo, cx)
                    .opacity(if can_redo { 1.0 } else { 0.45 }),
            )
            .child(self.icon("+", "add track", AddTrack, cx))
            .child(self.icon(
                "♪",
                "add four-beat midi clip · cmd+shift+m",
                AddMidiClip,
                cx,
            ))
            .child(self.icon("−", "remove selected track", RemoveTrack, cx))
            .child(div().w(px(1.0)).h(px(20.0)).mx_1().bg(rgb(Palette::BORDER)))
            .child(self.control_icon(
                "←",
                "pan left · alt+left",
                |view, _, _| view.pan_timeline(false),
                cx,
            ))
            .child(self.control_icon(
                "→",
                "pan right · alt+right",
                |view, _, _| view.pan_timeline(true),
                cx,
            ))
            .child(self.control_icon(
                "z+",
                "zoom in · cmd++",
                |view, _, _| view.zoom_timeline(true),
                cx,
            ))
            .child(self.control_icon(
                "z−",
                "zoom out · cmd+-",
                |view, _, _| view.zoom_timeline(false),
                cx,
            ))
            .children([self.snap_control(cx)])
            .child(self.control_icon(
                "fit",
                "fit timeline · cmd+0",
                |view, _, _| view.fit_timeline(),
                cx,
            ))
            .child(
                self.control_icon(
                    "▤",
                    "show arrangement",
                    |view, _, _| view.set_mode(EditorMode::Arrangement),
                    cx,
                )
                .text_color(rgb(if self.mode == EditorMode::Arrangement {
                    Palette::CYAN
                } else {
                    Palette::MUTED
                })),
            )
            .child(
                self.control_icon(
                    "▦",
                    "show session",
                    |view, _, _| view.set_mode(EditorMode::Session),
                    cx,
                )
                .text_color(rgb(if self.mode == EditorMode::Session {
                    Palette::CYAN
                } else {
                    Palette::MUTED
                })),
            )
            .child(
                div()
                    .w(px(82.0))
                    .text_xs()
                    .text_color(rgb(Palette::TEXT))
                    .child(theme::text(self.position_label())),
            )
            .child(div().flex_1())
            .child(self.control_icon(
                "⟳",
                "restart default audio output · stays stopped",
                |view, _, _| view.recover_audio(),
                cx,
            ))
            .child(
                div()
                    .id("audio output status")
                    .flex_1()
                    .max_w(px(230.0))
                    .min_w_0()
                    // Keep a definite wrap width during GPUI's second layout
                    // pass so ellipsis is shaped for the allocated flex width.
                    .line_clamp(1)
                    .text_ellipsis()
                    .text_xs()
                    .text_color(rgb(Palette::MUTED))
                    .tooltip({
                        let description = theme::text(&output_label);
                        move |_, cx| {
                            cx.new(|_| MiniTooltip {
                                description: description.clone(),
                            })
                            .into()
                        }
                    })
                    .child(theme::text(&output_label)),
            );
        if self.state().is_none() {
            row = row.child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(Palette::MUTED))
                    .child(theme::text("loading")),
            );
        }
        row
    }

    fn icon(
        &self,
        label: &'static str,
        description: &'static str,
        action: impl gpui::Action,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        self.control_icon(
            label,
            description,
            move |_, window, cx| {
                window.dispatch_action(action.boxed_clone(), cx);
            },
            cx,
        )
    }

    fn control_icon(
        &self,
        label: &'static str,
        description: &'static str,
        handler: impl Fn(&mut EditorView, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id(description)
            .w(px(25.0))
            .h(px(25.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_sm()
            .bg(rgb(Palette::RAISED))
            .text_color(rgb(Palette::TEXT))
            .hover(|style| style.bg(rgb(Palette::HOVER)))
            .tooltip(move |_, cx| {
                cx.new(|_| MiniTooltip {
                    description: description.into(),
                })
                .into()
            })
            .on_click(cx.listener(move |view, _, window, cx| {
                handler(view, window, cx);
                cx.notify();
            }))
            .child(theme::text(label))
    }

    fn snap_control(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        div()
            .id("clip move snap grid")
            .w(px(36.0))
            .h(px(25.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_xs()
            .bg(rgb(Palette::RAISED))
            .text_color(rgb(Palette::TEXT))
            .hover(|style| style.bg(rgb(Palette::HOVER)))
            .tooltip(|_, cx| {
                cx.new(|_| MiniTooltip {
                    description: "clip move snap · click to cycle off, quarter, eighth, sixteenth"
                        .into(),
                })
                .into()
            })
            .on_click(cx.listener(|view, _, _, cx| {
                view.cycle_snap_grid();
                cx.notify();
            }))
            .child(theme::text(self.snap_grid.label()))
    }

    fn position_label(&self) -> String {
        let frame = self.cursor_position();
        let rate = self
            .state()
            .map_or(48_000_u64, |state| u64::from(state.project.sample_rate()));
        let millis = frame.saturating_mul(1_000) / rate.max(1);
        format!(
            "{}:{:02}.{:03}",
            millis / 60_000,
            (millis / 1_000) % 60,
            millis % 1_000
        )
    }

    fn output_label(&self) -> String {
        self.audio_status().map_or_else(
            || "offline".into(),
            |_| self.audio_label().unwrap_or("audio ready").to_string(),
        )
    }
}

pub(super) struct MiniTooltip {
    pub(super) description: gpui::SharedString,
}

impl Render for MiniTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .border_1()
            .border_color(rgb(Palette::BORDER))
            .bg(rgb(Palette::RAISED))
            .font_family(theme::FONT_FAMILY)
            .text_xs()
            .text_color(rgb(Palette::TEXT))
            .child(theme::text(&self.description))
    }
}

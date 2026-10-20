use super::{
    AddMidiClip, AddTrack, EditorMode, EditorView, ImportAudio, Redo, RemoveTrack, SplitClip, Stop,
    ToggleBrowser, TogglePlay, Undo, header,
};
use crate::theme;
use gpui::{Context, Div, Stateful, Window, div, prelude::*, px, rgb};

impl EditorView {
    pub(super) fn transport(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let playing = self
            .audio_status()
            .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Relaxed));
        let can_undo = self.state().is_some_and(|state| state.can_undo);
        let can_redo = self.state().is_some_and(|state| state.can_redo);
        let output_label = self.output_label();
        let keyboard_description = format!(
            "computer midi keyboard · m · base midi {} · velocity {} · z/x octave · c/v velocity",
            self.computer_keyboard.base_pitch, self.computer_keyboard.velocity
        );
        let field = self.project_tempo_input(cx);
        let mut row = header::toolbar_row()
            .child(field)
            .child(
                self.control_icon(
                    "tuning",
                    "project tuning · cmd+alt+t",
                    |view, window, cx| view.open_tuning(window, cx),
                    cx,
                )
                .w(px(58.0)),
            )
            .child(self.icon("↓", "import audio · cmd+i", ImportAudio, cx))
            .child(self.icon("▤", "toggle browser · cmd+alt+b", ToggleBrowser, cx))
            .child(self.control_icon(
                "↑",
                "export wav · cmd+shift+e",
                |view, window, cx| view.export_dialog(window, cx),
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
                self.control_icon_description(
                    "⌨",
                    "computer midi keyboard",
                    keyboard_description.into(),
                    |view, window, cx| view.toggle_computer_keyboard(window, cx),
                    cx,
                )
                .text_color(rgb(theme::color(if self.computer_keyboard.enabled {
                    theme::PaletteRole::Primary
                } else {
                    theme::PaletteRole::Muted
                }))),
            )
            .child(
                self.control_icon(
                    "m",
                    "toggle metronome",
                    |view, _, _| view.toggle_metronome(),
                    cx,
                )
                .opacity(if self.metronome_pending() { 0.45 } else { 1.0 })
                .text_color(rgb(theme::color(if self.metronome_enabled() {
                    theme::PaletteRole::Primary
                } else {
                    theme::PaletteRole::Muted
                }))),
            )
            .child(
                self.control_icon(
                    "↻",
                    "toggle loop on/off · l or cmd+l sets selection",
                    |view, _, _| view.toggle_loop(),
                    cx,
                )
                .text_color(rgb(
                    if self
                        .state()
                        .and_then(|state| state.project.loop_region())
                        .is_some_and(|region| region.enabled())
                    {
                        theme::color(theme::PaletteRole::Primary)
                    } else {
                        theme::color(theme::PaletteRole::Muted)
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
            .child(
                div()
                    .w(px(1.0))
                    .h(px(20.0))
                    .mx(px(1.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Border))),
            )
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
                    "show arrange view · tab",
                    |view, _, _| view.set_mode(EditorMode::Arrangement),
                    cx,
                )
                .text_color(rgb(if self.mode == EditorMode::Arrangement {
                    theme::color(theme::PaletteRole::SecondaryText)
                } else {
                    theme::color(theme::PaletteRole::Muted)
                })),
            )
            .child(
                self.control_icon(
                    "▦",
                    "show console view · tab",
                    |view, _, _| view.set_mode(EditorMode::Session),
                    cx,
                )
                .text_color(rgb(if self.mode == EditorMode::Session {
                    theme::color(theme::PaletteRole::SecondaryText)
                } else {
                    theme::color(theme::PaletteRole::Muted)
                })),
            )
            .child(
                div()
                    .debug_selector({
                        let name = self.workspace_view_name();
                        move || name.to_owned()
                    })
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_xs()
                    .child(theme::text(self.workspace_view_name())),
            )
            .child(
                div()
                    .debug_selector(|| "project position".into())
                    // Size the short readout to its text; retain the existing
                    // upper bound for unusually long project positions.
                    .max_w(px(82.0))
                    .flex_shrink_0()
                    .line_clamp(1)
                    .text_ellipsis()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Text)))
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
                    .debug_selector(|| "audio output status".into())
                    // Status takes available width before the empty spacer;
                    // a zero flex basis could collapse it at minimum width.
                    .w(px(230.0))
                    .max_w(px(230.0))
                    .min_w_0()
                    // Keep a definite wrap width during GPUI's second layout
                    // pass so ellipsis is shaped for the allocated flex width.
                    .line_clamp(1)
                    .text_ellipsis()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
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
                    .debug_selector(|| "project loading".into())
                    .px_2()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("loading")),
            );
        }
        row
    }

    pub(super) fn title_bar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let title = header::project_title(
            self.state()
                .and_then(|state| state.document_path.as_deref()),
        );
        header::title_bar(title, |control| match control {
            header::TitlebarControl::Close => Box::new(cx.listener(|view, _, window, cx| {
                if view.request_window_close(window, cx) {
                    window.remove_window();
                }
            })),
            header::TitlebarControl::Minimize => {
                Box::new(cx.listener(|_, _, window, _| window.minimize_window()))
            }
            header::TitlebarControl::Fullscreen => {
                Box::new(cx.listener(|_, event: &gpui::ClickEvent, window, _| {
                    if event.modifiers().alt {
                        window.zoom_window();
                    } else {
                        window.toggle_fullscreen();
                    }
                }))
            }
        })
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
        self.control_icon_description(label, description, description.into(), handler, cx)
    }

    fn control_icon_description(
        &self,
        label: &'static str,
        description: &'static str,
        tooltip_description: gpui::SharedString,
        handler: impl Fn(&mut EditorView, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id(description)
            .debug_selector(move || description.into())
            .w(px(header::CONTROL_SIZE))
            .h(px(header::CONTROL_SIZE))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_xs()
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
            .tooltip(move |_, cx| {
                cx.new(|_| MiniTooltip {
                    description: tooltip_description.clone(),
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
            .debug_selector(|| "arranger-snap-toggle".to_owned())
            .w(px(68.0))
            .h(px(header::CONTROL_SIZE))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_xs()
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
            .tooltip(|_, cx| {
                cx.new(|_| MiniTooltip {
                    description: "arranger snap · click to toggle adaptive snapping".into(),
                })
                .into()
            })
            .on_click(cx.listener(|view, _, _, cx| {
                view.toggle_snap_grid();
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
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .font_family(theme::FONT_FAMILY)
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(theme::text(&self.description))
    }
}

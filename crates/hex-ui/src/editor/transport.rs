use super::{
    AddMidiClip, AddTrack, EditorMode, EditorView, ImportAudio, Redo, RemoveTrack, SplitClip, Stop,
    ToggleBrowser, TogglePlay, Undo, header,
};
use crate::theme;
use gpui::{Context, Div, Stateful, Window, div, prelude::*, px, rgb};

#[derive(Clone, Copy)]
enum TransportIcon {
    Play,
    Pause,
    Stop,
    Split,
    Undo,
    Redo,
    AddTrack,
    AddMidiClip,
    RemoveTrack,
}

impl TransportIcon {
    fn asset(self) -> (&'static str, &'static str) {
        match self {
            Self::Play => ("icons/transport/play.svg", "transport-play-icon"),
            Self::Pause => ("icons/transport/pause.svg", "transport-pause-icon"),
            Self::Stop => ("icons/transport/stop.svg", "transport-stop-icon"),
            Self::Split => ("icons/transport/split.svg", "transport-split-icon"),
            Self::Undo => ("icons/transport/undo.svg", "transport-undo-icon"),
            Self::Redo => ("icons/transport/redo.svg", "transport-redo-icon"),
            Self::AddTrack => ("icons/transport/add-track.svg", "transport-add-track-icon"),
            Self::AddMidiClip => (
                "icons/transport/add-midi-clip.svg",
                "transport-add-midi-icon",
            ),
            Self::RemoveTrack => (
                "icons/transport/remove-track.svg",
                "transport-remove-track-icon",
            ),
        }
    }
}

impl EditorView {
    pub(super) fn transport(&mut self, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let playing = self
            .audio_status()
            .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Relaxed));
        let can_undo = self.state().is_some_and(|state| state.can_undo);
        let can_redo = self.state().is_some_and(|state| state.can_redo);
        let automation_active =
            self.automation_view.enabled && self.mode == EditorMode::Arrangement;
        let automation_hint = "automation view · a · all arrangement tracks · track gain";
        let mut automation_style = theme::action_style();
        automation_style.text = theme::color(theme::PaletteRole::Muted);
        if automation_active {
            let style = theme::control_style();
            automation_style.background = style.accent;
            automation_style.text = style.accent_text;
            automation_style.hover_background = style.accent;
            automation_style.hover_text = Some(style.accent_text);
        }
        let output_label = self.output_label();
        let keyboard_description = format!(
            "computer midi keyboard · m · base midi {} · velocity {} · z/x octave · c/v velocity",
            self.computer_keyboard.base_pitch, self.computer_keyboard.velocity
        );
        let field = self.project_tempo_input(cx);
        let controls = div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .child(field)
            .child(super::gain_recording::retained_controls(self, cx))
            .child(self.control_icon(
                "♯",
                "project tuning · cmd+alt+t",
                |view, window, cx| view.open_tuning(window, cx),
                cx,
            ))
            .child(self.icon("↓", "import audio · cmd+i", ImportAudio, cx))
            .child(
                self.icon("▤", "toggle browser · cmd+alt+b", ToggleBrowser, cx)
                    .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS)),
            )
            .child(self.control_icon(
                "↑",
                "export wav · cmd+shift+e",
                |view, window, cx| view.export_dialog(window, cx),
                cx,
            ))
            .child(
                self.vector_icon(
                    if playing {
                        TransportIcon::Pause
                    } else {
                        TransportIcon::Play
                    },
                    "play or pause · space",
                    TogglePlay,
                    cx,
                )
                .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS)),
            )
            .child(
                self.vector_icon(TransportIcon::Stop, "stop · escape", Stop, cx)
                    .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS)),
            )
            .child(
                self.control_icon(
                    "●",
                    self.midi_recorder.label(),
                    |view, _, cx| {
                        view.toggle_midi_recording();
                        cx.notify();
                    },
                    cx,
                )
                .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
                .text_color(rgb(theme::color(
                    if self.midi_recorder.blocks_edits() {
                        theme::PaletteRole::Primary
                    } else {
                        theme::PaletteRole::Muted
                    },
                ))),
            )
            .children(self.midi_recorder.blocks_edits().then(|| {
                self.control_icon(
                    "×",
                    "discard retained midi take",
                    |view, _, cx| {
                        view.discard_midi_take();
                        cx.notify();
                    },
                    cx,
                )
            }))
            .child(
                self.control_icon(
                    "▶↻",
                    "section autoplay and loop · click a section to launch",
                    |view, _, _| view.toggle_section_autoplay(),
                    cx,
                )
                .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
                .debug_selector(|| "section-autoplay-toggle".into())
                .text_color(rgb(theme::color(if self.section_autoplay {
                    theme::PaletteRole::Primary
                } else {
                    theme::PaletteRole::Muted
                }))),
            )
            .children([self.launch_quantization_control(cx)])
            .children([self.input_quantization_control(cx)])
            .child(
                self.control_icon_description(
                    "⌨",
                    "computer midi keyboard",
                    keyboard_description.into(),
                    theme::action_style(),
                    |view, window, cx| view.toggle_computer_keyboard(window, cx),
                    cx,
                )
                .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
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
                .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
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
                    "toggle loop · cmd+l · l/w set selection or toggle matching loop",
                    |view, _, _| view.toggle_loop(),
                    cx,
                )
                .debug_selector(|| "transport-loop-toggle".into())
                .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
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
            .child(self.vector_icon(
                TransportIcon::Split,
                "split selection edges or edit cursor · e",
                SplitClip,
                cx,
            ))
            .child(
                self.vector_icon(TransportIcon::Undo, "undo · cmd+z", Undo, cx)
                    .opacity(if can_undo { 1.0 } else { 0.45 }),
            )
            .child(
                self.vector_icon(TransportIcon::Redo, "redo · cmd+shift+z", Redo, cx)
                    .opacity(if can_redo { 1.0 } else { 0.45 }),
            )
            .child(self.vector_icon(TransportIcon::AddTrack, "add track", AddTrack, cx))
            .child(self.vector_icon(
                TransportIcon::AddMidiClip,
                "add four-beat midi clip · cmd+shift+m",
                AddMidiClip,
                cx,
            ))
            .child(self.vector_icon(
                TransportIcon::RemoveTrack,
                "remove selected track",
                RemoveTrack,
                cx,
            ))
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
            .child(
                self.control_icon_description(
                    "a",
                    automation_hint,
                    automation_hint.into(),
                    automation_style,
                    |view, _, cx| {
                        view.toggle_automation_view(cx);
                    },
                    cx,
                )
                .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
                .debug_selector(|| "automation-view-toggle".into()),
            )
            .children(self.has_overridden_device_automation().then(|| {
                self.control_icon(
                    "↻",
                    "re-enable all device automation",
                    |view, _, cx| {
                        view.reenable_device_automation(None, cx);
                    },
                    cx,
                )
                .debug_selector(|| "reenable-device-automation".into())
            }))
            .children([self.snap_control(cx)])
            .children([self.section_focus_toggle(cx)])
            .child(self.track_tag_filter_control(cx))
            .child(self.zoom_fit_button(super::zoom_fit::Target::Arrange, false, cx))
            .child(self.zoom_fit_button(super::zoom_fit::Target::Arrange, true, cx));
        let mut status = div()
            .flex()
            .items_center()
            .min_w_0()
            .flex_1()
            .child(
                self.control_icon(
                    "▤",
                    "show arrange view · tab",
                    |view, _, _| view.set_mode(EditorMode::Arrangement),
                    cx,
                )
                .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
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
                .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
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
                                kind: crate::tooltips::Kind::Identification,
                                description: description.clone(),
                            })
                            .into()
                        }
                    })
                    .child(theme::text(&output_label)),
            );
        if self.state().is_none() && self.service.startup_failure().is_none() {
            status = status.child(
                div()
                    .debug_selector(|| "project loading".into())
                    .px_2()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("loading")),
            );
        }
        let narrow = f32::from(window.viewport_size().width) < header::WIDE_TOOLBAR_MIN_WIDTH;
        let row = header::toolbar_row().h(px(header::toolbar_height(f32::from(
            window.viewport_size().width,
        ))));
        if narrow {
            row.flex_col()
                .items_start()
                .children([controls.h(px(header::TOOLBAR_HEIGHT))])
                .children([status
                    .w_full()
                    .h(px(header::TOOLBAR_HEIGHT))
                    .flex_shrink_0()])
        } else {
            row.children([controls, status])
        }
    }

    pub(super) fn title_bar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let title = header::project_title(
            self.state()
                .and_then(|state| state.document_path.as_deref()),
        );
        let description = theme::text(self.cpu_meter.description());
        let cpu_meter = div()
            .id("editor-cpu-meter")
            .debug_selector(|| "editor-cpu-meter".into())
            .w(px(68.0))
            .h_full()
            .relative()
            .flex()
            .items_center()
            .justify_end()
            .flex_shrink_0()
            .text_xs()
            .text_right()
            .cursor_pointer()
            .text_color(rgb(theme::color(if self.cpu_meter.warning() {
                theme::PaletteRole::Primary
            } else {
                theme::PaletteRole::Muted
            })))
            .tooltip(move |_, cx| {
                cx.new(|_| MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: description.clone(),
                })
                .into()
            })
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|view, _, _, cx| {
                let misses = view.audio_status().map_or(0, |status| {
                    status.cpu_meter.reset_peak();
                    status
                        .deadline_misses
                        .load(std::sync::atomic::Ordering::Relaxed)
                });
                view.cpu_meter.reset(misses);
                cx.notify();
            }))
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .h(px(2.0))
                    .w(px(68.0 * self.cpu_meter.fraction()))
                    .bg(rgb(theme::color(if self.cpu_meter.warning() {
                        theme::PaletteRole::Primary
                    } else {
                        theme::PaletteRole::Secondary
                    }))),
            )
            .child(theme::text(self.cpu_meter.label()));
        header::title_bar(title, cpu_meter, |control| match control {
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

    fn vector_icon(
        &self,
        icon: TransportIcon,
        description: &'static str,
        action: impl gpui::Action,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let (path, selector) = icon.asset();
        let style = theme::action_style();
        let hover_text = style.hover_text.unwrap_or(style.text);
        hex_widgets::action_surface(style)
            .group(description)
            .rounded_full()
            .child(
                div().debug_selector(move || selector.into()).child(
                    gpui::svg()
                        .path(path)
                        .size(px(14.0))
                        .text_color(rgb(style.text))
                        .group_hover(description, move |style| style.text_color(rgb(hover_text))),
                ),
            )
            .id(description)
            .debug_selector(move || description.into())
            .w(px(header::CONTROL_SIZE))
            .h(px(header::CONTROL_SIZE))
            .text_xs()
            .tooltip(move |_, cx| {
                cx.new(|_| MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: description.into(),
                })
                .into()
            })
            .on_click(cx.listener(move |_, _, window, cx| {
                window.dispatch_action(action.boxed_clone(), cx);
                cx.notify();
            }))
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
        self.control_icon_description(
            label,
            description,
            description.into(),
            theme::action_style(),
            handler,
            cx,
        )
    }

    fn control_icon_description(
        &self,
        label: &'static str,
        description: &'static str,
        tooltip_description: gpui::SharedString,
        style: hex_widgets::ActionStyle,
        handler: impl Fn(&mut EditorView, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        hex_widgets::action(theme::text(label), style)
            .id(description)
            .debug_selector(move || description.into())
            .w(px(header::CONTROL_SIZE))
            .h(px(header::CONTROL_SIZE))
            .text_xs()
            .tooltip(move |_, cx| {
                cx.new(|_| MiniTooltip {
                    // Dynamic keyboard values and recorder refusal/status text are functional.
                    kind: crate::tooltips::Kind::Identification,
                    description: tooltip_description.clone(),
                })
                .into()
            })
            .on_click(cx.listener(move |view, _, window, cx| {
                handler(view, window, cx);
                cx.notify();
            }))
    }

    fn launch_quantization_control(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        hex_widgets::action(
            theme::text(format!("{} ▾", self.session_quantization.label())),
            theme::action_style(),
        )
        .id("global-launch-quantization")
        .debug_selector(|| "global-launch-quantization".into())
        .w(px(66.0))
        .h(px(header::CONTROL_SIZE))
        .text_xs()
        .tooltip(|_, cx| {
            cx.new(|_| MiniTooltip {
                kind: crate::tooltips::Kind::control("global launch quantization"),
                description: "global launch quantization · sections and console clips".into(),
            })
            .into()
        })
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                view.open_launch_quantization_menu(event.position, window, cx);
                cx.stop_propagation();
            }),
        )
    }

    pub(super) fn open_launch_quantization_menu(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        let previous_focus = window.focused(cx);
        focus.focus(window);
        self.context_menu = Some(crate::context_menu::PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            position,
            icon_grid: None,
            selected: hex_engine::LaunchQuantization::GLOBAL_CHOICES
                .iter()
                .position(|choice| *choice == self.session_quantization)
                .unwrap_or(0),
            focus,
            previous_focus,
            items: hex_engine::LaunchQuantization::GLOBAL_CHOICES
                .into_iter()
                .map(|quantization| crate::context_menu::MenuItem {
                    label: quantization.label(),
                    swatch: None,
                    checked: self.session_quantization == quantization,
                    action: super::context_menu::EditorContextAction::LaunchQuantization {
                        generation: self.project_generation,
                        quantization,
                    },
                })
                .collect(),
        });
        cx.notify();
    }

    fn snap_control(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        hex_widgets::toggle_action(theme::text(self.snap_grid.label()), theme::action_style())
            .id("clip move snap grid")
            .debug_selector(|| "arranger-snap-toggle".to_owned())
            .w(px(68.0))
            .h(px(header::CONTROL_SIZE))
            .text_xs()
            .tooltip(|_, cx| {
                cx.new(|_| MiniTooltip {
                    kind: crate::tooltips::Kind::control("arranger snap"),
                    description: "arranger snap · click to toggle adaptive snapping".into(),
                })
                .into()
            })
            .on_click(cx.listener(|view, _, _, cx| {
                view.toggle_snap_grid();
                cx.notify();
            }))
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
    pub(super) kind: crate::tooltips::Kind,
    pub(super) description: gpui::SharedString,
}

impl Render for MiniTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::tooltips::view(&self.kind, &self.description, cx)
    }
}

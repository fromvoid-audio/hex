//! Piano-roll keyboard strip, audition toggle and toolbar button elements.
use super::*;

impl EditorView {
    pub(super) fn piano_keys(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = self.midi_editor.piano_pitch;
        let held = self.midi_live_pitches();
        let entity = cx.entity();
        let input = self.piano_pitch_input(cx);
        let pointer_layer = input.pointer_layer();
        let mut keys = input
            .handle(
                "midi-piano-keys".into(),
                hex_widgets::SplitterHandleConfig::thin(
                    hex_widgets::SplitterAxis::Horizontal,
                    theme::color(theme::PaletteRole::Surface),
                ),
            )
            .relative()
            .w(px(38.))
            .mt(px(NOTE_GRID_BORDER))
            .h(px(self.piano_grid_height()))
            .flex_shrink_0();
        for pitch in viewport.coverage().rev() {
            let Some((top, bottom)) = viewport.visible_row(pitch) else {
                continue;
            };
            let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            let is_c = pitch % 12 == 0;
            let label = if is_c {
                format!("c{} {}", i16::from(pitch) / 12 - 2, pitch)
            } else {
                pitch.to_string()
            };
            keys = keys.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(relative(top))
                    .h(relative(bottom - top))
                    .flex()
                    .items_center()
                    .pl(px(2.0))
                    .text_size(px(7.0))
                    .debug_selector(move || format!("midi-piano-key-{pitch}"))
                    .text_color(rgb(if held[usize::from(pitch)] {
                        theme::color(theme::PaletteRole::Background)
                    } else if black {
                        theme::color(theme::PaletteRole::Muted)
                    } else {
                        theme::color(theme::PaletteRole::Text)
                    }))
                    .bg(rgb(if held[usize::from(pitch)] {
                        theme::color(theme::PaletteRole::Primary)
                    } else if black {
                        theme::color(theme::PaletteRole::Background)
                    } else {
                        theme::color(theme::PaletteRole::Raised)
                    }))
                    .border_b_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .child(theme::text(label)),
            );
        }
        keys.children([pointer_layer]).child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let wheel_entity = entity.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                        if !phase.capture() || !bounds.contains(&event.position) {
                            return;
                        }
                        // The keyboard owns only the pitch axis: Alt zooms it and a
                        // plain wheel scrolls it; time pan and Cmd zoom are ignored.
                        let (zoom, delta) =
                            match crate::editor::midi_wheel::resolve(
                                event.modifiers,
                                event.delta,
                                event.physical_delta(),
                            ) {
                                Some(crate::editor::midi_wheel::WheelNav::ZoomPitch(delta)) => {
                                    (true, delta)
                                }
                                Some(crate::editor::midi_wheel::WheelNav::Scroll {
                                    pitch, ..
                                }) if pitch != 0.0 => (false, pitch),
                                _ => return,
                            };
                        cx.stop_propagation();
                        wheel_entity.update(cx, |view, cx| {
                            let canvas = gesture_bounds_from(bounds);
                            let changed = if zoom {
                                view.zoom_piano_pitch_wheel_at(
                                    canvas,
                                    f32::from(event.position.y),
                                    delta,
                                )
                            } else {
                                view.scroll_piano_wheel(canvas, delta, 0.0)
                            };
                            if changed {
                                cx.notify();
                            }
                        });
                    });
                },
            )
            .absolute()
            .size_full(),
        )
    }

    pub(super) fn midi_audition_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        const HINT: &str =
            "toggle note audition · selection plays up to 17 notes · live midi stays connected";
        let mut style = theme::action_style();
        if self.midi_editor.audition.enabled {
            let control = theme::control_style();
            style.background = control.accent;
            style.text = control.accent_text;
            style.hover_background = control.accent;
            style.hover_text = Some(control.accent_text);
        }
        let ink = style.text;
        hex_widgets::action_surface(style)
            .id(HINT)
            .debug_selector(|| HINT.into())
            .w(px(25.0))
            .h(px(22.0))
            .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
            .children([gpui::svg()
                .path("icons/midi/audition.svg")
                .size(px(14.0))
                .text_color(rgb(ink))
                .debug_selector(|| "midi-audition-speaker".into())])
            .tooltip(|_, cx| {
                cx.new(|_| crate::editor::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: HINT.into(),
                })
                .into()
            })
            .on_click(cx.listener(|v, _, window, cx| {
                v.midi_editor.audition.enabled = !v.midi_editor.audition.enabled;
                v.cancel_midi_audition();
                if v.midi_editor.audition.enabled {
                    v.computer_keyboard.suspended = false;
                    v.wake_poll();
                }
                v.midi_editor.focus.focus(window);
                cx.notify();
            }))
    }

    pub(in crate::editor) fn midi_button(
        &self,
        label: &'static str,
        tooltip: &'static str,
        click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> gpui::Stateful<Div> {
        self.midi_button_with_id(tooltip, label, tooltip, click)
    }

    pub(in crate::editor) fn midi_button_with_id(
        &self,
        id: &'static str,
        label: &'static str,
        tooltip: &'static str,
        click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> gpui::Stateful<Div> {
        let button = hex_widgets::action(theme::text(label), theme::action_style())
            .id(id)
            .w(px(25.0))
            .h(px(22.0))
            .text_xs()
            .tooltip(move |_, cx| {
                cx.new(|_| crate::editor::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: tooltip.into(),
                })
                .into()
            })
            .on_click(click);
        #[cfg(feature = "ui-test-support")]
        let button = button.debug_selector(move || tooltip.into());
        button
    }
}

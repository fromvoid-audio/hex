//! Compact pinned return/mix controls shared by Arrangement and track headers.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, ElementId, div, prelude::*, px, rgb};
use hex_project::{Command, Track};

const RETURN_ROW_HEIGHT: f32 = super::tracks::TRACK_ROW_HEIGHT;
const RETURN_SECTION_MAX_HEIGHT: f32 = 2.0 * RETURN_ROW_HEIGHT;

pub(super) fn section_height(count: usize) -> f32 {
    (count as f32 * RETURN_ROW_HEIGHT).min(RETURN_SECTION_MAX_HEIGHT)
}

impl EditorView {
    pub(super) fn return_send_menu_overlay(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        if self.mode != super::EditorMode::Arrangement {
            return None;
        }
        let track_id = self.send_menu_track?;
        let state = self.state.as_ref().cloned()?;
        let Some(track) = state.project.track(track_id).cloned() else {
            super::device_panel::retire_removed_send_controls(self, cx);
            self.send_menu_track = None;
            self.focus.focus(window);
            cx.notify();
            return None;
        };
        let Some(track_index) = state
            .project
            .tracks()
            .iter()
            .position(|candidate| candidate.id() == track_id)
        else {
            super::device_panel::retire_removed_send_controls(self, cx);
            self.send_menu_track = None;
            self.focus.focus(window);
            cx.notify();
            return None;
        };
        let Some(row_index) = self.arrangement_row_index(track_index) else {
            // A section selection change can hide an open menu's track. Finalize
            // through its existing owner before retiring that input surface.
            if super::device_panel::finalize_send_controls_for_track(self, cx, track_id) {
                self.send_menu_track = None;
                self.focus.focus(window);
            }
            cx.notify();
            return None;
        };
        let body = self.arrangement_bounds?;
        let scroll_y = f32::from(self.track_scroll.offset().y);
        let expected_top = body.top
            + self
                .arrangement_tracks(&state.project)
                .take(row_index)
                .map(|(_, track)| self.track_height(track.id()))
                .sum::<f32>()
            + scroll_y;
        let expected_bottom = expected_top + self.track_height(track_id);
        let body_bottom = body.top + body.height;
        let row_is_visible = expected_bottom > body.top && expected_top < body_bottom;
        if !row_is_visible {
            if super::device_panel::finalize_send_controls_for_track(self, cx, track_id) {
                self.send_menu_track = None;
                self.focus.focus(window);
            } else {
                self.track_scroll
                    .scroll_to_item(row_index, gpui::ScrollStrategy::Top);
            }
            cx.notify();
            return None;
        }
        if self.arrangement_bounds_epoch != self.drag_layout_generation.wrapping_sub(1) {
            return None;
        }
        let lane = self.drag_lanes.get(&track_id).copied()?;
        if self.drag_lane_epochs.get(&track_id).copied()
            != Some(self.drag_layout_generation.wrapping_sub(1))
            || (lane.top - expected_top).abs() > 1.0
        {
            return None;
        }
        let return_count = state.project.return_tracks().len();
        let menu_height = if return_count == 0 {
            34.0
        } else {
            (6.0 + (return_count as f32 * 26.0)).min(140.0)
        };
        let window_bounds = window.bounds();
        let window_width = f32::from(window_bounds.size.width);
        let window_height = f32::from(window_bounds.size.height);
        let left = (lane.left + lane.width + self.track_header_width() - 245.0)
            .clamp(0.0, (window_width - 245.0).max(0.0));
        let above = lane.top - menu_height - 1.0;
        let top = if above >= 0.0 {
            above
        } else {
            lane.top + self.track_height(track_id) + 1.0
        }
        .clamp(0.0, (window_height - menu_height).max(0.0));
        Some(
            div()
                .id("return-send-menu-overlay")
                .absolute()
                .left(px(left.max(0.0)))
                .top(px(top))
                .child(self.return_send_menu(&track, cx)),
        )
    }

    pub(super) fn return_tracks_view(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let Some(state) = self.state.as_ref().cloned() else {
            return div()
                .id("pinned-return-tracks-empty")
                .h(px(0.0))
                .flex_shrink_0();
        };
        let returns = state.project.return_tracks();
        let viewport = self.visible_viewport(state.timeline_frames);
        let extra = if self.automation_view.enabled {
            returns
                .iter()
                .map(|track| {
                    self.automation_view
                        .devices
                        .extra_height(super::device_panel::DevicePanelOwner::Return(track.id()))
                })
                .sum::<f32>()
        } else {
            0.0
        };
        let height = (section_height(returns.len()) + extra).min(RETURN_SECTION_MAX_HEIGHT);
        let mut rows = div()
            .id("pinned-return-tracks")
            .debug_selector(|| "pinned-return-tracks".to_owned())
            .h(px(height))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .border_color(rgb(theme::color(theme::PaletteRole::Border)));
        for track in returns {
            let id = track.id();
            let selected = self.focused_return == Some(id);
            let muted = track.muted();
            let (active_background, active_text) = super::track_gain::activator_colors(!muted);
            let gain = track.gain();
            let gain_control = if self.document_audio_mode
                == hex_session::DocumentAudioMode::Resident
            {
                div()
                    .w(px(82.0))
                    .flex()
                    .flex_shrink_0()
                    .child(self.return_volume(track, cx))
            } else {
                // Preserve the real explicit command path when held previews are unavailable.
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .child(
                        return_button(
                            format!("return-gain-down-{}", id.value()),
                            "−",
                            "decrease return gain",
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            super::device_panel::enqueue_return_edit(
                                view,
                                Command::ReturnGain {
                                    return_track: id,
                                    gain: (gain - 0.1).clamp(0.0, 2.0),
                                },
                                cx,
                            );
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .w(px(32.0))
                            .text_xs()
                            .text_right()
                            .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
                            .child(theme::text(format!("{:.0}%", gain * 100.0))),
                    )
                    .child(
                        return_button(
                            format!("return-gain-up-{}", id.value()),
                            "+",
                            "increase return gain",
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            super::device_panel::enqueue_return_edit(
                                view,
                                Command::ReturnGain {
                                    return_track: id,
                                    gain: (gain + 0.1).clamp(0.0, 2.0),
                                },
                                cx,
                            );
                            cx.notify();
                        })),
                    )
            };
            let track_name_view = super::track_header::name()
                .id(ElementId::Name(
                    format!("return-track-focus-{}", id.value()).into(),
                ))
                .debug_selector(move || format!("return-track-focus-{}", id.value()))
                .child(self.track_name_label(
                    super::track_name::Target::Return(id),
                    track.name(),
                    cx,
                ));
            let controls = div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(3.0))
                .child(gain_control)
                .child(self.sample_arm_button(
                    super::sample_recorder::Arm::Output(hex_engine::OutputCaptureId::Return(id)),
                    cx,
                ))
                .child(
                    return_button(
                        format!("return-mute-{}", id.value()),
                        if muted { "○" } else { "●" },
                        if muted {
                            "activate return"
                        } else {
                            "deactivate return"
                        },
                    )
                    .w(px(28.0))
                    .h(px(18.0))
                    .bg(rgb(active_background))
                    .text_color(rgb(active_text))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        super::device_panel::enqueue_return_edit(
                            view,
                            Command::ReturnMute {
                                return_track: id,
                                muted: !muted,
                            },
                            cx,
                        );
                        cx.notify();
                    })),
                )
                .child(
                    return_button(
                        format!("return-remove-{}", id.value()),
                        "×",
                        "remove return track",
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        super::device_panel::enqueue_return_edit(
                            view,
                            Command::RemoveReturnTrack(id),
                            cx,
                        );
                        cx.notify();
                    })),
                );
            let header = super::track_header::surface(
                ElementId::Name(format!("return-track-header-{}", id.value()).into()),
                self.track_header_width(),
                RETURN_ROW_HEIGHT,
                selected,
                None,
            )
            .debug_selector(move || format!("return-track-header-{}", id.value()))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(track_name_view)
                            .child(controls),
                    )
                    .children(
                        self.track_performance_indicator(hex_engine::OutputCaptureId::Return(id)),
                    )
                    .children(self.track_latency_indicator(super::track_latency::Owner::Return(id)))
                    .children([super::track_meters::render_return(
                        id,
                        self.track_meters.return_level(id),
                        RETURN_ROW_HEIGHT,
                    )]),
            )
            .children(selected.then(|| {
                super::track_header::selection_outline(format!(
                    "return-selection-outline-{}",
                    id.value()
                ))
            }));
            let automation = super::device_automation::overlay(
                self,
                &state,
                super::device_panel::DevicePanelOwner::Return(id),
                viewport,
                cx,
            );
            let row = div()
                .id(ElementId::Name(
                    format!("return-track-{}", id.value()).into(),
                ))
                .debug_selector(move || format!("return-track-row-{}", id.value()))
                .h(px(RETURN_ROW_HEIGHT))
                .w_full()
                .flex_shrink_0()
                .flex()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .relative()
                        .h_full()
                        .children(automation)
                        .border_b_1()
                        .border_color(rgb(theme::color(theme::PaletteRole::Border))),
                )
                .child(header)
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        view.open_return_name_context_menu(event.position, id, window, cx);
                    }),
                );
            let panel = super::device_automation::rows(
                self,
                &state,
                super::device_panel::DevicePanelOwner::Return(id),
                viewport,
                cx,
            );
            rows = rows.child(row).child(panel);
        }
        rows
    }

    pub(super) fn return_send_menu(
        &mut self,
        track: &Track,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let track_id = track.id();
        let mut menu = div()
            .id(("return-send-menu", track_id.value()))
            .debug_selector(|| format!("return-send-menu-{}", track_id.value()))
            .w(px(245.0))
            .max_h(px(140.0))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .p(px(3.0))
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .shadow_lg();
        let Some(state) = self.state.as_ref().cloned() else {
            return menu;
        };
        let returns = state.project.return_tracks();
        if returns.is_empty() {
            return menu.child(
                div()
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .px(px(4.0))
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("no return tracks")),
            );
        }
        for return_track in returns {
            let return_id = return_track.id();
            let name = return_track.name().to_owned();
            let gain = track
                .sends()
                .iter()
                .find(|send| send.return_track() == return_id)
                .map_or(0.0, hex_project::SendLevel::gain);
            let mut row = div()
                .id(ElementId::Name(
                    format!("return-send-row-{}-{}", track_id.value(), return_id.value()).into(),
                ))
                .debug_selector(|| {
                    format!("return-send-row-{}-{}", track_id.value(), return_id.value())
                })
                .h(px(26.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(3.0))
                .px(px(3.0))
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                .child(div().flex_1().min_w_0().truncate().child(theme::text(name)))
                .child(super::device_panel::return_send_numeric_control(
                    self, cx, track_id, return_id, gain,
                ));
            if gain > 0.0 {
                row = row.bg(rgb(theme::color(theme::PaletteRole::Surface)));
            }
            menu = menu.child(row);
        }
        menu
    }
}

fn return_button(id: String, icon: &'static str, tooltip: &'static str) -> gpui::Stateful<Div> {
    hex_widgets::action(theme::text(icon), theme::action_style())
        .id(ElementId::Name(id.clone().into()))
        .debug_selector(move || id.clone())
        .size(px(15.0))
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
        .cursor(gpui::CursorStyle::PointingHand)
        .tooltip(move |_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: tooltip.into(),
            })
            .into()
        })
}

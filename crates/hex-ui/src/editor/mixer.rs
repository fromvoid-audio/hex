//! Compact canonical mixer controls. All musical changes stay on the actor.
use super::{EditorView, waveform::button};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_project::Command;
use hex_session::service::Request;

impl EditorView {
    pub(super) fn mixer_strip(&mut self, cx: &mut Context<Self>, show_main: bool) -> Div {
        if show_main {
            super::track_gain::retire_stale(self, cx);
        }
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
        let Some(state) = self.state.clone() else {
            return strip;
        };
        if let Some(track) = self.selected_track.and_then(|id| state.project.track(id))
            && !show_main
        {
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
                        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
                        .child(theme::text(balance_label(track.balance()))),
                );
        }
        if show_main {
            let active_gain = self.device_gesture.snapshot().and_then(|snapshot| {
                if snapshot.key.generation != self.project_generation {
                    return None;
                }
                snapshot.target.track_gain_anchor()
            });
            let owner = active_gain
                .or_else(|| {
                    self.track_gain_controls
                        .pending_track_gain(self.project_generation, cx)
                })
                .and_then(|id| state.project.track(id))
                .or_else(|| self.selected_track.and_then(|id| state.project.track(id)));
            if let Some(track) = owner.filter(|track| !self.console_mixer_contains(track.id())) {
                let id = track.id();
                if self.selected_track != Some(id) {
                    strip = strip.child(
                        div()
                            .max_w(px(140.0))
                            .truncate()
                            .debug_selector(|| "console-track-gain-owner".into())
                            .child(theme::text(format!("editing {}", track.name()))),
                    );
                }
                let gain = self.track_volume(track, cx);
                strip = strip.child(
                    div()
                        .w(px(110.0))
                        .flex_shrink_0()
                        .debug_selector(move || format!("console-track-gain-{}", id.value()))
                        .child(gain),
                );
            }
        }
        if let Some(fades) = self.selected_fade_controls(&state, cx) {
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
                .child(self.sample_arm_button(super::sample_recorder::Arm::Main, cx))
                .children([self.main_track_controls(cx)]);
        }
        strip
    }

    pub(super) fn main_track_controls(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .debug_selector(|| "main-track-controls".into())
            .w(px(110.0))
            .h(px(18.0))
            .flex()
            .items_center()
            .child(self.main_volume(cx))
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

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use crate::editor::{
        EditorMode,
        clip_drag_ui_test::{audio_project, editor_window},
    };
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, px, size};

    #[gpui::test]
    fn off_page_numeric_draft_keeps_one_footer_owner(cx: &mut TestAppContext) {
        cx.update(crate::editor::bind_editor_keys);
        let (project, first, second, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(640.0), px(800.0)));
        editor.update(&mut visual, |view, cx| {
            view.mode = EditorMode::Session;
            view.selected_track = Some(first);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.session_visible_columns, 1)
        });
        let field_id = Box::leak(format!("track-volume-{}-value", first.value()).into_boxed_str());
        let field = visual.debug_bounds(field_id).unwrap().center();
        visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("+100 db");
        let next = visual.debug_bounds("next tracks").unwrap().center();
        visual.simulate_mouse_down(next, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(next, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.session_track_start, 0,
                "paging cannot hide an invalid draft"
            )
        });
        // A retained/programmatic selection may still move the page. Its draft
        // then moves into the existing footer instead of being mounted twice.
        editor.update(&mut visual, |view, cx| {
            view.session_track_start = 1;
            view.selected_track = Some(second);
            cx.notify();
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds(field_id).is_some());
        assert!(visual.debug_bounds("console-track-gain-owner").is_some());
        let old_column =
            Box::leak(format!("console-mixer-column-{}", first.value()).into_boxed_str());
        assert!(visual.debug_bounds(old_column).is_none());
        let field = visual.debug_bounds(field_id).unwrap().center();
        visual.simulate_mouse_down(field, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(field, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(visual.debug_bounds(field_id).is_none());
        assert!(visual.debug_bounds("console-track-gain-owner").is_none());
        let previous = visual.debug_bounds("previous tracks").unwrap().center();
        visual.simulate_mouse_down(previous, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(previous, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.session_track_start, 0, "paging resumes after cancel")
        });
    }
}

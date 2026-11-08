//! Console column presentation reuses canonical controls, gestures and meters.
//! The page is shared with the clip grid; no numeric entity is mounted twice.
use super::{
    EditorView,
    console_track_page::{TrackPage, visible_track_range},
    waveform::toggle_button_with_style,
};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_project::TrackId;

pub(super) fn height(send_count: usize) -> f32 {
    70.0 + send_count as f32 * 22.0
}

impl EditorView {
    pub(super) fn console_main_header(
        &self,
        width: f32,
        returns: &super::console_returns::Page,
        frame: super::console_columns::ColumnFrame,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .debug_selector(|| "session-scenes-header".into())
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .child(
                div()
                    .id("console-main-header")
                    .debug_selector(|| "console-main-header".into())
                    .size_full()
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .text_size(px(11.0))
                    .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                    .on_click(cx.listener(|view, _, window, cx| {
                        super::device_panel::select_main(view, window, cx)
                    }))
                    .child(div().flex_1().child(theme::text("main")))
                    .children(self.console_return_paging(returns, frame, cx)),
            )
    }

    pub(super) fn console_mixer_contains(&self, id: TrackId) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        if let Some(page) = self.pending_console_reorder_page() {
            return page.columns.iter().any(|column| {
                column.id == id
                    && state
                        .project
                        .tracks()
                        .get(column.index)
                        .is_some_and(|track| track.id() == id)
            });
        }
        let indices = &self.arrangement_focus.projection.console_indices;
        let range = visible_track_range(
            indices.len(),
            self.session_track_start,
            self.session_visible_columns,
        );
        indices[range].iter().any(|&index| {
            state
                .project
                .tracks()
                .get(index)
                .is_some_and(|track| track.id() == id)
        })
    }

    pub(super) fn console_mixer_row(
        &mut self,
        page: &TrackPage,
        column_width: f32,
        scene_width: f32,
        returns: &super::console_returns::Page,
        cx: &mut Context<Self>,
    ) -> Div {
        super::track_gain::retire_stale(self, cx);
        let mut row = div()
            .h(px(height(returns.range.len())))
            .flex_shrink_0()
            .flex()
            .overflow_hidden()
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)));
        let Some(state) = self.state.clone() else {
            return row;
        };
        for track in page.columns.iter().filter_map(|column| {
            state
                .project
                .tracks()
                .get(column.index)
                .filter(|track| track.id() == column.id)
        }) {
            let id = track.id();
            let (background, text) = super::track_gain::activator_colors(!track.muted());
            let (solo_background, solo_text) =
                super::track_gain::activator_colors(self.displayed_solo(id, track.soloed()));
            let activator = toggle_button_with_style(
                "on",
                if track.muted() {
                    "activate track"
                } else {
                    "deactivate track"
                },
                ("console-track-mute", id.value()),
                theme::action_style().with_state_colors(background, text),
            )
            .debug_selector(move || format!("console-track-mute-{}", id.value()))
            .w(px(24.0))
            .h(px(14.0))
            .rounded(px(2.0))
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .on_click(cx.listener(move |view, _, _, cx| {
                cx.stop_propagation();
                view.toggle_mute(id);
                cx.notify();
            }));
            let solo = toggle_button_with_style(
                "s",
                "solo track · s · cmd+click additive",
                ("console-track-solo", id.value()),
                theme::action_style().with_state_colors(solo_background, solo_text),
            )
            .debug_selector(move || format!("console-track-solo-{}", id.value()))
            .w(px(18.0))
            .h(px(14.0))
            .rounded(px(2.0))
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
                cx.stop_propagation();
                view.toggle_solo(id, !event.modifiers().platform);
                cx.notify();
            }));
            let gain = self.track_volume(track, cx);
            let pan = self.track_pan(track, cx);
            let toggles = div()
                .flex()
                .items_center()
                .gap(px(3.0))
                .child(activator)
                .child(solo)
                .child(self.sample_arm_button(
                    super::sample_recorder::Arm::Output(hex_engine::OutputCaptureId::Track(id)),
                    cx,
                ));
            let mut controls = div().min_w_0().flex_1().flex().flex_col().gap(px(4.0));
            for return_track in state
                .project
                .return_tracks()
                .get(returns.range.clone())
                .unwrap_or_default()
            {
                let return_id = return_track.id();
                let gain = track
                    .sends()
                    .iter()
                    .find(|send| send.return_track() == return_id)
                    .map_or(0.0, hex_project::SendLevel::gain);
                let control =
                    super::device_panel::return_send_numeric_control(self, cx, id, return_id, gain)
                        .w_auto()
                        .min_w(px(60.0))
                        .flex_1();
                let tooltip = theme::text(format!("send to {} · db", return_track.name()));
                controls = controls.child(
                    div()
                        .id(gpui::ElementId::Name(
                            format!("console-send-{}-{}", id.value(), return_id.value()).into(),
                        ))
                        .debug_selector(move || {
                            format!("console-send-{}-{}", id.value(), return_id.value())
                        })
                        .h(px(18.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .tooltip(move |_, cx| {
                            cx.new(|_| super::transport::MiniTooltip {
                                kind: crate::tooltips::Kind::Identification,
                                description: tooltip.clone(),
                            })
                            .into()
                        })
                        .child(
                            div()
                                .w(px(36.0))
                                .flex_shrink_0()
                                .truncate()
                                .text_size(px(10.0))
                                .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
                                .child(theme::text(return_track.name())),
                        )
                        .child(control),
                );
            }
            let controls = controls
                .child(
                    div()
                        .debug_selector(move || format!("console-track-gain-{}", id.value()))
                        .h(px(18.0))
                        .flex_shrink_0()
                        .flex()
                        .child(gain),
                )
                .child(div().h(px(18.0)).flex_shrink_0().flex().child(pan))
                .child(toggles);
            let column = div()
                .debug_selector(move || format!("console-mixer-column-{}", id.value()))
                .w(px(column_width))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_end()
                .pb(px(6.0))
                .gap(px(6.0))
                .px(px(6.0))
                .border_l_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                .child(controls)
                .child(
                    div()
                        .debug_selector(move || format!("console-track-meter-{}", id.value()))
                        .child(super::track_meters::render(
                            id,
                            self.track_meters.level(id),
                            50.0,
                        )),
                );
            row = row.child(column);
        }
        let main = div()
            .debug_selector(|| "console-main-mixer-column".into())
            .w(px(scene_width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_end()
            .pb(px(6.0))
            .gap(px(4.0))
            .px(px(6.0))
            .border_l_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .children([
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(self.main_track_controls(cx))
                    .child(self.sample_arm_button(super::sample_recorder::Arm::Main, cx)),
                super::track_meters::render_main(self.track_meters.main_level(), 50.0),
            ]);
        super::console_columns::pinned_row(
            row.child(div().flex_1().min_w(px(12.0))),
            returns.width(),
            main,
            height(returns.range.len()),
        )
        .debug_selector(|| "console-mixer-row".into())
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::super::clip_drag_ui_test::{audio_project, editor_window};
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext, size};

    #[gpui::test]
    fn console_sends_follow_all_return_pages_and_keep_drafts_on_resize(cx: &mut TestAppContext) {
        cx.update(super::super::bind_editor_keys);
        let (project, first, second, _) = audio_project();
        let mut model = hex_project::Editor::new(project, 32).unwrap();
        for index in 0..16 {
            model
                .execute(hex_project::Command::AddReturnTrack {
                    name: format!("bus {index}"),
                })
                .unwrap();
        }
        let ids: Vec<_> = model
            .project()
            .return_tracks()
            .iter()
            .map(hex_project::ReturnTrack::id)
            .collect();
        let (handle, editor) = editor_window(cx, model.project().clone(), Default::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |view, cx| {
            view.mode = super::super::EditorMode::Session;
            cx.notify();
        });
        visual.simulate_resize(size(px(1400.0), px(800.0)));
        visual.run_until_parked();
        let selector = |track: TrackId, id: hex_project::ReturnId, suffix: &str| {
            Box::leak(
                format!("return-send-{}-{}{suffix}", track.value(), id.value()).into_boxed_str(),
            ) as &'static str
        };
        let field_id = selector(second, ids[1], "-value");
        let field = visual.debug_bounds(field_id).unwrap().center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("unfinished");
        visual.run_until_parked();
        let next = visual.debug_bounds("console-return-next").unwrap().center();
        visual.simulate_click(next, Modifiers::default());
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(field_id).is_some(),
            "paging cannot remove a send draft"
        );
        visual.simulate_resize(size(px(640.0), px(540.0)));
        visual.run_until_parked();
        let field = visual.debug_bounds(field_id).unwrap();
        let column = visual
            .debug_bounds(Box::leak(
                format!("console-mixer-column-{}", second.value()).into_boxed_str(),
            ))
            .unwrap();
        assert!(
            column.contains(&field.center()),
            "both draft axes survive a narrow page"
        );
        editor.read_with(&visual, |view, cx| {
            assert_eq!(
                super::super::device_panel::pending_send(view, cx)
                    .unwrap()
                    .0,
                second
            )
        });
        visual.simulate_click(field.center(), Modifiers::default());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert!(
                super::super::device_panel::pending_send(view, cx).is_none(),
                "escape retires the invalid draft before paging"
            )
        });
        visual.simulate_resize(size(px(1400.0), px(800.0)));
        visual.run_until_parked();
        // Restore the first page through the same public pager the user uses.
        let previous = visual
            .debug_bounds("console-return-previous")
            .unwrap()
            .center();
        visual.simulate_click(previous, Modifiers::default());
        editor.update(&mut visual, |view, cx| {
            view.session_track_start = 0;
            cx.notify();
        });
        visual.run_until_parked();
        for pair in ids.chunks(2) {
            for track in [first, second] {
                let column = visual
                    .debug_bounds(Box::leak(
                        format!("console-mixer-column-{}", track.value()).into_boxed_str(),
                    ))
                    .unwrap();
                let header = visual
                    .debug_bounds(Box::leak(
                        format!("session-track-{}", track.value()).into_boxed_str(),
                    ))
                    .unwrap();
                assert_eq!(column.left(), header.left());
                for id in pair {
                    let field = visual
                        .debug_bounds(selector(track, *id, "-value"))
                        .unwrap_or_else(|| {
                            panic!("missing send field for track {track:?}, Return {id:?}")
                        });
                    assert!(column.contains(&field.center()));
                    let row = visual
                        .debug_bounds(Box::leak(
                            format!("console-send-{}-{}", track.value(), id.value())
                                .into_boxed_str(),
                        ))
                        .unwrap();
                    assert_eq!(row.size.height, px(18.0));
                    assert!(row.contains(&field.center()));
                    assert!(field.size.width >= px(60.0));
                    assert!(field.right() <= column.right());
                }
            }
            let next = visual.debug_bounds("console-return-next").unwrap().center();
            visual.simulate_click(next, Modifiers::default());
            visual.run_until_parked();
        }
        assert_eq!(
            editor.read_with(&visual, |view, _| view
                .state
                .as_ref()
                .unwrap()
                .project
                .clone()),
            model.project().clone().into(),
            "presentation and canceled drafts do not edit the project"
        );
    }
    #[gpui::test]
    fn console_columns_align_and_keep_distinct_controls(cx: &mut TestAppContext) {
        let (project, first, second, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |view, cx| {
            view.mode = super::super::EditorMode::Session;
            view.selected_track = Some(first);
            cx.notify();
        });
        visual.run_until_parked();
        for id in [first, second] {
            let selector = |prefix: &str| {
                Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str()) as &'static str
            };
            let column = visual
                .debug_bounds(selector("console-mixer-column"))
                .unwrap();
            let header = visual.debug_bounds(selector("session-track")).unwrap();
            assert_eq!(column.left(), header.left());
            assert_eq!(column.size.width, header.size.width);
            let gain = visual.debug_bounds(selector("console-track-gain")).unwrap();
            let mute = visual.debug_bounds(selector("console-track-mute")).unwrap();
            let solo = visual.debug_bounds(selector("console-track-solo")).unwrap();
            assert_eq!(mute.size.height, px(14.0));
            assert_eq!(solo.size.width, px(18.0));
            assert!(gain.top() < mute.top() && mute.bottom() <= column.bottom());
            assert!(visual.debug_bounds(selector("track-pan")).is_some());
            assert!(
                visual
                    .debug_bounds(selector("console-track-meter"))
                    .is_some()
            );
        }
        assert!(
            visual.debug_bounds("console-track-gain-owner").is_none(),
            "visible controls are not mounted again in the footer"
        );
    }

    #[gpui::test]
    fn pinned_main_column_aligns_and_selects_its_actual_device_chain(cx: &mut TestAppContext) {
        let (project, first, _, _) = audio_project();
        let mut model = hex_project::Editor::new(project, 8).unwrap();
        let hex_project::Created::Scene(scene) = model
            .execute(hex_project::Command::AddScene { name: "one".into() })
            .unwrap()
        else {
            panic!("scene result")
        };
        let (handle, editor) = editor_window(cx, model.project().clone(), Default::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |view, cx| {
            view.mode = super::super::EditorMode::Session;
            view.selected_track = Some(first);
            cx.notify();
        });
        for width in [640.0, 1200.0] {
            visual.simulate_resize(size(px(width), px(800.0)));
            visual.run_until_parked();
            let header = visual.debug_bounds("console-main-header").unwrap();
            let scenes = visual.debug_bounds("session-scenes-header").unwrap();
            let column = visual.debug_bounds("console-main-mixer-column").unwrap();
            let slider = visual.debug_bounds("main-volume-slider").unwrap();
            let arm = visual.debug_bounds("sample-arm-main-0").unwrap();
            let scene_label = visual
                .debug_bounds(Box::leak(
                    format!("console-scene-menu-{}", scene.value()).into_boxed_str(),
                ))
                .unwrap();
            assert_eq!(header, scenes);
            assert_eq!(header.left(), column.left());
            assert_eq!(header.size.width, column.size.width);
            assert_eq!(scene_label.left(), column.left() + px(8.0));
            assert!(slider.left() >= column.left() && slider.right() <= column.right());
            assert!(slider.top() >= column.top() && slider.bottom() <= column.bottom());
            assert!(arm.top() >= slider.bottom() && arm.bottom() <= column.bottom());
            assert!(visual.debug_bounds("main-gain-up").is_none());
        }
        let header = visual.debug_bounds("console-main-header").unwrap();
        visual.simulate_click(header.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.selected_track.is_none());
            assert!(view.focused_return.is_none());
            assert_eq!(
                view.device_panel.main_selected,
                Some(view.project_generation)
            );
            assert!(view.device_panel_active && view.clip_view_open);
        });
    }
}

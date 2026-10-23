use super::clip_drag_ui_test::{audio_project, editor_window};
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use std::sync::Arc;

#[gpui::test]
fn track_headers_toggle_range_and_render_explicit_members_including_empty_tracks(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    let (project, source, empty, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let names = [source, empty]
        .map(|id| Box::leak(format!("track-name-{}", id.value()).into_boxed_str()) as &'static str);
    let first = visual.debug_bounds(names[0]).expect("source name").center();
    let last = visual.debug_bounds(names[1]).expect("empty name").center();
    visual.simulate_click(first, Modifiers::default());
    visual.simulate_click(
        last,
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_track_ids(), [source, empty])
    });
    for id in [source, empty] {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("selected-track-{}", id.value()).into_boxed_str()
                ))
                .is_some()
        );
    }
    visual.simulate_click(
        first,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert_eq!(view.selected_track_ids(), [empty]));
    visual.simulate_click(
        last,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.selected_track_ids().is_empty()));
    visual.simulate_keystrokes("up");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_track_ids(), [source]);
        assert_eq!(
            view.status, "track selected",
            "keyboard selection updates member-count feedback"
        );
    });
    visual.simulate_click(last, Modifiers::default());
    let clip_bounds = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("audio clip");
    visual.simulate_click(clip_bounds.center(), Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.selected_track_ids(),
            [source],
            "a clip click selects its unselected canonical owner"
        );
    });
    visual.simulate_click(first, Modifiers::default());
    editor.update(cx, |view, _| {
        view.time_selection_project_scope = true;
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: source,
            end_track: empty,
            anchor_frame: 0,
            end_frame: 48_000,
        });
        assert_eq!(
            view.selected_track_ids(),
            [source],
            "time-highlight tracks are not selected source members"
        );
    });
}

#[gpui::test]
fn explicit_track_members_survive_insertions_and_prune_removed_ids(cx: &mut TestAppContext) {
    let (project, source, empty, _) = audio_project();
    let (_handle, editor) = editor_window(cx, project.clone(), Arc::default());
    editor.update(cx, |view, _| {
        assert!(view.track_selection.select(&project, source, false, false));
        assert!(view.track_selection.select(&project, empty, false, true));
        let mut model = hex_project::Editor::new(project, 8).expect("history");
        let hex_project::Created::Track(new) = model
            .execute(hex_project::Command::AddTrack { name: "new".into() })
            .expect("insert")
        else {
            panic!("track");
        };
        view.track_selection.reconcile(model.project());
        assert_eq!(
            view.track_selection.ordered(model.project()),
            [source, empty]
        );
        assert!(!view.track_selection.contains(new));
        model
            .execute(hex_project::Command::RemoveTrack(source))
            .expect("remove");
        view.track_selection.reconcile(model.project());
        assert_eq!(view.track_selection.ordered(model.project()), [empty]);
        assert!(model.undo());
        view.track_selection.reconcile(model.project());
        assert_eq!(
            view.track_selection.ordered(model.project()),
            [empty],
            "Undo does not invent explicit membership for a removed ID"
        );
    });
}

#[gpui::test]
fn export_sources_use_explicit_ids_and_names_not_time_scope_or_focused_track(
    cx: &mut TestAppContext,
) {
    let (project, source, empty, _) = audio_project();
    let mut model = hex_project::Editor::new(project, 8).expect("history");
    let hex_project::Created::Track(third) = model
        .execute(hex_project::Command::AddTrack {
            name: "Unselected Case".into(),
        })
        .expect("third")
    else {
        panic!("track");
    };
    let (_handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
    editor.update(cx, |view, _| {
        assert!(
            view.track_selection
                .select(model.project(), source, false, false)
        );
        assert!(
            view.track_selection
                .select(model.project(), empty, false, true)
        );
        view.selected_track = Some(third);
        view.time_selection_project_scope = true;
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: source,
            end_track: third,
            anchor_frame: 0,
            end_frame: 48_000,
        });
        let stems = view.selected_export_tracks();
        assert_eq!(stems.len(), 2);
        assert_eq!(stems[0].id, hex_engine::OutputCaptureId::Track(source));
        assert_eq!(stems[1].id, hex_engine::OutputCaptureId::Track(empty));
        assert_eq!(
            stems[0].name,
            model.project().track(source).expect("source").name()
        );
        assert_eq!(
            stems[1].name,
            model.project().track(empty).expect("empty").name()
        );
        assert!(
            !stems
                .iter()
                .any(|stem| stem.id == hex_engine::OutputCaptureId::Track(third))
        );
        assert!(
            view.track_selection
                .select(model.project(), source, false, true)
        );
        assert!(
            view.track_selection
                .select(model.project(), empty, false, true)
        );
        assert!(
            view.selected_export_tracks().is_empty(),
            "zero explicit members never fall back to a focused or time-highlight track"
        );
    });
}

#[gpui::test]
fn selected_track_lanes_paint_subtle_tint_and_both_edges_only_for_explicit_members(
    cx: &mut TestAppContext,
) {
    let (project, source, empty, _) = audio_project();
    let before = project.snapshot();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = |prefix: &str, id: hex_project::TrackId| {
        Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str()) as &'static str
    };
    let names = [source, empty].map(|id| {
        visual
            .debug_bounds(selector("track-name", id))
            .unwrap()
            .center()
    });
    for id in [source, empty] {
        assert!(
            visual
                .debug_bounds(selector("selected-track-lane", id))
                .is_none()
        );
    }
    for (position, modifiers, selected) in [
        (names[0], Modifiers::default(), vec![source]),
        (
            names[1],
            Modifiers {
                shift: true,
                ..Default::default()
            },
            vec![source, empty],
        ),
        (
            names[0],
            Modifiers {
                platform: true,
                ..Default::default()
            },
            vec![empty],
        ),
        (
            names[1],
            Modifiers {
                platform: true,
                ..Default::default()
            },
            vec![],
        ),
    ] {
        visual.simulate_click(position, modifiers);
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_track_ids(), selected);
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
        for id in [source, empty] {
            let active = selected.contains(&id);
            for prefix in [
                "selected-track-lane",
                "selected-track-lane-top",
                "selected-track-lane-bottom",
            ] {
                let name = selector(prefix, id);
                assert_eq!(
                    visual.debug_bounds(name).is_some(),
                    active,
                    "{prefix} {id:?}"
                );
                if active {
                    assert!(
                        visual.rendered_background(name).is_some(),
                        "actual completed selection paint"
                    );
                }
            }
        }
    }
}

#[gpui::test]
fn selected_lane_palette_and_paint_order_preserve_clip_fill_and_outer_selection_stroke(
    cx: &mut TestAppContext,
) {
    use crate::theme;
    struct Restore(hex_preferences::ThemePreferences);
    impl Drop for Restore {
        fn drop(&mut self) {
            theme::apply_preferences(self.0);
        }
    }
    let _restore = Restore(theme::preferences());
    let (project, source, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let name = Box::leak(format!("track-name-{}", source.value()).into_boxed_str());
    let at = visual.debug_bounds(name).unwrap().center();
    visual.simulate_click(at, Modifiers::default());
    let clip_name = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
    let at = visual.debug_bounds(clip_name).unwrap().center();
    visual.simulate_click(at, Modifiers::default());
    let tint = Box::leak(format!("selected-track-lane-{}", source.value()).into_boxed_str());
    let clip_visual = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    for palette in [
        hex_preferences::ThemePreferences::default(),
        hex_preferences::ThemePreferences {
            background: hex_preferences::RgbColor::new(240, 240, 240),
            text: hex_preferences::RgbColor::new(16, 16, 16),
            primary: hex_preferences::RgbColor::new(166, 65, 47),
            secondary: hex_preferences::RgbColor::new(40, 88, 170),
        },
    ] {
        theme::apply_preferences(palette);
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_background(tint),
            Some(
                gpui::Hsla::from(gpui::rgb(theme::color(theme::PaletteRole::Secondary)))
                    .alpha(theme::TRACK_SELECTION_OPACITY)
                    .into()
            )
        );
        let fill = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            theme::clip_fill(
                p.track(source).unwrap().color(),
                p.clip(clip).unwrap().flavor(),
                true,
            )
        });
        assert_eq!(
            visual.rendered_background(clip_visual),
            Some(gpui::rgb(fill).into())
        );
        assert_eq!(
            visual.rendered_border(clip_visual).unwrap().0,
            gpui::Hsla::from(gpui::rgb(theme::color(theme::PaletteRole::Primary)))
        );
        let tint_layers = visual.rendered_quad_paint_layers(tint, false);
        let clip_layers = visual.rendered_quad_paint_layers(clip_visual, false);
        assert!(!tint_layers.is_empty() && !clip_layers.is_empty());
        assert!(
            tint_layers.iter().map(|x| x.0).max().unwrap()
                < clip_layers.iter().map(|x| x.0).min().unwrap()
        );
        let lane = visual.debug_bounds(tint).unwrap();
        let scale = visual.update(|window, _| window.scale_factor());
        for edge in ["top", "bottom"] {
            let selector = Box::leak(
                format!("selected-track-lane-{edge}-{}", source.value()).into_boxed_str(),
            );
            let bounds = visual.debug_bounds(selector).unwrap();
            assert!((f32::from(bounds.size.height) * scale - 1.0).abs() < 0.001);
            assert!(
                bounds.top() >= lane.top() + gpui::px(1.0)
                    && bounds.bottom() <= lane.bottom() - gpui::px(1.0)
            );
            assert_eq!(
                visual.rendered_background(selector),
                Some(
                    gpui::Hsla::from(gpui::rgb(theme::color(theme::PaletteRole::Secondary)))
                        .alpha(theme::TRACK_SELECTION_EDGE_OPACITY)
                        .into()
                )
            );
        }
    }
}

#[gpui::test]
fn removed_track_retires_selected_lane_paint_after_actual_worker_completion(
    cx: &mut TestAppContext,
) {
    use super::drag::group_drag_ui_test::{fixture, pump};
    use hex_session::service::Request;
    let (_files, handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let source = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(hex_project::Command::AddTrack {
            name: "empty".into()
        })))
    });
    pump(&editor, &mut visual);
    let empty = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[1].id()
    });
    for (id, shift) in [(source, false), (empty, true)] {
        let name = Box::leak(format!("track-name-{}", id.value()).into_boxed_str());
        let at = visual.debug_bounds(name).unwrap().center();
        visual.simulate_click(
            at,
            Modifiers {
                shift,
                ..Default::default()
            },
        );
    }
    visual.run_until_parked();
    let source_tint = Box::leak(format!("selected-track-lane-{}", source.value()).into_boxed_str());
    let empty_tint = Box::leak(format!("selected-track-lane-{}", empty.value()).into_boxed_str());
    assert!(
        visual.rendered_background(source_tint).is_some()
            && visual.rendered_background(empty_tint).is_some()
    );
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(hex_project::Command::RemoveTrack(source))))
    });
    pump(&editor, &mut visual);
    visual.run_until_parked();
    assert!(visual.debug_bounds(source_tint).is_none());
    assert!(visual.rendered_background(empty_tint).is_some());
    editor.read_with(&visual, |v, _| assert_eq!(v.selected_track_ids(), [empty]));
}

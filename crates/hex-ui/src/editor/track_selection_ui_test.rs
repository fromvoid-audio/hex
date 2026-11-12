use super::clip_drag_ui_test::{audio_project, editor_window};
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use std::sync::Arc;

#[gpui::test]
fn track_switch_observation_paints_without_installing_project_and_rejects_stale_scope(
    cx: &mut TestAppContext,
) {
    use hex_session::service::{TrackSwitchState, TrackSwitchViewObservation};
    let (project, first, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (generation, revision) = editor.read_with(&visual, |view, _| {
        (
            view.project_generation,
            view.state.as_ref().unwrap().revision,
        )
    });
    let selector = Box::leak(format!("track-solo-{}", first.value()).into_boxed_str());
    let before = visual.rendered_background(selector);
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_track_switch_view(TrackSwitchViewObservation {
            project_generation: generation,
            first_previous_revision: revision,
            previous_revision: revision,
            revision: revision + 1,
            tracks: vec![TrackSwitchState {
                track: first,
                muted: false,
                soloed: true,
            }],
        }));
        assert!(
            !view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(first)
                .unwrap()
                .soloed()
        );
        cx.notify();
    });
    visual.run_until_parked();
    let after = visual.rendered_background(selector);
    assert_ne!(before, after);
    editor.update(&mut visual, |view, cx| {
        let track = view.state.as_ref().unwrap().project.track(first).unwrap();
        assert_eq!(view.displayed_track_switches(track), (false, true));
        assert!(!view.observe_track_switch_view(TrackSwitchViewObservation {
            project_generation: generation + 1,
            first_previous_revision: revision,
            previous_revision: revision,
            revision: revision + 1,
            tracks: vec![TrackSwitchState {
                track: first,
                muted: false,
                soloed: false,
            }],
        }));
        let track = view.state.as_ref().unwrap().project.track(first).unwrap();
        assert_eq!(view.displayed_track_switches(track), (false, true));
        view.project_generation += 1;
        assert_eq!(view.displayed_track_switches(track), (false, false));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.rendered_background(selector), before);
}

#[gpui::test]
fn coalesced_track_switch_view_requires_a_valid_contiguous_span(cx: &mut TestAppContext) {
    use hex_session::service::{TrackSwitchState, TrackSwitchViewObservation};
    let (project, track, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        let revision = view.state.as_ref().unwrap().revision;
        let make = |generation, first, previous| TrackSwitchViewObservation {
            project_generation: generation,
            first_previous_revision: first,
            previous_revision: previous,
            revision: previous + 1,
            tracks: vec![TrackSwitchState {
                track,
                muted: true,
                soloed: false,
            }],
        };
        let generation = view.project_generation;
        assert!(!view.observe_track_switch_view(make(generation, revision + 1, revision + 1)));
        assert!(!view.observe_track_switch_view(make(generation, revision + 2, revision + 1)));
        assert!(view.observe_track_switch_view(make(generation, revision, revision + 1)));
        let canonical = view.state.as_ref().unwrap().project.track(track).unwrap();
        assert!(view.displayed_track_switches(canonical).0);
        view.project_generation += 1;
        assert!(!view.observe_track_switch_view(make(
            view.project_generation,
            revision + 2,
            revision + 2
        )));
        let canonical = view.state.as_ref().unwrap().project.track(track).unwrap();
        assert!(!view.displayed_track_switches(canonical).0);
    });
}

#[gpui::test]
fn consecutive_switch_views_stay_visible_through_earlier_terminal(cx: &mut TestAppContext) {
    use gpui::AppContext;
    use hex_session::service::{Request, Service};
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let next = |service: &Service| {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            if let Ok(event) = service.try_event() {
                break event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "initial service event"
            );
            std::thread::yield_now();
        }
    };
    next(&service);
    service
        .submit(Request::Edit(hex_project::Command::AddTrack {
            name: "track".into(),
        }))
        .unwrap();
    let state = next(&service).state;
    let track = state.project.tracks()[0].id();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                super::EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(state);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(format!("track-mute-{}", track.value()).into_boxed_str());
    let original = visual.rendered_background(selector);
    let mut events = Vec::new();
    for expected in [true, false] {
        editor.read_with(&visual, |view, _| {
            view.service
                .submit(Request::Edit(hex_project::Command::ToggleMute(track)))
                .unwrap()
        });
        let deadline = crate::test_wait::hang_deadline();
        let event = loop {
            if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
                break event;
            }
            assert!(std::time::Instant::now() < deadline, "switch terminal");
            std::thread::yield_now();
        };
        let observation = editor.read_with(&visual, |view, _| {
            view.service.try_track_switch_view().unwrap()
        });
        editor.update(&mut visual, |view, cx| {
            assert!(view.observe_track_switch_view(observation));
            let canonical = view.state.as_ref().unwrap().project.track(track).unwrap();
            assert_eq!(view.displayed_track_switches(canonical).0, expected);
            cx.notify();
        });
        visual.run_until_parked();
        if expected {
            assert_ne!(visual.rendered_background(selector), original);
        } else {
            assert_eq!(visual.rendered_background(selector), original);
        }
        events.push(event);
    }
    let second = events.pop().unwrap();
    let first = events.pop().unwrap();
    editor.update(&mut visual, |view, cx| {
        view.receive(first);
        let canonical = view.state.as_ref().unwrap().project.track(track).unwrap();
        assert!(!view.displayed_track_switches(canonical).0);
        assert!(view.track_switch_observation.is_some());
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.rendered_background(selector), original);
    editor.update(&mut visual, |view, cx| {
        view.receive(second);
        assert!(view.track_switch_observation.is_none());
        cx.notify();
    });
}

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
        let name = Box::leak(format!("track-name-{}", id.value()).into_boxed_str());
        let marker = visual
            .debug_bounds(Box::leak(
                format!("selected-track-marker-{}", id.value()).into_boxed_str(),
            ))
            .expect("selected member has a compact inset");
        let name_bounds = visual.debug_bounds(name).unwrap();
        assert!(name_bounds.contains(&marker.center()));
        assert_eq!(marker.size.width, gpui::px(4.5));
        assert_eq!(marker.size.height, gpui::px(15.0));
        assert!(
            visual
                .rendered_border(name)
                .is_none_or(|(_, widths, _)| widths == Default::default())
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
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("selected-track-marker-{}", source.value()).into_boxed_str()
            ))
            .is_none()
    );
    visual.simulate_click(
        last,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.selected_track_ids().is_empty()));
    for id in [source, empty] {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("selected-track-marker-{}", id.value()).into_boxed_str()
                ))
                .is_none()
        );
    }
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("selected-track-marker-{}", source.value()).into_boxed_str()
            ))
            .is_some()
    );
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
            anchor_track: Some(source),
            end_track: Some(empty),
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
            anchor_track: Some(source),
            end_track: Some(third),
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
fn selected_track_lanes_fill_bounds_only_for_explicit_members(cx: &mut TestAppContext) {
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
            let tint = selector("selected-track-lane", id);
            assert_eq!(visual.debug_bounds(tint).is_some(), active);
            for edge in ["selected-track-lane-top", "selected-track-lane-bottom"] {
                assert!(visual.debug_bounds(selector(edge, id)).is_none());
            }
            if active {
                assert!(visual.rendered_background(tint).is_some());
                assert_eq!(
                    visual.debug_bounds(tint),
                    visual.debug_bounds(selector("arrangement-lane", id))
                );
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
    for (palette, expected_tint) in [
        (hex_preferences::ThemePreferences::default(), 0xa4ccd7),
        (
            hex_preferences::ThemePreferences {
                background: hex_preferences::RgbColor::new(240, 240, 240),
                text: hex_preferences::RgbColor::new(16, 16, 16),
                primary: hex_preferences::RgbColor::new(166, 65, 47),
                secondary: hex_preferences::RgbColor::new(40, 88, 170),
            },
            0x2858aa,
        ),
    ] {
        theme::apply_preferences(palette);
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_background(tint),
            Some(
                gpui::Hsla::from(gpui::rgb(expected_tint))
                    .alpha(0.28)
                    .into()
            )
        );
        let fill = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            theme::arrangement_clip_body_fill(
                theme::clip_fill(
                    p.track(source).unwrap().color(),
                    p.clip(clip).unwrap().flavor(),
                    false,
                ),
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

#[gpui::test]
fn empty_lane_click_moves_header_and_lane_highlight_with_active_track(cx: &mut TestAppContext) {
    use gpui::{MouseButton, point};
    let (project, source, empty, _) = audio_project();
    let before = project.snapshot();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = |prefix: &str, id: hex_project::TrackId| {
        Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str()) as &'static str
    };
    for (previous, target, modifiers) in [
        (source, empty, Modifiers::default()),
        (
            empty,
            source,
            Modifiers {
                platform: true,
                ..Default::default()
            },
        ),
        (
            source,
            empty,
            Modifiers {
                shift: true,
                ..Default::default()
            },
        ),
    ] {
        let header = visual
            .debug_bounds(selector("track-name", previous))
            .unwrap();
        visual.simulate_click(header.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_track_ids(), [previous])
        });
        let lane = visual
            .debug_bounds(selector("arrangement-lane", target))
            .unwrap();
        let position = point(
            lane.left() + lane.size.width * if modifiers.shift { 0.8 } else { 0.7 },
            lane.center().y,
        );
        visual.simulate_mouse_down(position, MouseButton::Left, modifiers);
        visual.run_until_parked();
        let selected = if modifiers.shift {
            vec![source, empty]
        } else {
            vec![target]
        };
        for released in [false, true] {
            if released {
                visual.simulate_mouse_up(position, MouseButton::Left, modifiers);
                visual.run_until_parked();
            }
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.selected_track, Some(target));
                assert_eq!(view.selected_track_ids(), selected);
                assert_eq!(
                    view.cursor_position(),
                    0,
                    "lane selection does not seek playback"
                );
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
                if !modifiers.shift {
                    assert_eq!(view.edit_cursor.unwrap().track, Some(target));
                }
            });
            for id in [source, empty] {
                let name = selector("selected-track-lane", id);
                assert_eq!(visual.debug_bounds(name).is_some(), selected.contains(&id));
                if selected.contains(&id) {
                    assert!(
                        visual.rendered_background(name).is_some(),
                        "completed lane paint"
                    );
                }
                for prefix in [
                    "selected-track",
                    "selected-track-lane-top",
                    "selected-track-lane-bottom",
                ] {
                    assert!(visual.debug_bounds(selector(prefix, id)).is_none());
                }
            }
        }
    }
}

#[gpui::test]
fn track_switch_clicks_paint_before_the_terminal_and_solo_preserves_selection(
    cx: &mut TestAppContext,
) {
    use gpui::AppContext;
    use hex_project::{Command, TrackId};
    use hex_session::service::{Event, Request, Service};
    fn next(service: &Service) -> Event {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "next: hang guard expired"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    fn flush(editor: &gpui::Entity<super::EditorView>, cx: &mut TestAppContext) {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            editor.update(cx, |view, cx| {
                view.tick(cx);
                cx.notify();
            });
            if editor.read_with(cx, |view, _| {
                view.worker_requests_pending == 0 && view.pending_requests.is_empty()
            }) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "flush: hang guard expired"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    fn observe_switch(editor: &gpui::Entity<super::EditorView>, visual: &mut VisualTestContext) {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            let observed = editor.update(&mut *visual, |view, cx| {
                if let Ok(observation) = view.service.try_track_switch_view() {
                    let accepted = view.observe_track_switch_view(observation);
                    cx.notify();
                    accepted
                } else {
                    false
                }
            });
            if observed {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "track switch observation missing"
            );
            std::thread::yield_now();
        }
    }
    cx.update(super::bind_editor_keys);
    let service = Service::new(48_000, 16, 64).unwrap();
    let _ = next(&service);
    let mut state = None;
    for name in ["first", "outside", "last"] {
        service
            .submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .unwrap();
        state = Some(next(&service).state);
    }
    let state = state.unwrap();
    let tracks: Vec<TrackId> = state
        .project
        .tracks()
        .iter()
        .map(|track| track.id())
        .collect();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.activate_window();
            let editor = window.replace_root(cx, |window, cx| {
                super::EditorView::new(window, cx, service, Err("offline solo test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(state);
                view.selected_track = Some(tracks[0]);
                view.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = |visual: &mut VisualTestContext, prefix: &str, track: TrackId| {
        visual
            .debug_bounds(Box::leak(
                format!("{prefix}-{}", track.value()).into_boxed_str(),
            ))
            .unwrap()
    };
    let first = bounds(&mut visual, "track-name", tracks[0]).center();
    visual.simulate_click(first, Modifiers::default());
    let last = bounds(&mut visual, "track-name", tracks[2]).center();
    visual.simulate_click(
        last,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    let check = |expected: [bool; 3], cx: &TestAppContext| {
        editor.read_with(cx, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert_eq!(
                tracks
                    .iter()
                    .map(|id| state.project.track(*id).unwrap().soloed())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                view.selected_track_ids(),
                [tracks[0], tracks[2]],
                "solo preserves explicit selection"
            );
        })
    };
    let revision = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().revision);
    let solo = bounds(&mut visual, "track-solo", tracks[0]);
    visual.simulate_click(solo.center(), Modifiers::default());
    observe_switch(&editor, &mut visual);
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        for id in [tracks[0], tracks[2]] {
            assert_eq!(
                view.displayed_track_switches(state.project.track(id).unwrap()),
                (false, true)
            );
        }
        assert_eq!(state.revision, revision);
    });
    flush(&editor, cx);
    check([true, false, true], cx);
    editor.read_with(cx, |view, _| {
        assert!(view.track_switch_observation.is_none())
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision + 1)
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    flush(&editor, cx);
    check([false; 3], cx);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    flush(&editor, cx);
    check([true, false, true], cx);
    visual.simulate_keystrokes("s");
    flush(&editor, cx);
    check([false; 3], cx);
    visual.run_until_parked();
    let outside = bounds(&mut visual, "track-solo", tracks[1]);
    visual.simulate_click(outside.center(), Modifiers::default());
    flush(&editor, cx);
    check([false, true, false], cx);
    visual.run_until_parked();
    let solo = bounds(&mut visual, "track-solo", tracks[0]);
    visual.simulate_click(
        solo.center(),
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    flush(&editor, cx);
    check([true; 3], cx);
    for (mode, prefix, muted) in [
        (super::EditorMode::Arrangement, "track-mute", true),
        (super::EditorMode::Session, "console-track-mute", false),
    ] {
        editor.update(&mut visual, |view, cx| {
            view.set_mode(mode);
            cx.notify();
        });
        visual.run_until_parked();
        let selector = Box::leak(format!("{prefix}-{}", tracks[0].value()).into_boxed_str());
        let before = visual
            .rendered_background(selector)
            .expect("track activator paints");
        let hit = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(hit, Modifiers::default());
        visual.simulate_mouse_move(
            gpui::point(gpui::px(-1.0), gpui::px(-1.0)),
            None,
            Modifiers::default(),
        );
        observe_switch(&editor, &mut visual);
        visual.run_until_parked();
        let after = visual.rendered_background(selector).unwrap();
        let (background, _) = super::track_gain::activator_colors(!muted);
        assert_eq!(after, gpui::rgb(background).into());
        assert_ne!(before, after);
        editor.read_with(&visual, |view, _| {
            let track = view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(tracks[0])
                .unwrap();
            assert_eq!(track.muted(), !muted, "terminal has not been installed");
            assert_eq!(view.displayed_track_switches(track), (muted, true));
        });
        flush(&editor, cx);
        visual.run_until_parked();
        assert_eq!(visual.rendered_background(selector), Some(after));
        editor.read_with(&visual, |view, _| {
            assert!(view.track_switch_observation.is_none())
        });
    }
}

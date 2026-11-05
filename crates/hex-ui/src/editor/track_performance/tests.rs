use super::*;
use hex_project::{Command, Created, Editor, Project};

fn owner() -> OutputCaptureId {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Track(id) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    OutputCaptureId::Track(id)
}

fn fixture(owner: OutputCaptureId, now: Instant, load: u32) -> State {
    let mut state = State {
        enabled: true,
        identity: Some((1, 2, 10)),
        availability: Availability::Fresh,
        ..Default::default()
    };
    state.entries.insert(
        key(owner),
        Entry {
            sample: RenderWorkSample {
                epoch: 1,
                sequence: 1,
                hundredths_percent: load,
            },
            received: now,
            smoothed: load as f32,
            progress_verified: true,
        },
    );
    state
}

#[test]
fn unknown_stale_and_stopped_are_distinct_from_measured_zero() {
    let owner = owner();
    let now = Instant::now();
    let mut state = fixture(owner, now, 0);
    assert_eq!(state.appearance(owner, now), (Availability::Fresh, 0));
    assert_eq!(
        state.appearance(owner, now + STALE + REFRESH),
        (Availability::Stale, 0)
    );
    assert!(state.unavailable(Availability::Pending));
    assert_eq!(state.appearance(owner, now), (Availability::Pending, 0));
    assert!(state.unavailable(Availability::Stopped));
    assert_eq!(state.appearance(owner, now), (Availability::Stopped, 0));
    state.availability = Availability::Fresh;
    assert_eq!(state.appearance(owner, now), (Availability::Waiting, 0));
    for (load, segments) in [(0, 0), (10, 1), (49, 1), (50, 2), (1000, 6), (10_000, 6)] {
        let state = fixture(owner, now, load);
        assert_eq!(state.appearance(owner, now).1, segments);
    }
}

#[test]
fn progress_freshness_and_peak_decay_require_actual_new_samples() {
    let owner = owner();
    let now = Instant::now();
    let mut state = fixture(owner, now, 1000);
    let entry = state.entries.get_mut(&key(owner)).unwrap();
    entry.progress_verified = false;
    let original = entry.sample;
    entry.observe(original, now + REFRESH, 0.1);
    assert!(!entry.progress_verified);
    let quiet = RenderWorkSample {
        sequence: 2,
        hundredths_percent: 0,
        ..original
    };
    entry.observe(quiet, now + 2 * REFRESH, 0.1);
    assert!(entry.progress_verified);
    assert!(entry.smoothed > 900.0 && entry.smoothed < 1000.0);
    entry.observe(quiet, now + 3 * REFRESH, 0.1);
    assert_eq!(
        entry.received,
        now + 2 * REFRESH,
        "rereading a snapshot never refreshes its age"
    );
    let later = RenderWorkSample {
        sequence: 3,
        ..quiet
    };
    entry.observe(later, now + STALE * 2, 6.0);
    assert!(
        !entry.progress_verified,
        "first read after a long gap has unknown producer age"
    );
    let current = RenderWorkSample {
        sequence: 4,
        ..quiet
    };
    entry.observe(current, now + STALE * 2 + REFRESH, 0.1);
    assert!(entry.progress_verified);
    assert_eq!(
        state.appearance(owner, now + STALE * 2 + REFRESH),
        (Availability::Fresh, 0)
    );
}

#[cfg(feature = "ui-test-support")]
mod rendered {
    use super::*;
    use gpui::{Context, IntoElement, Render, TestAppContext, VisualTestContext, Window};
    struct Fixture {
        state: State,
        owner: OutputCaptureId,
        now: Instant,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .font_family(theme::FONT_FAMILY)
                .child(super::super::render(&self.state, self.owner, self.now))
        }
    }
    #[gpui::test]
    fn six_segments_fit_compact_width_and_fill_only_measured_work(cx: &mut TestAppContext) {
        let owner = owner();
        let now = Instant::now();
        let window = cx.add_window(|_, _| Fixture {
            state: fixture(owner, now, 250),
            owner,
            now,
        });
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        assert_eq!(key(owner), (0, 1));
        let bounds = visual.debug_bounds("track-performance-0-1").unwrap();
        assert_eq!(bounds.size.width, px(23.0));
        assert_eq!(bounds.size.height, px(8.0));
        for (index, selector) in [
            "track-performance-segment-0-1-0",
            "track-performance-segment-0-1-1",
            "track-performance-segment-0-1-2",
            "track-performance-segment-0-1-3",
            "track-performance-segment-0-1-4",
            "track-performance-segment-0-1-5",
        ]
        .into_iter()
        .enumerate()
        {
            assert!(visual.debug_bounds(selector).is_some());
            assert_eq!(
                visual.rendered_background(selector),
                Some(
                    rgb(theme::color(if index < 4 {
                        theme::PaletteRole::Secondary
                    } else {
                        theme::PaletteRole::Background
                    }))
                    .into()
                )
            );
        }
    }
}

#[cfg(feature = "ui-test-support")]
#[gpui::test]
fn view_action_toggles_actual_arrangement_and_console_headers(cx: &mut gpui::TestAppContext) {
    use crate::editor::clip_drag_ui_test::editor_window;
    use crate::editor::{EditorMode, ToggleTrackPerformance};
    use gpui::{VisualTestContext, size};
    use std::sync::Arc;
    let mut model = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Track(track) = model
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    assert_eq!(track.value(), 1);
    let (window, editor) = editor_window(cx, model.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-performance-0-1").is_none());
    visual.update(|window, cx| window.dispatch_action(Box::new(ToggleTrackPerformance), cx));
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-performance-0-1").is_some());
    editor.update(cx, |view, cx| {
        view.mode = EditorMode::Session;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-performance-0-1").is_some());
    visual.update(|window, cx| window.dispatch_action(Box::new(ToggleTrackPerformance), cx));
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-performance-0-1").is_none());
}

#[cfg(feature = "ui-test-support")]
#[gpui::test]
fn missing_output_invalidates_cached_work_inside_the_sample_interval(
    cx: &mut gpui::TestAppContext,
) {
    use crate::editor::clip_drag_ui_test::editor_window;
    use std::sync::Arc;

    let (_, editor) = editor_window(cx, Project::new(48_000).unwrap(), Arc::default());
    let now = Instant::now();
    let owner = owner();
    editor.update(cx, |view, _| {
        view.track_performance = fixture(owner, now, 250);
        view.track_performance.updated = Some(now);
        assert_eq!(
            view.track_performance.appearance(owner, now),
            (Availability::Fresh, 4)
        );
        assert!(view.tick_track_performance(now));
        assert_eq!(
            view.track_performance.appearance(owner, now),
            (Availability::Unavailable, 0)
        );
    });
}

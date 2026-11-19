//! Manual headless GPUI framework profile, distinct from native feedback latency.
use super::*;
use gpui::{
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput,
    TestAppContext, VisualTestContext, point, px,
};
use hex_session::service::{Request, Service};
use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[path = "navigation_profile.rs"]
mod navigation;

struct Scratch {
    path: PathBuf,
    palette: hex_preferences::ThemePreferences,
    created: bool,
}
impl Drop for Scratch {
    fn drop(&mut self) {
        theme::apply_preferences(self.palette);
        if self.created {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
struct ProfileRoot {
    editor: Entity<EditorView>,
    profile_phases: bool,
}
impl Render for ProfileRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let content = self.editor.clone().into_any_element();
        let content = if self.profile_phases {
            crate::editor::navigation_metrics::PhaseProbe::new(content).into_any_element()
        } else {
            content
        };
        div().size_full().relative().child(content)
    }
}

#[gpui::test]
#[ignore = "manual optimized headless selection framework profile; requires baseline fixture directory"]
fn headless_time_selection_framework_profile(cx: &mut TestAppContext) {
    let smoke =
        std::env::var_os("HEX_TIME_SELECTION_PROFILE_SMOKE").is_some_and(|value| value == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug is allowed only with explicit smoke=1"
    );
    let fixtures = PathBuf::from(
        std::env::var_os("HEX_TIME_SELECTION_FIXTURE_DIR")
            .expect("baseline fixture directory environment variable"),
    );
    assert!(
        fixtures.is_absolute(),
        "fixture directory must be an absolute path"
    );
    let path = fixtures.join("100.hex");
    assert!(path.is_file(), "existing 100-track fixture required");
    let mut scratch = Scratch {
        path: std::env::temp_dir().join(format!(
            "hex-selection-framework-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        )),
        palette: theme::preferences(),
        created: false,
    };
    std::fs::create_dir(&scratch.path).expect("create isolated profile settings");
    scratch.created = true;
    theme::apply_preferences(hex_preferences::ThemePreferences::default());
    // TestPlatform uses NoopTextSystem: no native font registration/shaping or
    // rasterization. This explicit diagnostic setup matches ordinary UI tests;
    // production startup still requires the bundled font without a fallback.
    cx.update(super::super::bind_editor_keys);
    let service = Service::new(48_000, 8, 8_000_000).expect("offline profile worker");
    let handle = cx.add_window(|window, cx| {
        let editor = cx.new(|cx| {
            EditorView::new_with_browser_preferences_path(
                window,
                cx,
                service,
                Err("offline headless profile".into()),
                scratch.path.join("browser.json"),
                scratch.path.join("recent.json"),
                scratch.path.join("theme.json"),
            )
        });
        ProfileRoot {
            editor,
            profile_phases: false,
        }
    });
    let root = handle.root(cx).expect("profile root");
    let editor = root.read_with(cx, |root, _| root.editor.clone());
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(gpui::size(px(1240.0), px(760.0)));
    editor.update(cx, |view, cx| {
        view.begin_open_request(Request::Open(path));
        cx.notify();
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        let ready = editor.read_with(cx, |view, _| {
            !view.opening_project
                && !view.edit_pending()
                && view
                    .state
                    .as_ref()
                    .is_some_and(|state| state.project.tracks().len() == 100)
        });
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "bounded profile open timed out: {:?}",
            editor.read_with(cx, |view, _| view.error.clone())
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let first_track = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("opened state");
        assert_eq!(state.project.sample_rate(), 48_000);
        assert!(state.project.tracks().iter().all(|track| {
            track.clips().len() == 50
                && track.midi_clips().len() == 50
                && track
                    .midi_clips()
                    .iter()
                    .all(|clip| clip.notes().len() == 8)
        }));
        state.project.tracks()[0].id()
    });
    visual.update(|window, cx| {
        assert_eq!(window.scale_factor(), 2.0);
        editor.update(cx, |view, cx| {
            view.viewport = Some(Viewport::fit(
                view.state.as_ref().expect("state").timeline_frames,
            ));
            view.focus.focus(window);
            cx.notify();
        });
        window.draw(cx).clear();
        window.draw(cx).clear();
    });
    let lane = editor.read_with(cx, |view, _| {
        view.main_track_lane_bounds.expect("painted main lane")
    });
    let at = |fraction| {
        point(
            px(lane.left + lane.width * fraction),
            px(lane.top + lane.height * 0.8),
        )
    };
    let modifiers = Modifiers {
        platform: true,
        ..Default::default()
    };
    visual.update(|window, cx| {
        window.dispatch_input(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: at(0.213),
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.draw(cx).clear();
    });
    let builds = |cx: &TestAppContext| {
        editor.read_with(cx, |view, app| {
            view.clip_content_cache
                .entries
                .values()
                .map(|child| child.read(app).builds)
                .sum::<usize>()
        })
    };
    let selector =
        Box::leak(format!("edit-time-selection-{}", first_track.value()).into_boxed_str());
    let mut previous_width = None;
    let mut rows = Vec::with_capacity(45);
    for iteration in 0..45 {
        let before = builds(cx);
        let end = at(0.4 + iteration as f32 / 225.0);
        let elapsed = visual.update(|window, cx| {
            // Dispatch is outside the interval and does not run draw itself.
            // No executor pumping or simulate_mouse_move can paint between it
            // and this explicit framework draw.
            window.dispatch_input(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: end,
                    pressed_button: Some(MouseButton::Left),
                    modifiers,
                }),
                cx,
            );
            let started = Instant::now();
            let arena = window.draw(cx);
            let elapsed = started.elapsed();
            arena.clear(); // Outside the explicit draw interval.
            elapsed
        });
        let after = builds(cx);
        let overlay = visual
            .debug_bounds(selector)
            .expect("changed range painted in explicit draw");
        if let Some(width) = previous_width {
            assert!(overlay.size.width > width);
        }
        previous_width = Some(overlay.size.width);
        editor.read_with(cx, |view, _| {
            assert!(view.time_selection_project_scope && view.time_selection_gesture.is_some());
            assert_eq!(
                view.normalized_time_selection()
                    .expect("range")
                    .tracks()
                    .len(),
                100
            );
            assert!(!view.edit_pending());
        });
        if iteration >= 5 {
            assert_eq!(
                after, before,
                "steady selection must reuse all warmed clip children"
            );
        }
        rows.push((iteration, elapsed, after - before));
    }
    visual.update(|window, cx| {
        window.dispatch_input(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: at(0.4 + 44.0 / 225.0),
                modifiers,
                click_count: 1,
            }),
            cx,
        )
    });
    println!(
        "headless_gpui_framework_only=true; noop_text_system=true; native_font_shaping_raster=false; physical_input_gpu_present_audio=false; debug_smoke={smoke}; size=1240x760; scale=2; tracks=100; warmup=5; measured=40"
    );
    println!("iteration,warmup,explicit_draw_us,clip_builds");
    for (iteration, elapsed, builds) in rows {
        println!(
            "{iteration},{},{:.3},{builds}",
            iteration < 5,
            elapsed.as_secs_f64() * 1_000_000.0
        );
    }
}

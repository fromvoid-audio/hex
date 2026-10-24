//! Manual optimized input-to-CPU-frame measurements. TestPlatform has no native
//! text shaping, GPU presentation or audio output; these are framework timings.
use super::*;
use gpui::{ScrollDelta, ScrollWheelEvent, size};

fn fixture(
    cx: &mut TestAppContext,
    directory: &std::path::Path,
    tracks: usize,
) -> (VisualTestContext, Entity<EditorView>, Scratch) {
    let path = directory.join(format!("{tracks}.hex"));
    assert!(
        path.is_file(),
        "generate the create-new time-selection fixtures"
    );
    let mut scratch = Scratch {
        path: std::env::temp_dir().join(format!(
            "hex-navigation-framework-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )),
        palette: theme::preferences(),
        created: false,
    };
    std::fs::create_dir(&scratch.path).unwrap();
    scratch.created = true;
    theme::apply_preferences(hex_preferences::ThemePreferences::default());
    cx.update(crate::editor::bind_editor_keys);
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    let handle = cx.add_window(|window, cx| ProfileRoot {
        editor: cx.new(|cx| {
            EditorView::new_with_browser_preferences_path(
                window,
                cx,
                service,
                Err("offline headless navigation profile".into()),
                scratch.path.join("browser.json"),
                scratch.path.join("recent.json"),
                scratch.path.join("theme.json"),
            )
        }),
    });
    let root = handle.root(cx).unwrap();
    let editor = root.read_with(cx, |root, _| root.editor.clone());
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(1240.0), px(760.0)));
    editor.update(cx, |view, cx| {
        view.begin_open_request(Request::Open(path));
        cx.notify();
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        if editor.read_with(cx, |view, _| {
            !view.opening_project
                && !view.edit_pending()
                && view
                    .state
                    .as_ref()
                    .is_some_and(|state| state.project.tracks().len() == tracks)
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "bounded profile open: {:?}",
            editor.read_with(cx, |v, _| v.error.clone())
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project.sample_rate(), 48_000);
        assert!(state.project.tracks().iter().all(|track| {
            track.clips().len() == 50
                && track.midi_clips().len() == 50
                && track
                    .midi_clips()
                    .iter()
                    .all(|clip| clip.notes().len() == 8)
        }));
    });
    visual.update(|window, cx| {
        assert_eq!(window.scale_factor(), 2.0);
        editor.update(cx, |view, cx| {
            view.clip_view_open = false;
            view.device_panel_active = false;
            view.edit_cursor = None;
            view.focus.focus(window);
            cx.notify();
        });
        window.draw(cx).clear();
        window.draw(cx).clear();
    });
    (visual, editor, scratch)
}

#[derive(Default)]
struct Observation {
    builds: usize,
    visible_tracks: usize,
    audio_clips: usize,
    midi_clips: usize,
    waveform_paints: usize,
    geometry_entries: usize,
}

fn observe(editor: &Entity<EditorView>, visual: &VisualTestContext) -> Observation {
    editor.read_with(visual, |view, app| {
        let mut result = Observation {
            builds: view
                .clip_content_cache
                .entries
                .values()
                .map(|child| child.read(app).builds)
                .sum(),
            ..Default::default()
        };
        let state = view.state.as_ref().unwrap();
        let viewport = view.visible_viewport(state.timeline_frames);
        let end = viewport.end().unwrap();
        let body = view.arrangement_bounds.unwrap();
        for track in state.project.tracks() {
            if view.drag_lane_epochs.get(&track.id()) != Some(&view.drag_layout_generation) {
                continue;
            }
            let Some(lane) = view.drag_lanes.get(&track.id()) else {
                continue;
            };
            if lane.top >= body.top + body.height || lane.top + lane.height <= body.top {
                continue;
            }
            result.visible_tracks += 1;
            result.audio_clips += track
                .clips()
                .iter()
                .filter(|clip| {
                    clip.start() < end
                        && clip.start().saturating_add(clip.duration()) > viewport.start
                })
                .count();
            result.midi_clips += track
                .midi_clips()
                .iter()
                .filter(|clip| {
                    state.midi_preview.get(clip.id()).is_some_and(|preview| {
                        preview.frame_start < end && preview.frame_end > viewport.start
                    })
                })
                .count();
        }
        let cache = view.waveform_geometry_cache.borrow();
        result.waveform_paints = cache.painted_waveforms().len();
        result.geometry_entries = cache.entry_count();
        result
    })
}

#[gpui::test]
#[ignore = "manual optimized pan/zoom framework profile; generate time_selection_fixture first"]
fn headless_navigation_framework_profile(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_NAVIGATION_PROFILE_SMOKE").is_some_and(|value| value == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let directory = PathBuf::from(
        std::env::var_os("HEX_NAVIGATION_FIXTURE_DIR").expect("absolute fixture directory"),
    );
    assert!(directory.is_absolute());
    let rounds = if smoke { 4 } else { 48 };
    let warmup = if smoke { 1 } else { 8 };
    println!(
        "\nNAV_META,headless_cpu_only=true,noop_text_system=true,native_gpu_present_audio=false,size=1240x760,scale=2,debug_smoke={smoke},warmup={warmup},rounds={rounds},paint_observations_capped=512"
    );
    println!(
        "NAV_CSV,tracks,row_mode,density,action,iteration,warmup,event_us,draw_us,arena_clear_us,cpu_total_us,warm_cpu_total_us,viewport_start,viewport_length,visible_tracks,audio_clips,midi_clips,child_builds,waveform_paints,geometry_entries"
    );
    for (tracks, single_row) in [(1, false), (100, false), (100, true)] {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _scratch) = fixture(cx, &directory, tracks);
        if single_row {
            let (track, body) = editor.read_with(cx, |view, _| {
                (
                    view.state.as_ref().unwrap().project.tracks()[0].id(),
                    view.arrangement_bounds.unwrap(),
                )
            });
            let selector =
                Box::leak(format!("track-height-resize-{}", track.value()).into_boxed_str());
            let grip = visual.debug_bounds(selector).unwrap().center();
            visual.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
            let end = point(grip.x, grip.y + px(body.height));
            visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert!(view.track_height(track) >= body.height - 1.0)
            });
        }
        let revision = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().revision);
        for (density, divisor) in [("dense", 1_u64), ("zoomed", 16)] {
            for action in ["pan", "zoom"] {
                visual.update(|window, cx| {
                    editor.update(cx, |view, cx| {
                        let horizon = view.state.as_ref().unwrap().timeline_frames;
                        let length = horizon * 4 / 5 / divisor;
                        view.viewport = Some(Viewport {
                            start: (horizon - length) / 2,
                            length,
                        });
                        cx.notify();
                    });
                    window.draw(cx).clear();
                    window.draw(cx).clear();
                });
                let body = editor.read_with(cx, |view, _| view.arrangement_bounds.unwrap());
                let width = editor.read_with(cx, |view, _| body.width - view.track_header_width());
                let origin = point(
                    px(body.left + width * 0.5),
                    px(body.top + body.height.min(40.0) * 0.5),
                );
                let mut pointer = origin;
                if action == "pan" {
                    visual.update(|window, cx| {
                        window.dispatch_input(
                            PlatformInput::MouseDown(MouseDownEvent {
                                position: origin,
                                button: MouseButton::Middle,
                                modifiers: Modifiers::default(),
                                click_count: 1,
                                first_mouse: false,
                            }),
                            cx,
                        )
                    });
                    editor.read_with(cx, |view, _| {
                        assert!(view.arrangement_pan.is_some(), "actual middle capture")
                    });
                }
                editor.update(cx, |view, _| {
                    view.waveform_geometry_cache.borrow_mut().clear()
                });
                let mut rows = Vec::with_capacity(rounds);
                for iteration in 0..rounds {
                    let before = editor.read_with(cx, |view, _| {
                        view.waveform_geometry_cache
                            .borrow_mut()
                            .clear_paint_samples();
                        view.visible_viewport(view.state.as_ref().unwrap().timeline_frames)
                    });
                    let builds_before = observe(&editor, &visual).builds;
                    if action == "pan" {
                        pointer.x += px(if iteration % 20 < 10 { -4.0 } else { 4.0 });
                    }
                    let (event, draw, clear, total) = visual.update(|window, cx| {
                        let started = Instant::now();
                        if action == "pan" {
                            window.dispatch_input(
                                PlatformInput::MouseMove(MouseMoveEvent {
                                    position: pointer,
                                    pressed_button: Some(MouseButton::Middle),
                                    modifiers: Modifiers::default(),
                                }),
                                cx,
                            );
                        } else {
                            window.dispatch_input(
                                PlatformInput::ScrollWheel(ScrollWheelEvent {
                                    position: origin,
                                    delta: ScrollDelta::Pixels(point(
                                        px(0.0),
                                        px(if iteration % 16 < 8 { 80.0 } else { -80.0 }),
                                    )),
                                    ..Default::default()
                                }),
                                cx,
                            );
                        }
                        let event = started.elapsed();
                        let draw_start = Instant::now();
                        let arena = window.draw(cx);
                        let draw = draw_start.elapsed();
                        let clear_start = Instant::now();
                        arena.clear();
                        (event, draw, clear_start.elapsed(), started.elapsed())
                    });
                    let observation = observe(&editor, &visual);
                    let after = editor.read_with(cx, |view, _| {
                        view.visible_viewport(view.state.as_ref().unwrap().timeline_frames)
                    });
                    assert_ne!(before, after, "the actual input must move the viewport");
                    let warm = visual.update(|window, cx| {
                        let started = Instant::now();
                        window.draw(cx).clear();
                        started.elapsed()
                    });
                    rows.push((
                        iteration,
                        event,
                        draw,
                        clear,
                        total,
                        warm,
                        after,
                        observation,
                        builds_before,
                    ));
                }
                if action == "pan" {
                    visual.update(|window, cx| {
                        window.dispatch_input(
                            PlatformInput::MouseUp(MouseUpEvent {
                                position: pointer,
                                button: MouseButton::Middle,
                                modifiers: Modifiers::default(),
                                click_count: 1,
                            }),
                            cx,
                        )
                    });
                }
                for (
                    iteration,
                    event,
                    draw,
                    clear,
                    total,
                    warm,
                    viewport,
                    observation,
                    builds_before,
                ) in rows
                {
                    println!(
                        "NAV_CSV,{tracks},{},{density},{action},{iteration},{},{:.3},{:.3},{:.3},{:.3},{:.3},{},{},{},{},{},{},{},{}",
                        if single_row {
                            "single_visible"
                        } else {
                            "normal"
                        },
                        iteration < warmup,
                        event.as_secs_f64() * 1e6,
                        draw.as_secs_f64() * 1e6,
                        clear.as_secs_f64() * 1e6,
                        total.as_secs_f64() * 1e6,
                        warm.as_secs_f64() * 1e6,
                        viewport.start,
                        viewport.length,
                        observation.visible_tracks,
                        observation.audio_clips,
                        observation.midi_clips,
                        observation.builds.saturating_sub(builds_before),
                        observation.waveform_paints,
                        observation.geometry_entries
                    );
                }
            }
        }
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().revision,
                revision,
                "navigation must not author edits"
            );
            assert!(!view.edit_pending());
        });
    }
}

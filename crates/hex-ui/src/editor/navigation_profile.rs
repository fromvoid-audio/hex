//! Manual optimized input-to-CPU-frame measurements. TestPlatform has no native
//! text shaping, GPU presentation or audio output; these are framework timings.
//! Set HEX_NAVIGATION_PROJECT_PATH to a disposable project copy with resolved
//! media paths to profile its actual rate/state instead of generated fixtures.
//! The probe never saves or edits that document; omit the override to retain the
//! original generated workload and its density assertions.
//! HEX_NAVIGATION_SCOPE defaults to all. audio-100-dense requires the audio
//! fixture directory and selects its 100-track normal-row dense pan/zoom cases;
//! project-dense requires the project override and selects its corresponding
//! normal-row cases. Both retain eight warmups and forty measured events per case.
//! HEX_NAVIGATION_COMPARE_LAYOUTS=1 runs reference/current/current/reference in
//! one persistent fixture, retaining every warmup/event. Trial switches invalidate
//! retained content and geometry; layout identity also participates in cache keys.
//! The reference hook is test-only; on legacy stereo both variants calibrate A/A.
//! HEX_NAVIGATION_SUPPRESS_WAVEFORMS=1 performs diagnostic arranger waveform
//! ablation with the same layout/input; it is not a shipped display mode.
//! HEX_NAVIGATION_INTERLEAVE=1 pauses the dense generated probe before each
//! viewport iteration so a controller can alternate two optimized binaries.
//! The turn handshake is outside the measured input, draw and clear interval.
#[path = "navigation_profile/selection.rs"]
mod selection;
use selection::Selection;

use super::*;
use crate::editor::navigation_metrics;
use gpui::{ScrollDelta, ScrollWheelEvent, size};
use std::io::{Read, Write};

// A project override must point at a disposable copy with media references
// already resolved. Opening is read-only; this probe never submits Save/Edit.
struct ProfileInput {
    path: PathBuf,
    tracks: usize,
    content: &'static str,
    sample_rate: u32,
    memory_bytes: u64,
    original_bytes: Option<Vec<u8>>,
}

fn profile_inputs(selection: Selection) -> Vec<(ProfileInput, Vec<bool>)> {
    if selection != Selection::Audio100Dense
        && let Some(path) = std::env::var_os("HEX_NAVIGATION_PROJECT_PATH")
    {
        let path = PathBuf::from(path);
        assert!(
            path.is_absolute(),
            "absolute disposable project copy required"
        );
        let bytes = std::fs::read(&path).expect("read disposable project copy");
        // Test setup metadata only; the real Service Open below owns full saved
        // format validation, migration and media admission.
        let header: serde_json::Value = serde_json::from_slice(&bytes).expect("project metadata");
        let sample_rate = header["sample_rate"]
            .as_u64()
            .and_then(|rate| u32::try_from(rate).ok())
            .expect("project sample rate");
        let tracks = header["tracks"].as_array().expect("project tracks").len();
        assert!(tracks > 0, "navigation probe needs an ordinary track");
        return vec![(
            ProfileInput {
                path,
                tracks,
                content: "project",
                sample_rate,
                // Control-side diagnostic budget, not production admission.
                memory_bytes: 1024 * 1024 * 1024,
                original_bytes: Some(bytes),
            },
            vec![false, true],
        )];
    }
    if selection == Selection::Audio100Dense {
        let directory = PathBuf::from(
            std::env::var_os("HEX_NAVIGATION_AUDIO_FIXTURE_DIR").expect("audio fixture directory"),
        );
        assert!(directory.is_absolute());
        return vec![(
            ProfileInput {
                path: directory.join("100.hex"),
                tracks: 100,
                content: "audio",
                sample_rate: 48_000,
                memory_bytes: 8_000_000,
                original_bytes: None,
            },
            vec![false],
        )];
    }
    let directory = PathBuf::from(
        std::env::var_os("HEX_NAVIGATION_FIXTURE_DIR").expect("absolute fixture directory"),
    );
    assert!(directory.is_absolute());
    let mut directories = vec![(directory, "mixed")];
    if let Some(path) = std::env::var_os("HEX_NAVIGATION_AUDIO_FIXTURE_DIR") {
        let path = PathBuf::from(path);
        assert!(path.is_absolute());
        directories.push((path, "audio"));
    }
    directories
        .into_iter()
        .flat_map(|(directory, content)| {
            [1, 100].into_iter().map(move |tracks| {
                (
                    ProfileInput {
                        path: directory.join(format!("{tracks}.hex")),
                        tracks,
                        content,
                        sample_rate: 48_000,
                        memory_bytes: 8_000_000,
                        original_bytes: None,
                    },
                    if tracks == 1 {
                        vec![false]
                    } else {
                        vec![false, true]
                    },
                )
            })
        })
        .collect()
}

fn fixture(
    cx: &mut TestAppContext,
    input: &ProfileInput,
    profile_phases: bool,
) -> (VisualTestContext, Entity<EditorView>, Scratch) {
    let path = input.path.clone();
    let tracks = input.tracks;
    assert!(path.is_file(), "existing profile fixture required");
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
    let service = Service::new(input.sample_rate, 8, input.memory_bytes).unwrap();
    let handle = cx.add_window(|window, cx| ProfileRoot {
        profile_phases,
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
        editor.read_with(cx, |view, _| {
            assert!(
                view.opening_project || view.error.is_none(),
                "profile open refused: {:?}",
                view.error
            );
        });
        assert!(
            Instant::now() < deadline,
            "bounded profile open: {:?}",
            editor.read_with(cx, |v, _| v.error.clone())
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project.sample_rate(), input.sample_rate);
        println!(
            "NAV_PROJECT,tracks={},assets={},audio_clips={},midi_clips={},sample_rate={}",
            state.project.tracks().len(),
            state.project.assets().count(),
            state
                .project
                .tracks()
                .iter()
                .map(|track| track.clips().len())
                .sum::<usize>(),
            state
                .project
                .tracks()
                .iter()
                .map(|track| track.midi_clips().len())
                .sum::<usize>(),
            state.project.sample_rate()
        );
        if input.content != "project" {
            assert!(state.project.tracks().iter().all(|track| {
                track.clips().len() == 50
                    && track.midi_clips().len() == if input.content == "audio" { 0 } else { 50 }
                    && track
                        .midi_clips()
                        .iter()
                        .all(|clip| clip.notes().len() == 8)
            }));
        }
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
    envelope_points: usize,
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
        result.envelope_points = cache
            .painted_waveforms()
            .iter()
            .map(|sample| sample.envelope_points)
            .sum();
        result
    })
}

fn await_navigation_interleave(action: &str, iteration: usize) {
    println!("NAV_READY,{action},{iteration}");
    std::io::stdout()
        .flush()
        .expect("flush navigation interleave readiness");
    let mut response = [0; 3];
    std::io::stdin()
        .lock()
        .read_exact(&mut response)
        .expect("read navigation interleave command");
    assert_eq!(&response, b"GO\n", "invalid navigation interleave command");
}

#[gpui::test]
#[ignore = "manual optimized pan/zoom framework profile; requires generated fixtures or a project copy"]
fn headless_navigation_framework_profile(cx: &mut TestAppContext) {
    let suppression = std::env::var_os("HEX_NAVIGATION_SUPPRESS_WAVEFORMS");
    assert!(
        suppression
            .as_ref()
            .is_none_or(|value| value == "0" || value == "1"),
        "waveform ablation must be explicitly 0 or 1"
    );
    let suppressed = suppression.is_some_and(|value| value == "1");
    let _suppression = navigation_metrics::suppress_waveforms(suppressed);
    let smoke = std::env::var_os("HEX_NAVIGATION_PROFILE_SMOKE").is_some_and(|value| value == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let (rounds, warmup) = selection::iterations(smoke);
    let interleave = match std::env::var("HEX_NAVIGATION_INTERLEAVE") {
        Ok(value) if value == "0" || value == "1" => value == "1",
        Ok(_) => panic!("navigation interleave must be 0 or 1"),
        Err(std::env::VarError::NotPresent) => false,
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("navigation interleave must be UTF-8")
        }
    };
    let comparison = std::env::var("HEX_NAVIGATION_COMPARE_LAYOUTS");
    assert!(
        !matches!(comparison, Err(std::env::VarError::NotUnicode(_))),
        "navigation layout comparison must be UTF-8"
    );
    let trials = selection::comparison_trials(comparison.as_deref().ok())
        .expect("valid navigation layout comparison");
    let scope = std::env::var("HEX_NAVIGATION_SCOPE");
    assert!(
        !matches!(scope, Err(std::env::VarError::NotUnicode(_))),
        "navigation scope must be UTF-8"
    );
    let selection = Selection::parse(scope.as_deref().ok()).expect("valid finite navigation scope");
    if interleave {
        assert_eq!(
            selection,
            Selection::Audio100Dense,
            "navigation interleave requires audio-100-dense scope"
        );
        assert!(
            !smoke,
            "navigation interleave requires the full 8+40 rounds"
        );
        assert_eq!(
            trials,
            &[false],
            "navigation interleave requires the current single-layout probe"
        );
        assert_eq!((rounds, warmup), (48, 8));
    }
    selection
        .validate_inputs(
            std::env::var_os("HEX_NAVIGATION_PROJECT_PATH").is_some(),
            std::env::var_os("HEX_NAVIGATION_FIXTURE_DIR").is_some(),
            std::env::var_os("HEX_NAVIGATION_AUDIO_FIXTURE_DIR").is_some(),
        )
        .expect("selected navigation inputs");
    let inputs = profile_inputs(selection);
    let selected_cases: usize = inputs
        .iter()
        .map(|(input, rows)| {
            rows.iter()
                .map(|row| selection.cases(input.content, input.tracks, *row).len())
                .sum::<usize>()
        })
        .sum();
    let selected_cases = selected_cases * trials.len();
    assert!(selected_cases > 0, "navigation selection cannot be empty");
    println!(
        "NAV_SELECTION,scope={},cases={selected_cases},warmup={warmup},measured={}",
        selection.label(),
        rounds - warmup
    );
    if trials.len() > 1 {
        println!(
            "NAV_COMPARE,trials={},order=reference/current/current/reference",
            trials.len()
        );
    }
    let stage_timing =
        std::env::var_os("HEX_NAVIGATION_STAGE_TIMING").is_some_and(|value| value == "1");
    println!(
        "\nNAV_META,headless_cpu_only=true,noop_text_system=true,native_gpu_present_audio=false,size=1240x760,scale=2,debug_smoke={smoke},stage_timing={stage_timing},warmup={warmup},rounds={rounds},paint_observations_capped=512,waveforms_suppressed={suppressed}"
    );
    println!(
        "NAV_CSV,content,tracks,row_mode,density,action,iteration,warmup,event_us,draw_us,arena_clear_us,cpu_total_us,warm_cpu_total_us,viewport_start,viewport_length,visible_tracks,audio_clips,midi_clips,child_builds,waveform_paints,geometry_entries"
    );
    println!(
        "NAV_WAVEFORM_CSV,content,tracks,row_mode,density,action,iteration,warmup,projected_envelope_points"
    );
    if stage_timing {
        println!(
            "NAV_STAGE_CSV,phase,content,tracks,row_mode,density,action,iteration,warmup,editor_us,row_us,clip_model_us,wave_lookup_us,wave_build_us,wave_provider_us,diagnostic_us,request_layout_us,prepaint_us,paint_us,wave_sampling_us,wave_paths_us,editor_calls,row_calls,clip_model_calls,wave_lookup_calls,wave_build_calls,wave_provider_calls,diagnostic_calls,request_layout_calls,prepaint_calls,paint_calls,wave_sampling_calls,wave_paths_calls"
        );
    }
    for (input, row_modes) in inputs {
        let tracks = input.tracks;
        let content = input.content;
        for single_row in row_modes {
            if interleave {
                assert!(
                    !single_row,
                    "navigation interleave requires normal row layout"
                );
            }
            let cases = selection.cases(content, tracks, single_row);
            if cases.is_empty() {
                continue;
            }
            println!(
                "NAV_INPUT,content={content},tracks={tracks},sample_rate={},memory_bytes={}",
                input.sample_rate, input.memory_bytes
            );
            let cx = &mut cx.new_app();
            let (mut visual, editor, _scratch) = fixture(cx, &input, stage_timing);
            let initial_project = editor.read_with(cx, |view, _| {
                std::sync::Arc::clone(&view.state.as_ref().unwrap().project)
            });
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
            for (trial, &reference) in trials.iter().enumerate() {
                let _reference = navigation_metrics::use_reference_stereo(reference);
                if trials.len() > 1 {
                    println!(
                        "NAV_TRIAL,content={content},tracks={tracks},row_mode={},index={trial},layout={}",
                        if single_row {
                            "single_visible"
                        } else {
                            "normal"
                        },
                        if reference { "reference" } else { "current" },
                    );
                    editor.update(cx, |view, cx| {
                        view.waveform_geometry_cache.borrow_mut().clear();
                        view.clip_content_cache.entries.clear();
                        cx.notify();
                    });
                }
                for case in &cases {
                    let (density, divisor, action) = (case.density, case.divisor, case.action);
                    visual.update(|window, cx| {
                        editor.update(cx, |view, cx| {
                            let horizon = view.state.as_ref().unwrap().timeline_frames;
                            let length = ((horizon - horizon.div_ceil(5)) / divisor).max(1);
                            view.viewport = Some(Viewport {
                                start: horizon.saturating_sub(length) / 2,
                                length,
                            });
                            cx.notify();
                        });
                        window.draw(cx).clear();
                        window.draw(cx).clear();
                    });
                    let body = editor.read_with(cx, |view, _| view.arrangement_bounds.unwrap());
                    let width =
                        editor.read_with(cx, |view, _| body.width - view.track_header_width());
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
                        if interleave {
                            await_navigation_interleave(action, iteration);
                        }
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
                        navigation_metrics::reset(stage_timing);
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
                        let stages = navigation_metrics::snapshot();
                        navigation_metrics::reset(false);
                        let observation = observe(&editor, &visual);
                        if suppressed {
                            assert_eq!(
                                observation.waveform_paints, 0,
                                "ablation painted waveforms"
                            );
                            assert_eq!(
                                observation.geometry_entries, 0,
                                "ablation prepared waveforms"
                            );
                        } else if observation.audio_clips > 0 {
                            assert!(
                                observation.waveform_paints > 0,
                                "normal display lost waveforms"
                            );
                        }
                        let after = editor.read_with(cx, |view, _| {
                            view.visible_viewport(view.state.as_ref().unwrap().timeline_frames)
                        });
                        assert_ne!(before, after, "the actual input must move the viewport");
                        navigation_metrics::reset(stage_timing);
                        let warm = visual.update(|window, cx| {
                            let started = Instant::now();
                            window.draw(cx).clear();
                            started.elapsed()
                        });
                        let warm_stages = navigation_metrics::snapshot();
                        navigation_metrics::reset(false);
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
                            stages,
                            warm_stages,
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
                        stages,
                        warm_stages,
                    ) in rows
                    {
                        println!(
                            "NAV_WAVEFORM_CSV,{content},{tracks},{},{density},{action},{iteration},{},{}",
                            if single_row {
                                "single_visible"
                            } else {
                                "normal"
                            },
                            iteration < warmup,
                            observation.envelope_points,
                        );
                        println!(
                            "NAV_CSV,{content},{tracks},{},{density},{action},{iteration},{},{:.3},{:.3},{:.3},{:.3},{:.3},{},{},{},{},{},{},{},{}",
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
                        if stage_timing {
                            let row_mode = if single_row {
                                "single_visible"
                            } else {
                                "normal"
                            };
                            for (phase, measures) in [("event", stages), ("warm", warm_stages)] {
                                let times = measures
                                    .ns
                                    .iter()
                                    .map(|ns| format!("{:.3}", *ns as f64 / 1000.0))
                                    .collect::<Vec<_>>()
                                    .join(",");
                                let calls = measures
                                    .calls
                                    .iter()
                                    .map(u64::to_string)
                                    .collect::<Vec<_>>()
                                    .join(",");
                                println!(
                                    "NAV_STAGE_CSV,{phase},{content},{tracks},{row_mode},{density},{action},{iteration},{},{times},{calls}",
                                    iteration < warmup
                                );
                            }
                        }
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
                assert_eq!(
                    view.state.as_ref().unwrap().project.as_ref(),
                    initial_project.as_ref(),
                    "navigation must preserve the complete project"
                );
            });
        }
        if let Some(bytes) = input.original_bytes {
            assert_eq!(
                std::fs::read(&input.path).unwrap(),
                bytes,
                "navigation must not rewrite the project copy"
            );
        }
    }
}

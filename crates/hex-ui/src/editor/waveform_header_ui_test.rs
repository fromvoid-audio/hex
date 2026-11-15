//! Observe actual waveform paint callbacks/masks, never a mock header property.
use crate::editor::{
    EditorView, clip_header_loop::gradient_quads, external_audio_drop_ui_test::BatchFiles,
    musical_time::SnapGrid,
};
use gpui::{AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::{ClipId, Command, TrackId};
use hex_session::service::{Event, Request, Service};
use std::time::Duration;

fn next(service: &Service) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(e) = service.try_event() {
            assert!(e.error.is_none(), "{:?}", e.error);
            return e;
        }
        assert!(
            std::time::Instant::now() < until,
            "next: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(
    cx: &mut TestAppContext,
    looping: bool,
) -> (
    BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    TrackId,
    ClipId,
) {
    cx.update(crate::editor::bind_editor_keys);
    let files = BatchFiles::new();
    let path = files.wav("body.wav", 48_000, 96_000);
    let folder = path.parent().unwrap();
    std::fs::create_dir(folder.join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(path.clone())).unwrap();
    let e = next(&service);
    let track = e.state.project.tracks()[0].id();
    let clip = e.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(Command::MoveClip {
            clip,
            track,
            start: 96_000,
        }))
        .unwrap();
    let mut e = next(&service);
    if looping {
        service
            .submit(Request::Edit(Command::SetClipLooping {
                clip,
                looping: true,
            }))
            .unwrap();
        e = next(&service);
    }
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, folder)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, folder.join("samples"));
                assert_eq!(v.browser.places, vec![folder.join("samples")]);
                v.state = Some(e.state);
                v.snap_grid = SnapGrid::default().with_platform_modifier(true);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (files, handle, editor, track, clip)
}
fn assert_body(visual: &mut VisualTestContext, editor: &Entity<EditorView>, clip: ClipId) {
    let selector = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
    let header = visual
        .debug_bounds(selector)
        .expect("actual retained clip title");
    let clip_bounds = visual
        .debug_bounds("trim-live-preview")
        .or_else(|| {
            visual.debug_bounds(Box::leak(
                format!("clip-visual-{}", clip.value()).into_boxed_str(),
            ))
        })
        .unwrap();
    editor.update(&mut *visual, |v, cx| {
        v.clip_content_cache = Default::default();
        v.waveform_geometry_cache.borrow_mut().clear();
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&*visual, |v, _| {
        let cache = v.waveform_geometry_cache.borrow();
        let paints: Vec<_> = cache
            .painted_waveforms()
            .iter()
            .filter(|p| {
                p.bounds.top() >= clip_bounds.top() && p.bounds.top() < clip_bounds.bottom()
            })
            .collect();
        assert!(
            !paints.is_empty(),
            "actual Arrangement waveform painter must run"
        );
        for p in paints {
            eprintln!(
                "waveform header bottom={:?}, actual paint={:?}, mask={:?}, paths={:?}",
                header.bottom(),
                p.bounds,
                p.mask,
                p.ordinates()
            );
            assert!(
                p.bounds.top() >= header.bottom(),
                "waveform geometry must begin below actual title"
            );
            assert!(
                p.mask.top() >= header.bottom(),
                "actual fill/stroke content mask must exclude title"
            );
            for &(lo, hi) in p.ordinates() {
                assert!(lo >= f32::from(header.bottom()) && hi >= lo);
            }
        }
    });
}
#[gpui::test]
fn waveform_body_painter_stays_below_ordinary_selected_and_looped_headers(cx: &mut TestAppContext) {
    for looping in [false, true] {
        let (_files, handle, editor, _track, clip) = fixture(cx, looping);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        assert_body(&mut visual, &editor, clip);
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap();
        visual.simulate_mouse_down(header.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(header.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_body(&mut visual, &editor, clip);
        editor.update(&mut visual, |v, cx| {
            v.viewport = Some(crate::editor::viewport::Viewport {
                start: 120_000,
                length: 48_000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        assert_body(&mut visual, &editor, clip);
    }
}

#[gpui::test]
fn looped_header_paints_canonical_visible_cycle_edges_at_two_zooms(cx: &mut TestAppContext) {
    let (_files, handle, editor, track, clip) = fixture(cx, false);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let header = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
    assert!(gradient_quads(&mut visual, header).is_empty());
    for command in [
        Command::SetClipLooping {
            clip,
            looping: true,
        },
        Command::SetClipSourcePhase {
            clip,
            phase: 24_000,
        },
        Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Right,
            frame: 312_000,
        },
    ] {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(command)))
        });
        pump(&editor, &mut visual);
    }
    for (start, length) in [(48_000, 320_000), (120_000, 200_000)] {
        editor.update(&mut visual, |v, cx| {
            v.viewport = Some(crate::editor::viewport::Viewport { start, length });
            cx.notify();
        });
        visual.run_until_parked();
        let (edges, lane, viewport) = editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let clip = state.project.clip(clip).unwrap();
            let cycles = clip
                .repeat_cycles(state.project.tempo_map())
                .unwrap()
                .unwrap();
            let plan = cycles.cycles().seams().unwrap();
            let edges = (0..plan.count())
                .map(|index| {
                    cycles
                        .frame_at_elapsed(i128::from(plan.boundary(index).unwrap()))
                        .unwrap()
                })
                .collect::<Vec<_>>();
            assert_eq!(edges, [168_000, 264_000]);
            assert_eq!(clip.start() + clip.duration(), 312_000);
            (
                edges,
                v.drag_lanes[&track],
                v.visible_viewport(state.timeline_frames),
            )
        });
        let scale = visual.update(|window, _| f64::from(window.scale_factor()));
        let gradients = gradient_quads(&mut visual, header);
        assert_eq!(gradients.len(), 3);
        let x = |frame: i128| {
            scale
                * (f64::from(lane.left)
                    + f64::from(lane.width)
                        * ((frame - i128::from(viewport.start)) as f64 / viewport.length as f64))
        };
        for (pair, frame) in gradients.windows(2).zip(edges) {
            assert!((f64::from(pair[0].0.right()) - x(frame)).abs() <= 1.0);
            assert!((f64::from(pair[1].0.left()) - x(frame)).abs() <= 1.0);
        }
        assert!((f64::from(gradients[2].0.right()) - x(312_000)).abs() <= 1.0);
        assert_body(&mut visual, &editor, clip);
    }
}

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.worker_requests_pending == 0 && v.pending_requests.is_empty()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(
            std::time::Instant::now() < until,
            "pump: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[gpui::test]
fn waveform_body_painter_stays_below_live_normal_and_shift_resize_titles(cx: &mut TestAppContext) {
    use gpui::{point, px};
    for looping in [false, true] {
        for stretch in [false, true] {
            for right in [false, true] {
                let (_files, handle, editor, _track, clip) = fixture(cx, looping);
                let mut visual = VisualTestContext::from_window(handle, cx);
                visual.run_until_parked();
                let before =
                    editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
                let edge = if right { "right" } else { "left" };
                let down = visual
                    .debug_bounds(Box::leak(
                        format!("clip-trim-{edge}-{}", clip.value()).into_boxed_str(),
                    ))
                    .unwrap()
                    .center();
                let modifiers = Modifiers {
                    shift: stretch,
                    ..Modifiers::default()
                };
                let direction = if right && !stretch { -1.0 } else { 1.0 };
                visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
                visual.simulate_mouse_move(
                    point(down.x + px(12.0 * direction), down.y),
                    MouseButton::Left,
                    modifiers,
                );
                visual.run_until_parked();
                let target = point(down.x + px(40.0 * direction), down.y);
                visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
                visual.run_until_parked();
                let desired = editor.read_with(&visual, |v, _| {
                    assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                    v.active_trim_drag
                        .as_ref()
                        .unwrap()
                        .preview
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .clip
                        .clone()
                });
                assert_body(&mut visual, &editor, clip);
                visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
                pump(&editor, &mut visual);
                editor.read_with(&visual, |v, _| {
                    assert!(v.error.is_none(), "{:?}", v.error);
                    assert_eq!(v.state.as_ref().unwrap().project.clip(clip), Some(&desired));
                });
                assert_body(&mut visual, &editor, clip);
                visual.simulate_keystrokes("cmd-z");
                pump(&editor, &mut visual);
                editor.read_with(&visual, |v, _| {
                    assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
                });
            }
        }
    }
}

#[gpui::test]
fn waveform_body_painter_omits_header_only_rows_and_clips_tiny_bodies(cx: &mut TestAppContext) {
    use crate::editor::{
        clip_name::CLIP_NAME_HEIGHT,
        tracks::clip_waveform_body,
        waveform_geometry::{WaveformGeometryCache, WaveformMapping},
    };
    use gpui::{IntoElement, div, point, prelude::*, px, size};
    use hex_session::{CachedPeak, CachedWaveform};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        sync::Arc,
    };
    let visual = cx.add_empty_window();
    // Actual waveform canvas/path/mask, with a deterministic bipolar peak source.
    let peaks = Arc::new(
        CachedWaveform::from_peaks(
            96_000,
            vec![
                [CachedPeak {
                    min: -1.0,
                    max: 1.0
                }; 2];
                128
            ],
        )
        .unwrap(),
    );
    let cache = Rc::new(crate::editor::waveform_geometry::WaveformViewCache::new(
        Rc::new(RefCell::new(WaveformGeometryCache::default())),
    ));
    for height in [
        CLIP_NAME_HEIGHT - 1.0,
        CLIP_NAME_HEIGHT,
        CLIP_NAME_HEIGHT + 2.0,
        44.0,
    ] {
        cache.borrow_mut().clear();
        let title = Rc::new(Cell::new(None));
        let observed_title = Rc::clone(&title);
        visual.draw(
            point(px(0.0), px(0.0)),
            size(px(260.0), px(height)),
            |_, _| {
                div()
                    .relative()
                    .w(px(260.0))
                    .h(px(height))
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(CLIP_NAME_HEIGHT))
                            .child(
                                gpui::canvas(
                                    |_, _, _| (),
                                    move |bounds, _, _, _| observed_title.set(Some(bounds)),
                                )
                                .absolute()
                                .inset_0(),
                            ),
                    )
                    .child(clip_waveform_body(super::waveform_canvas(
                        Arc::clone(&peaks),
                        WaveformMapping::Source { timeline_origin: 0 },
                        0,
                        96_000,
                        Some(Rc::clone(&cache)),
                        None,
                        crate::theme::color(crate::theme::PaletteRole::Waveform),
                    )))
                    .into_element()
            },
        );
        let title = title
            .get()
            .expect("actual title-size paint even when no body remains");
        let cache = cache.borrow();
        if height <= CLIP_NAME_HEIGHT {
            assert!(
                cache.painted_waveforms().is_empty(),
                "header-only row must not squeeze a waveform into the title"
            );
        } else {
            assert!(!cache.painted_waveforms().is_empty());
            for paint in cache.painted_waveforms() {
                assert!(paint.bounds.top() >= title.bottom());
                assert!(paint.mask.top() >= title.bottom());
                assert!(paint.mask.bottom() <= px(height));
                for &(lo, hi) in paint.ordinates() {
                    assert!(lo.max(f32::from(paint.mask.top())) >= f32::from(title.bottom()));
                    assert!(hi.min(f32::from(paint.mask.bottom())) <= height);
                }
            }
        }
    }
}

#[gpui::test]
fn waveform_scale_is_applied_to_canonical_and_preview_paints(cx: &mut TestAppContext) {
    use crate::editor::waveform::WaveformAmplitudeScale;
    use crate::editor::waveform_geometry::{
        WaveformGeometryCache, WaveformMapping, WaveformViewCache,
    };
    use gpui::{point, prelude::*, px, size};
    use hex_project::{Command, Created, Editor, Project};
    use hex_session::{CachedPeak, CachedWaveform};
    use std::{cell::RefCell, rc::Rc, sync::Arc};

    let peak = CachedPeak {
        min: -0.1,
        max: 0.1,
    };
    let waveform = Arc::new(CachedWaveform::from_peaks(100, vec![[peak, peak]; 100]).unwrap());
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "scale".into(),
            path: "scale.wav".into(),
            sample_rate: 48_000,
            frames: 100,
        })
        .unwrap()
    else {
        panic!("imported clip")
    };
    let clip = editor.project().clip(clip).unwrap().clone();
    let shared = Rc::new(RefCell::new(WaveformGeometryCache::default()));
    let arrangement = Rc::new(WaveformViewCache::new(Rc::clone(&shared)));
    let detail = Rc::new(WaveformViewCache::new(shared));
    let visual = cx.add_empty_window();
    let paint = |visual: &mut gpui::VisualTestContext,
                 cache: &Rc<WaveformViewCache>,
                 mapping: WaveformMapping| {
        cache.borrow_mut().clear();
        visual.draw(
            point(px(0.0), px(0.0)),
            size(px(200.0), px(80.0)),
            |_, _| {
                gpui::div()
                    .relative()
                    .w(px(200.0))
                    .h(px(80.0))
                    .children([super::waveform_canvas(
                        Arc::clone(&waveform),
                        mapping,
                        0,
                        100,
                        Some(Rc::clone(cache)),
                        None,
                        crate::theme::color(crate::theme::PaletteRole::Waveform),
                    )])
                    .into_element()
            },
        );
        cache
            .borrow()
            .painted_waveforms()
            .back()
            .unwrap()
            .ordinates()[0]
    };
    let canonical = || WaveformMapping::Clip {
        clip: clip.clone(),
        timeline_origin: 0,
        fade: None,
    };
    let preview = || WaveformMapping::Prepared {
        timeline_origin: 0,
        root_offset: 0,
        gain: hex_project::ClipGain::UNITY,
    };
    for mapping in [canonical(), preview()] {
        let linear = paint(visual, &arrangement, mapping.clone());
        arrangement.set_amplitude_scale(WaveformAmplitudeScale::Decibel);
        let expanded = paint(visual, &arrangement, mapping.clone());
        assert!(expanded.0 < linear.0 && expanded.1 > linear.1);
        assert_eq!(paint(visual, &detail, mapping.clone()), linear);
        arrangement.set_amplitude_scale(WaveformAmplitudeScale::Linear);
        assert_eq!(paint(visual, &arrangement, mapping), linear);
    }
}

#[gpui::test]
fn arrangement_drag_waveform_helpers_follow_view_scale(cx: &mut TestAppContext) {
    use crate::editor::waveform::WaveformAmplitudeScale;
    use crate::editor::waveform_geometry::{WaveformGeometryCache, WaveformViewCache};
    use gpui::{point, prelude::*, px, size};
    use hex_session::{CachedPeak, CachedWaveform};
    use std::{cell::RefCell, rc::Rc, sync::Arc};

    let peak = CachedPeak {
        min: -0.1,
        max: 0.1,
    };
    let waveform = Arc::new(CachedWaveform::from_peaks(100, vec![[peak, peak]; 100]).unwrap());
    let shared = Rc::new(RefCell::new(WaveformGeometryCache::default()));
    let arrangement = Rc::new(WaveformViewCache::new(Rc::clone(&shared)));
    let other_view = Rc::new(WaveformViewCache::new(shared));
    let visual = cx.add_empty_window();
    let paint =
        |cache: &Rc<WaveformViewCache>, summary: bool, visual: &mut gpui::VisualTestContext| {
            cache.borrow_mut().clear();
            visual.draw(
                point(px(0.0), px(0.0)),
                size(px(200.0), px(80.0)),
                |_, _| {
                    let ink = crate::theme::color(crate::theme::PaletteRole::Waveform);
                    let waveform = if summary {
                        super::waveform_summary_view_cached(
                            Arc::clone(&waveform),
                            Rc::clone(cache),
                            ink,
                        )
                    } else {
                        super::waveform_source_segment_view_cached(
                            Arc::clone(&waveform),
                            0,
                            0,
                            100,
                            Rc::clone(cache),
                            ink,
                        )
                    };
                    gpui::div()
                        .relative()
                        .w(px(200.0))
                        .h(px(80.0))
                        .children([waveform])
                        .into_element()
                },
            );
            cache
                .borrow()
                .painted_waveforms()
                .back()
                .unwrap()
                .ordinates()[0]
        };
    for summary in [false, true] {
        let linear = paint(&arrangement, summary, visual);
        arrangement.set_amplitude_scale(WaveformAmplitudeScale::Decibel);
        let expanded = paint(&arrangement, summary, visual);
        assert!(expanded.0 < linear.0 && expanded.1 > linear.1);
        assert_eq!(paint(&other_view, summary, visual), linear);
        arrangement.set_amplitude_scale(WaveformAmplitudeScale::Linear);
        assert_eq!(paint(&arrangement, summary, visual), linear);
    }
}

#[gpui::test]
fn clip_waveform_palette_uses_a_contrasting_hue(cx: &mut TestAppContext) {
    use crate::theme::{self, ClipWaveformBackground};
    let (_files, handle, editor, track, clip) = fixture(cx, false);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    visual.run_until_parked();
    for identity in [
        hex_project::ElementColor::Green,
        hex_project::ElementColor::Rose,
    ] {
        editor.update(&mut visual, |view, _| {
            view.waveform_geometry_cache
                .borrow_mut()
                .clear_paint_samples();
            assert!(view.enqueue(Request::Edit(Command::SetTrackColor {
                track,
                color: identity
            })));
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            let model = state.project.clip(clip).unwrap();
            let expected = theme::clip_waveform_ink(
                identity,
                ClipWaveformBackground::Arrangement {
                    flavor: model.flavor(),
                    selected: view.selected_clip == Some(clip),
                },
                true,
            );
            let cache = view.waveform_geometry_cache.borrow();
            let paint = cache
                .painted_waveforms()
                .back()
                .expect("actual completed Arrangement paint");
            assert_eq!(
                paint.colors.waveform, expected,
                "latest Arrangement paint follows canonical hue without clearing geometry"
            );
            assert_ne!(paint.colors.waveform, 0);
            let fill = theme::arrangement_clip_body_fill(
                theme::clip_activity_fill(identity, model.flavor(), false, true),
                view.selected_clip == Some(clip),
            );
            assert!(theme::contrast_ratio(paint.colors.waveform, fill) >= 3.0);
        });
    }
    for source in [false, true] {
        editor.update(&mut visual, |view, cx| {
            view.selected_track = Some(track);
            view.selected_clip = Some(clip);
            view.clip_view_open = true;
            view.device_panel_active = false;
            view.audio_event_editor.source_view = source;
            view.waveform_geometry_cache
                .borrow_mut()
                .clear_paint_samples();
            cx.notify();
        });
        visual.run_until_parked();
        let panel = visual.debug_bounds("arrangement-audio-clip-view").unwrap();
        editor.read_with(&visual, |view, _| {
            let expected = theme::clip_waveform_ink(
                hex_project::ElementColor::Rose,
                if source {
                    ClipWaveformBackground::Source
                } else {
                    ClipWaveformBackground::Events
                },
                true,
            );
            let cache = view.waveform_geometry_cache.borrow();
            let paints: Vec<_> = cache
                .painted_waveforms()
                .iter()
                .filter(|p| p.bounds.top() >= panel.top())
                .collect();
            assert!(
                !paints.is_empty(),
                "actual clip-editor painter must run: source={source}"
            );
            for paint in paints {
                assert_eq!(paint.colors.waveform, expected);
                let fill = theme::color(if source {
                    theme::PaletteRole::Surface
                } else {
                    theme::PaletteRole::Clip
                });
                assert!(theme::contrast_ratio(paint.colors.waveform, fill) >= 3.0);
            }
        });
    }
}

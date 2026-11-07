//! Observe actual waveform paint callbacks/masks, never a mock header property.
use crate::editor::{EditorView, external_audio_drop_ui_test::BatchFiles, musical_time::SnapGrid};
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
                p.ordinates
            );
            assert!(
                p.bounds.top() >= header.bottom(),
                "waveform geometry must begin below actual title"
            );
            assert!(
                p.mask.top() >= header.bottom(),
                "actual fill/stroke content mask must exclude title"
            );
            for (lo, hi) in p.ordinates {
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
    let cache = Rc::new(RefCell::new(WaveformGeometryCache::default()));
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
                for (lo, hi) in paint.ordinates {
                    assert!(lo.max(f32::from(paint.mask.top())) >= f32::from(title.bottom()));
                    assert!(hi.min(f32::from(paint.mask.bottom())) <= height);
                }
            }
        }
    }
}

use super::*;
use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
use hex_project::{Command, Editor, Project};

#[gpui::test]
fn audio_and_midi_grid_paint_stays_below_titles_on_shared_lane_columns(
    cx: &mut gpui::TestAppContext,
) {
    let (project, track, _, audio) = audio_project();
    let asset = project.clip(audio).unwrap().asset();
    let mut canonical = Editor::new(project, 16).unwrap();
    let hex_project::Created::Clip(midi) = canonical
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(4).unwrap(),
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("midi clip")
    };
    let media = std::collections::BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.0; 2]; 96_000]).unwrap(),
    )]);
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(canonical.project(), &media).unwrap();
    let (handle, editor) = editor_window(
        cx,
        canonical.project().clone(),
        std::sync::Arc::new(preview),
    );
    let mut visual = gpui::VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.viewport = Some(Viewport {
            start: 0,
            length: 192_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let scale = visual.update(|window, _| window.scale_factor());
    for (clip, frame) in [(audio, 48_000), (midi, 144_000)] {
        let selector =
            |prefix| Box::leak(format!("{prefix}-{frame}").into_boxed_str()) as &'static str;
        let header = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
        let header = visual.debug_bounds(header).unwrap().scale(scale);
        let grid_selector = selector("clip-grid-line");
        let grid = visual.debug_bounds(grid_selector).unwrap().scale(scale);
        let lane = visual
            .debug_bounds(selector("arrangement-grid-line"))
            .unwrap()
            .scale(scale);
        assert!(
            grid.top() >= header.bottom(),
            "grid paint must not cross the transparent title or its corner cut"
        );
        assert_eq!(
            grid.left(),
            lane.left(),
            "body restriction preserves physical timeline columns"
        );
        assert_eq!(f64::from(grid.size.width), 1.0);
        assert!(
            visual.rendered_surface_geometry(grid_selector).is_some(),
            "actual completed scene contains the grid stroke"
        );
    }
}

struct ThumbnailOracle(hex_project::ClipId);
impl gpui::Render for ThumbnailOracle {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
        // A five-row fitted range: thick rows, top and bottom at the body edges.
        let shapes = [
            (0.0, 0.1, 64),
            (0.13, 0.31, 60),
            (0.57, 0.005, 62),
            (0.9, 0.1, 60),
        ];
        let fit = super::super::midi_thumbnail::PitchFit::from_pitches([64, 60, 62]);
        let mut legacy = div().absolute().inset_0();
        for (x, width, pitch) in shapes {
            let (top, height) = super::super::midi_thumbnail::note_rows(fit, pitch, 73.25).unwrap();
            legacy = legacy.child(
                div()
                    .absolute()
                    .left(relative(x))
                    .top(px(top))
                    .w(relative(width))
                    .h(px(height))
                    .bg(rgb(0x00ff00)),
            );
        }
        div().size_full().child(
            div()
                .debug_selector(|| "thumbnail-oracle".into())
                .absolute()
                .left(px(13.25))
                .top(px(37.25))
                .w(px(193.25))
                .h(px(73.25))
                .overflow_hidden()
                .children([legacy, midi_note_body(self.0, shapes, fit, false).0]),
        )
    }
}

#[gpui::test]
fn batched_note_quads_match_fitted_layout_rows_and_body_clipping(cx: &mut gpui::TestAppContext) {
    let clip = audio_project().3;
    let handle = cx.add_window(|_, _| ThumbnailOracle(clip));
    let mut visual = gpui::VisualTestContext::from_window(*handle, cx);
    visual.run_until_parked();
    let quads = visual.rendered_filled_quads("thumbnail-oracle");
    let rectangles = |color| {
        let mut found: Vec<_> = quads
            .iter()
            .filter(|(_, fill)| *fill == gpui::Background::from(rgb(color)))
            .map(|(bounds, _)| *bounds)
            .collect();
        found.sort_by_key(|a| a.left());
        found
    };
    let old = rectangles(0x00ff00);
    let new = rectangles(0x000000);
    assert_eq!(old.len(), 4);
    assert_eq!(new.len(), old.len());
    for (old, new) in old.iter().zip(new) {
        // Taffy rounds using its unrounded ancestor offsets; the canvas
        // receives the rounded body. They may differ by one physical pixel.
        for (old, new) in [
            (old.left(), new.left()),
            (old.right(), new.right()),
            (old.top(), new.top()),
            (old.bottom(), new.bottom()),
        ] {
            assert!(
                (f64::from(old - new)).abs() <= 1.0,
                "old={old:?}, new={new:?}"
            );
        }
    }
}

#[gpui::test]
fn drop_overlap_spans_clip_merge_and_ignore_exact_touches(cx: &mut gpui::TestAppContext) {
    let (project, source, _, clip) = audio_project();
    let mut model = Editor::new(project, 8).unwrap();
    model
        .execute(Command::CopyClipTo {
            clip,
            track: source,
            start: 48_000,
        })
        .unwrap();
    let (_handle, editor) = editor_window(cx, model.project().clone(), Default::default());
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        let track = state.project.track(source).unwrap();
        assert_eq!(
            drop_overlap_spans(state, track, (24_000, 120_000), None),
            vec![(24_000, 120_000)]
        );
        assert_eq!(
            drop_overlap_spans(state, track, (24_000, 120_000), Some(clip)),
            vec![(48_000, 120_000)]
        );
        assert!(drop_overlap_spans(state, track, (144_000, 192_000), None).is_empty());
        assert!(drop_overlap_spans(state, track, (12_000, 12_000), None).is_empty());
    });
}
fn drag_content_next(service: &hex_session::service::Service) -> hex_session::service::Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual Service fixture response missing"
        );
        std::thread::yield_now();
    }
}

#[gpui::test]
fn held_midi_drag_keeps_actual_note_paint_and_colored_name_below_header(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{AppContext, Modifiers, MouseButton, VisualTestContext};
    use hex_project::{BeatTime, Created};
    use hex_session::service::{Request, Service};
    cx.update(crate::editor::bind_editor_keys);
    let environment = crate::editor::test_environment::Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    drag_content_next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "source".into(),
        }))
        .unwrap();
    let e = drag_content_next(&service);
    let source = e.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "target".into(),
        }))
        .unwrap();
    let e = drag_content_next(&service);
    let target = e.state.project.tracks()[1].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track: source,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let e = drag_content_next(&service);
    let Created::Clip(clip) = e.created.unwrap() else {
        panic!("actual MIDI clip");
    };
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .unwrap();
    let e = drag_content_next(&service);
    assert!(!e.state.midi_preview.get(clip).unwrap().samples.is_empty());
    let before = e.state.project.clone();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(window, cx, service, environment.clone())
            });
            editor.update(cx, |view, cx| {
                assert!(std::sync::Arc::ptr_eq(&view.test_environment, &environment));
                view.state = Some(e.state);
                view.snap_grid = view.snap_grid.with_platform_modifier(true);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    let (original, original_fill) =
        super::super::midi_thumbnail::rendered_notes(&mut visual, clip, false)
            .into_iter()
            .next()
            .expect("normal production note quad");
    let header = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    let start = header.center();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(start.x + px(12.0), start.y),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    let lane = editor.read_with(&visual, |view, _| view.drag_lanes[&target]);
    let end = point(start.x + px(72.0), px(lane.top + lane.height * 0.5));
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let payload = view.active_clip_drag.as_ref().expect("real captured drag");
        assert_eq!(payload.preview.get().unwrap().target_track, target);
        assert_eq!(view.state.as_ref().unwrap().project, before);
    });
    let ghost = visual
        .debug_bounds("clip-drop-preview")
        .expect("actual target preview");
    super::super::clip_boundary::tests::assert_open_upper_right(
        &mut visual,
        "clip-drop-preview",
        "clip-drop-preview-trailing",
        Some(Box::leak(
            format!("clip-drag-name-{}", clip.value()).into_boxed_str(),
        )),
    );
    let (actual, fill) = super::super::midi_thumbnail::rendered_notes(&mut visual, clip, true)
        .into_iter()
        .next()
        .expect("held target production MIDI note quad");
    assert_eq!(fill, original_fill);
    assert!(actual.size.width > px(0.0) && actual.size.height == original.size.height);
    assert!(actual.top() >= ghost.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT));
    assert!(actual.bottom() <= ghost.bottom());
    let title = Box::leak(format!("clip-drag-name-{}", clip.value()).into_boxed_str());
    assert!(
        visual.rendered_background(title).is_some(),
        "actual colored clone title"
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("clip-drop-preview").is_none());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before)
    });
}

#[gpui::test]
fn held_processed_audio_drag_reuses_ready_root_on_lane_and_floating(cx: &mut gpui::TestAppContext) {
    assert_audio_drag_content(cx, false);
}

#[gpui::test]
fn held_looped_warp_drag_keeps_last_ready_content_in_mixed_copy_group(
    cx: &mut gpui::TestAppContext,
) {
    assert_audio_drag_content(cx, true);
}

fn assert_audio_drag_content(cx: &mut gpui::TestAppContext, warped_loop: bool) {
    use gpui::{AppContext, Modifiers, MouseButton, VisualTestContext};
    use hex_session::service::{Request, Service};
    use std::sync::Arc;
    cx.update(crate::editor::bind_editor_keys);
    let environment = crate::editor::test_environment::Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let path = environment.root().join("ready-root.wav");
    let frames = 96_000_u32;
    let data_bytes = frames * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&96_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for i in 0..frames {
        wav.extend_from_slice(&(((i % 120) as i16 - 60) * 256).to_le_bytes());
    }
    std::fs::write(&path, wav).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    drag_content_next(&service);
    // On a setup failure, retain the path until the real import ACK proves
    // the worker has completed its filesystem access.
    service.submit(Request::Import(path.clone())).unwrap();
    let deadline = crate::test_wait::hang_deadline();
    let e = loop {
        if let Ok(event) = service.try_event() {
            if event.error.is_some() {
                environment.retain_for_unacknowledged_setup_failure();
                panic!("actual import failed: {:?}", event.error);
            }
            break event;
        }
        if std::time::Instant::now() >= deadline {
            environment.retain_for_unacknowledged_setup_failure();
            panic!("actual import ACK required before releasing fixture paths");
        }
        std::thread::yield_now();
    };
    let clip = e.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(Command::TrimClip {
            clip,
            start: 48_000,
            duration: 48_000,
            source_offset: 12_000,
            source_length: 48_000,
        }))
        .unwrap();
    drag_content_next(&service);
    service
        .submit(Request::Edit(Command::SetClipFades {
            clip,
            fade_in: 4_000,
            fade_out: 6_000,
        }))
        .unwrap();
    drag_content_next(&service);
    service
        .submit(Request::Edit(Command::SetAudioClipPitch {
            clip,
            cents: 300,
        }))
        .unwrap();
    let e = drag_content_next(&service);
    if warped_loop {
        service
            .submit(Request::Edit(Command::SetClipLooping {
                clip,
                looping: true,
            }))
            .unwrap();
        let e = drag_content_next(&service);
        let settings = hex_project::default_audio_warp(
            e.state.project.clip(clip).unwrap(),
            e.state.project.tempo_map(),
            hex_project::WarpMode::Beats {
                preserve: hex_project::BeatsPreserve::Transients,
                tail: hex_project::BeatsTail::Forward,
                decay: 100,
            },
        )
        .unwrap();
        service
            .submit(Request::Edit(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            }))
            .unwrap();
        drag_content_next(&service);
    } else {
        assert!(e.state.project.clip(clip).unwrap().audio_edit().is_some());
    }
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "target".into(),
        }))
        .unwrap();
    let e = drag_content_next(&service);
    let target = e.state.project.tracks()[1].id();
    let source_track = e.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track: source_track,
            start: hex_project::BeatTime::from_quarters(5).unwrap(),
            length: hex_project::BeatTime::ONE,
        }))
        .unwrap();
    let e = drag_content_next(&service);
    let hex_project::Created::Clip(midi) = e.created.unwrap() else {
        panic!("actual mixed-group MIDI member");
    };
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip: midi,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 100,
        }))
        .unwrap();
    let e = drag_content_next(&service);
    let source = e.state.project.clip(clip).unwrap().clone();
    assert!(source.audio_edit().is_some());
    let raw_waveform = Arc::clone(e.state.waveforms.get(&source.asset()).unwrap());
    let ready = e.state.audio_clip_waveforms.get(&clip).unwrap().clone();
    let expected_source = ready
        .event_source
        .as_ref()
        .map(|event_source| Arc::clone(&event_source.waveform))
        .unwrap_or_else(|| Arc::clone(&ready.waveform));
    assert!(
        !Arc::ptr_eq(&expected_source, &raw_waveform),
        "ready processed waveform must be distinct from the raw asset waveform"
    );
    let before = e.state.project.clone();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(window, cx, service, environment.clone())
            });
            editor.update(cx, |view, cx| {
                assert!(Arc::ptr_eq(&view.test_environment, &environment));
                view.state = Some(e.state);
                view.snap_grid = view.snap_grid.with_platform_modifier(true);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    let header = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    let start = header.center();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(start.x + px(12.0), start.y),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    let lane = editor.read_with(&visual, |v, _| v.drag_lanes[&target]);
    let landing = point(start.x + px(90.0), px(lane.top + lane.height * 0.5));
    let floating = point(px(60.0), px(100.0));
    for (pointer, selector) in [
        (landing, "clip-drop-preview"),
        (floating, "clip-drag-floating-preview"),
    ] {
        visual.simulate_mouse_move(pointer, Some(MouseButton::Left), Modifiers::default());
        editor.update(&mut visual, |v, cx| {
            v.clip_content_cache = Default::default();
            v.waveform_geometry_cache.borrow_mut().clear();
            cx.notify();
        });
        visual.run_until_parked();
        let bounds = visual.debug_bounds(selector).expect("real clone surface");
        if selector == "clip-drag-floating-preview" {
            super::super::clip_boundary::tests::assert_open_upper_right(
                &mut visual,
                selector,
                "clip-drag-floating-preview-trailing",
                Some(Box::leak(
                    format!("clip-drag-name-{}", clip.value()).into_boxed_str(),
                )),
            );
            let (corners, _) = visual.rendered_surface_geometry(selector).unwrap();
            assert_eq!(
                corners,
                gpui::Corners::default(),
                "floating source keeps square bottom corners"
            );
        }

        let title = visual
            .debug_bounds(Box::leak(
                format!("clip-drag-name-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap();
        assert!(
            visual.rendered_border(selector).is_some(),
            "actual highlighted border"
        );
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project, before);
            assert_eq!(v.worker_requests_pending, 0);
            let cache = v.waveform_geometry_cache.borrow();
            let paints: Vec<_> = cache
                .painted_waveforms()
                .iter()
                .filter(|p| {
                    p.bounds.top() >= bounds.top()
                        && p.bounds.top() < bounds.bottom()
                        && p.bounds.left() >= bounds.left()
                        && p.bounds.left() < bounds.right()
                })
                .collect();
            assert!(
                !paints.is_empty(),
                "ready processed waveform must actually paint in {selector}"
            );
            for paint in paints {
                assert!(paint.bounds.top() >= title.bottom());
                assert!(paint.mask.top() >= title.bottom());
                assert!(paint.mask.bottom() <= bounds.bottom());
            }
            let rendered = cache.rendered_waveforms();
            assert!(
                rendered.iter().any(|sample| {
                    Arc::ptr_eq(&sample.source, &expected_source)
                        && sample.prepared == Some((source.start(), ready.offset))
                        && sample.spans[0] > 0.0
                }),
                "clone must use original ready processed root and source offset"
            );
            assert!(
                !rendered
                    .iter()
                    .any(|sample| Arc::ptr_eq(&sample.source, &raw_waveform)),
                "processed drag clone must not fall back to the raw asset waveform"
            );
        });
    }
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(floating, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("clip-drag-floating-preview").is_none());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
    // Actual additive header selection, then drag a non-primary member.
    for (index, id) in [clip, midi].into_iter().enumerate() {
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", id.value()).into_boxed_str(),
            ))
            .unwrap();
        visual.simulate_click(
            header.center(),
            Modifiers {
                platform: index != 0,
                ..Modifiers::default()
            },
        );
    }
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert_eq!(v.selected_clip_ids().len(), 2));
    let header = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    let down = header.center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    for duplicate in [false, true] {
        visual.simulate_modifiers_change(Modifiers {
            alt: duplicate,
            ..Modifiers::default()
        });
        let pointer = point(
            down.x + px(if duplicate { 110.0 } else { 80.0 }),
            px(lane.top + lane.height * 0.5),
        );
        visual.simulate_mouse_move(
            pointer,
            Some(MouseButton::Left),
            Modifiers {
                alt: duplicate,
                ..Modifiers::default()
            },
        );
        editor.update(&mut visual, |v, cx| {
            v.clip_content_cache = Default::default();
            v.waveform_geometry_cache.borrow_mut().clear();
            cx.notify();
        });
        visual.run_until_parked();
        let audio_bounds = visual
            .debug_bounds(Box::leak(
                format!("group-clip-preview-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap();
        let midi_bounds = visual
            .debug_bounds(Box::leak(
                format!("group-clip-preview-{}", midi.value()).into_boxed_str(),
            ))
            .unwrap();
        let (note, _) = super::super::midi_thumbnail::rendered_notes(&mut visual, midi, true)
            .into_iter()
            .next()
            .expect("actual mixed-group note quad");
        assert!(note.top() >= midi_bounds.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT));
        assert!(note.bottom() <= midi_bounds.bottom());
        for id in [clip, midi] {
            let title = Box::leak(format!("clip-drag-name-{}", id.value()).into_boxed_str());
            assert!(visual.rendered_background(title).is_some());
        }
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project, before);
            let cache = v.waveform_geometry_cache.borrow();
            assert!(
                cache.painted_waveforms().iter().any(|p| {
                    p.bounds.top()
                        >= audio_bounds.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT)
                        && p.bounds.top() < audio_bounds.bottom()
                        && p.bounds.left() >= audio_bounds.left()
                        && p.bounds.left() < audio_bounds.right()
                }),
                "mixed-group audio must actually paint in both move/copy modes"
            );
            let rendered = cache.rendered_waveforms();
            assert!(
                rendered.iter().any(|sample| {
                    Arc::ptr_eq(&sample.source, &expected_source)
                        && sample.prepared == Some((source.start(), ready.offset))
                        && sample.spans[0] > 0.0
                }),
                "mixed-group clone must keep original ready processed root and source offset"
            );
            assert!(
                !rendered
                    .iter()
                    .any(|sample| Arc::ptr_eq(&sample.source, &raw_waveform)),
                "mixed-group processed clone must not fall back to the raw asset waveform"
            );
            assert_eq!(
                v.active_clip_drag.as_ref().unwrap().copy_modifier.get(),
                duplicate
            );
            assert_eq!(v.worker_requests_pending, 0);
        });
    }
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.active_clip_drag.is_none());
        assert_eq!(v.state.as_ref().unwrap().project, before);
    });
}

#[gpui::test]
fn track_row_focus_gap_cues_stay_in_lane_and_ignore_fold_tag_gaps(cx: &mut gpui::TestAppContext) {
    let mut model = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut tracks = Vec::new();
    for name in ["before", "grouped", "folded_sibling", "after"] {
        let hex_project::Created::Track(id) = model
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        tracks.push(id);
    }
    let hex_project::Created::Track(group) = model
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[tracks[1], tracks[2]]).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    let hex_project::Created::ArrangementSection(section) = model
        .execute(Command::AddArrangementSection {
            name: "s0".into(),
            color: hex_project::ElementColor::Cyan,
            start: BeatTime::from_quarters(1).unwrap(),
            end: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    else {
        panic!("section")
    };
    // Give `tracks[1]` a clip inside `section` and a native device so its automation lane can expand.
    model
        .execute(Command::PlaceMidiClip {
            track: tracks[1],
            start: BeatTime::from_quarters(1).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap();
    let state = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let hex_project::Created::Device(device) = model
        .execute(Command::InsertDevice {
            track: tracks[1],
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    model
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: hex_lattice_contract::DeviceAutomationTarget::HostOutputGain,
            points: vec![hex_project::AutomationPoint {
                beat: BeatTime::ZERO,
                value: hex_project::AutomationValue::new(0.5).unwrap(),
                outgoing: hex_project::AutomationInterpolation::Step,
            }],
        })
        .unwrap();
    let tag = hex_project::TrackTag::new("focus").unwrap();
    for track in [group, tracks[1]] {
        model
            .execute(Command::SetTrackTags {
                track,
                tags: vec![tag.clone()],
            })
            .unwrap();
    }
    let (_, preview) = hex_engine::compile::project_with_midi_preview(
        model.project(),
        &std::collections::BTreeMap::new(),
    )
    .unwrap();
    let (handle, editor) = editor_window(cx, model.project().clone(), std::sync::Arc::new(preview));
    let mut visual = gpui::VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();

    // Expand an extra automation lane on `tracks[1]`.
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.automation_view.enabled = true;
            view.automation_menu_action(
                super::super::device_automation::Action::Reveal {
                    generation: view.project_generation,
                    owner: super::super::device_panel::DevicePanelOwner::Track(tracks[1]),
                    used_only: false,
                },
                window,
                cx,
            );
            cx.notify();
        });
    });
    visual.run_until_parked();

    let sel = |prefix: &str, id: u64| -> &'static str {
        Box::leak(format!("{prefix}-{id}").into_boxed_str())
    };
    let secondary: gpui::Background = rgb(theme::color(theme::PaletteRole::Secondary)).into();
    let expected_color: gpui::Background =
        super::super::arrangement_focus::hidden_tracks::line_color().into();

    // 1. Resting borders with section focus OFF: zero section-hidden cues and no Secondary borders.
    for &id in &[tracks[0], group, tracks[1], tracks[2], tracks[3]] {
        assert!(
            visual
                .debug_bounds(sel("section-hidden-before", id.value()))
                .is_none()
        );
        assert!(
            visual
                .debug_bounds(sel("section-hidden-after", id.value()))
                .is_none()
        );
    }
    for border_sel in [
        sel("lane-separator", tracks[1].value()),
        sel("lane-right-divider", tracks[1].value()),
        sel("arrangement-track-header", tracks[1].value()),
    ] {
        let quads = visual.rendered_filled_quads(border_sel);
        assert!(
            quads.iter().all(|(_, bg)| *bg != secondary),
            "resting border {border_sel} must never paint PaletteRole::Secondary"
        );
    }

    // 2. Section focus ON with tag filter leaving only `group` and `tracks[1]` (non-contiguous
    // because `tracks[0]` is excluded by tag): zero section-hidden cues!
    editor.update(&mut visual, |view, cx| {
        view.sections.selected = Some(section);
        view.arrangement_focus.projection.set_tag_filter(Some(tag));
        view.toggle_arrangement_focus();
        cx.notify();
    });
    visual.run_until_parked();
    for &id in &[group, tracks[1]] {
        assert!(
            visual
                .debug_bounds(sel("section-hidden-before", id.value()))
                .is_none()
        );
        assert!(
            visual
                .debug_bounds(sel("section-hidden-after", id.value()))
                .is_none()
        );
    }

    // 3. Clear tag filter while keeping section focus ON: now `tracks[0]` (before `group`)
    // and `tracks[2]`, `tracks[3]` (after `tracks[1]`) are hidden by section focus.
    editor.update(&mut visual, |view, cx| {
        view.arrangement_focus.projection.set_tag_filter(None);
        view.reconcile_arrangement_focus();
        cx.notify();
    });
    visual.run_until_parked();

    let before_group = visual
        .debug_bounds(sel("section-hidden-before", group.value()))
        .expect("section gap before group");
    let group_lane = visual
        .debug_bounds(sel("arrangement-lane", group.value()))
        .unwrap();
    let group_divider = visual
        .debug_bounds(sel("lane-right-divider", group.value()))
        .unwrap();
    let group_header = visual
        .debug_bounds(sel("arrangement-track-header", group.value()))
        .unwrap();
    let group_strip = visual
        .debug_bounds(sel("track-color-strip", group.value()))
        .unwrap();
    assert_eq!(before_group.top(), group_lane.top());
    assert_eq!(before_group.left(), group_lane.left());
    assert!(before_group.right() <= group_divider.left());
    assert!(before_group.right() < group_header.left());
    assert!(before_group.right() < group_strip.left());

    let after_child = visual
        .debug_bounds(sel("section-hidden-after", tracks[1].value()))
        .expect("section gap after child with expanded automation lane");
    let child_lane = visual
        .debug_bounds(sel("arrangement-lane", tracks[1].value()))
        .unwrap();
    let child_divider = visual
        .debug_bounds(sel("lane-right-divider", tracks[1].value()))
        .unwrap();
    let child_header = visual
        .debug_bounds(sel("arrangement-track-header", tracks[1].value()))
        .unwrap();
    let child_rail = visual
        .debug_bounds(Box::leak(
            format!("track-group-rail-{}-{}", tracks[1].value(), group.value()).into_boxed_str(),
        ))
        .unwrap();
    let child_strip = visual
        .debug_bounds(sel("track-color-strip", tracks[1].value()))
        .unwrap();
    let full_height = editor.read_with(&visual, |view, _| view.track_height(tracks[1]));
    assert!(
        after_child.bottom() > child_lane.bottom(),
        "after-cue sits below expanded automation lane, never across track body"
    );
    assert_eq!(after_child.bottom(), child_lane.top() + px(full_height));
    assert_eq!(after_child.left(), child_lane.left());
    assert!(after_child.right() <= child_divider.left());
    assert!(after_child.right() < child_header.left());
    assert!(after_child.right() < child_rail.left());
    assert!(after_child.right() < child_strip.left());

    for name in [
        sel("section-hidden-before", group.value()),
        sel("section-hidden-after", tracks[1].value()),
    ] {
        let quads = visual.rendered_filled_quads(name);
        assert!(quads.iter().any(|(_, bg)| *bg == expected_color));
        assert!(quads.iter().all(|(_, bg)| *bg != secondary));
    }
}

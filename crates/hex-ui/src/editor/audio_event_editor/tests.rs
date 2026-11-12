use super::*;
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{AudioEventEdit, Command};

#[gpui::test]
fn clip_waveform_scale_toggle_is_independent_and_survives_view_switches(cx: &mut TestAppContext) {
    use super::super::waveform::WaveformAmplitudeScale;

    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.waveform_geometry_cache
            .set_amplitude_scale(WaveformAmplitudeScale::Decibel);
        assert_eq!(
            view.clip_waveform_geometry_cache.amplitude_scale(),
            WaveformAmplitudeScale::Linear
        );
        cx.notify();
    });
    visual.run_until_parked();
    let toggle = visual.debug_bounds("audio-waveform-scale").unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.clip_waveform_geometry_cache.amplitude_scale(),
            WaveformAmplitudeScale::Decibel
        );
        assert_eq!(
            view.waveform_geometry_cache.amplitude_scale(),
            WaveformAmplitudeScale::Decibel
        );
    });
    let source = visual.debug_bounds("audio-source-tab").unwrap();
    visual.simulate_click(source.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.audio_event_editor.source_view);
        assert_eq!(
            view.clip_waveform_geometry_cache.amplitude_scale(),
            WaveformAmplitudeScale::Decibel
        );
    });
    let toggle = visual.debug_bounds("audio-waveform-scale").unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.clip_waveform_geometry_cache.amplitude_scale(),
            WaveformAmplitudeScale::Linear
        );
        assert_eq!(
            view.waveform_geometry_cache.amplitude_scale(),
            WaveformAmplitudeScale::Decibel
        );
    });
}

#[gpui::test]
fn retained_source_selector_dispatches_and_acknowledges_event_activation(cx: &mut TestAppContext) {
    use super::super::drag::group_drag_ui_test::pump;
    use hex_session::service::Request;
    cx.update(super::super::bind_editor_keys);
    let (files, handle, editor) = super::super::clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    let (clip, generation) = editor.read_with(&visual, |view, _| {
        (
            view.state.as_ref().unwrap().project.tracks()[0].clips()[0].id(),
            view.project_generation,
        )
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::SetActive {
                events: vec![1],
                active: false
            }
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::DuplicateClip(clip))))
    });
    pump(&editor, &mut visual);
    let (clips, revision) = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        (
            state.project.tracks()[0]
                .clips()
                .iter()
                .map(|clip| clip.id())
                .collect(),
            state.revision,
        )
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::GroupClips {
            project_generation: generation,
            revision,
            clips
        }))
    });
    pump(&editor, &mut visual);
    let group = editor.update(&mut visual, |view, cx| {
        let group = view.state.as_ref().unwrap().project.tracks()[0].clips()[0].id();
        view.selected_clip = Some(group);
        view.clip_view_open = true;
        view.device_panel_active = false;
        cx.notify();
        group
    });
    visual.run_until_parked();
    let event = visual.debug_bounds("audio-event-1").unwrap();
    let down = point(
        event.left() + event.size.width * 0.25,
        event.top() + px(2.0),
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let source = visual
        .debug_bounds("audio-retained-source")
        .expect("retained source selector");
    let button = point(
        source.left() + source.size.width * 0.5,
        source.top() + source.size.height * 0.5,
    );
    visual.simulate_mouse_down(button, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(button, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("0");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let events = view
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(group)
            .unwrap()
            .audio_events();
        let root = events
            .sources
            .iter()
            .find(|root| root.id == events.events[0].source)
            .unwrap();
        assert!(root.clip.audio_events().events[0].active);
    });
    let before_gain = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    let grip = visual.debug_bounds("audio-event-gain-1").unwrap();
    let from = grip.center();
    let to = point(from.x, from.y + px(20.0));
    visual.simulate_mouse_down(
        from,
        MouseButton::Left,
        Modifiers {
            alt: true,
            ..Modifiers::default()
        },
    );
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let events = view
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(group)
            .unwrap()
            .audio_events();
        let root = events
            .sources
            .iter()
            .find(|root| root.id == events.events[0].source)
            .unwrap();
        assert_eq!(root.clip.audio_events().events[0].gain.millidb(), -2000);
        assert_eq!(events.events[0].gain, hex_project::ClipGain::UNITY);
    });
    let after_gain = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(&visual, |view, _| view
            .state
            .as_ref()
            .unwrap()
            .project
            .snapshot()),
        before_gain
    );
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(&visual, |view, _| view
            .state
            .as_ref()
            .unwrap()
            .project
            .snapshot()),
        after_gain
    );
    let path = files.root().join("retained-event-gain.hex");
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Open(path)))
    });
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(&visual, |view, _| view
            .state
            .as_ref()
            .unwrap()
            .project
            .snapshot()),
        after_gain
    );
}

#[gpui::test]
fn nested_source_controls_reactivate_the_selected_inner_event(cx: &mut TestAppContext) {
    use super::super::drag::group_drag_ui_test::pump;
    use hex_session::service::Request;
    cx.update(super::super::bind_editor_keys);
    let (_files, handle, editor) = super::super::clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    let clip = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].clips()[0].id()
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::SetActive {
                events: vec![1],
                active: false
            },
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::DuplicateClip(clip))))
    });
    pump(&editor, &mut visual);
    let group = |editor: &gpui::Entity<super::super::EditorView>,
                 visual: &mut VisualTestContext| {
        let (clips, revision, generation) =
            editor.read_with(visual, |view: &super::super::EditorView, _| {
                (
                    view.state.as_ref().unwrap().project.tracks()[0]
                        .clips()
                        .iter()
                        .map(|clip| clip.id())
                        .collect(),
                    view.state.as_ref().unwrap().revision,
                    view.project_generation,
                )
            });
        editor.update(visual, |view: &mut super::super::EditorView, _| {
            assert!(view.enqueue(Request::GroupClips {
                project_generation: generation,
                revision,
                clips
            }))
        });
        pump(editor, visual);
        editor.read_with(visual, |view: &super::super::EditorView, _| {
            view.state.as_ref().unwrap().project.tracks()[0].clips()[0].id()
        })
    };
    let first = group(&editor, &mut visual);
    let end = editor.read_with(&visual, |view, _| {
        let clip = view.state.as_ref().unwrap().project.clip(first).unwrap();
        clip.start() + clip.duration() + clip.duration() / 2
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::ResizeAudioClipView {
            clip: first,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame: end },
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::DuplicateClip(first))))
    });
    pump(&editor, &mut visual);
    let outer = group(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .clip(outer)
                .unwrap()
                .audio_events()
                .sources
                .iter()
                .any(|root| root.clip.has_retained_audio_roots())
        );
    });
    editor.update(&mut visual, |view, cx| {
        view.selected_clip = Some(outer);
        view.clip_view_open = true;
        view.device_panel_active = false;
        cx.notify();
    });
    visual.run_until_parked();
    let waveform = visual.debug_bounds("audio-event-1").unwrap();
    let event = point(
        waveform.left() + waveform.size.width * 0.25,
        waveform.top() + px(2.0),
    );
    visual.simulate_mouse_down(event, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(event, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.audio_event_editor.selected.is_some(),
            "selection={:?}, bounds={:?}, events={:?}, error={:?}",
            view.audio_event_editor.selected,
            view.audio_event_editor.frame_bounds,
            view.state
                .as_ref()
                .unwrap()
                .project
                .clip(outer)
                .unwrap()
                .audio_events()
                .events,
            view.error
        );
    });
    for selector in ["audio-retained-source", "audio-retained-down"] {
        let bounds = visual.debug_bounds(selector).expect(selector);
        let button = point(
            bounds.left() + bounds.size.width * 0.5,
            bounds.top() + bounds.size.height * 0.5,
        );
        visual.simulate_mouse_down(button, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(button, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
    visual.simulate_keystrokes("0");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let outer = view
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(outer)
            .unwrap()
            .audio_events();
        let inner = outer
            .sources
            .iter()
            .find(|root| root.clip.has_retained_audio_roots())
            .unwrap()
            .clip
            .audio_events();
        let leaf = inner
            .sources
            .iter()
            .find(|root| root.id == inner.events[0].source)
            .unwrap();
        assert!(leaf.clip.audio_events().events[0].active);
    });
    let bounds = visual.debug_bounds("audio-retained-up").unwrap();
    let up = point(
        bounds.left() + bounds.size.width * 0.5,
        bounds.top() + bounds.size.height * 0.5,
    );
    visual.simulate_mouse_down(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let (path, event) = view.audio_event_editor.retained_selection.as_ref().unwrap();
        assert_eq!(path.len(), 1);
        assert_eq!(*event, 1);
    });
}

#[gpui::test]
fn zero_toggles_the_focused_selected_audio_event_and_paints_it_inactive(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let point = point(
        bounds.origin.x + bounds.size.width * 0.125,
        bounds.top() + px(26.0),
    );
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("0");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(matches!(&view.last_enqueued_edit, Some(Command::EditAudioEvents { clip: id, operation: AudioEventEdit::SetActive { events, active: false } }) if *id == clip && events == &[1]));
    });
    assert!(visual.debug_bounds("audio-event-1-inactive").is_some());
}

#[gpui::test]
fn play_start_marker_follows_clip_phase_and_viewport_without_editing(cx: &mut TestAppContext) {
    use crate::theme;
    struct Restore(hex_preferences::ThemePreferences);
    impl Drop for Restore {
        fn drop(&mut self) {
            theme::apply_preferences(self.0);
        }
    }
    let _restore = Restore(theme::preferences());

    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body = visual.debug_bounds("audio-events-waveform").unwrap();
    let marker = visual
        .debug_bounds("audio-play-start-marker")
        .expect("plain audio play start");
    assert!((marker.left() - body.left()).abs() < px(1.0));
    editor.update(&mut visual, |v, cx| {
        let prior = v.state.as_ref().unwrap();
        let mut model = hex_project::Editor::new(prior.project.as_ref().clone(), 8).unwrap();
        model
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: hex_project::AudioViewResizeOperation::CycleRange {
                    start: 24_000,
                    end: 48_000,
                    looping: true,
                },
            })
            .unwrap();
        model
            .execute(Command::TrimClipEdge {
                clip,
                edge: hex_project::TrimEdge::Left,
                frame: 6_000,
            })
            .unwrap();
        v.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
            project: std::sync::Arc::new(model.project().clone()),
            canonical_audio_source: prior.canonical_audio_source.clone(),
            comparison_audio: prior.comparison_audio.clone(),
            waveforms: prior.waveforms.clone(),
            source_overviews: prior.source_overviews.clone(),
            audio_clip_waveforms: prior.audio_clip_waveforms.clone(),
            midi_preview: prior.midi_preview.clone(),
            gain_preview: prior.gain_preview.clone(),
            timeline_frames: prior.timeline_frames,
            can_undo: prior.can_undo,
            can_redo: prior.can_redo,
            revision: prior.revision + 1,
            unsaved: prior.unsaved,
            document_path: prior.document_path.clone(),
        }));
        cx.notify();
    });
    visual.run_until_parked();
    let original = editor.read_with(&visual, |v, _| {
        (v.state.as_ref().unwrap().project.clone(), v.selected_clip)
    });
    for (index, (viewport, expected)) in [
        (None, Some(30_000.0 / 96_000.0)),
        (
            Some(Viewport {
                start: 24_000,
                length: 24_000,
            }),
            Some(0.25),
        ),
        (
            Some(Viewport {
                start: 48_000,
                length: 24_000,
            }),
            None,
        ),
        (None, Some(30_000.0 / 96_000.0)),
    ]
    .into_iter()
    .enumerate()
    {
        theme::apply_preferences(if index % 2 == 0 {
            hex_preferences::ThemePreferences::default()
        } else {
            hex_preferences::ThemePreferences {
                background: hex_preferences::RgbColor::new(240, 240, 240),
                text: hex_preferences::RgbColor::new(16, 16, 16),
                primary: hex_preferences::RgbColor::new(166, 65, 47),
                secondary: hex_preferences::RgbColor::new(40, 88, 170),
            }
        });
        editor.update(&mut visual, |v, cx| {
            v.audio_event_editor.cursor = 40_000;
            v.audio_event_editor.viewport = viewport;
            cx.notify();
        });
        visual.run_until_parked();
        let marker = visual.debug_bounds("audio-play-start-marker");
        if let Some(fraction) = expected {
            let body = visual.debug_bounds("audio-events-waveform").unwrap();
            let marker = marker.expect("visible playback-source cursor");
            assert!((marker.left() - (body.left() + body.size.width * fraction)).abs() < px(1.0));
            assert_eq!(
                visual.rendered_background("audio-play-start-marker"),
                Some(gpui::Hsla::from(gpui::rgb(theme::color(theme::PaletteRole::Text))).into())
            );
            assert_eq!(marker.size.width, px(1.0));
            assert_eq!(marker.size.height, body.size.height);
            assert!(
                !visual
                    .rendered_quad_paint_layers("audio-play-start-marker", false)
                    .is_empty()
            );
        } else {
            assert!(marker.is_none());
        }
        editor.read_with(&visual, |v, _| {
            assert_eq!(*v.state.as_ref().unwrap().project, *original.0);
            assert_eq!(v.selected_clip, original.1);
            assert!(v.last_enqueued_edit.is_none());
        });
    }
}

#[gpui::test]
fn event_grid_paints_over_opaque_audio_at_full_and_zoomed_extents(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for viewport in [
        None,
        Some(Viewport {
            start: 12_000,
            length: 48_000,
        }),
    ] {
        editor.update(&mut visual, |v, cx| {
            v.audio_event_editor.grid = SnapGrid::Quarter;
            v.audio_event_editor.viewport = viewport;
            cx.notify();
        });
        visual.run_until_parked();
        let body = visual.debug_bounds("audio-events-waveform").unwrap();
        let grid = visual
            .debug_bounds("audio-event-musical-grid")
            .expect("visible event grid");
        assert!(grid.top() >= body.top() + px(22.0));
        let frame = editor.read_with(&visual, |v, _| {
            let root = v.event_context().unwrap().1;
            let viewport = v
                .event_project_window(&root)
                .unwrap()
                .visible_unsigned()
                .unwrap();
            super::super::musical_time::grid_lines(
                &v.state.as_ref().unwrap().project,
                viewport,
                f32::from(body.size.width),
                SnapGrid::Quarter,
            )
            .into_iter()
            .find(|l| l.frame > viewport.start && l.frame < viewport.end().unwrap())
            .unwrap()
            .frame
        });
        let selector = Box::leak(format!("clip-grid-line-{frame}").into_boxed_str());
        let line = visual.debug_bounds(selector).unwrap();
        assert!(line.top() >= grid.top() && line.bottom() <= grid.bottom());
        let lines = visual.rendered_quad_paint_layers(selector, false);
        let events = visual.rendered_quad_paint_layers("audio-event-1", false);
        assert!(!lines.is_empty() && !events.is_empty());
        assert!(
            lines.iter().map(|x| x.0).min().unwrap() > events.iter().map(|x| x.0).max().unwrap(),
            "grid must remain visible over the opaque event fill"
        );
    }
}

#[gpui::test]
fn event_pointer_matches_arrangement_and_retains_move_and_trim_targets(cx: &mut TestAppContext) {
    let (handle, _, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let b = visual.debug_bounds("audio-events-waveform").unwrap();
    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    for (position, modifiers, expected) in [
        (
            point(b.center().x, b.bottom() - px(15.0)),
            Modifiers::default(),
            gpui::CursorStyle::Arrow,
        ),
        (
            point(b.center().x, b.top() + px(26.0)),
            Modifiers::default(),
            gpui::CursorStyle::OpenHand,
        ),
        (
            point(b.center().x, b.top() + px(26.0)),
            option,
            gpui::CursorStyle::ResizeUpDown,
        ),
        (
            point(b.left() + px(2.0), b.center().y),
            Modifiers::default(),
            gpui::CursorStyle::ResizeLeftBracket,
        ),
        (
            point(b.right() - px(2.0), b.center().y),
            Modifiers::default(),
            gpui::CursorStyle::ResizeRightBracket,
        ),
    ] {
        visual.simulate_modifiers_change(modifiers);
        visual.simulate_mouse_move(position, None, modifiers);
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(expected));
    }
}

#[gpui::test]
fn x_deletes_the_focused_audio_event_range(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let down = point(
        bounds.origin.x + bounds.size.width * 0.25,
        bounds.origin.y + px(10.0),
    );
    let up = point(bounds.origin.x + bounds.size.width * 0.75, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    let range = editor.read_with(&visual, |view, _| view.audio_event_editor.range.unwrap());
    editor.update(&mut visual, |view, _| view.close_prompt_open = true);
    visual.simulate_keystrokes("x");
    editor.read_with(&visual, |view, _| {
        assert!(view.last_enqueued_edit.is_none())
    });
    editor.update(&mut visual, |view, _| view.close_prompt_open = false);
    visual.simulate_keystrokes("x");
    editor.read_with(&visual, |view, _| assert!(matches!(view.last_enqueued_edit, Some(Command::EditAudioEvents { clip: id, operation: AudioEventEdit::Delete { start, end } }) if id == clip && (start, end) == range)));
}

#[gpui::test]
fn event_ruler_selection_grid_and_shortcuts_stay_inside_clip_editor(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual
        .debug_bounds("audio-events-waveform")
        .expect("event canvas");
    assert!(bounds.size.width > px(100.0) && bounds.size.height > px(40.0));
    let down = point(
        bounds.origin.x + bounds.size.width * 0.25,
        bounds.origin.y + px(10.0),
    );
    let up = point(bounds.origin.x + bounds.size.width * 0.75, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("audio-event-time-selection").is_some());
    let cursor = editor.read_with(cx, |view, _| {
        let range = view.audio_event_editor.range.expect("selected time");
        assert!(range.0 < range.1);
        assert_eq!(view.audio_event_editor.grid, SnapGrid::Adaptive);
        view.audio_event_editor.cursor
    });
    visual.simulate_keystrokes("cmd-4");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.audio_event_editor.grid, SnapGrid::Off);
        assert_eq!(view.snap_grid, SnapGrid::Adaptive);
    });
    visual.simulate_keystrokes("e");
    editor.read_with(cx,|view,_|assert!(matches!(view.last_enqueued_edit,Some(Command::EditAudioEvents{clip:id,operation:AudioEventEdit::SplitRange{start:_,end}}) if id==clip && end==cursor)));
}

#[gpui::test]
fn g_extracts_a_selected_audio_event_span_as_one_clip(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (_files, handle, editor) = super::super::clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    let clip = editor.update(&mut visual, |view, cx| {
        let track = &view.state.as_ref().unwrap().project.tracks()[0];
        let clip = track.clips()[0].id();
        view.selected_track = Some(track.id());
        view.selected_clip = Some(clip);
        view.clip_view_open = true;
        view.device_panel_active = false;
        cx.notify();
        clip
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("audio-events-waveform").is_some());
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Edit(
            Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 48_000 },
            },
        )));
    });
    super::super::drag::group_drag_ui_test::pump(&editor, &mut visual);
    visual.run_until_parked();
    let bounds = visual
        .debug_bounds("audio-events-waveform")
        .expect("event canvas");
    let down = point(
        bounds.origin.x + bounds.size.width * 0.2,
        bounds.origin.y + px(10.0),
    );
    let up = point(
        bounds.origin.x + bounds.size.width * 0.8,
        bounds.origin.y + px(10.0),
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let before = visual.update(|window, cx| {
        editor.read_with(cx, |view, _| {
            assert!(view.audio_event_focus.is_focused(window));
            let range = view.audio_event_editor.range.expect("event range");
            assert!(range.0 > 0 && range.1 < 96_000);
            view.state.as_ref().unwrap().project.snapshot()
        })
    });
    visual.simulate_keystrokes("g");
    super::super::drag::group_drag_ui_test::pump(&editor, &mut visual);
    let (_grouped, after) = editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        let grouped = view.selected_clip.expect("created event span selected");
        let item = project.clip(grouped).expect("created clip exists");
        assert!(item.start() > 0 && item.start() < 96_000);
        assert!(item.duration() < 96_000);
        assert!(item.editable_audio_events().unwrap().events.len() >= 2);
        assert_eq!(project.tracks()[0].clips().len(), 3);
        (grouped, project.snapshot())
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Undo));
    });
    super::super::drag::group_drag_ui_test::pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let restored = view.state.as_ref().unwrap().project.snapshot();
        let mut expected = before;
        expected.next_id = restored.next_id;
        assert_eq!(restored, expected);
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Redo));
    });
    super::super::drag::group_drag_ui_test::pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after);
    });
}

#[gpui::test]
fn grouped_audio_hides_single_source_view_and_anchor_waveform_fallback(cx: &mut TestAppContext) {
    use super::super::clip_drag_ui_test::upper_rail_owned_fixture;
    use super::super::drag::group_drag_ui_test::pump;
    use hex_session::service::Request;

    cx.update(super::super::bind_editor_keys);
    let (_files, handle, editor) = upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();

    let (clip, generation) = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        (
            state.project.tracks()[0].clips()[0].id(),
            view.project_generation,
        )
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::DuplicateClip(clip))));
    });
    pump(&editor, &mut visual);
    let clips = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0]
            .clips()
            .iter()
            .map(|clip| clip.id())
            .collect::<Vec<_>>()
    });
    let revision = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::GroupClips {
            project_generation: generation,
            revision,
            clips,
        }));
    });
    pump(&editor, &mut visual);

    editor.update(&mut visual, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let grouped = state.project.clip(view.selected_clip.unwrap()).unwrap();
        assert!(!grouped.audio_events().sources.is_empty());
        assert!(state.waveforms.contains_key(&grouped.asset()));
        view.audio_event_editor.source_view = true;
        // Exercise the no-prepared-composite case while retaining the original
        // anchor waveform that used to be incorrectly shown for every event.
        let mut audio_clip_waveforms = state.audio_clip_waveforms.clone();
        audio_clip_waveforms.remove(&grouped.id());
        let changed = hex_session::service::ViewState {
            canonical_audio_source: state.canonical_audio_source.clone(),
            comparison_audio: state.comparison_audio.clone(),
            project: std::sync::Arc::clone(&state.project),
            waveforms: state.waveforms.clone(),
            source_overviews: state.source_overviews.clone(),
            audio_clip_waveforms,
            midi_preview: std::sync::Arc::clone(&state.midi_preview),
            gain_preview: state.gain_preview.clone(),
            timeline_frames: state.timeline_frames,
            can_undo: state.can_undo,
            can_redo: state.can_redo,
            revision: state.revision,
            unsaved: state.unsaved,
            document_path: state.document_path.clone(),
        };
        view.state = Some(std::sync::Arc::new(changed));
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("audio-source-tab-unavailable")
            .is_some()
    );
    assert!(visual.debug_bounds("audio-events-waveform").is_some());
    assert!(visual.debug_bounds("audio-event-waveform-1").is_none());
    editor.read_with(&visual, |view, _| {
        assert!(!view.audio_event_editor.source_view)
    });
}

#[gpui::test]
fn event_drag_preview_cancel_and_revision_change_never_edit_outer_clip(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let down = point(
        bounds.origin.x + bounds.size.width * 0.3,
        bounds.top() + px(26.0),
    );
    let moved = point(down.x + bounds.size.width * 0.2, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.audio_event_editor.gesture.is_some())
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let changed = hex_session::service::ViewState {
            canonical_audio_source: state.canonical_audio_source.clone(),
            comparison_audio: None,
            project: state.project.clone(),
            waveforms: state.waveforms.clone(),
            source_overviews: state.source_overviews.clone(),
            audio_clip_waveforms: state.audio_clip_waveforms.clone(),
            midi_preview: state.midi_preview.clone(),
            gain_preview: state.gain_preview.clone(),
            timeline_frames: state.timeline_frames,
            can_undo: state.can_undo,
            can_redo: state.can_redo,
            revision: state.revision + 1,
            unsaved: state.unsaved,
            document_path: state.document_path.clone(),
        };
        view.state = Some(std::sync::Arc::new(changed));
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
}

#[gpui::test]
fn event_move_preview_and_release_use_snapped_inner_coordinates(cx: &mut TestAppContext) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let mut canonical = hex_project::Editor::new((*state.project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 24000 },
            })
            .unwrap();
        let next = hex_session::service::ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: std::sync::Arc::new(canonical.project().clone()),
            waveforms: state.waveforms.clone(),
            source_overviews: state.source_overviews.clone(),
            audio_clip_waveforms: Default::default(),
            midi_preview: state.midi_preview.clone(),
            gain_preview: state.gain_preview.clone(),
            timeline_frames: state.timeline_frames,
            can_undo: true,
            can_redo: false,
            revision: 2,
            unsaved: true,
            document_path: None,
        };
        // TestPlatform has no worker/audio output; real input still goes through
        // the mounted editor and its normal command queue boundary.
        view.state = Some(std::sync::Arc::new(next));
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let original = visual.debug_bounds("audio-event-1").unwrap();
    let down = point(
        bounds.origin.x + bounds.size.width * 0.125,
        bounds.top() + px(31.0),
    );
    let up = point(bounds.origin.x + bounds.size.width * 0.5, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let preview = visual.debug_bounds("audio-event-1").unwrap();
    assert!(preview.origin.x > original.origin.x + px(20.0));
    editor.read_with(cx, |view, _| {
        assert!(
            view.last_enqueued_edit.is_none(),
            "motion is a local preview"
        )
    });
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.last_enqueued_edit,
            Some(Command::EditAudioEvents { clip: id, operation: AudioEventEdit::Place { event: 1, start: 36000 } }) if id == clip));
    });
}

#[gpui::test]
fn selecting_an_off_grid_event_without_dragging_never_moves_audio(cx: &mut TestAppContext) {
    let (handle, editor, track, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let mut canonical = hex_project::Editor::new((*state.project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 24000 },
            })
            .unwrap();
        canonical
            .execute(Command::MoveClip {
                clip,
                track,
                start: 17000,
            })
            .unwrap();
        view.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: std::sync::Arc::new(canonical.project().clone()),
            waveforms: state.waveforms.clone(),
            source_overviews: state.source_overviews.clone(),
            audio_clip_waveforms: Default::default(),
            midi_preview: state.midi_preview.clone(),
            gain_preview: state.gain_preview.clone(),
            timeline_frames: state.timeline_frames,
            can_undo: true,
            can_redo: false,
            revision: 2,
            unsaved: true,
            document_path: None,
        }));
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let click = point(
        bounds.origin.x + bounds.size.width * 0.125,
        bounds.top() + px(26.0),
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            view.last_enqueued_edit.is_none(),
            "selection alone must not snap the event's off-grid start"
        )
    });
}

#[gpui::test]
fn copying_after_an_outer_cut_uses_only_the_visible_inner_event(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let mut canonical = hex_project::Editor::new((*state.project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 24000 },
            })
            .unwrap();
        let hex_project::Created::Clip(right) = canonical
            .execute(Command::SplitClip { clip, frame: 48000 })
            .unwrap()
        else {
            panic!("fixture")
        };
        view.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: std::sync::Arc::new(canonical.project().clone()),
            waveforms: state.waveforms.clone(),
            source_overviews: state.source_overviews.clone(),
            audio_clip_waveforms: Default::default(),
            midi_preview: state.midi_preview.clone(),
            gain_preview: state.gain_preview.clone(),
            timeline_frames: state.timeline_frames,
            can_undo: true,
            can_redo: false,
            revision: 2,
            unsaved: true,
            document_path: None,
        }));
        view.selected_clip = Some(right);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let click = point(
        bounds.origin.x + bounds.size.width * 0.25,
        bounds.top() + px(26.0),
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-c");
    editor.read_with(cx, |view, _| {
        let clipboard = view.audio_event_editor.clipboard.as_ref().unwrap();
        assert_eq!(clipboard.length, 48000);
        assert_eq!(clipboard.events[0].source_start, 48000);
        assert!(view.last_enqueued_edit.is_none());
    });
}

#[gpui::test]
fn closing_events_releases_focus_and_gesture(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body = visual.debug_bounds("audio-events-waveform").unwrap();
    let click = point(body.center().x, body.center().y + px(12.0));
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    let toggle = visual.debug_bounds("clip-view-toggle").unwrap().center();
    visual.simulate_mouse_down(toggle, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        editor.read_with(cx, |view, _| {
            assert!(!view.clip_view_open);
            assert!(
                !view.audio_event_focus.is_focused(window),
                "closed events cannot retain shortcut focus"
            );
            assert!(view.focus.is_focused(window));
            assert!(view.audio_event_editor.gesture.is_none());
        })
    })
    .unwrap();
}

#[gpui::test]
fn cropped_legacy_range_moves_stay_bounded_and_free_event_nudge_uses_canonical_placement(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let mut canonical = hex_project::Editor::new((*state.project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 48000 },
            })
            .unwrap();
        let hex_project::Created::Clip(middle) = canonical
            .execute(Command::SplitClip { clip, frame: 24000 })
            .unwrap()
        else {
            panic!("split")
        };
        canonical
            .execute(Command::SplitClip {
                clip: middle,
                frame: 72000,
            })
            .unwrap();
        view.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: std::sync::Arc::new(canonical.project().clone()),
            waveforms: state.waveforms.clone(),
            source_overviews: state.source_overviews.clone(),
            audio_clip_waveforms: Default::default(),
            midi_preview: state.midi_preview.clone(),
            gain_preview: state.gain_preview.clone(),
            timeline_frames: state.timeline_frames,
            can_undo: true,
            can_redo: false,
            revision: 2,
            unsaved: true,
            document_path: None,
        }));
        view.selected_clip = Some(middle);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    editor.update(cx, |view, _| {
        view.audio_event_editor.grid = SnapGrid::Off;
        let (_, root, _) = view.event_context().unwrap();
        let events = view
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(view.selected_clip.unwrap())
            .unwrap()
            .editable_audio_events()
            .unwrap();
        let first = *events
            .events
            .iter()
            .find(|e| e.start == root.view_start)
            .unwrap();
        let last = *events
            .events
            .iter()
            .find(|e| e.end == root.view_end)
            .unwrap();
        let left = f32::from(bounds.origin.x);
        let width = f32::from(bounds.size.width);
        for (kind, anchor, origin, x) in [
            (Kind::Move(first), 30000, left + width * 0.125, left),
            (Kind::Move(last), 60000, left + width * 0.75, left + width),
            (
                Kind::RangeMove {
                    start: root.view_start,
                    end: root.view_end,
                },
                48000,
                left + width * 0.5,
                left,
            ),
        ] {
            view.audio_event_editor.gesture = Some(Gesture {
                anchor,
                pointer_x: origin,
                origin_x: origin,
                moved: false,
                kind,
                placement: None,
                operation: None,
            });
            view.event_update_pointer(x, false);
            let operation = view
                .audio_event_editor
                .gesture
                .as_ref()
                .unwrap()
                .operation
                .as_ref()
                .unwrap();
            let (start, span) = match operation {
                AudioEventEdit::Move { event, start } => (
                    *start,
                    events
                        .events
                        .iter()
                        .find(|e| e.id == *event)
                        .unwrap()
                        .length(),
                ),
                AudioEventEdit::MoveRange {
                    start,
                    end,
                    destination,
                } => (*destination, end - start),
                _ => panic!("move"),
            };
            assert!(
                start >= root.view_start && start + span <= root.view_end,
                "move escaped visible crop: {operation:?}"
            );
        }
        view.audio_event_editor.gesture = None;
        view.audio_event_editor.selected = Some(last.id);
        view.event_nudge(true, false, false);
        assert!(
            matches!(
                view.last_enqueued_edit,
                Some(Command::EditAudioEvents {
                    operation: AudioEventEdit::Place { start: 48001, .. },
                    ..
                })
            ),
            "a proved fixed-frame nudge may move beyond the cropped clip window: {:?}",
            view.last_enqueued_edit
        );
    });
}

#[test]
fn moved_and_pasted_waveform_positions_read_event_composed_pcm() {
    use std::{collections::BTreeMap, sync::Arc};
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
    let hex_project::Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "wave".into(),
            path: "wave.wav".into(),
            sample_rate: 48000,
            frames: 100,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 20 },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Move {
                event: 1,
                start: 40,
            },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Paste {
                at: 0,
                length: 10,
                events: vec![AudioEvent {
                    active: true,
                    gain: hex_project::ClipGain::UNITY,
                    source: 0,
                    id: 0,
                    start: 0,
                    end: 10,
                    source_start: 70,
                    fade: None,
                }],
            },
        })
        .unwrap();
    let frames: Vec<_> = (0..100).map(|i| [i as f32 / 100.0; 2]).collect();
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48000, frames.clone()).unwrap(),
    )]);
    let prepared = hex_engine::PreparedRevision::prepare_with_audio_transforms(
        editor.project(),
        &media,
        Arc::new(hex_engine::SineTable::new()),
        None,
        &hex_engine::audio_transform::AudioTransformPreparation::default(),
    )
    .unwrap();
    let pcm = prepared.arrangement().audio_clip_views()[0].audio.frames();
    let events = editor.project().clip(clip).unwrap().audio_events();
    for event in &events.events {
        let position = view::prepared_waveform_position(*event, *event, event.start).unwrap();
        assert_eq!(pcm[position as usize], frames[event.source_start as usize]);
    }
    let original = *events.events.iter().find(|e| e.id == 1).unwrap();
    let preview = AudioEvent {
        start: 60,
        end: 80,
        ..original
    };
    let position = view::prepared_waveform_position(preview, original, preview.start).unwrap();
    assert_eq!(position, 40);
    assert_eq!(pcm[position as usize], frames[0]);
    assert_ne!(
        pcm[original.source_start as usize], frames[0],
        "reading raw source_start in composed PCM paints pasted content instead"
    );
}

fn replace_event_project(view: &mut EditorView, project: hex_project::Project, revision: u64) {
    let state = view.state.as_ref().unwrap();
    view.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        project: std::sync::Arc::new(project),
        waveforms: state.waveforms.clone(),
        source_overviews: state.source_overviews.clone(),
        audio_clip_waveforms: Default::default(),
        midi_preview: state.midi_preview.clone(),
        gain_preview: state.gain_preview.clone(),
        timeline_frames: state.timeline_frames,
        can_undo: true,
        can_redo: false,
        revision,
        unsaved: true,
        document_path: None,
    }));
}

#[gpui::test]
fn outer_trim_clamps_stale_cursor_range_and_hidden_selection(cx: &mut TestAppContext) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let mut canonical =
            hex_project::Editor::new((*view.state.as_ref().unwrap().project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 24000 },
            })
            .unwrap();
        replace_event_project(view, canonical.project().clone(), 2);
        cx.notify();
    });
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.audio_event_editor.range = Some((0, 24000));
        view.audio_event_editor.cursor = 0;
        view.audio_event_editor.selected = Some(1);
        view.event_copy(false);
        let mut canonical =
            hex_project::Editor::new((*view.state.as_ref().unwrap().project).clone(), 8).unwrap();
        let frame = canonical.project().clip(clip).unwrap().start() + 48000;
        canonical
            .execute(Command::TrimClipEdge {
                clip,
                edge: hex_project::TrimEdge::Left,
                frame,
            })
            .unwrap();
        replace_event_project(view, canonical.project().clone(), 3);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        let (_, root, _) = view.event_context().unwrap();
        assert_eq!(root.view_start, 48000);
        assert_eq!(
            view.audio_event_editor.cursor, root.view_start,
            "cursor must follow crop bounds"
        );
        assert!(
            view.audio_event_editor.range.is_none(),
            "hidden time range must be cleared"
        );
        assert!(
            view.audio_event_editor.selected.is_none(),
            "hidden event must be deselected"
        );
        view.event_delete();
        assert!(
            view.last_enqueued_edit.is_none(),
            "delete must not edit an old hidden selection"
        );
        view.event_paste(false);
        assert!(
            matches!(
                view.last_enqueued_edit,
                Some(Command::EditAudioEvents {
                    operation: AudioEventEdit::Paste { at: 48000, .. },
                    ..
                })
            ),
            "paste must use visible cursor"
        );
    });
}

#[gpui::test]
fn duplicate_and_clipboard_paste_refuse_beyond_visible_child(cx: &mut TestAppContext) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let mut canonical =
            hex_project::Editor::new((*view.state.as_ref().unwrap().project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 24000 },
            })
            .unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Delete {
                    start: 60000,
                    end: 96000,
                },
            })
            .unwrap();
        canonical
            .execute(Command::SplitClip { clip, frame: 48000 })
            .unwrap();
        replace_event_project(view, canonical.project().clone(), 2);
        cx.notify();
    });
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        view.audio_event_editor.range = Some((24000, 48000));
        view.event_copy(false);
        view.event_duplicate();
        assert!(
            view.last_enqueued_edit.is_none(),
            "duplicate at crop end must not overwrite hidden audio"
        );
        assert!(view.error.is_some(), "refusal must be visible");
        view.audio_event_editor.cursor = 48000;
        for insert in [false, true] {
            view.error = None;
            view.event_paste(insert);
            assert!(
                view.last_enqueued_edit.is_none(),
                "clipboard cannot write past crop"
            );
            assert!(view.error.is_some());
        }
        view.audio_event_editor.cursor = 0;
        view.error = None;
        view.event_paste(true);
        assert!(
            view.last_enqueued_edit.is_none(),
            "insert must not shift a tail into hidden root space"
        );
        assert!(view.error.is_some());
        view.audio_event_editor.cursor = 24000;
        view.event_paste(false);
        assert!(
            matches!(
                view.last_enqueued_edit,
                Some(Command::EditAudioEvents {
                    operation: AudioEventEdit::Paste {
                        at: 24000,
                        length: 24000,
                        ..
                    },
                    ..
                })
            ),
            "a fitting visible paste must still work"
        );
    });
}

#[gpui::test]
fn audio_fit_selection_and_back_use_local_waveform_bounds_without_model_history(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let b = visual.debug_bounds("audio-events-waveform").unwrap();
    let at = |f| point(b.origin.x + b.size.width * f, b.origin.y + px(10.0));
    visual.simulate_mouse_down(at(0.75), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(at(0.25), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at(0.25), MouseButton::Left, Modifiers::default());
    let (before, expected, prior) = editor.update(&mut visual, |v, cx| {
        let (_, root, _) = v.event_context().unwrap();
        let state = v.state.as_ref().unwrap();
        let first = root
            .root_frame_at(root.view_start, state.project.tempo_map())
            .unwrap();
        let (a, b) = v.audio_event_editor.range.unwrap();
        let a = root
            .root_frame_at(a.min(b), state.project.tempo_map())
            .unwrap();
        let b = root
            .root_frame_at(
                v.audio_event_editor
                    .range
                    .unwrap()
                    .0
                    .max(v.audio_event_editor.range.unwrap().1),
                state.project.tempo_map(),
            )
            .unwrap();
        let expected = Viewport {
            start: a - first,
            length: b - a,
        };
        let prior = Viewport {
            start: 1_000,
            length: 12_000,
        };
        v.audio_event_editor.viewport = Some(prior);
        cx.notify();
        (
            (
                state.project.snapshot(),
                state.can_undo,
                state.can_redo,
                v.offline_cursor,
            ),
            expected,
            prior,
        )
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("z");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.audio_event_editor.viewport, Some(expected))
    });
    visual.simulate_keystrokes("z");
    let back = visual.debug_bounds("audio-zoom-back").unwrap();
    visual.simulate_click(back.center(), Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.audio_event_editor.viewport, Some(prior))
    });
    editor.update(&mut visual, |v, cx| {
        v.audio_event_editor.range = None;
        cx.notify();
    });
    visual.run_until_parked();
    let fit = visual.debug_bounds("audio-zoom-fit").unwrap();
    visual.simulate_click(fit.center(), Modifiers::default());
    editor.read_with(&visual, |v, _| {
        let (_, root, _) = v.event_context().unwrap();
        assert_eq!(v.event_viewport(&root).start, 0);
        let s = v.state.as_ref().unwrap();
        assert_eq!(
            (
                s.project.snapshot(),
                s.can_undo,
                s.can_redo,
                v.offline_cursor
            ),
            before
        );
    });
    visual.simulate_keystrokes("v");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.audio_event_editor.viewport, Some(prior))
    });
    visual.simulate_click(fit.center(), Modifiers::default());
    editor.update(&mut visual, |v, cx| {
        v.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("v");
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.fit_back.is_none())
    });
    editor.update(&mut visual, |v, cx| {
        v.audio_event_editor.viewport = None;
        v.audio_event_editor.range = Some((12_000, 36_000));
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("z v");
    editor.read_with(&visual, |v, _| {
        assert!(
            v.audio_event_editor.viewport.is_none(),
            "back preserves automatic full-view mode"
        )
    });
}

#[gpui::test]
fn loop_brace_matches_retained_source_coordinates_during_zoom_and_pan(cx: &mut TestAppContext) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, cx| {
        let mut model =
            hex_project::Editor::new(v.state.as_ref().unwrap().project.as_ref().clone(), 1)
                .unwrap();
        model
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: hex_project::AudioViewResizeOperation::CycleRange {
                    start: 24000,
                    end: 48000,
                    looping: true,
                },
            })
            .unwrap();
        let prior = v.state.as_ref().unwrap();
        v.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
            project: std::sync::Arc::new(model.project().clone()),
            canonical_audio_source: prior.canonical_audio_source.clone(),
            comparison_audio: prior.comparison_audio.clone(),
            waveforms: prior.waveforms.clone(),
            source_overviews: prior.source_overviews.clone(),
            audio_clip_waveforms: prior.audio_clip_waveforms.clone(),
            midi_preview: prior.midi_preview.clone(),
            gain_preview: prior.gain_preview.clone(),
            timeline_frames: prior.timeline_frames,
            can_undo: prior.can_undo,
            can_redo: prior.can_redo,
            revision: prior.revision,
            unsaved: prior.unsaved,
            document_path: prior.document_path.clone(),
        }));
        cx.notify();
    });
    for (viewport, first, last) in [
        (None, 0.25, 0.5),
        (
            Some(Viewport {
                start: 0,
                length: 48000,
            }),
            0.5,
            1.0,
        ),
        (
            Some(Viewport {
                start: 24000,
                length: 48000,
            }),
            0.0,
            0.5,
        ),
    ] {
        editor.update(&mut visual, |v, cx| {
            v.audio_event_editor.viewport = viewport;
            cx.notify();
        });
        visual.run_until_parked();
        let body = visual.debug_bounds("audio-events-waveform").unwrap();
        let bar = visual.debug_bounds("audio-loop-ruler").unwrap();
        assert_eq!((body.left(), body.size.width), (bar.left(), bar.size.width));
        let a = visual.debug_bounds("audio-loop-start-marker").unwrap();
        let b = visual.debug_bounds("audio-loop-end-marker").unwrap();
        // Layout rounds the independently positioned edges to physical pixels.
        let pixel = 1.0 / visual.update(|window, _| window.scale_factor());
        assert!(
            (f32::from(a.left() - bar.left()) - f32::from(bar.size.width) * first).abs() <= pixel,
            "bar={bar:?} a={a:?} first={first}"
        );
        assert!(
            (f32::from(b.right() - bar.left()) - f32::from(bar.size.width) * last).abs() <= pixel,
            "bar={bar:?} b={b:?} last={last}"
        );
    }
}

#[gpui::test]
fn navigation_reveals_blank_space_and_fit_back_preserves_extended_view(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("audio-event-1").is_some());
    editor.update(&mut visual, |v, cx| {
        let before = v.state.as_ref().unwrap().project.snapshot();
        let (_, root, _) = v.event_context().unwrap();
        let fitted = v.event_viewport(&root);
        v.event_zoom(false);
        assert!(
            v.event_viewport(&root).length > fitted.length,
            "zoom out must reveal blank time"
        );
        v.audio_event_editor.viewport = None;
        for _ in 0..6 {
            v.event_pan(true);
        }
        let extended = v.event_viewport(&root);
        assert!(extended.start >= fitted.length, "pan must reach blank time");
        assert!(v.event_fit_selection(false));
        assert_eq!(v.event_viewport(&root), fitted);
        assert!(v.event_fit_selection(true));
        assert_eq!(v.event_viewport(&root), extended);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(v.last_enqueued_edit.is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("audio-event-1").is_none());
}

#[gpui::test]
fn wheel_navigation_anchors_pointer_and_keeps_tabs_independent(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent};
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let b = visual.debug_bounds("audio-events-waveform").unwrap();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let at = point(b.left() + b.size.width * 0.75, b.center().y);
    visual.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let events = editor.read_with(&visual, |v, _| {
        let (_, root, _) = v.event_context().unwrap();
        let vp = v.event_viewport(&root);
        assert_eq!(vp.length, 76_800);
        assert!((vp.start as f64 + 0.75 * vp.length as f64 - 72_000.0).abs() <= 1.0);
        vp
    });
    let tab = visual.debug_bounds("audio-source-tab").unwrap();
    visual.simulate_click(tab.center(), Modifiers::default());
    visual.run_until_parked();
    let b = visual.debug_bounds("arrangement-audio-waveform").unwrap();
    visual.simulate_event(ScrollWheelEvent {
        position: b.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let body = visual.debug_bounds("arrangement-audio-waveform").unwrap();
    let marker = visual.debug_bounds("source-end-marker").unwrap();
    let painted = visual
        .debug_bounds("audio-source-visible-waveform")
        .unwrap();
    assert!((marker.left() - (body.left() + body.size.width * 0.9)).abs() < px(2.0));
    assert!((painted.left() - (body.left() + body.size.width * 0.1)).abs() < px(2.0));
    assert!((painted.size.width - body.size.width * 0.8).abs() < px(2.0));
    editor.update(&mut visual, |v, cx| {
        assert_eq!(v.audio_source_viewport().length, 120_000);
        assert_eq!(v.audio_source_viewport().start, -12_000);
        assert_eq!(v.audio_event_editor.viewport, Some(events));
        v.event_pan(false);
        v.event_pan(false);
        assert_eq!(v.audio_source_viewport().start, -132_000);
        assert!(v.event_fit_selection(false));
        assert!(v.event_fit_selection(true));
        assert_eq!(v.audio_source_viewport().start, -132_000);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("audio-source-visible-waveform")
            .is_none()
    );
    assert!(visual.debug_bounds("source-start-marker").is_none());
    editor.update(&mut visual, |v, cx| {
        for _ in 0..12 {
            v.event_pan(true);
        }
        assert!(v.audio_source_viewport().start > 96_000 * 4);
        assert!(v.event_fit_selection(false));
        assert!(v.event_fit_selection(true));
        assert!(v.audio_source_viewport().start > 96_000 * 4);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(v.last_enqueued_edit.is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("source-start-marker").is_none());
    assert!(visual.debug_bounds("source-end-marker").is_none());
    assert!(visual.debug_bounds("play-start-marker").is_none());
}

#[gpui::test]
fn navigation_rejects_bad_wheel_and_held_input_and_resets_on_source_replacement(
    cx: &mut TestAppContext,
) {
    use gpui::{ScrollDelta, ScrollWheelEvent};
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let b = visual.debug_bounds("audio-events-waveform").unwrap();
    let at = point(b.left() + b.size.width * 0.75, b.top() + px(4.0));
    for delta in [f32::NAN, f32::INFINITY] {
        visual.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.0), px(delta))),
            ..Default::default()
        });
    }
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.viewport.is_none())
    });
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    assert!(editor.read_with(&visual, |v, _| v.audio_event_editor.gesture.is_some()));
    visual.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    editor.update(&mut visual, |v, _| {
        v.event_zoom(true);
        v.event_pan(true);
        assert!(!v.event_fit_selection(false));
        assert!(v.audio_event_editor.viewport.is_none());
        assert!(v.audio_event_editor.gesture.is_some());
    });
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    editor.update(&mut visual, |v, cx| {
        for _ in 0..20 {
            v.event_zoom(true);
        }
        let (_, root, _) = v.event_context().unwrap();
        assert_eq!(v.event_viewport(&root).length, 1);
        v.audio_event_editor.source_view = true;
        v.event_pan(true);
        assert!(v.audio_source_viewport().start > 0);
        v.project_generation += 1;
        v.sync_audio_navigation();
        assert!(v.audio_event_editor.source_view);
        assert_eq!(v.audio_source_viewport(), Viewport::fit(96_000).into());
        assert!(v.audio_event_editor.viewport.is_none());
        assert!(v.audio_event_editor.source_fit_back.is_none());
        assert!(v.last_enqueued_edit.is_none());
        cx.notify();
    });
    visual.run_until_parked();
}

#[gpui::test]
fn plain_event_top_edge_drag_moves_the_event_without_changing_gain(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        view.audio_event_editor.grid = SnapGrid::Off;
    });
    let down = visual.debug_bounds("audio-event-gain-1").unwrap().center();
    let up = point(down.x + px(80.0), down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(matches!(
            view.audio_event_editor
                .gesture
                .as_ref()
                .map(|gesture| gesture.kind),
            Some(Kind::Move(_) | Kind::FreeMove { .. })
        ));
    });
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        let pending = view
            .audio_event_editor
            .pending
            .as_ref()
            .expect("event move");
        assert!(
            matches!(
                pending.operation,
                AudioEventEdit::Place { event: 1, start } if start > 0
            ) || matches!(
                pending.operation,
                AudioEventEdit::Move { event: 1, start } if start > 0
            )
        );
        assert_eq!(pending.preview.events[0].gain, hex_project::ClipGain::UNITY);
    });
}

#[gpui::test]
fn event_gain_drag_keeps_preview_until_receipt(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let handle = visual
        .debug_bounds("audio-event-gain-1")
        .expect("event gain handle");
    let down = handle.center();
    let up = point(down.x, down.y + px(20.0));
    visual.simulate_mouse_down(
        down,
        MouseButton::Left,
        Modifiers {
            alt: true,
            ..Modifiers::default()
        },
    );
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    let fine = point(down.x, up.y + px(10.0));
    visual.simulate_mouse_move(
        fine,
        MouseButton::Left,
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.audio_event_editor.gesture.as_ref().and_then(|g| g.operation.as_ref()), Some(AudioEventEdit::SetGain { event: 1, gain }) if gain.millidb() == -2100));
    });
    visual.simulate_mouse_up(
        fine,
        MouseButton::Left,
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let pending = view.audio_event_editor.pending.as_ref().expect("pending event gain");
        assert!(matches!(pending.operation, AudioEventEdit::SetGain { event: 1, gain } if gain.millidb() == -2100));
        assert_eq!(pending.preview.events[0].gain.millidb(), -2100);
    });
}

#[gpui::test]
fn event_gain_escape_discards_held_edit(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let down = visual.debug_bounds("audio-event-gain-1").unwrap().center();
    let up = point(down.x, down.y + px(20.0));
    visual.simulate_mouse_down(
        down,
        MouseButton::Left,
        Modifiers {
            alt: true,
            ..Modifiers::default()
        },
    );
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.audio_event_editor.gesture.is_none());
        assert!(view.audio_event_editor.pending.is_none());
        assert!(view.last_enqueued_edit.is_none());
    });
}

#[gpui::test]
fn middle_drag_pans_audio_waveform_without_editing(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let start = bounds.center();
    let end = point(bounds.left() - px(20.0), start.y);
    let (before, viewport) = editor.read_with(&visual, |view, _| {
        (
            view.state.as_ref().unwrap().project.snapshot(),
            view.event_frame_window(&view.event_context().unwrap().1),
        )
    });
    visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
    assert!(editor.read_with(&visual, |view, _| view.audio_event_editor.pan.is_some()));
    visual.simulate_mouse_move(end, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.audio_event_editor.pan.is_none());
        assert!(
            view.event_frame_window(&view.event_context().unwrap().1)
                .start
                > viewport.start
        );
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(!view.edit_pending());
    });
}

#[gpui::test]
fn middle_drag_pans_source_and_releases_on_view_change(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    visual.simulate_click(
        visual.debug_bounds("audio-source-tab").unwrap().center(),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let bounds = visual.debug_bounds("arrangement-audio-waveform").unwrap();
    let start = bounds.center();
    let end = point(start.x - px(80.0), start.y);
    let before = editor.read_with(&visual, |view, _| {
        (
            view.audio_source_viewport(),
            view.state.as_ref().unwrap().project.snapshot(),
        )
    });
    visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.audio_event_editor.pan.is_some());
        assert!(view.audio_source_viewport().start > before.0.start);
    });
    editor.update(&mut visual, |view, cx| {
        view.clip_view_open = false;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.audio_event_editor.pan.is_none());
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before.1);
    });
}

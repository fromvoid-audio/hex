use super::*;
use gpui::{Modifiers, TestAppContext, VisualTestContext, point, size};
use hex_project::{Command, Created, Editor, Project};

pub(super) fn fixture(
    cx: &mut TestAppContext,
    frames: u64,
    stereo: bool,
) -> (gpui::AnyWindowHandle, Entity<EditorView>, TrackId, DeviceId) {
    let mut model = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = model
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Asset(asset) = model
        .execute(Command::ImportDeviceAsset {
            path: "original.wav".into(),
            sample_rate: 44_100,
            frames,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let device_state =
        hex_engine::native_device_resources::current_sampler_fixture::one_shot_state(
            frames as usize,
            asset,
        )
        .unwrap();
    let Created::Device(device) = model
        .execute(Command::InsertInstrument {
            track,
            state: device_state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    let facts = hex_project::OriginalSourceFacts::new(1, [7; 32]).unwrap();
    let original = hex_session::source_overview_test_support::Fixture::new(
        asset,
        &vec![[0.5, if stereo { -0.25 } else { 0.5 }]; frames as usize],
        44_100,
        facts,
        None,
    )
    .unwrap();
    let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        model.project().clone(),
        Arc::default(),
    );
    editor.update(cx, |view, cx| {
        let mut state = copy_fixture_state(view.state.as_ref().unwrap());
        state.source_overviews = original.view(hex_engine::AudioSourceContext {
            project_generation: view.project_generation,
            editor_revision: state.revision,
        });
        view.state = Some(Arc::new(state));
        view.selected_track = Some(track);
        view.device_panel.owner = Some(DevicePanelOwner::Track(track));
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    (handle, editor, track, device)
}

#[test]
fn sampler_cursor_original_frame_mapping_bounds_and_time() {
    for width in [1.0, 113.5, 1200.0] {
        assert_eq!(frame_at_x(0.0, width, 441), Some(0));
        assert_eq!(frame_at_x(width * 0.5, width, 441), Some(221));
        assert_eq!(frame_at_x(width, width, 441), Some(441));
        assert_eq!(frame_at_x(width * 0.5, width, 1), Some(1));
        assert_eq!(frame_at_x(width, width, u64::MAX), Some(u64::MAX));
    }
    for (x, width, frames) in [
        (f32::NAN, 1.0, 1),
        (0.0, f32::INFINITY, 1),
        (0.0, 0.0, 1),
        (-1.0, 10.0, 1),
        (11.0, 10.0, 1),
        (0.0, 1.0, 0),
    ] {
        assert_eq!(frame_at_x(x, width, frames), None);
    }
    assert_eq!(
        cursor_label(22_050, 44_100),
        "cursor · 22050 frames · 0.500 s"
    );
}

#[gpui::test]
fn sampler_cursor_clicks_original_frames_without_project_or_audio_edits(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = fixture(cx, 44_100, true);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    for width in [1600.0, 900.0] {
        visual.simulate_resize(size(px(width), px(700.0)));
        visual.run_until_parked();
        for fraction in [0.0, 0.25, 0.5, 0.98] {
            let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
            let click = point(
                bounds.left() + bounds.size.width.min(px(width - 16.0) - bounds.left()) * fraction,
                bounds.top() + px(8.0),
            );
            visual.simulate_click(click, Modifiers::default());
            visual.run_until_parked();
            let frame = frame_at_x(
                f32::from(click.x - bounds.left()),
                f32::from(bounds.size.width),
                44_100,
            )
            .unwrap();
            let marker = visual.debug_bounds("sampler-source-cursor").unwrap();
            assert!(
                (f32::from(marker.left() - bounds.left())
                    - frame as f32 / 44_100.0 * f32::from(bounds.size.width))
                .abs()
                    < 1.1,
                "bounds={bounds:?}, marker={marker:?}, frame={frame}, fraction={fraction}"
            );
            assert!(
                visual
                    .rendered_background("sampler-source-cursor")
                    .is_some()
            );
            assert!(
                visual
                    .debug_bounds("sampler-source-cursor-readout")
                    .is_some()
            );
            editor.read_with(visual, |view, _| {
                assert_eq!(view.device_panel.sampler_cursor.cursor.unwrap().1, frame);
                let state = view.state.as_ref().unwrap();
                assert_eq!(state.project.snapshot(), before);
                assert!(view.pending_requests.is_empty());
                assert!(view.pending_audio_commands.is_empty());
                assert!(!state.can_undo && !state.can_redo);
            });
        }
    }
    // The ruler is a readout, not an extra source hit area.
    let before = editor.read_with(visual, |view, _| {
        view.device_panel.sampler_cursor.cursor.unwrap().1
    });
    let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
    visual.simulate_click(
        point(bounds.left(), bounds.bottom() - px(1.0)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert_eq!(view.device_panel.sampler_cursor.cursor.unwrap().1, before)
    });
}

#[gpui::test]
fn sampler_cursor_is_retired_on_source_owner_and_generation_changes(cx: &mut TestAppContext) {
    let (handle, editor, track, _device) = fixture(cx, 1, false);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
    visual.simulate_click(
        point(bounds.center().x, bounds.top() + px(8.0)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = editor.read_with(visual, |view, _| {
        view.device_panel.sampler_cursor.cursor.unwrap().0
    });
    assert_eq!(
        editor.read_with(visual, |view, _| view
            .device_panel
            .sampler_cursor
            .cursor
            .unwrap()
            .1),
        1
    );
    // Same-source parameter/history revisions keep a valid cursor; changed source
    // facts with the same asset id must retire it, including after Undo/Redo.
    let original_state =
        editor.read_with(visual, |view, _| Arc::clone(view.state.as_ref().unwrap()));
    for case in 0..5 {
        editor.update(visual, |view, cx| {
            view.device_panel.sampler_cursor.cursor = Some((target, 1));
            let mut state = copy_fixture_state(&original_state);
            let mut model = Editor::new((*state.project).clone(), 16).unwrap();
            match case {
                0 => view.project_generation += 1,
                1 => view.selected_track = None,
                2 => {
                    model.execute(Command::RemoveInstrument { track }).unwrap();
                }
                3 => {
                    state.source_overviews =
                        hex_session::source_overview_test_support::Fixture::new(
                            target.asset,
                            &[[0.1, 0.1]],
                            44_100,
                            hex_project::OriginalSourceFacts::new(1, [8; 32]).unwrap(),
                            None,
                        )
                        .unwrap()
                        .view(hex_engine::AudioSourceContext {
                            project_generation: view.project_generation,
                            editor_revision: state.revision,
                        });
                }
                _ => state.source_overviews = Default::default(),
            }
            state.project = Arc::new(model.project().clone());
            view.state = Some(Arc::new(state));
            reconcile_cursor(view);
            assert!(view.device_panel.sampler_cursor.cursor.is_none());
            // Restore the original fixture identity for the next independent case.
            view.project_generation = target.generation;
            view.selected_track = Some(track);
            view.state = Some(Arc::clone(&original_state));
            cx.notify();
        });
        // A discarded target cannot reattach just because its numeric ids recur.
        visual.run_until_parked();
        assert!(visual.debug_bounds("sampler-source-cursor").is_none());
    }
}

pub(super) fn copy_fixture_state(state: &ViewState) -> ViewState {
    ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        project: Arc::clone(&state.project),
        waveforms: Default::default(),
        source_overviews: state.source_overviews.clone(),
        audio_clip_waveforms: Default::default(),
        midi_preview: Arc::clone(&state.midi_preview),
        gain_preview: Ok(Arc::default()),
        timeline_frames: state.timeline_frames,
        can_undo: state.can_undo,
        can_redo: state.can_redo,
        revision: state.revision,
        unsaved: state.unsaved,
        document_path: None,
    }
}

#[gpui::test]
fn sampler_cursor_source_replacement_undo_and_stale_click(cx: &mut TestAppContext) {
    let (handle, editor, _, device) = fixture(cx, 441, false);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
    let hit = point(
        bounds.left() + bounds.size.width * 0.25,
        bounds.top() + px(8.0),
    );
    visual.simulate_click(hit, Modifiers::default());
    visual.run_until_parked();
    let original = editor.read_with(visual, |view, _| Arc::clone(view.state.as_ref().unwrap()));
    let mut model = Editor::new((*original.project).clone(), 16).unwrap();
    let Created::Asset(replacement) = model
        .execute(Command::ImportDeviceAsset {
            path: "replacement.wav".into(),
            sample_rate: 44_100,
            frames: 441,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    model
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_engine::native_device_resources::current_sampler_fixture::one_shot_state(
                441,
                replacement,
            )
            .unwrap(),
        })
        .unwrap();
    editor.update(visual, |view, _| {
        let mut state = copy_fixture_state(&original);
        state.project = Arc::new(model.project().clone());
        view.state = Some(Arc::new(state));
        // Deliberately no notify/repaint: dispatch against the old painted source.
    });
    visual.simulate_click(hit, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert!(view.device_panel.sampler_cursor.cursor.is_none());
        assert!(view.pending_requests.is_empty());
    });
    assert!(model.undo());
    editor.update(visual, |view, cx| {
        let mut state = copy_fixture_state(&original);
        state.project = Arc::new(model.project().clone());
        view.state = Some(Arc::new(state));
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("sampler-source-waveform").is_some());
    assert!(
        visual.debug_bounds("sampler-source-cursor").is_none(),
        "undo cannot resurrect a retired cursor"
    );
    let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
    visual.simulate_click(
        point(bounds.center().x, bounds.top() + px(8.0)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(visual.debug_bounds("sampler-source-cursor").is_some());
}

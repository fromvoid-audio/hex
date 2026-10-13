use super::clip_drag_ui_test::{audio_project, editor_window};
use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext, point};
use std::{path::PathBuf, sync::Arc};

#[gpui::test]
fn external_audio_drop_uses_regular_lane_and_gap_import_contracts(cx: &mut TestAppContext) {
    for gap in [false, true] {
        let (project, source, target, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let source_lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", source.value()).into_boxed_str(),
            ))
            .expect("source lane");
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", target.value()).into_boxed_str(),
            ))
            .expect("empty target lane");
        let pointer = point(
            lane.origin.x + lane.size.width * 0.5,
            if gap {
                source_lane.bottom_left().y
            } else {
                lane.origin.y + lane.size.height * 0.5
            },
        );
        visual.simulate_mouse_move(pointer, None, Modifiers::default());
        let path = PathBuf::from("/Original Case/Loop.WAV");
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.drop_external_audio_paths(std::slice::from_ref(&path), window, cx)
            });
        })
        .expect("native path adapter");
        editor.read_with(cx, |view, _| {
            if gap {
                let (retained, before, _) = view
                    .last_enqueued_import_to_new_track
                    .as_ref()
                    .expect("gap creates track");
                assert_eq!(retained, &path);
                assert_eq!(*before, Some(target));
                assert!(view.last_enqueued_import_to_track.is_none());
            } else {
                let (retained, track, start, overwrite) = view
                    .last_enqueued_import_to_track
                    .as_ref()
                    .expect("existing lane import");
                assert_eq!(retained, &path);
                assert_eq!(*track, target);
                assert!(*start > 0);
                assert!(*overwrite);
                assert!(view.last_enqueued_import_to_new_track.is_none());
            }
        });
    }
}

#[gpui::test]
fn external_audio_drop_refuses_main_stale_geometry_busy_and_multiple_paths(
    cx: &mut TestAppContext,
) {
    for refusal in 0..4 {
        let (project, _, target, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(if refusal == 0 {
                "main-track-lane"
            } else {
                Box::leak(format!("arrangement-lane-{}", target.value()).into_boxed_str())
            })
            .expect("drop lane");
        let pointer = point(
            lane.origin.x + lane.size.width * 0.5,
            lane.origin.y + lane.size.height * 0.5,
        );
        visual.simulate_mouse_move(pointer, None, Modifiers::default());
        let mut paths = vec![PathBuf::from("/Original Case/Loop.WAV")];
        if refusal == 3 {
            paths.push(PathBuf::from("/other.wav"));
        }
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                if refusal == 1 {
                    view.drag_layout_generation = view.drag_layout_generation.wrapping_add(1);
                }
                if refusal == 2 {
                    view.opening_project = true;
                }
                view.drop_external_audio_paths(&paths, window, cx);
                assert!(view.last_enqueued_import_to_track.is_none());
                assert!(view.last_enqueued_import_to_new_track.is_none());
                assert!(view.error.is_some());
                assert_eq!(
                    view.state.as_ref().expect("project").project.tracks().len(),
                    2
                );
            });
        })
        .expect("refused drop");
    }
}

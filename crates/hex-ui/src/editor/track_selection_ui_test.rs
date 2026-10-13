use super::clip_drag_ui_test::{audio_project, editor_window};
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use std::sync::Arc;

#[gpui::test]
fn track_headers_toggle_range_and_render_explicit_members_including_empty_tracks(
    cx: &mut TestAppContext,
) {
    let (project, source, empty, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let names = [source, empty]
        .map(|id| Box::leak(format!("track-name-{}", id.value()).into_boxed_str()) as &'static str);
    let first = visual.debug_bounds(names[0]).expect("source name").center();
    let last = visual.debug_bounds(names[1]).expect("empty name").center();
    visual.simulate_click(first, Modifiers::default());
    visual.simulate_click(
        last,
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_track_ids(), [source, empty])
    });
    for id in [source, empty] {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("selected-track-{}", id.value()).into_boxed_str()
                ))
                .is_some()
        );
    }
    visual.simulate_click(
        first,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert_eq!(view.selected_track_ids(), [empty]));
    visual.simulate_click(
        last,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.selected_track_ids().is_empty()));
    visual.simulate_click(first, Modifiers::default());
    editor.update(cx, |view, _| {
        view.time_selection_project_scope = true;
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: source,
            end_track: empty,
            anchor_frame: 0,
            end_frame: 48_000,
        });
        assert_eq!(
            view.selected_track_ids(),
            [source],
            "time-highlight tracks are not selected source members"
        );
    });
}

#[gpui::test]
fn explicit_track_members_survive_insertions_and_prune_removed_ids(cx: &mut TestAppContext) {
    let (project, source, empty, _) = audio_project();
    let (_handle, editor) = editor_window(cx, project.clone(), Arc::default());
    editor.update(cx, |view, _| {
        assert!(view.track_selection.select(&project, source, false, false));
        assert!(view.track_selection.select(&project, empty, false, true));
        let mut model = hex_project::Editor::new(project, 8).expect("history");
        let hex_project::Created::Track(new) = model
            .execute(hex_project::Command::AddTrack { name: "new".into() })
            .expect("insert")
        else {
            panic!("track");
        };
        view.track_selection.reconcile(model.project());
        assert_eq!(
            view.track_selection.ordered(model.project()),
            [source, empty]
        );
        assert!(!view.track_selection.contains(new));
        model
            .execute(hex_project::Command::RemoveTrack(source))
            .expect("remove");
        view.track_selection.reconcile(model.project());
        assert_eq!(view.track_selection.ordered(model.project()), [empty]);
        assert!(model.undo());
        view.track_selection.reconcile(model.project());
        assert_eq!(
            view.track_selection.ordered(model.project()),
            [empty],
            "Undo does not invent explicit membership for a removed ID"
        );
    });
}

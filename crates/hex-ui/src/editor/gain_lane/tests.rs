use super::*;
use hex_project::{Command, Created, Editor, GainInterpolation, Project};
fn point(raw: u64, gain: f32) -> GainPoint {
    GainPoint {
        incoming_gain: None,
        beat: BeatTime::from_raw(raw),
        gain,
        outgoing: GainInterpolation::Linear,
    }
}
fn project() -> (Project, TrackId, TrackId) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Track(a) = editor
        .execute(Command::AddTrack {
            name: "first".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Track(b) = editor
        .execute(Command::AddTrack {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track: a,
            points: vec![
                point(1, 0.5),
                point(2, 1.5),
                point(BeatTime::ONE.raw(), 0.0),
            ],
        })
        .unwrap();
    (editor.project().clone(), a, b)
}
#[test]
fn occupied_move_refuses_complete_candidate_without_losing_either_authored_point() {
    let original = vec![point(1, 0.5), point(2, 1.5)];
    assert!(replacement(&original, Some(BeatTime::from_raw(1)), point(2, 0.25)).is_err());
    assert_eq!(original[0].gain, 0.5);
    assert_eq!(original[1].gain, 1.5);
    let moved = replacement(&original, Some(BeatTime::from_raw(1)), point(3, 0.25)).unwrap();
    let (project, id, _) = project();
    let mut editor = Editor::new(project, 8).unwrap();
    let before = editor.project().clone();
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track: id,
            points: moved.clone(),
        })
        .unwrap();
    assert_eq!(editor.project().track(id).unwrap().gain_envelope(), moved);
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
}
#[test]
fn free_geometry_preserves_raw_keys_and_snap_respects_time_viewport() {
    let (project, _, _) = project();
    let viewport = Viewport {
        start: 24000,
        length: 48000,
    };
    let rect = [100.0, 20.0, 200.0, 40.0];
    assert_eq!(
        pointer_beat(
            &project,
            super::super::musical_time::SnapGrid::Off,
            viewport,
            rect,
            200.0
        )
        .unwrap(),
        BeatTime::from_quarters(2).unwrap()
    );
    assert_eq!(pointer_gain(rect, 20.0), 2.0);
    assert_eq!(pointer_gain(rect, 60.0), 0.0);
    assert_eq!(pointer_gain(rect, 40.0), 1.0);
    let raw = BeatTime::from_raw(2);
    let mut lane = GainLane {
        track: Some(project.tracks()[0].id()),
        selected: Some(raw),
        ..Default::default()
    };
    lane.reconcile(0, 1, &project, false);
    assert_eq!(lane.selected, Some(raw));
}
#[test]
fn generation_revision_and_owner_changes_cancel_drafts_without_reinterpreting_ids() {
    let (project, a, b) = project();
    let draft = Draft {
        generation: 0,
        revision: 1,
        track: a,
        from: Some(BeatTime::from_raw(1)),
        point: point(1, 0.75),
        pointer: None,
    };
    for (generation, revision, replaced) in [(1, 1, false), (0, 2, false), (0, 1, true)] {
        let mut lane = GainLane {
            track: Some(a),
            draft: Some(draft),
            ..Default::default()
        };
        lane.reconcile(generation, revision, &project, replaced);
        assert!(lane.draft.is_none());
    }
    let mut lane = GainLane {
        track: Some(a),
        draft: Some(draft),
        ..Default::default()
    };
    lane.change_owner(b, &project);
    assert_eq!(lane.track, Some(b));
    assert!(lane.draft.is_none());
}

#[test]
fn presentation_distinguishes_local_drafts_canonical_pending_and_exact_applied_revision() {
    assert_eq!(
        presentation(None, true, true, 7, Some(7)),
        "draft · not applied"
    );
    assert!(presentation(None, false, true, 7, Some(6)).contains("canonical revision unchanged"));
    assert!(presentation(None, false, false, 7, Some(6)).contains("application pending"));
    assert!(presentation(None, false, false, 7, Some(7)).contains("curve is a prepared preview"));
    assert_eq!(
        presentation(Some("occupied beat"), true, false, 7, None),
        "occupied beat"
    );
}
#[cfg(feature = "ui-test-support")]
pub(in crate::editor) mod native {
    use super::*;
    use gpui::{
        Modifiers, MouseButton, TestAppContext, VisualTestContext, point as gpui_point, px, size,
    };
    use std::sync::Arc;
    pub(in crate::editor) fn window(
        cx: &mut TestAppContext,
    ) -> (gpui::AnyWindowHandle, Entity<EditorView>, TrackId) {
        cx.update(super::super::super::bind_editor_keys);
        let (project, track, _) = project();
        let preview = Arc::new(hex_engine::GainAutomationPreview::prepare(&project).unwrap());
        let (handle, entity) =
            super::super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        entity.update(cx, |view, cx| {
            let old = view.state.as_ref().unwrap();
            view.state = Some(Arc::new(ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: old.project.clone(),
                waveforms: old.waveforms.clone(),
                source_overviews: old.source_overviews.clone(),
                audio_clip_waveforms: old.audio_clip_waveforms.clone(),
                midi_preview: old.midi_preview.clone(),
                gain_preview: Ok(preview),
                timeline_frames: old.timeline_frames,
                can_undo: true,
                can_redo: false,
                revision: 1,
                unsaved: true,
                document_path: None,
            }));
            view.clip_view_open = false;
            view.gain_lane.track = Some(track);
            view.gain_lane.selected = Some(BeatTime::from_raw(2));
            cx.notify();
        });
        (handle, entity, track)
    }
    #[gpui::test]
    fn pane_preserves_fixed_rows_and_right_header_alignment_at_minimum_size(
        cx: &mut TestAppContext,
    ) {
        let (handle, entity, _) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(880.0), px(540.0)));
        visual.run_until_parked();
        let lane = visual.debug_bounds("gain-lane").unwrap();
        let plot = visual.debug_bounds("gain-lane-plot").unwrap();
        let header = visual.debug_bounds("main-track-header").unwrap();
        assert_eq!(lane.size.height, px(96.0));
        assert_eq!(plot.origin.x + plot.size.width, header.origin.x);
        assert!(
            visual
                .debug_bounds("arrangement-content")
                .unwrap()
                .size
                .height
                > px(1.0)
        );
        entity.update(&mut visual, |view, cx| {
            view.cycle_gain_point(false, cx);
            assert_eq!(view.gain_lane.selected, Some(BeatTime::from_raw(1)));
            view.cycle_gain_point(true, cx);
            assert_eq!(view.gain_lane.selected, Some(BeatTime::from_raw(2)));
        });
    }
    #[gpui::test]
    fn click_on_coincident_authored_point_keeps_exact_beat_and_drag_is_local_until_release(
        cx: &mut TestAppContext,
    ) {
        let (handle, entity, _) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("gain-lane-plot").unwrap();
        let position = gpui_point(bounds.origin.x, bounds.origin.y + bounds.size.height * 0.25);
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        entity.update(&mut visual, |view, _| {
            let draft = view.gain_lane.draft.unwrap();
            assert_eq!(draft.point.beat, BeatTime::from_raw(2));
            assert_eq!(view.state.as_ref().unwrap().revision, 1);
            assert!(view.pending_requests.is_empty());
        });
        visual.simulate_mouse_move(
            position + gpui_point(px(20.0), px(8.0)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        entity.update(&mut visual, |view, cx| {
            assert!(view.gain_lane.draft.is_some());
            assert_eq!(view.state.as_ref().unwrap().revision, 1);
            assert!(view.cancel_gain_draft(cx));
            assert!(!view.cancel_gain_draft(cx));
        });
    }
    #[gpui::test]
    fn invalid_numeric_draft_refuses_actions_and_undo_cancels_before_history(
        cx: &mut TestAppContext,
    ) {
        let (handle, entity, _) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("gain-point-value-value").unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("nan");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        entity.update(&mut visual, |view, cx| {
            assert!(
                view.gain_lane
                    .gain
                    .as_ref()
                    .unwrap()
                    .entity
                    .read(cx)
                    .is_editing()
            );
            view.gain_edit(GainLaneEdit::Clear, cx);
            assert!(
                view.gain_lane
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("local draft")
            );
            assert_eq!(view.worker_requests_pending, 0);
        });
        visual.simulate_keystrokes("cmd-z");
        visual.run_until_parked();
        entity.update(&mut visual, |view, cx| {
            assert!(view.gain_lane.draft.is_none());
            assert!(
                !view
                    .gain_lane
                    .gain
                    .as_ref()
                    .is_some_and(|binding| binding.entity.read(cx).is_editing())
            );
            assert_eq!(view.worker_requests_pending, 0);
        });
    }

    #[gpui::test]
    fn command_t_and_r_preserve_active_gain_numeric_draft(cx: &mut TestAppContext) {
        let (handle, entity, _) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("gain-point-value-value").unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid");
        visual.run_until_parked();
        let focus = visual.update(|window, cx| window.focused(cx).unwrap());
        for key in ["cmd-t", "cmd-r"] {
            visual.simulate_keystrokes(key);
            visual.update(|window, _| assert!(focus.is_focused(window)));
            entity.update(&mut visual, |view, cx| {
                assert!(view.gain_numeric_editing(cx));
                assert!(!view.track_name.is_editing());
                assert!(view.clip_name_edit.is_none());
                assert!(view.last_enqueued_edit.is_none());
                assert_eq!(view.worker_requests_pending, 0);
            });
        }
    }

    #[gpui::test]
    fn command_f_preserves_invalid_gain_focus_until_draft_is_cancelled(cx: &mut TestAppContext) {
        let (handle, entity, _) = window(cx);
        let visual = &mut VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("gain-point-value-value").unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        let focus = visual.update(|window, cx| window.focused(cx).unwrap());
        visual.simulate_keystrokes("cmd-f");
        visual.run_until_parked();
        visual.update(|window, _| {
            assert!(focus.is_focused(window), "invalid gain must retain focus")
        });
        entity.update(visual, |view, cx| {
            assert!(view.gain_numeric_editing(cx));
            assert!(view.error.as_ref().unwrap().contains("finish or cancel"));
            assert_eq!(view.worker_requests_pending, 0);
        });
        visual.simulate_keystrokes("escape cmd-f");
        visual.simulate_input("utility");
        visual.simulate_keystrokes("down escape");
        entity.read_with(visual, |view, cx| {
            assert!(
                view.browser.search.read(cx).text().is_empty(),
                "browser escape must resolve after cancelling gain edit"
            )
        });
    }

    #[gpui::test]
    fn escape_cancels_invalid_numeric_text_without_an_edit(cx: &mut TestAppContext) {
        let (handle, entity, _) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("gain-point-value-value").unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid");
        visual.run_until_parked();
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        entity.update(&mut visual, |view, cx| {
            assert!(view.gain_numeric_editing(cx));
            assert_eq!(view.worker_requests_pending, 0);
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        entity.update(&mut visual, |view, cx| {
            assert!(view.gain_lane.draft.is_none());
            assert!(
                !view
                    .gain_lane
                    .gain
                    .as_ref()
                    .is_some_and(|binding| binding.entity.read(cx).is_editing())
            );
            assert_eq!(view.worker_requests_pending, 0);
            assert_eq!(view.gain_point().unwrap().beat.raw(), 2);
        });
    }
    #[gpui::test]
    fn pointer_release_submits_once_and_shared_viewport_survives_zoom(cx: &mut TestAppContext) {
        let (handle, entity, _) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        entity.update(&mut visual, |view, cx| {
            view.viewport = Some(Viewport {
                start: 24000,
                length: 48000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let bounds = visual.debug_bounds("gain-lane-plot").unwrap();
        let p = bounds.center();
        visual.simulate_mouse_down(p, MouseButton::Left, Modifiers::default());
        entity.update(&mut visual, |view, _| {
            assert_eq!(
                view.gain_lane.draft.unwrap().point.beat,
                BeatTime::from_quarters(2).unwrap()
            );
            assert_eq!(view.worker_requests_pending, 0);
        });
        visual.simulate_mouse_up(p, MouseButton::Left, Modifiers::default());
        entity.update(&mut visual, |view, _| {
            assert!(view.gain_lane.draft.is_none());
            assert_eq!(view.worker_requests_pending, 1);
        });
        visual.simulate_mouse_up(p, MouseButton::Left, Modifiers::default());
        entity.update(&mut visual, |view, _| {
            assert_eq!(view.worker_requests_pending, 1);
        });
    }

    #[gpui::test]
    fn unrelated_edit_refuses_during_gain_gesture_but_release_commits(cx: &mut TestAppContext) {
        let (handle, entity, track) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let position = visual.debug_bounds("gain-lane-plot").unwrap().center();
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        entity.update(&mut visual, |view, _| {
            assert!(view.gain_lane.draft.is_some());
            assert!(!view.enqueue(hex_session::service::Request::Edit(
                hex_project::Command::Mute { track, muted: true }
            )));
            assert!(view.gain_lane.draft.is_some());
            assert_eq!(view.worker_requests_pending, 0);
            assert!(
                view.error
                    .as_deref()
                    .unwrap()
                    .contains("gain point gesture")
            );
        });
        visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        entity.update(&mut visual, |view, _| {
            assert!(view.gain_lane.draft.is_none());
            assert_eq!(view.worker_requests_pending, 1);
            assert!(view.gain_lane.pending.is_some());
        });
    }

    #[gpui::test]
    fn unrelated_edit_preserves_numeric_gain_text_and_valid_commit(cx: &mut TestAppContext) {
        let (handle, entity, track) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let position = visual
            .debug_bounds("gain-point-value-value")
            .unwrap()
            .center();
        visual.simulate_click(position, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid");
        entity.update(&mut visual, |view, cx| {
            assert!(view.gain_numeric_editing(cx));
            assert!(!view.enqueue(hex_session::service::Request::Edit(
                hex_project::Command::Mute { track, muted: true }
            )));
            assert!(view.gain_numeric_editing(cx));
            assert_eq!(
                view.gain_lane
                    .gain
                    .as_ref()
                    .unwrap()
                    .entity
                    .read(cx)
                    .display_text(),
                "invalid"
            );
            assert_eq!(view.worker_requests_pending, 0);
        });
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("75");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        entity.update(&mut visual, |view, cx| {
            assert!(!view.gain_numeric_editing(cx));
            assert_eq!(view.worker_requests_pending, 1);
            assert!(view.gain_lane.pending.is_some());
        });
    }
    #[gpui::test]
    fn header_a_opens_selected_lane_and_retains_operable_rows(cx: &mut TestAppContext) {
        let (handle, entity, track) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(880.0), px(540.0)));
        entity.update(&mut visual, |view, cx| {
            view.gain_lane.track = None;
            view.clip_view_open = true;
            cx.notify();
        });
        visual.run_until_parked();
        let selector = Box::leak(format!("gain-lane-show-{}", track.value()).into_boxed_str());
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("gain-lane").is_some());
        assert!(
            visual
                .debug_bounds("arrangement-content")
                .unwrap()
                .size
                .height
                > px(1.0)
        );
        entity.update(&mut visual, |view, _| {
            assert_eq!(view.selected_track, Some(track));
            assert!(!view.clip_view_open);
            assert_eq!(view.gain_lane.selected, Some(BeatTime::from_raw(1)));
        });
    }
    #[gpui::test]
    fn reopening_details_closes_local_lane_without_consuming_history(cx: &mut TestAppContext) {
        let (handle, entity, _) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        entity.update(&mut visual, |view, cx| {
            let p = view.gain_point().unwrap();
            assert!(view.begin_gain_draft(Some(p.beat), p));
            view.clip_view_open = true;
            cx.notify();
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("gain-lane").is_none());
        entity.update(&mut visual, |view, _| {
            assert!(view.gain_lane.draft.is_none());
            assert_eq!(view.worker_requests_pending, 0);
            assert_eq!(view.state.as_ref().unwrap().revision, 1);
        });
    }
    #[gpui::test]
    fn numeric_gain_commit_retains_exact_authored_beat_and_is_a_single_request(
        cx: &mut TestAppContext,
    ) {
        let (handle, entity, _) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("gain-point-value-value").unwrap();
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("125%");
        visual.simulate_keystrokes("a");
        visual.run_until_parked();
        entity.update(&mut visual, |view, cx| {
            assert!(!view.automation_view.enabled);
            assert!(view.gain_numeric_editing(cx));
            assert_eq!(view.worker_requests_pending, 0);
            assert_eq!(view.gain_lane.selected, Some(BeatTime::from_raw(2)));
        });
        // The text editor owns the typed a; replace that invalid numeric text.
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("125%");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        entity.update(&mut visual, |view, _| {
            assert_eq!(view.worker_requests_pending, 1);
            assert_eq!(view.gain_lane.selected, Some(BeatTime::from_raw(2)));
            assert!(view.gain_lane.draft.is_none());
        });
    }
    #[gpui::test]
    fn occupied_pointer_move_refuses_before_actor_and_keeps_visible_draft(cx: &mut TestAppContext) {
        let (handle, entity, _) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("gain-lane-plot").unwrap();
        let start = gpui_point(bounds.origin.x, bounds.origin.y + bounds.size.height * 0.25);
        let to = gpui_point(
            bounds.origin.x + bounds.size.width * (24000.0 / 384000.0),
            start.y,
        );
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        entity.update(&mut visual, |view, cx| {
            assert_eq!(view.worker_requests_pending, 0);
            assert!(view.gain_lane.draft.is_some());
            assert!(view.gain_lane.error.as_ref().unwrap().contains("occupied"));
            assert_eq!(view.gain_point().unwrap().beat.raw(), 2);
            assert!(view.cancel_gain_draft(cx));
        });
    }
    #[gpui::test]
    fn authored_points_beyond_content_remain_visible_without_extending_export_extent(
        cx: &mut TestAppContext,
    ) {
        let (handle, entity, track) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        entity.update(&mut visual, |view, cx| {
            // This test exercises auto-fit before a canonical lane install.
            view.viewport = None;
            let old = view.state.as_ref().unwrap();
            let mut project = Editor::new((*old.project).clone(), 8).unwrap();
            project
                .execute(Command::UpsertTrackGainPoint {
                    track,
                    point: point(100 * BeatTime::ONE.raw(), 0.5),
                })
                .unwrap();
            let project = Arc::new(project.project().clone());
            let preview = hex_engine::GainAutomationPreview::prepare(&project).unwrap();
            let content_extent = old.timeline_frames;
            view.state = Some(Arc::new(ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: project.clone(),
                gain_preview: Ok(Arc::new(preview)),
                waveforms: old.waveforms.clone(),
                source_overviews: old.source_overviews.clone(),
                audio_clip_waveforms: old.audio_clip_waveforms.clone(),
                midi_preview: old.midi_preview.clone(),
                timeline_frames: content_extent,
                revision: 2,
                can_undo: true,
                can_redo: false,
                unsaved: true,
                document_path: None,
            }));
            assert!(
                view.visible_viewport(content_extent)
                    .fraction(2400000)
                    .is_some()
            );
            assert_eq!(view.state.as_ref().unwrap().timeline_frames, content_extent);
            view.gain_lane.track = None;
            assert!(
                view.visible_viewport(content_extent)
                    .fraction(2400000)
                    .is_none()
            );
            cx.notify();
        });
    }
    #[gpui::test]
    fn touch_only_lane_preview_ownership_and_display_extent_are_truthful(cx: &mut TestAppContext) {
        let (handle, entity, track) = window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        entity.update(&mut visual, |view, cx| {
            // This test exercises auto-fit before a canonical lane install.
            view.viewport = None;
            let old = view.state.as_ref().unwrap();
            let mut snapshot = old.project.snapshot();
            let owner = snapshot
                .tracks
                .iter_mut()
                .find(|owner| owner.id == track.value())
                .unwrap();
            owner.gain_envelope.clear();
            owner.gain = 0.25;
            owner.gain_touch_epoch = 1;
            owner.gain_touch_spans = vec![hex_project::GainTouchSpan {
                source_start: BeatTime::ONE,
                start: BeatTime::ONE,
                end: BeatTime::from_quarters(100).unwrap(),
                context: hex_project::GainTouchContext {
                    recording_epoch: 1,
                    project_generation: 0,
                    canonical_revision: 1,
                    audio_revision: 1,
                    transport_epoch: 1,
                    gesture_epoch: 1,
                    gesture_serial: 1,
                    sample_rate: 48000,
                    loop_pass: 0,
                },
                steps: vec![hex_project::GainTouchStep {
                    beat: BeatTime::ONE,
                    gain: 0.8,
                    origin_beat: BeatTime::ONE,
                    source_sequence: 1,
                }],
            }];
            let project = Arc::new(Project::from_snapshot(snapshot).unwrap());
            let preview = Arc::new(hex_engine::GainAutomationPreview::prepare(&project).unwrap());
            let content_extent = old.timeline_frames;
            view.state = Some(Arc::new(ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project,
                gain_preview: Ok(preview),
                waveforms: old.waveforms.clone(),
                source_overviews: old.source_overviews.clone(),
                audio_clip_waveforms: old.audio_clip_waveforms.clone(),
                midi_preview: old.midi_preview.clone(),
                timeline_frames: old.timeline_frames,
                can_undo: old.can_undo,
                can_redo: old.can_redo,
                revision: old.revision,
                unsaved: old.unsaved,
                document_path: old.document_path.clone(),
            }));
            let state = view.state.as_ref().unwrap();
            assert_eq!(preview_gain(state, track, 0), Some(0.25));
            assert_eq!(preview_gain(state, track, 24000), Some(0.8));
            assert_eq!(preview_gain(state, track, 2400000), Some(0.25));
            assert!(
                view.visible_viewport(content_extent)
                    .fraction(2400000)
                    .is_some()
            );
            assert_eq!(state.timeline_frames, content_extent);
            assert!(!view.begin_gain_draft(None, point(0, 0.5)));
            assert!(view.gain_lane.error.as_ref().unwrap().contains("read-only"));
            assert!(
                view.gain_action_admitted(cx),
                "clear remains an available recovery action"
            );
            cx.notify();
        });
    }
}

use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};

#[gpui::test]
fn draw_order_queries_match_independent_scan(cx: &mut TestAppContext) {
    cx.verify_draw_order_queries()
        .expect("private GPUI draw-order oracle");
}

#[gpui::test]
fn cached_scene_bulk_replay_matches_legacy_painter_and_refusal_state(cx: &mut TestAppContext) {
    cx.verify_cached_scene_replay()
        .expect("private GPUI replay oracle");
}

#[gpui::test]
fn held_pan_reuses_current_scene_and_release_keeps_first_clip_drag(cx: &mut TestAppContext) {
    let (project, source, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let before = project.clone();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.viewport = Some(super::super::viewport::Viewport {
            start: 0,
            length: 192_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let lane = editor.read_with(&visual, |view, _| view.drag_lanes[&source]);
    let start = point(gpui::px(lane.left + 40.0), gpui::px(lane.top + 25.0));
    visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
    let end = point(start.x - gpui::px(20.0), start.y);
    visual.simulate_mouse_move(end, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    let content = editor.read_with(&visual, |view, _| {
        assert!(view.arrangement_pan.is_some());
        assert!(
            view.visible_viewport(view.state.as_ref().unwrap().timeline_frames)
                .start
                > 0
        );
        view.clip_content_cache.entries[&source].clone()
    });
    let builds = content.read_with(&visual, |content, _| content.builds);
    for _ in 0..2 {
        visual.update(|window, cx| {
            editor.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear();
        });
    }
    assert_eq!(
        content.read_with(&visual, |content, _| content.builds),
        builds
    );
    visual.simulate_mouse_up(end, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    let surface = Box::leak(format!("clip-drag-surface-{}", clip.value()).into_boxed_str());
    let down = visual
        .debug_bounds(surface)
        .expect("current clip move hit")
        .center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + gpui::px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    assert!(
        cx.update(|app| app.has_active_drag()),
        "first motion after pan release"
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &before)
    });
}

fn publish_snapshot(view: &mut EditorView, project: Arc<hex_project::Project>, revision: u64) {
    let old = view.state.as_ref().expect("current state");
    view.state = Some(Arc::new(ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        project,
        waveforms: old.waveforms.clone(),
        source_overviews: old.source_overviews.clone(),
        audio_clip_waveforms: old.audio_clip_waveforms.clone(),
        midi_preview: Arc::clone(&old.midi_preview),
        gain_preview: old.gain_preview.clone(),
        timeline_frames: old.timeline_frames,
        can_undo: old.can_undo,
        can_redo: old.can_redo,
        revision,
        unsaved: old.unsaved,
        document_path: old.document_path.clone(),
    }));
}

struct RestoreTheme(hex_preferences::ThemePreferences);
impl Drop for RestoreTheme {
    fn drop(&mut self) {
        theme::apply_preferences(self.0);
    }
}

#[gpui::test]
fn palette_grid_and_resize_invalidate_content_with_current_geometry(cx: &mut TestAppContext) {
    let _restore = RestoreTheme(theme::preferences());
    let (project, source, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let content = || {
        editor.read_with(cx, |view, _| {
            view.clip_content_cache.entries[&source].clone()
        })
    };
    let original = content();
    let original_epoch = original.read_with(cx, |content, _| content.epoch);
    let mut palette = theme::preferences();
    palette.background = hex_preferences::RgbColor::new(23, 31, 47);
    theme::apply_preferences(palette);
    visual.update(|window, cx| {
        editor.update(cx, |_, cx| cx.notify());
        window.draw(cx).clear();
    });
    let themed = content();
    assert_eq!(original.entity_id(), themed.entity_id());
    let themed_epoch = themed.read_with(cx, |content, _| content.epoch);
    assert_ne!(original_epoch, themed_epoch);
    let grid_frame = themed.read_with(cx, |content, _| {
        content
            .key
            .grid
            .iter()
            .find(|line| line.frame > 0 && line.frame < 96_000)
            .expect("interior clip grid line")
            .frame
    });
    let grid_selector = Box::leak(format!("clip-grid-line-{grid_frame}").into_boxed_str());
    assert!(visual.debug_bounds(grid_selector).is_some());
    let selector = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    assert_eq!(
        visual.rendered_background(selector),
        Some(
            gpui::rgb(theme::clip_fill(
                editor.read_with(cx, |view, _| view
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(source)
                    .unwrap()
                    .color()),
                hex_project::ClipFlavor::Muted,
                false
            ))
            .into()
        )
    );
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.snap_grid = SnapGrid::Off;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    let ungridded = content();
    assert_eq!(themed.entity_id(), ungridded.entity_id());
    let ungridded_epoch = ungridded.read_with(cx, |content, _| content.epoch);
    let ungridded_builds = ungridded.read_with(cx, |content, _| content.builds);
    assert_ne!(themed_epoch, ungridded_epoch);
    assert!(
        visual.debug_bounds(grid_selector).is_none(),
        "snap off paints no musical grid"
    );
    visual.simulate_resize(gpui::size(gpui::px(980.0), gpui::px(680.0)));
    // GPUI's changed bounds rebuild the resize scene. The following owner
    // notification must reuse it rather than rebuild for the consumed lane.
    for _ in 0..2 {
        visual.update(|window, cx| window.draw(cx).clear());
    }
    let resized = content();
    assert_eq!(ungridded.entity_id(), resized.entity_id());
    assert_eq!(
        resized.read_with(cx, |content, _| content.builds),
        ungridded_builds + 1,
        "one resize build, with no redundant following-frame rebuild"
    );
    editor.read_with(cx, |view, app| {
        let lane = view.drag_lanes[&source];
        assert_eq!(resized.read(app).key.lane, Some(lane));
        assert_eq!(view.drag_lane_epochs[&source], view.drag_layout_generation);
    });
    let bounds = visual.debug_bounds(selector).expect("resized visible clip");
    let expected = editor.read_with(cx, |view, _| view.drag_lanes[&source].width * 0.25);
    assert!((f32::from(bounds.size.width) - expected).abs() <= 1.0);
}

#[gpui::test]
fn busy_to_idle_rebuilds_handles_and_pending_mouse_down_keeps_identity(cx: &mut TestAppContext) {
    let (project, source, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-drag-surface-{}", clip.value()).into_boxed_str());
    let original = editor.read_with(cx, |view, _| {
        view.clip_content_cache.entries[&source].entity_id()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.worker_requests_pending = 1;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    editor.read_with(cx, |view, _| {
        assert!(view.clip_content_cache.entries.contains_key(&source))
    });
    assert!(
        visual.debug_bounds(selector).is_none(),
        "busy content has no editable drag surface"
    );
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.worker_requests_pending = 0;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    let surface = visual
        .debug_bounds(selector)
        .expect("idle drag surface restored");
    let idle = editor.read_with(cx, |view, _| {
        view.clip_content_cache.entries[&source].clone()
    });
    assert_eq!(idle.entity_id(), original);
    let down = surface.center();
    visual.simulate_mouse_move(down, None, Modifiers::default());
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            assert!(view.pointer_down.is_some());
            cx.notify();
        });
        window.draw(cx).clear();
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.clip_content_cache.entries[&source].entity_id(),
            idle.entity_id()
        )
    });
    let moved = point(down.x + gpui::px(12.0), down.y);
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    assert!(
        cx.update(|app| app.has_active_drag()),
        "ordinary redraw between mouse-down and threshold must preserve drag capture"
    );
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    assert!(!cx.update(|app| app.has_active_drag()));
}

#[gpui::test]
fn same_revision_snapshot_refresh_preserves_pending_clip_drag(cx: &mut TestAppContext) {
    let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-drag-surface-{}", clip.value()).into_boxed_str());
    let down = visual
        .debug_bounds(selector)
        .expect("drag surface")
        .center();
    visual.simulate_mouse_move(down, None, Modifiers::default());
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            assert!(view.pointer_down.is_some());
            let state = view.state.as_ref().expect("state");
            let revision = state.revision;
            let project = Arc::clone(&state.project);
            publish_snapshot(view, project, revision);
            cx.notify();
        });
        window.draw(cx).clear();
    });
    let moved = point(down.x + gpui::px(12.0), down.y);
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    assert!(
        cx.update(|app| app.has_active_drag()),
        "read-only snapshot refresh must not erase otherwise-current mouse-down capture"
    );
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn snapshot_history_and_project_removal_release_cached_ownership(cx: &mut TestAppContext) {
    use hex_project::{Command, Editor};
    let (project, source, target, clip) = super::super::clip_drag_ui_test::audio_project();
    let mut history = Editor::new(project.clone(), 8).expect("history");
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (old_snapshot, old_source, old_target) = editor.read_with(cx, |view, _| {
        (
            Arc::downgrade(view.state.as_ref().expect("state")),
            view.clip_content_cache.entries[&source].downgrade(),
            view.clip_content_cache.entries[&target].downgrade(),
        )
    });
    // Metadata may replace the immutable Arc without changing the project revision.
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let project = Arc::clone(&view.state.as_ref().expect("state").project);
            publish_snapshot(view, project, 1);
            cx.notify();
        });
        window.draw(cx).clear();
    });
    visual.run_until_parked();
    assert!(
        old_source.upgrade().is_some(),
        "same-project snapshot refresh keeps track identity"
    );
    assert!(old_target.upgrade().is_some());
    assert!(
        old_snapshot.upgrade().is_none(),
        "offscreen cache must not retain an older view snapshot"
    );
    history
        .execute(Command::MoveClip {
            clip,
            track: source,
            start: 24_000,
        })
        .expect("move");
    let moved = history.project().clone();
    assert!(history.undo());
    let undone = history.project().clone();
    assert!(history.redo());
    let redone = history.project().clone();
    let selector = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    for (index, project) in [moved, undone, redone].into_iter().enumerate() {
        let expected_start = project.clip(clip).expect("clip").start();
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                publish_snapshot(view, Arc::new(project), index as u64 + 2);
                cx.notify();
            });
            window.draw(cx).clear();
        });
        let bounds = visual
            .debug_bounds(selector)
            .expect("current canonical history content");
        let expected = editor.read_with(cx, |view, _| {
            let lane = view.drag_lanes[&source];
            lane.left + lane.width * expected_start as f32 / 384_000.0
        });
        assert!((f32::from(bounds.origin.x) - expected).abs() <= 1.0);
    }
    history
        .execute(Command::RemoveTrack(source))
        .expect("remove source");
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            publish_snapshot(view, Arc::new(history.project().clone()), 5);
            cx.notify();
        });
        window.draw(cx).clear();
    });
    assert!(visual.debug_bounds(selector).is_none());
    editor.read_with(cx, |view, _| {
        assert!(!view.clip_content_cache.entries.contains_key(&source));
        assert!(view.clip_content_cache.entries.contains_key(&target));
        assert!(view.clip_content_cache.entries.len() <= hex_project::MAX_TRACKS);
    });
    let previous = editor.read_with(cx, |view, _| {
        view.clip_content_cache.entries[&target].downgrade()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.project_generation += 1;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    visual.run_until_parked();
    assert!(
        previous.upgrade().is_none(),
        "generation changes release children even with the same snapshot Arc"
    );
}

fn cached<V: Render>(entity: Entity<V>) -> AnyView {
    let mut layout = div().absolute().inset_0();
    AnyView::from(entity).cached(layout.style().clone())
}

fn metadata_probe(selector: &'static str, left: f32) -> gpui::Div {
    div()
        .debug_selector(move || selector.to_owned())
        .absolute()
        .left(gpui::px(left))
        .top_0()
        .w(gpui::px(10.0))
        .h(gpui::px(10.0))
}
struct MetadataInner {
    left: f32,
    builds: usize,
}
impl Render for MetadataInner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.builds += 1;
        div().size_full().relative().children([
            metadata_probe("metadata-inner", self.left),
            metadata_probe("metadata-duplicate", self.left),
        ])
    }
}
struct MetadataOuter {
    inner: Entity<MetadataInner>,
    builds: usize,
}

struct EpochRoot {
    child: Entity<MetadataInner>,
    epoch: u64,
}
impl Render for EpochRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut layout = div().absolute().inset_0();
        div()
            .size_full()
            .relative()
            .children([AnyView::from(self.child.clone())
                .cached_with_epoch(layout.style().clone(), self.epoch)])
    }
}

#[gpui::test]
fn epoch_rebuilds_current_draw_without_changing_identity_or_dirtying_next(cx: &mut TestAppContext) {
    let handle = cx.add_window(|_, cx| EpochRoot {
        child: cx.new(|_| MetadataInner {
            left: 5.0,
            builds: 0,
        }),
        epoch: 0,
    });
    let root = handle.root(cx).expect("epoch root");
    let child = root.read_with(cx, |root, _| root.child.clone());
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.run_until_parked();
    let builds = child.read_with(cx, |child, _| child.builds);
    visual.update(|window, cx| {
        child.update(cx, |child, _| child.left = 13.0); // Deliberately no child notify.
        root.update(cx, |root, cx| {
            root.epoch = 1;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    assert_eq!(
        visual
            .debug_bounds("metadata-inner")
            .expect("new epoch current bounds")
            .origin
            .x,
        gpui::px(13.0)
    );
    assert_eq!(child.read_with(cx, |child, _| child.builds), builds + 1);
    visual.update(|window, cx| {
        root.update(cx, |_, cx| cx.notify());
        window.draw(cx).clear();
    });
    assert_eq!(
        child.read_with(cx, |child, _| child.builds),
        builds + 1,
        "unchanged epoch reuses the current ranges without a delayed notification"
    );
    assert_eq!(
        root.read_with(cx, |root, _| root.child.entity_id()),
        child.entity_id()
    );
}
impl Render for MetadataOuter {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.builds += 1;
        div()
            .size_full()
            .relative()
            .children([cached(self.inner.clone())])
            .children([metadata_probe("metadata-duplicate", 20.0)])
    }
}
struct MetadataRoot {
    outer: Entity<MetadataOuter>,
    show_outer: bool,
    show_outside: bool,
}
impl Render for MetadataRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .children(self.show_outer.then(|| cached(self.outer.clone())))
            .children(
                self.show_outside
                    .then(|| metadata_probe("metadata-duplicate", 40.0)),
            )
            .children(
                self.show_outside
                    .then(|| metadata_probe("metadata-removed", 60.0)),
            )
    }
}

#[gpui::test]
fn cached_metadata_replays_nested_ranges_and_preserves_current_overwrites(cx: &mut TestAppContext) {
    let handle = cx.add_window(|_, cx| {
        let inner = cx.new(|_| MetadataInner {
            left: 5.0,
            builds: 0,
        });
        let outer = cx.new(|_| MetadataOuter { inner, builds: 0 });
        MetadataRoot {
            outer,
            show_outer: true,
            show_outside: true,
        }
    });
    let root = handle.root(cx).expect("metadata root");
    let outer = root.read_with(cx, |root, _| root.outer.clone());
    let inner = outer.read_with(cx, |outer, _| outer.inner.clone());
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds("metadata-duplicate")
            .expect("outside overwrite")
            .origin
            .x,
        gpui::px(40.0)
    );
    for _ in 0..2 {
        visual.update(|window, cx| {
            root.update(cx, |root, cx| {
                root.show_outside = false;
                cx.notify();
            });
            window.draw(cx).clear();
        });
        assert_eq!(
            visual
                .debug_bounds("metadata-inner")
                .expect("nested cached bounds")
                .origin
                .x,
            gpui::px(5.0)
        );
        assert_eq!(
            visual
                .debug_bounds("metadata-duplicate")
                .expect("outer cached overwrite")
                .origin
                .x,
            gpui::px(20.0)
        );
        assert!(
            visual.debug_bounds("metadata-removed").is_none(),
            "unrelated removed selector must not be replayed"
        );
        assert_eq!(outer.read_with(cx, |outer, _| outer.builds), 1);
        assert_eq!(inner.read_with(cx, |inner, _| inner.builds), 1);
    }
    visual.update(|window, cx| {
        inner.update(cx, |inner, cx| {
            inner.left = 12.0;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    assert_eq!(
        visual
            .debug_bounds("metadata-inner")
            .expect("moved nested bounds")
            .origin
            .x,
        gpui::px(12.0)
    );
    visual.update(|window, cx| {
        root.update(cx, |root, cx| {
            root.show_outer = false;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    assert!(visual.debug_bounds("metadata-inner").is_none());
    assert!(visual.debug_bounds("metadata-duplicate").is_none());
}

#[gpui::test]
fn changed_key_paints_current_content_in_same_draw_then_reuses(cx: &mut TestAppContext) {
    let (project, source, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let previous = editor.read_with(cx, |view, _| {
        view.clip_content_cache
            .entries
            .get(&source)
            .expect("cached source")
            .clone()
    });
    let previous_builds = previous.read_with(cx, |content, _| content.builds);
    let selector = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.selected_clip = Some(clip);
            cx.notify();
        });
        window.draw(cx).clear();
    });
    assert_eq!(
        visual.rendered_background(selector),
        Some(
            gpui::rgb(theme::arrangement_clip_body_fill(
                theme::clip_fill(
                    editor.read_with(cx, |view, _| view
                        .state
                        .as_ref()
                        .unwrap()
                        .project
                        .track(source)
                        .unwrap()
                        .color()),
                    hex_project::ClipFlavor::Muted,
                    false
                ),
                true
            ))
            .into()
        ),
        "the first changed-key draw must paint selected content, without an extra frame"
    );
    let original_bounds = visual.debug_bounds(selector).expect("selected clip bounds");
    let current = editor.read_with(cx, |view, _| {
        view.clip_content_cache
            .entries
            .get(&source)
            .expect("selected source")
            .clone()
    });
    assert_eq!(current.entity_id(), previous.entity_id());
    let builds = current.read_with(cx, |content, _| content.builds);
    assert_eq!(builds, previous_builds + 1);
    visual.update(|window, cx| {
        editor.update(cx, |_, cx| cx.notify());
        window.draw(cx).clear();
    });
    assert_eq!(
        current.read_with(cx, |content, _| content.builds),
        builds,
        "unchanged following draw must reuse newly seeded cache ranges"
    );
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.viewport = Some(Viewport {
                start: 24_000,
                length: 192_000,
            });
            cx.notify();
        });
        window.draw(cx).clear();
    });
    let zoomed_bounds = visual
        .debug_bounds(selector)
        .expect("current viewport clip bounds");
    assert!(
        (f32::from(zoomed_bounds.size.width) - f32::from(original_bounds.size.width) * 1.5).abs()
            <= 1.0,
        "the first viewport-change draw paints the cropped 72k/192k interval"
    );
    let zoomed = editor.read_with(cx, |view, _| {
        view.clip_content_cache
            .entries
            .get(&source)
            .expect("zoomed source")
            .clone()
    });
    assert_eq!(zoomed.entity_id(), current.entity_id());
    assert_eq!(
        zoomed.read_with(cx, |content, _| content.builds),
        builds + 1
    );
    visual.update(|window, cx| {
        editor.update(cx, |_, cx| cx.notify());
        window.draw(cx).clear();
    });
    assert_eq!(
        zoomed.read_with(cx, |content, _| content.builds),
        builds + 1,
        "viewport invalidation must not queue a redundant following-frame rebuild"
    );
}

#[gpui::test]
fn cold_main_selection_reuses_clip_scene_with_current_geometry(cx: &mut TestAppContext) {
    use gpui::{MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, px};
    let (project, source, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    // The window helper may paint during root replacement. Recreate the cold
    // lane/cache state of project replacement, then draw exactly once without
    // parking the executor or settling another frame.
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.clip_content_cache = TrackCache::default();
            view.drag_lanes.clear();
            view.drag_lane_epochs.clear();
            cx.notify();
        });
        window.draw(cx).clear();
    });
    let content = editor.read_with(cx, |view, _| {
        view.clip_content_cache.entries[&source].clone()
    });
    let builds = content.read_with(cx, |content, _| content.builds);
    assert_eq!(builds, 1, "one cold child render");
    let clip_selector = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    let clip_bounds = visual.debug_bounds(clip_selector).expect("cold clip scene");
    editor.read_with(cx, |view, _| {
        let lane = view.drag_lanes[&source];
        assert!((f32::from(clip_bounds.origin.x) - lane.left).abs() <= 1.0);
        assert!((f32::from(clip_bounds.size.width) - lane.width * 0.25).abs() <= 1.0);
    });
    let trim_selector = Box::leak(format!("clip-trim-right-{}", clip.value()).into_boxed_str());
    let handle_bounds = visual
        .debug_bounds(trim_selector)
        .expect("cold trim handle");
    assert!((f32::from(handle_bounds.right() - clip_bounds.right())).abs() <= 1.0);
    let lane = visual.debug_bounds("main-track-lane").expect("main lane");
    let at = |fraction| point(lane.origin.x + lane.size.width * fraction, lane.center().y);
    visual.update(|window, cx| {
        window.dispatch_input(
            PlatformInput::MouseDown(MouseDownEvent {
                position: at(0.3),
                button: MouseButton::Left,
                ..Default::default()
            }),
            cx,
        );
    });
    let selector = Box::leak(format!("edit-time-selection-{}", source.value()).into_boxed_str());
    let mut previous_width = None;
    for fraction in [0.4, 0.6] {
        visual.update(|window, cx| {
            window.dispatch_input(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at(fraction),
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
            window.draw(cx).clear();
        });
        let overlay = visual
            .debug_bounds(selector)
            .expect("current selection paint");
        if let Some(width) = previous_width {
            assert!(
                overlay.size.width > width,
                "overlay follows each pointer update"
            );
        }
        previous_width = Some(overlay.size.width);
        assert_eq!(
            content.read_with(cx, |content, _| content.builds),
            builds,
            "immediate selection must reuse the first cold clip scene"
        );
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.drag_lane_epochs.get(&source),
                Some(&view.drag_layout_generation)
            );
            assert!(view.time_selection_project_scope);
        });
    }
    visual.update(|window, cx| {
        window.dispatch_input(
            PlatformInput::MouseUp(MouseUpEvent {
                position: at(0.6),
                button: MouseButton::Left,
                ..Default::default()
            }),
            cx,
        );
        window.draw(cx).clear();
    });
    assert!(
        visual.debug_bounds(selector).is_some(),
        "completed overlay stays live"
    );
    assert_eq!(content.read_with(cx, |content, _| content.builds), builds);
    // The first scene's cached handle still captures an ordinary trim drag.
    let down = crate::editor::clip_drag_ui_test::trim_grab_point(
        visual
            .debug_bounds(trim_selector)
            .expect("reused trim handle"),
    );
    visual.simulate_mouse_move(down, None, Modifiers::default());
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x - px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    assert!(
        cx.update(|app| app.has_active_drag()),
        "cached cold trim handle captures drag"
    );
    editor.read_with(cx, |view, _| {
        let drag = view
            .active_trim_drag
            .as_ref()
            .expect("current trim gesture");
        assert_eq!(drag.source_track, source);
        assert!(view.trim_drag_current(drag));
    });
}

#[gpui::test]
fn track_color_repaints_inherited_clips_while_flavor_stays_independent(cx: &mut TestAppContext) {
    use hex_project::{ClipFlavor, Command, Editor, ElementColor};
    let (project, track, _, first) = super::super::clip_drag_ui_test::audio_project();
    let mut model = Editor::new(project, 8).unwrap();
    model.execute(Command::DuplicateClip(first)).unwrap();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, model.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for (index, command) in [
        Command::SetTrackColor {
            track,
            color: ElementColor::Cyan,
        },
        Command::SetClipFlavor {
            clip: first,
            flavor: ClipFlavor::Accent,
        },
        Command::SetTrackColor {
            track,
            color: ElementColor::Orange,
        },
    ]
    .into_iter()
    .enumerate()
    {
        model.execute(command).unwrap();
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                publish_snapshot(view, Arc::new(model.project().clone()), index as u64 + 2);
                cx.notify();
            });
            window.draw(cx).clear();
        });
        let owner = model.project().track(track).unwrap();
        for clip in owner.clips() {
            let selector = Box::leak(format!("clip-visual-{}", clip.id().value()).into_boxed_str());
            assert_eq!(
                visual.rendered_background(selector),
                Some(gpui::rgb(theme::clip_fill(owner.color(), clip.flavor(), false)).into())
            );
        }
    }
}

#[gpui::test]
fn deep_element_id_paths_keep_distinct_click_state_and_focus_across_repaints(
    cx: &mut TestAppContext,
) {
    use gpui::{Context, FocusHandle, Render, Window, div, prelude::*, px, rgb};
    struct IdentityFixture {
        depth: usize,
        counts: [usize; 2],
        focus: [FocusHandle; 2],
    }
    impl Render for IdentityFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
            let mut row = div().flex().pl(px(theme::TITLEBAR_LEADING_INSET));
            for index in 0..2 {
                let control = div()
                    // Identical leaf names remain distinct through their full
                    // ancestor path, including ancestors past inline storage.
                    .id("leaf")
                    .debug_selector(move || format!("identity-leaf-{index}"))
                    .track_focus(&self.focus[index])
                    .w(px(160.0))
                    .h(px(40.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Grid)))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.counts[index] += 1;
                        view.focus[index].focus(window);
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(move |view, event: &gpui::KeyDownEvent, _, cx| {
                        if event.keystroke.key == "up" {
                            view.counts[index] += 1;
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .child(theme::text(format!(
                        "identity {index} · {}",
                        self.counts[index]
                    )));
                row = row.child(div().id(("branch", index)).child(control));
            }
            let mut content = row.into_any_element();
            for level in 0..self.depth {
                content = div()
                    .id(("ancestor", level))
                    .w_full()
                    .child(content)
                    .into_any_element();
            }
            content
        }
    }
    for depth in [0, 40, 80] {
        let handle = cx.add_window(|_, cx| IdentityFixture {
            depth,
            counts: [0; 2],
            focus: std::array::from_fn(|_| cx.focus_handle()),
        });
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.run_until_parked();
        let left = visual.debug_bounds("identity-leaf-0").unwrap().center();
        let right = visual.debug_bounds("identity-leaf-1").unwrap().center();
        visual.simulate_click(left, Modifiers::default());
        visual.simulate_keystrokes("up");
        visual.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.counts), [2, 0]);
        for _ in 0..3 {
            view.update(cx, |_, cx| cx.notify());
            visual.run_until_parked();
        }
        visual.update(|window, cx| assert!(view.read(cx).focus[0].is_focused(window)));
        visual.simulate_click(right, Modifiers::default());
        visual.simulate_keystrokes("up");
        visual.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.counts), [2, 2]);
        visual.update(|window, cx| assert!(view.read(cx).focus[1].is_focused(window)));
        // GPUI must find the same pending click after rebuilding all ancestors;
        // retaining just entity-local counters would not establish this.
        visual.simulate_mouse_down(left, MouseButton::Left, Modifiers::default());
        view.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        visual.simulate_mouse_up(left, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.counts), [3, 2]);
    }
}

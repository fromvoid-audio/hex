//! Bounded cached legacy clip content; lane geometry and selection overlays stay live.
use super::{
    EditorView, ViewState,
    drag::LaneBounds,
    musical_time::{GridLine, SnapGrid},
    tracks::ClipViewTimeline,
    viewport::Viewport,
};
use crate::theme;
use gpui::{
    AnyElement, AnyView, Context, Entity, IntoElement, Render, WeakEntity, Window, div, prelude::*,
};
use hex_project::{ClipId, Track, TrackId};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, PartialEq)]
struct ContentKey {
    generation: u64,
    revision: u64,
    track: TrackId,
    track_index: usize,
    viewport: Viewport,
    grid: Vec<GridLine>,
    snap: SnapGrid,
    scale: u32,
    palette: hex_preferences::ThemePreferences,
    selected_clip: Option<ClipId>,
    lane: Option<LaneBounds>,
    drag_available: bool,
}

#[derive(Default)]
pub(super) struct TrackCache {
    generation: u64,
    snapshot: Option<Arc<ViewState>>,
    entries: HashMap<TrackId, Entity<ClipContents>>,
}
impl TrackCache {
    pub(super) fn reconcile(&mut self, generation: u64, snapshot: Option<&Arc<ViewState>>) {
        let same_snapshot = self
            .snapshot
            .as_ref()
            .zip(snapshot)
            .is_some_and(|(old, current)| {
                Arc::ptr_eq(old, current) && old.revision == current.revision
            });
        if self.generation != generation || !same_snapshot {
            self.entries.clear();
            self.snapshot = snapshot.cloned();
            self.generation = generation;
        }
    }
}
struct ClipContents {
    parent: WeakEntity<EditorView>,
    snapshot: Arc<ViewState>,
    key: ContentKey,
    #[cfg(test)]
    builds: usize,
}
impl Render for ClipContents {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.builds += 1;
        }
        super::render_diagnostic::record(
            super::render_diagnostic::Kind::ClipContentBuild,
            std::time::Duration::ZERO,
        );
        self.parent
            .update(cx, |view, cx| {
                if view.project_generation != self.key.generation
                    || !view
                        .state
                        .as_ref()
                        .is_some_and(|state| Arc::ptr_eq(state, &self.snapshot))
                {
                    return div().size_full();
                }
                let Some(track) = self
                    .snapshot
                    .project
                    .tracks()
                    .get(self.key.track_index)
                    .filter(|track| track.id() == self.key.track)
                else {
                    return div().size_full();
                };
                view.build_clip_contents(
                    &self.snapshot,
                    track,
                    self.key.track_index,
                    ClipViewTimeline {
                        viewport: self.key.viewport,
                        grid: &self.key.grid,
                        scale_factor: f32::from_bits(self.key.scale),
                    },
                    cx,
                )
            })
            .unwrap_or_else(|_| div().size_full())
    }
}
impl EditorView {
    fn clip_content_cache_allowed(&self) -> bool {
        !self.edit_pending()
            && !self.opening_project
            && !self.close_prompt_open
            && !self.save_dialog_open
            && !self.export_dialog_open
            && self.context_menu.is_none()
            && self.active_clip_drag.is_none()
            && self.active_browser_audio_drag.is_none()
            && self.active_native_device_drag.is_none()
            && self.active_trim_drag.is_none()
            && self.active_midi_trim_drag.is_none()
            && self.time_range_move_gesture.is_none()
            && self.pending_time_range_move.is_none()
            && self.pointer_down.is_none()
            && self.arrangement_pan.is_none()
            && self.loop_rail_gesture.is_none()
            && !self.theme_settings.open
    }
    pub(super) fn clip_content_element(
        &mut self,
        state: &ViewState,
        track: &Track,
        track_index: usize,
        timeline: ClipViewTimeline<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let snapshot = self
            .state
            .as_ref()
            .filter(|snapshot| std::ptr::eq(snapshot.as_ref(), state))
            .cloned();
        if !self.clip_content_cache_allowed()
            || snapshot.is_none()
            || (!self.clip_content_cache.entries.contains_key(&track.id())
                && self.clip_content_cache.entries.len() >= hex_project::MAX_TRACKS)
        {
            self.clip_content_cache.entries.remove(&track.id());
            return self
                .build_clip_contents(state, track, track_index, timeline, cx)
                .into_any_element();
        }
        let Some(snapshot) = snapshot else {
            return div().into_any_element();
        };
        let key = ContentKey {
            generation: self.project_generation,
            revision: state.revision,
            track: track.id(),
            track_index,
            viewport: timeline.viewport,
            grid: timeline.grid.to_vec(),
            snap: self.snap_grid,
            scale: timeline.scale_factor.to_bits(),
            palette: theme::preferences(),
            selected_clip: self.selected_clip(),
            lane: self.drag_lanes.get(&track.id()).copied(),
            drag_available: self.worker_requests_pending == 0 && self.pending_requests.is_empty(),
        };
        let parent = cx.weak_entity();
        let reusable = self
            .clip_content_cache
            .entries
            .get(&track.id())
            .filter(|entity| entity.read(cx).key == key)
            .cloned();
        let entity = if let Some(entity) = reusable {
            super::render_diagnostic::record(
                super::render_diagnostic::Kind::ClipContentReuse,
                std::time::Duration::ZERO,
            );
            entity
        } else {
            // Key changes happen during row prepaint, after GPUI drains dirty
            // notifications. A fresh identity misses the scene cache NOW;
            // notifying the old child would instead dirty the following frame.
            // Keep `.cached` on the new view to seed its ranges in this draw.
            let entity = cx.new(|_| ClipContents {
                parent,
                snapshot,
                key,
                #[cfg(test)]
                builds: 0,
            });
            self.clip_content_cache
                .entries
                .insert(track.id(), entity.clone());
            entity
        };
        let mut layout = div().absolute().inset_0();
        AnyView::from(entity)
            .cached(layout.style().clone())
            .into_any_element()
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};

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
            Some(gpui::rgb(theme::color(theme::PaletteRole::ClipSelected)).into()),
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
        assert_ne!(current.entity_id(), previous.entity_id());
        let builds = current.read_with(cx, |content, _| content.builds);
        assert_eq!(builds, 1);
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
            (f32::from(zoomed_bounds.size.width) - f32::from(original_bounds.size.width) * 1.5)
                .abs()
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
        assert_ne!(zoomed.entity_id(), current.entity_id());
        assert_eq!(zoomed.read_with(cx, |content, _| content.builds), 1);
        visual.update(|window, cx| {
            editor.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear();
        });
        assert_eq!(
            zoomed.read_with(cx, |content, _| content.builds),
            1,
            "viewport invalidation must not queue a redundant following-frame rebuild"
        );
    }

    #[gpui::test]
    fn main_selection_reuses_clip_scene_while_overlay_and_lane_epochs_change(
        cx: &mut TestAppContext,
    ) {
        let (project, source, _, _) = super::super::clip_drag_ui_test::audio_project();
        let (handle, editor) =
            super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual.debug_bounds("main-track-lane").expect("main lane");
        let at = |fraction| point(lane.origin.x + lane.size.width * fraction, lane.center().y);
        visual.simulate_mouse_down(at(0.2), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let content = editor.read_with(cx, |view, _| {
            view.clip_content_cache
                .entries
                .get(&source)
                .expect("cached source")
                .clone()
        });
        let builds = content.read_with(cx, |content, _| content.builds);
        assert!(builds > 0, "actual child scene was built");
        let selector =
            Box::leak(format!("edit-time-selection-{}", source.value()).into_boxed_str());
        let mut previous_width = None;
        for fraction in [0.4, 0.6] {
            visual.simulate_mouse_move(at(fraction), MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let overlay = visual
                .debug_bounds(selector)
                .expect("live project selection overlay");
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
                "selection changes must reuse the real cached clip scene"
            );
            editor.read_with(cx, |view, _| {
                assert_eq!(
                    view.drag_lane_epochs.get(&source),
                    Some(&view.drag_layout_generation),
                    "lane geometry remains current outside cached content"
                );
                assert!(view.time_selection_project_scope);
            });
        }
        visual.simulate_mouse_up(at(0.6), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(selector).is_some(),
            "completed overlay remains visible"
        );
        assert_eq!(content.read_with(cx, |content, _| content.builds), builds);
    }
}

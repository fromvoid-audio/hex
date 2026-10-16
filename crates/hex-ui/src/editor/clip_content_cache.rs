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
    pub(super) fn reconcile(
        &mut self,
        generation: u64,
        snapshot: Option<&Arc<ViewState>>,
        cx: &mut Context<EditorView>,
    ) {
        let same_snapshot = self
            .snapshot
            .as_ref()
            .zip(snapshot)
            .is_some_and(|(old, current)| {
                Arc::ptr_eq(old, current) && old.revision == current.revision
            });
        if self.generation != generation || snapshot.is_none() {
            self.entries.clear();
        } else if !same_snapshot && let Some(snapshot) = snapshot {
            let indices = snapshot
                .project
                .tracks()
                .iter()
                .enumerate()
                .map(|(index, track)| (track.id(), index))
                .collect::<HashMap<_, _>>();
            self.entries.retain(|track, entity| {
                let Some(index) = indices.get(track) else {
                    return false;
                };
                entity.update(cx, |content, _| {
                    // Retain identity/capture, but never retain older immutable
                    // snapshots in offscreen children after a worker update.
                    content.snapshot = Arc::clone(snapshot);
                    content.key.track_index = *index;
                    content.key.revision = snapshot.revision;
                    content.epoch = content.epoch.wrapping_add(1);
                });
                true
            });
        }
        self.snapshot = snapshot.cloned();
        self.generation = generation;
    }
}
struct ClipContents {
    parent: WeakEntity<EditorView>,
    snapshot: Arc<ViewState>,
    key: ContentKey,
    epoch: u64,
    fresh_last: bool,
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
        if snapshot.is_none()
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
        let fresh = !self.clip_content_cache_allowed();
        let entity = if let Some(entity) = self.clip_content_cache.entries.get(&track.id()).cloned()
        {
            entity.update(cx, |content, _| {
                if content.key != key || fresh || content.fresh_last {
                    content.key = key;
                    content.epoch = content.epoch.wrapping_add(1);
                } else {
                    super::render_diagnostic::record(
                        super::render_diagnostic::Kind::ClipContentReuse,
                        std::time::Duration::ZERO,
                    );
                }
                content.fresh_last = fresh;
            });
            entity
        } else {
            let entity = cx.new(|_| ClipContents {
                parent,
                snapshot,
                key,
                epoch: 0,
                fresh_last: fresh,
                #[cfg(test)]
                builds: 0,
            });
            self.clip_content_cache
                .entries
                .insert(track.id(), entity.clone());
            entity
        };
        // The epoch invalidates THIS draw without changing input element identity
        // or queuing a notification that GPUI would consume on the next frame.
        let epoch = entity.read(cx).epoch;
        let mut layout = div().absolute().inset_0();
        AnyView::from(entity)
            .cached_with_epoch(layout.style().clone(), epoch)
            .into_any_element()
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "clip_content_cache_ui_test.rs"]
mod tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "clip_content_profile.rs"]
mod profile;

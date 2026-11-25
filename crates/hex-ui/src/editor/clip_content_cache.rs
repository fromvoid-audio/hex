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
    grid: std::rc::Rc<[GridLine]>,
    snap: SnapGrid,
    scale: u32,
    palette: hex_preferences::ThemePreferences,
    selected_clip: Option<ClipId>,
    selected_clips_epoch: u64,
    lane: Option<LaneBounds>,
    drag_available: bool,
    fade_key_held: bool,
    fade_handle_hover: Option<(ClipId, super::fade_handles::Handle)>,
    hovered_fade_clip: Option<ClipId>,
    hovered_arrangement_clip: Option<super::track_lane_canvas::HoveredArrangementClip>,
    option_held: bool,
    time_selection_key_held: bool,
    automation_view: bool,
    focus_ranges: std::rc::Rc<[(u64, u64)]>,
    waveform_overrange_epoch: u64,
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

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn build_count(&self, track: TrackId, cx: &gpui::App) -> Option<usize> {
        self.entries
            .get(&track)
            .map(|entity| entity.read(cx).builds)
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn epoch(&self, track: TrackId, cx: &gpui::App) -> Option<u64> {
        self.entries.get(&track).map(|entity| entity.read(cx).epoch)
    }
}
struct ClipContents {
    parent: WeakEntity<EditorView>,
    snapshot: Arc<ViewState>,
    key: ContentKey,
    epoch: u64,
    fresh_last: bool,
    /// GPUI bakes an active drag's window cursor style into any cached view
    /// painted during the drag; invalidate once after the drag ends so stale
    /// drag cursors cannot replay into idle frames.
    painted_with_active_drag: bool,
    #[cfg(test)]
    builds: usize,
}
impl Render for ClipContents {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.builds += 1;
        }
        self.painted_with_active_drag = cx.has_active_drag();
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
                // Lane prepaint runs after key construction. Remember the
                // geometry consumed by this build so the next selection-only
                // draw does not invalidate an already-current clip scene.
                self.key.lane = view.drag_lanes.get(&self.key.track).copied();
                let caption_rem_size = (self.key.lane.is_some()
                    && view.drag_lane_epochs.get(&self.key.track)
                        == Some(&view.drag_layout_generation))
                .then(|| f32::from(window.rem_size()));
                view.build_clip_contents(
                    &self.snapshot,
                    track,
                    self.key.track_index,
                    ClipViewTimeline {
                        viewport: self.key.viewport,
                        grid: &self.key.grid,
                        scale_factor: f32::from_bits(self.key.scale),
                        caption_rem_size,
                    },
                    cx,
                )
            })
            .unwrap_or_else(|_| div().size_full())
    }
}
fn track_contains_clip(track: &Track, clip: ClipId) -> bool {
    track.clips().iter().any(|candidate| candidate.id() == clip)
        || track
            .midi_clips()
            .iter()
            .any(|candidate| candidate.id() == clip)
}

fn clip_drag_affects_track(drag: &super::drag::ClipDrag, track: &Track) -> bool {
    let track_id = track.id();
    if drag.source_track == track_id {
        return true;
    }
    if drag
        .preview
        .get()
        .is_some_and(|preview| preview.target_track == track_id)
    {
        return true;
    }
    if matches!(
        drag.destination.get(),
        Some(super::drag::ClipDropDestination::ExistingTrack(dest)) if dest == track_id
    ) {
        return true;
    }
    let group = drag.group.borrow();
    if group
        .previews()
        .iter()
        .any(|preview| preview.track == track_id)
    {
        return true;
    }
    group
        .ids
        .iter()
        .any(|&clip_id| track_contains_clip(track, clip_id))
}

fn browser_drag_affects_track(drag: &super::drag::BrowserAudioDrag, track_id: TrackId) -> bool {
    drag.preview
        .get()
        .is_some_and(|preview| preview.target_track == track_id)
        || matches!(
            drag.destination.get(),
            Some(super::drag::BrowserDropDestination::ExistingTrack { track, .. })
                if track == track_id
        )
}

impl EditorView {
    /// Global UI states that prevent reusing cached `ClipContents` scenes on any track.
    fn clip_content_cache_globally_allowed(&self) -> bool {
        !self.persistence.opening_project
            && !self.persistence.close_prompt_open
            && !self.persistence.save_dialog_open
            && !self.persistence.export_dialog_open
            && self.context_menu.is_none()
            && self.loop_rail_gesture.is_none()
            && !self.settings.theme.open
    }

    /// True when `track` has no active or in-flight preview that mutates its
    /// `ClipContents` scene, allowing untouched tracks to replay cached scenes
    /// during single-track clip drags, trims, fades, renames, or edits.
    fn clip_content_cache_allowed_for(&self, track: &Track, track_index: usize) -> bool {
        if !self.clip_content_cache_globally_allowed() {
            return false;
        }
        let track_id = track.id();
        if let Some(edit) = self.clip_name_edit.as_ref()
            && track_contains_clip(track, edit.clip)
        {
            return false;
        }
        if let Some(drag) = self.active_clip_drag.as_ref()
            && clip_drag_affects_track(drag, track)
        {
            return false;
        }
        if let Some(pending) = self.pending_clip_move.as_ref()
            && clip_drag_affects_track(&pending.drag, track)
        {
            return false;
        }
        if let Some(drag) = self.browser.active_audio_drag.as_ref()
            && browser_drag_affects_track(drag, track_id)
        {
            return false;
        }
        if self
            .active_fade_drag
            .as_ref()
            .is_some_and(|drag| drag.track == track_id)
            || self
                .pending_fade
                .as_ref()
                .is_some_and(|pending| pending.track() == track_id)
        {
            return false;
        }
        if self.clip_gain.affects_track(track, &self.clip_selection) {
            return false;
        }
        if let Some((clip, _, _)) = self.audio_detail.clip_controls.gain_preview(self)
            && (track_contains_clip(track, clip)
                || (self.clip_selection.contains(clip)
                    && track
                        .clips()
                        .iter()
                        .any(|candidate| self.clip_selection.contains(candidate.id()))))
        {
            return false;
        }
        if let Some(clip) = self.audio_detail.clip_controls.play_start_preview_clip_id()
            && track_contains_clip(track, clip)
        {
            return false;
        }
        if self.shows_audio_clip_loop_preview()
            && self
                .detail_clip_id()
                .is_none_or(|clip| track_contains_clip(track, clip))
        {
            return false;
        }
        if self.shows_midi_clip_loop_preview()
            && self
                .detail_clip_id()
                .is_none_or(|clip| track_contains_clip(track, clip))
        {
            return false;
        }
        if self.held_audio_play_start_position().is_some()
            && self
                .detail_clip_id()
                .is_none_or(|clip| track_contains_clip(track, clip))
        {
            return false;
        }
        if self
            .active_trim_drag
            .as_ref()
            .is_some_and(|drag| drag.source_track == track_id)
            || self
                .pending_trim
                .as_ref()
                .is_some_and(|pending| pending.track == track_id)
            || self
                .midi_editor
                .active_trim_drag
                .as_ref()
                .is_some_and(|drag| drag.source_track == track_id)
        {
            return false;
        }
        if let Some(display) = self.time_range_move_display() {
            let in_source = display.source.tracks().contains(&track_id);
            let in_dest = track_index >= display.destination_first_index
                && track_index <= display.destination_last_index;
            if in_source || in_dest {
                return false;
            }
        } else if self.time_range_move_gesture.is_some() || self.pending_time_range_move.is_some() {
            let in_pending = self
                .pending_time_range_move
                .as_ref()
                .is_some_and(|pending| {
                    pending.source.tracks().contains(&track_id)
                        || pending.range.tracks().contains(&track_id)
                        || (track_index >= pending.destination_first_index
                            && track_index <= pending.destination_last_index)
                });
            if in_pending || self.time_range_move_gesture.is_some() {
                return false;
            }
        }
        true
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
            grid: std::rc::Rc::clone(timeline.grid),
            snap: self.snap_grid,
            scale: timeline.scale_factor.to_bits(),
            palette: theme::preferences(),
            selected_clip: self.selected_clip(),
            selected_clips_epoch: self.clip_selection.epoch,
            lane: self.drag_lanes.get(&track.id()).copied(),
            fade_key_held: self.fade_key_held,
            fade_handle_hover: self
                .hovered_fade_handle
                .and_then(|hover| hover.in_track(self.project_generation, track.id())),
            hovered_fade_clip: self
                .hovered_fade_clip
                .filter(|(generation, clip)| {
                    *generation == self.project_generation
                        && track
                            .clips()
                            .iter()
                            .any(|candidate| candidate.id() == *clip)
                })
                .map(|(_, clip)| clip),
            hovered_arrangement_clip: self.hovered_arrangement_clip.filter(|hover| {
                hover.generation == self.project_generation && hover.track == track.id()
            }),
            option_held: self.option_held,
            time_selection_key_held: self.time_selection_key_held,
            automation_view: self.automation_view.enabled,
            focus_ranges: self.arrangement_focus_ranges(),
            waveform_overrange_epoch: self.waveform_overrange_epoch,
            // Match clip_view admission, including physically held track sizing.
            // Otherwise a body painted without controls can survive release.
            drag_available: self.drag_available(),
        };
        let parent = cx.weak_entity();
        let fresh = !self.clip_content_cache_allowed_for(track, track_index)
            || self
                .held_arrangement_event_clip()
                .is_some_and(|clip| track.clips().iter().any(|candidate| candidate.id() == clip));
        let has_active_drag = cx.has_active_drag();
        let entity = if let Some(entity) = self.clip_content_cache.entries.get(&track.id()).cloned()
        {
            entity.update(cx, |content, _| {
                let stale_drag_cursor = content.painted_with_active_drag && !has_active_drag;
                if content.key != key || fresh || content.fresh_last || stale_drag_cursor {
                    content.key = key;
                    content.epoch = content.epoch.wrapping_add(1);
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
                painted_with_active_drag: has_active_drag,
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

#[cfg(all(test, not(feature = "ui-test-support")))]
mod unit_tests {
    #[test]
    fn element_identity_keys_use_the_complete_path_independent_of_storage() {
        use gpui::{ElementId, GlobalElementId};
        use std::{
            collections::hash_map::DefaultHasher,
            hash::{Hash, Hasher},
        };
        let hash = |key: &GlobalElementId| {
            let mut hasher = DefaultHasher::new();
            key.hash(&mut hasher);
            hasher.finish()
        };
        for depth in [0, 1, 8, 9, 32, 33, 80] {
            let path = (0..depth)
                .map(|index| ElementId::Integer(index as u64))
                .collect::<Vec<_>>();
            let mut ordinary = GlobalElementId::default();
            let mut reserved = GlobalElementId::default();
            // Force dynamic storage even for an empty/shallow identity. Hashing and
            // equality must depend on the path, never its current allocation mode.
            reserved.reserve(128);
            for id in &path {
                ordinary.push(id.clone());
                reserved.push(id.clone());
            }
            assert_eq!(ordinary.as_slice(), path.as_slice());
            assert_eq!(ordinary, reserved);
            assert_eq!(hash(&ordinary), hash(&reserved));
            assert_eq!(
                ordinary.to_string(),
                path.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(".")
            );
            reserved.push(ElementId::Integer(999));
            assert_ne!(
                ordinary, reserved,
                "a suffix beyond inline storage belongs to the identity"
            );
            let mut map = std::collections::HashMap::new();
            map.insert(ordinary, "first");
            map.insert(reserved, "second");
            assert_eq!(map.len(), 2);
        }
    }
}

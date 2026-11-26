//! Bounded per-track cached Arrangement header views.
//!
//! Track headers (~35-45 GPUI widgets each) do not depend on horizontal timeline
//! viewport, grid changes, or 60 Hz meter levels. Caching each track's header
//! with `AnyView::cached_with_epoch` while overlaying its lightweight stereo
//! meter keeps all 32+ track headers 100% cache-hit across playback meter ticks,
//! horizontal pan, zoom, and idle frames, eliminating ~1,200+ widget builds,
//! CoreText track-name shapings, and Taffy flex layouts per frame.
use super::{EditorView, ViewState};
use crate::theme;
use gpui::{
    AnyElement, AnyView, Context, Entity, IntoElement, Render, WeakEntity, Window, div, prelude::*,
    px,
};
use hex_project::{Track, TrackId};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, PartialEq)]
struct HeaderKey {
    generation: u64,
    revision: u64,
    track: TrackId,
    track_index: usize,
    header_width: u32,
    header_height: u32,
    scale: u32,
    palette: hex_preferences::ThemePreferences,
    selected: bool,
    track_selected: bool,
    folded: bool,
    hierarchy_indent: u32,
    ancestors_selected_mask: u64,
    branch_ends_mask: u64,
    own_branch_ends: bool,
    muted_soloed: (bool, bool),
    audio_armed: bool,
    sample_armed: bool,
    resident_layout: bool,
    show_track_latency: bool,
    send_menu_open: bool,
    io_menu_open: Option<super::track_io::TrackIoMenuKind>,
    displayed_gain_bits: Option<u32>,
}

#[derive(Default)]
pub(super) struct TrackHeaderCache {
    generation: u64,
    snapshot: Option<Arc<ViewState>>,
    entries: HashMap<TrackId, Entity<TrackHeaderView>>,
}

impl TrackHeaderCache {
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
                entity.update(cx, |header, _| {
                    header.snapshot = Arc::clone(snapshot);
                    header.key.track_index = *index;
                    header.key.revision = snapshot.revision;
                    header.epoch = header.epoch.wrapping_add(1);
                });
                true
            });
        }
        self.snapshot = snapshot.cloned();
        self.generation = generation;
    }
}

struct TrackHeaderView {
    parent: WeakEntity<EditorView>,
    snapshot: Arc<ViewState>,
    key: HeaderKey,
    epoch: u64,
    fresh_last: bool,
    /// GPUI bakes an active drag's window cursor style into any cached view
    /// painted during the drag; invalidate once after the drag ends so stale
    /// drag cursors cannot replay into idle frames.
    painted_with_active_drag: bool,
    #[cfg(all(test, feature = "ui-test-support"))]
    render_count: usize,
}

impl Render for TrackHeaderView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(test, feature = "ui-test-support"))]
        {
            self.render_count = self.render_count.saturating_add(1);
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
                    return div().size_full().into_any_element();
                }
                let Some(track) = self
                    .snapshot
                    .project
                    .tracks()
                    .get(self.key.track_index)
                    .filter(|track| track.id() == self.key.track)
                else {
                    return div().size_full().into_any_element();
                };
                view.build_track_header_with_meter(
                    &self.snapshot,
                    track,
                    self.key.track_index,
                    false,
                    cx,
                )
                .into_any_element()
            })
            .unwrap_or_else(|_| div().size_full().into_any_element())
    }
}

impl EditorView {
    fn arrangement_device_preview_affects(&self, track_id: TrackId) -> bool {
        self.device_panel
            .arrangement_drop_preview
            .is_some_and(|preview| match preview.target {
                super::arrangement_device_drop::ArrangementDropTarget::ExistingTrack {
                    track,
                    ..
                } => track == track_id,
                super::arrangement_device_drop::ArrangementDropTarget::NewTrack {
                    before_track,
                    ..
                } => before_track == Some(track_id),
            })
    }

    fn track_header_cache_allowed_for(&self, track_id: TrackId, cx: &gpui::App) -> bool {
        self.track_name.editing_track(self.project_generation) != Some(track_id)
            && !self.track_resize_affects(track_id)
            && !self.track_reorder_active()
            && !self.track_gain_controls.has_pending_draft_for(track_id, cx)
            && !self.persistence.opening_project
            && !self.persistence.close_prompt_open
            && !self.persistence.save_dialog_open
            && !self.persistence.export_dialog_open
            && self.context_menu.is_none()
            && self.device_panel.active_native_drag.is_none()
            && self.browser.active_audio_drag.is_none()
            && !self.arrangement_device_preview_affects(track_id)
            && !self.settings.theme.open
            && !self.track_performance.enabled
            && !self.show_track_latency
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn track_header_render_count(&self, track_id: TrackId, cx: &gpui::App) -> usize {
        self.timeline_caches
            .track_header_cache
            .entries
            .get(&track_id)
            .map_or(0, |entity| entity.read(cx).render_count)
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn track_header_epoch(&self, track_id: TrackId, cx: &gpui::App) -> Option<u64> {
        self.timeline_caches
            .track_header_cache
            .entries
            .get(&track_id)
            .map(|entity| entity.read(cx).epoch)
    }

    pub(super) fn track_header_element(
        &mut self,
        state: &ViewState,
        track: &Track,
        track_index: usize,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let track_id = track.id();
        let snapshot = self
            .state
            .as_ref()
            .filter(|snapshot| std::ptr::eq(snapshot.as_ref(), state))
            .cloned();
        if snapshot.is_none()
            || (!self
                .timeline_caches
                .track_header_cache
                .entries
                .contains_key(&track_id)
                && self.timeline_caches.track_header_cache.entries.len() >= hex_project::MAX_TRACKS)
        {
            self.timeline_caches
                .track_header_cache
                .entries
                .remove(&track_id);
            return self
                .build_track_header(state, track, track_index, cx)
                .into_any_element();
        }
        let Some(snapshot) = snapshot else {
            return div().into_any_element();
        };
        let header_width = self.track_header_width();
        let header_height = self.track_body_height(track_id);
        let next_ancestors = self
            .arrangement_row_index(track_index)
            .and_then(|row| self.arrangement_track_index(row + 1))
            .and_then(|index| state.project.tracks().get(index))
            .map_or(&[][..], |next| {
                self.track_hierarchy_indent.ancestors(next.id())
            });
        let branch_ends = |ancestor| !next_ancestors.iter().any(|(id, _)| *id == ancestor);
        let own_branch_ends = branch_ends(track_id);
        let mut ancestors_selected_mask = 0u64;
        let mut branch_ends_mask = 0u64;
        for (depth, &(ancestor, _)) in self
            .track_hierarchy_indent
            .ancestors(track_id)
            .iter()
            .take(64)
            .enumerate()
        {
            if self.track_selection.contains(ancestor) {
                ancestors_selected_mask |= 1u64 << depth;
            }
            if branch_ends(ancestor) {
                branch_ends_mask |= 1u64 << depth;
            }
        }
        let sample_source =
            super::sample_recorder::Arm::Output(hex_engine::OutputCaptureId::Track(track_id));
        let key = HeaderKey {
            generation: self.project_generation,
            revision: state.revision,
            track: track_id,
            track_index,
            header_width: header_width.to_bits(),
            header_height: header_height.to_bits(),
            scale: scale_factor.to_bits(),
            palette: theme::preferences(),
            selected: self.selected_track() == Some(track_id),
            track_selected: self.track_selection.contains(track_id),
            folded: self.arrangement_focus.projection.folded(track_id),
            hierarchy_indent: self.track_hierarchy_indent.pixels(track_id).to_bits(),
            ancestors_selected_mask,
            branch_ends_mask,
            own_branch_ends,
            muted_soloed: self.displayed_track_switches(track),
            audio_armed: self
                .audio_recorder
                .is_audio_armed(self.project_generation, track_id),
            sample_armed: self
                .device_panel
                .sample_recorder
                .is_armed(self.project_generation, sample_source),
            resident_layout: self.audio_pub.document_audio_mode
                == hex_session::DocumentAudioMode::Resident,
            show_track_latency: self.show_track_latency,
            send_menu_open: self.session_grid.send_menu_track == Some(track_id),
            io_menu_open: self
                .track_io
                .menu
                .filter(|menu| menu.track == track_id && menu.generation == self.project_generation)
                .map(|menu| menu.kind),
            displayed_gain_bits: self
                .track_gain_controls
                .displayed_track_gain(self.project_generation, track_id)
                .or_else(|| {
                    self.device_panel
                        .gesture
                        .preview_track_gain(self.project_generation, track_id)
                })
                .map(f32::to_bits),
        };
        let parent = cx.weak_entity();
        let fresh = !self.track_header_cache_allowed_for(track_id, cx);
        let has_active_drag = cx.has_active_drag();
        let entity = if let Some(entity) = self
            .timeline_caches
            .track_header_cache
            .entries
            .get(&track_id)
            .cloned()
        {
            entity.update(cx, |header, _| {
                let stale_drag_cursor = header.painted_with_active_drag && !has_active_drag;
                if header.key != key || fresh || header.fresh_last || stale_drag_cursor {
                    header.key = key;
                    header.epoch = header.epoch.wrapping_add(1);
                }
                header.fresh_last = fresh;
            });
            entity
        } else {
            let entity = cx.new(|_| TrackHeaderView {
                parent,
                snapshot,
                key,
                epoch: 0,
                fresh_last: fresh,
                painted_with_active_drag: has_active_drag,
                #[cfg(all(test, feature = "ui-test-support"))]
                render_count: 0,
            });
            self.timeline_caches
                .track_header_cache
                .entries
                .insert(track_id, entity.clone());
            entity
        };
        let epoch = entity.read(cx).epoch;
        let mut layout = div()
            .w(px(header_width))
            .h(px(header_height))
            .flex_shrink_0();
        let meter_overlay = div().absolute().top_0().right(px(4.0)).children([self
            .meter_slot_or_inline(super::playback_subviews::MeterTarget::ArrangementTrack {
                id: track_id,
                height: header_height,
            })]);
        div()
            .relative()
            .w(px(header_width))
            .h(px(header_height))
            .flex_shrink_0()
            .children([
                AnyView::from(entity)
                    .cached_with_epoch(layout.style().clone(), epoch)
                    .into_any_element(),
                meter_overlay.into_any_element(),
            ])
            .into_any_element()
    }
}

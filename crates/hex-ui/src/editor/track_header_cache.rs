//! Bounded per-track cached Arrangement header views.
//!
//! Track headers (~35-45 GPUI widgets each) do not depend on horizontal timeline
//! viewport or grid changes. Caching each track's header with
//! `AnyView::cached_with_epoch` keeps all 32+ track headers 100% cache-hit
//! across horizontal pan, zoom, and idle frames, eliminating ~1,200+ widget
//! builds, CoreText track-name shapings, and Taffy flex layouts per frame.
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
    meter_level: [u32; 2],
    show_track_latency: bool,
    send_menu_open: bool,
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
}

impl Render for TrackHeaderView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                view.build_track_header(&self.snapshot, track, self.key.track_index, cx)
                    .into_any_element()
            })
            .unwrap_or_else(|_| div().size_full().into_any_element())
    }
}

impl EditorView {
    fn track_header_cache_allowed(&self) -> bool {
        !self.track_name.is_editing()
            && !self.track_resize_active()
            && !self.track_reorder_active()
            && !self.edit_pending()
            && !self.opening_project
            && !self.close_prompt_open
            && !self.save_dialog_open
            && !self.export_dialog_open
            && self.context_menu.is_none()
            && self.active_native_device_drag.is_none()
            && self.arrangement_device_preview.is_none()
            && self.pending_plugin_effect.is_none()
            && !self.theme_settings.open
            && !self.track_performance.enabled
            && !self.show_track_latency
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
            || (!self.track_header_cache.entries.contains_key(&track_id)
                && self.track_header_cache.entries.len() >= hex_project::MAX_TRACKS)
        {
            self.track_header_cache.entries.remove(&track_id);
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
        let level = self.track_meters.level(track_id);
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
                .sample_recorder
                .is_armed(self.project_generation, sample_source),
            resident_layout: self.document_audio_mode == hex_session::DocumentAudioMode::Resident,
            meter_level: [level[0].to_bits(), level[1].to_bits()],
            show_track_latency: self.show_track_latency,
            send_menu_open: self.send_menu_track == Some(track_id),
        };
        let parent = cx.weak_entity();
        let fresh = !self.track_header_cache_allowed()
            || cx.has_active_drag()
            || self.track_gain_controls.has_pending_draft(cx);
        let entity = if let Some(entity) = self.track_header_cache.entries.get(&track_id).cloned() {
            entity.update(cx, |header, _| {
                if header.key != key || fresh || header.fresh_last {
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
            });
            self.track_header_cache
                .entries
                .insert(track_id, entity.clone());
            entity
        };
        let epoch = entity.read(cx).epoch;
        let mut layout = div()
            .w(px(header_width))
            .h(px(header_height))
            .flex_shrink_0();
        AnyView::from(entity)
            .cached_with_epoch(layout.style().clone(), epoch)
            .into_any_element()
    }
}

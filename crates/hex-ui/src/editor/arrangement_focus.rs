//! Read-only Arrangement track visibility over selected canonical section intervals.
//! Rebuild once per document/revision/section selection or group-fold change, never per row.
//! Nonadjacent sections form a union, not a hull. No selection leaves the view
//! unfiltered by section while the mode stays armed. Ancestor groups of retained tracks
//! stay visible; retained tracks expose all their clips,
//! including outside the selected intervals. Edits retain canonical IDs/indices.
//! A newly acknowledged track stays visible while it is the active track. This
//! exception is view-only and ends on selection, section, focus-mode or document change.
//! Fold state is view-only: collapsed groups retain their row while hiding all
//! descendants. Nested folds persist when an outer group is opened; document
//! replacement clears them and deleted group identities are retired.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, Entity, Focusable, Subscription, Window, div, prelude::*, px};
use hex_project::{ClipId, Project, SectionId, TrackId};
use hex_widgets::{Toggle, ToggleAppearance, ToggleChanged, ToggleConfig};
use std::rc::Rc;

#[derive(Default)]
pub(super) struct State {
    pub(super) projection: visibility::Projection,
    restore_scroll: Option<(TrackId, gpui::Pixels)>,
    pub(super) pending_restore: Option<(TrackId, gpui::Pixels)>,
    toggle: Option<Entity<Toggle>>,
    toggle_subscription: Option<Subscription>,
}
mod hidden_tracks;
mod interval;
#[path = "arrangement_focus/visibility.rs"]
mod visibility;
pub(super) use interval::track_intersects_sections;

impl State {
    pub(super) fn observe_active_track(&mut self, active: Option<TrackId>) {
        self.projection.observe_active_track(active);
    }
    pub(super) fn reveal_created_track(&mut self, track: TrackId) {
        self.projection.reveal_created_track(track);
    }
    fn rebuild(
        &mut self,
        generation: u64,
        revision: u64,
        project: &Project,
        selected: &[SectionId],
        active_track: Option<TrackId>,
    ) -> bool {
        if self.projection.generation_changed(generation) {
            self.restore_scroll = None;
            self.pending_restore = None;
        }
        let had_ranges = !self.projection.ranges.is_empty();
        let changed =
            self.projection
                .rebuild(generation, revision, project, selected, active_track);
        if changed
            && had_ranges
            && self.projection.ranges.is_empty()
            && self.pending_restore.is_none()
        {
            self.pending_restore = self.restore_scroll;
        }
        changed
    }
}
impl EditorView {
    pub(super) fn decorate_focused_track_row(
        &self,
        index: usize,
        track: TrackId,
        scale: f32,
        row: Div,
    ) -> Div {
        hidden_tracks::row(&self.arrangement_focus.projection, index, track, scale, row)
    }
    pub(super) fn decorate_focused_empty_tracks(&self, scale: f32, content: Div) -> Div {
        hidden_tracks::empty(&self.arrangement_focus.projection, scale, content)
    }

    pub(super) fn arrangement_focus_ranges(&self) -> Rc<[(u64, u64)]> {
        Rc::clone(&self.arrangement_focus.projection.ranges)
    }
    pub(super) fn arrangement_revealed_track(&self) -> Option<TrackId> {
        self.arrangement_focus.projection.new_track
    }
    pub(super) fn arrangement_clip_visible(&self, id: ClipId) -> bool {
        !self.arrangement_focus.projection.filtering()
            || self.arrangement_focus.projection.clips.contains(&id)
    }
    pub(super) fn arrangement_track_visible(&self, id: TrackId) -> bool {
        !self.arrangement_focus.projection.filtering()
            || self.arrangement_focus.projection.tracks.contains(&id)
    }
    /// Canonical indices in visible row order; borrowed for pointer geometry.
    pub(super) fn arrangement_track_indices(&self) -> &[usize] {
        &self.arrangement_focus.projection.indices
    }
    pub(super) fn arrangement_track_index(&self, row: usize) -> Option<usize> {
        self.arrangement_focus.projection.indices.get(row).copied()
    }
    pub(super) fn arrangement_row_index(&self, canonical: usize) -> Option<usize> {
        self.arrangement_focus
            .projection
            .indices
            .binary_search(&canonical)
            .ok()
    }
    pub(super) fn focused_neighbor_track(
        &self,
        forward: bool,
    ) -> Option<(usize, TrackId, Option<ClipId>)> {
        let project = &self.state.as_ref()?.project;
        let indices = &self.arrangement_focus.projection.indices;
        let last = indices.len().checked_sub(1)?;
        let current = self
            .selected_track
            .and_then(|id| project.tracks().iter().position(|t| t.id() == id));
        let row = match current {
            Some(index) => match indices.binary_search(&index) {
                Ok(row) if forward => row.saturating_add(1).min(last),
                Ok(row) => row.saturating_sub(1),
                Err(row) if forward => row.min(last),
                Err(row) => row.saturating_sub(1),
            },
            None if forward => 0,
            None => last,
        };
        let track = project.tracks().get(*indices.get(row)?)?;
        let clip = track
            .clips()
            .iter()
            .map(|c| c.id())
            .chain(track.midi_clips().iter().map(|c| c.id()))
            .find(|id| self.arrangement_clip_visible(*id));
        Some((row, track.id(), clip))
    }
    pub(super) fn reconcile_arrangement_focus(&mut self) {
        if let Some(state) = &self.state {
            if self.arrangement_focus.rebuild(
                self.project_generation,
                state.revision,
                &state.project,
                self.sections.selected_ids(),
                self.selected_track,
            ) {
                self.invalidate_arrangement_track_layout();
            }
        } else {
            self.arrangement_focus = State::default();
        }
    }
    pub(super) fn toggle_arrangement_focus(&mut self) {
        // An explicit Q/toolbar choice supersedes a delayed automatic focus.
        self.cancel_pending_loop_focus();
        self.reconcile_arrangement_focus();
        if self.arrangement_focus.projection.enabled {
            self.arrangement_focus.projection.enabled = false;
            self.arrangement_focus.pending_restore = self.arrangement_focus.restore_scroll.take();
            self.status = "section focus off · q focuses selected sections".into();
        } else {
            let top = self.track_scroll.list.logical_scroll_top();
            self.arrangement_focus.restore_scroll = self
                .arrangement_list_track_index(top.item_ix)
                .and_then(|index| self.state.as_ref()?.project.tracks().get(index))
                .map(|t| (t.id(), top.offset_in_item));
            self.arrangement_focus.projection.enabled = true;
            self.status = "section focus on · select section(s) · q clears section focus".into();
        }
        self.reconcile_arrangement_focus();
    }
    pub(super) fn section_focus_toggle_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.arrangement_focus
            .toggle
            .as_ref()
            .is_some_and(|toggle| toggle.focus_handle(cx).is_focused(window))
    }
    pub(super) fn section_focus_toggle(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        if self.arrangement_focus.toggle.is_none() {
            let Ok(config) = ToggleConfig::new(
                "arrangement-focus-toggle",
                theme::text("q"),
                self.arrangement_focus.projection.enabled,
                theme::control_style(),
            ) else {
                return div().id("section-focus-control");
            };
            let toggle = cx.new(|cx| {
                Toggle::from_config(cx, config.with_appearance(ToggleAppearance::CompactButton))
            });
            toggle.update(cx, |toggle, cx| toggle.set_tab_navigation(true, cx));
            let sub = cx.subscribe(&toggle, |view, _, event: &ToggleChanged, cx| {
                if view.arrangement_view_change_available(cx)
                    && view.arrangement_focus.projection.enabled != event.checked
                {
                    view.toggle_arrangement_focus();
                }
                cx.notify();
            });
            self.arrangement_focus.toggle = Some(toggle);
            self.arrangement_focus.toggle_subscription = Some(sub);
        }
        let enabled = self.arrangement_view_change_available(cx);
        if let Some(toggle) = &self.arrangement_focus.toggle {
            toggle.update(cx, |toggle, cx| {
                toggle.set_checked(self.arrangement_focus.projection.enabled, cx);
                toggle.set_enabled(enabled, cx);
                let _ = toggle.set_external_style(theme::control_style(), cx);
            });
        }
        div()
            .id("section-focus-control")
            .debug_selector(|| "section-focus-control".into())
            .flex()
            .items_center()
            .w(px(20.0))
            .h(px(20.0))
            .flex_shrink_0()
            .children(self.arrangement_focus.toggle.clone())
            .tooltip(|_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: "focus selected sections · q".into(),
                })
                .into()
            })
            .on_click(|_, _, cx| cx.stop_propagation())
    }
}
#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

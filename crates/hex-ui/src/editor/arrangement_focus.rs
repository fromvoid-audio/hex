//! Read-only Arrangement track visibility over selected canonical section intervals.
//! Rebuild once per document/revision/section selection change, never per row.
//! Nonadjacent sections form a union, not a hull. No selection leaves the normal
//! view visible while the mode stays armed. Retained tracks expose all their clips,
//! including outside the selected intervals. Edits retain canonical IDs/indices.
//! A newly acknowledged track stays visible while it is the active track. This
//! exception is view-only and ends on selection, section, focus-mode or document change.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, Entity, Focusable, Subscription, Window, div, prelude::*, px};
use hex_project::{ClipId, Project, SectionId, Track, TrackId};
use hex_widgets::{Toggle, ToggleAppearance, ToggleChanged, ToggleConfig};
use std::{collections::BTreeSet, rc::Rc};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    generation: u64,
    revision: u64,
    enabled: bool,
    new_track: Option<TrackId>,
}
#[derive(Default)]
pub(super) struct State {
    pub(super) enabled: bool,
    stamp: Option<Stamp>,
    selected: Vec<SectionId>,
    new_track: Option<TrackId>,
    ranges: Rc<[(u64, u64)]>,
    indices: Vec<usize>,
    tracks: BTreeSet<TrackId>,
    clips: BTreeSet<ClipId>,
    restore_scroll: Option<(TrackId, gpui::Pixels)>,
    pub(super) pending_restore: Option<(TrackId, gpui::Pixels)>,
    toggle: Option<Entity<Toggle>>,
    toggle_subscription: Option<Subscription>,
}
pub(super) fn interval_visible(start: u64, end: u64, ranges: &[(u64, u64)]) -> bool {
    ranges.is_empty() || (start < end && ranges.iter().any(|r| start < r.1 && end > r.0))
}
/// Section intervals choose whole tracks; clip visibility never crops those tracks.
/// Empty intervals retain the ordinary unfiltered view.
pub(super) fn track_intersects_sections(
    project: &Project,
    track: &Track,
    ranges: &[(u64, u64)],
) -> bool {
    ranges.is_empty()
        || track.clips().iter().any(|clip| {
            clip.start()
                .checked_add(clip.duration())
                .is_some_and(|end| interval_visible(clip.start(), end, ranges))
        })
        || track.midi_clips().iter().any(|clip| {
            let Some(end) = clip.start().raw().checked_add(clip.length().raw()) else {
                return false;
            };
            let map = project.tempo_map();
            match (
                map.beat_to_frame(clip.start()),
                map.beat_to_frame(hex_project::BeatTime::from_raw(end)),
            ) {
                (Ok(start), Ok(end)) => interval_visible(start, end, ranges),
                _ => false,
            }
        })
}
fn section_intervals(project: &Project, selected: &[SectionId]) -> Rc<[(u64, u64)]> {
    let mut intervals = selected
        .iter()
        .filter_map(|id| {
            let section = project.arrangement_section(*id)?;
            let map = project.tempo_map();
            let start = map.beat_to_frame(section.start()).ok()?;
            let end = map.beat_to_frame(section.end()).ok()?;
            (start < end).then_some((start, end))
        })
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let mut union: Vec<(u64, u64)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        if let Some(last) = union.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            union.push((start, end));
        }
    }
    union.into()
}
impl State {
    /// Retire on the selection event, even when multiple events share one paint.
    pub(super) fn observe_active_track(&mut self, active: Option<TrackId>) {
        if self.new_track != active {
            self.new_track = None;
        }
    }

    pub(super) fn reveal_created_track(&mut self, track: TrackId) {
        self.new_track = (self.enabled && !self.ranges.is_empty()).then_some(track);
    }

    fn rebuild(
        &mut self,
        generation: u64,
        revision: u64,
        project: &Project,
        selected: &[SectionId],
        active_track: Option<TrackId>,
    ) -> bool {
        if self.stamp.is_some_and(|s| s.generation != generation) {
            self.enabled = false;
            self.restore_scroll = None;
            self.pending_restore = None;
            self.new_track = None;
        }
        self.observe_active_track(active_track);
        if !self.enabled || self.selected != selected {
            self.new_track = None;
        }
        let mut stamp = Stamp {
            generation,
            revision,
            enabled: self.enabled,
            new_track: self.new_track,
        };
        if self.stamp == Some(stamp) && self.selected == selected {
            return false;
        }
        let ranges = if self.enabled {
            section_intervals(project, selected)
        } else {
            Rc::from([])
        };
        // Validate track existence only on an invalidated projection, so an
        // unchanged repaint retains the constant-time stamp path.
        if self.ranges != ranges || self.new_track.is_some_and(|id| project.track(id).is_none()) {
            self.new_track = None;
            stamp.new_track = None;
        }
        if !self.ranges.is_empty() && ranges.is_empty() && self.pending_restore.is_none() {
            self.pending_restore = self.restore_scroll;
        }
        self.selected.clear();
        self.selected.extend_from_slice(selected);
        self.ranges = ranges;
        self.indices.clear();
        self.tracks.clear();
        self.clips.clear();
        for (index, track) in project.tracks().iter().enumerate() {
            if self.new_track != Some(track.id())
                && !track_intersects_sections(project, track, &self.ranges)
            {
                continue;
            }
            if !self.ranges.is_empty() {
                self.tracks.insert(track.id());
                self.clips.extend(
                    track
                        .clips()
                        .iter()
                        .map(|clip| clip.id())
                        .chain(track.midi_clips().iter().map(|clip| clip.id())),
                );
            }
            self.indices.push(index);
        }
        self.stamp = Some(stamp);
        true
    }
}
impl EditorView {
    pub(super) fn arrangement_focus_ranges(&self) -> Rc<[(u64, u64)]> {
        Rc::clone(&self.arrangement_focus.ranges)
    }
    pub(super) fn arrangement_revealed_track(&self) -> Option<TrackId> {
        self.arrangement_focus.new_track
    }
    pub(super) fn arrangement_clip_visible(&self, id: ClipId) -> bool {
        self.arrangement_focus.ranges.is_empty() || self.arrangement_focus.clips.contains(&id)
    }
    pub(super) fn arrangement_track_visible(&self, id: TrackId) -> bool {
        self.arrangement_focus.ranges.is_empty() || self.arrangement_focus.tracks.contains(&id)
    }
    pub(super) fn arrangement_tracks<'a>(
        &'a self,
        project: &'a Project,
    ) -> impl Iterator<Item = (usize, &'a Track)> {
        self.arrangement_focus
            .indices
            .iter()
            .copied()
            .filter_map(move |index| project.tracks().get(index).map(|track| (index, track)))
    }
    /// Canonical indices in visible row order; borrowed for pointer geometry.
    pub(super) fn arrangement_track_indices(&self) -> &[usize] {
        &self.arrangement_focus.indices
    }
    pub(super) fn arrangement_track_index(&self, row: usize) -> Option<usize> {
        self.arrangement_focus.indices.get(row).copied()
    }
    pub(super) fn arrangement_row_index(&self, canonical: usize) -> Option<usize> {
        self.arrangement_focus
            .indices
            .binary_search(&canonical)
            .ok()
    }
    pub(super) fn focused_neighbor_track(
        &self,
        forward: bool,
    ) -> Option<(usize, TrackId, Option<ClipId>)> {
        let project = &self.state.as_ref()?.project;
        let indices = &self.arrangement_focus.indices;
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
        if self.arrangement_focus.enabled {
            self.arrangement_focus.enabled = false;
            self.arrangement_focus.pending_restore = self.arrangement_focus.restore_scroll.take();
            self.status = "all arrangement tracks · q focuses selected sections".into();
        } else {
            let top = self.track_scroll.list.logical_scroll_top();
            self.arrangement_focus.restore_scroll = self
                .state
                .as_ref()
                .and_then(|s| s.project.tracks().get(top.item_ix))
                .map(|t| (t.id(), top.offset_in_item));
            self.arrangement_focus.enabled = true;
            self.status = "section focus on · select section(s) · q restores all tracks".into();
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
                self.arrangement_focus.enabled,
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
                    && view.arrangement_focus.enabled != event.checked
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
                toggle.set_checked(self.arrangement_focus.enabled, cx);
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

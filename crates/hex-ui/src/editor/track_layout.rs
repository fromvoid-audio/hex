//! Arrangement-only layout state; resizing never edits the project or audio.
//! Header overview has a separate transient height map: temporary resizing never
//! overwrites normal custom heights. Drag and Option-wheel use whole multiples
//! of the compact height. Option-wheel changes only the hovered row, after
//! nested controls have had a chance to consume the event. Main shares the
//! row-height and rollback map. Its top resize edge preserves the pinned bottom.
//! Stable track IDs survive topology changes. Project replacement clears both layouts.
//! Plain H belongs to this workspace
//! only while the computer MIDI keyboard is disabled. Command-H resets track
//! heights and exits overview without changing the pinned main row. Held sizing uses the shared
//! Window pointer lease; its local snapshot owns complete cancellation rollback.
use super::{
    EditorView,
    tracks::{DEFAULT_TRACK_HEADER_WIDTH, TRACK_HEADER_WIDTH, TRACK_ROW_HEIGHT},
};
use gpui::{
    Context, Div, KeyDownEvent, ListState, Modifiers, MouseButton, MouseMoveEvent, MouseUpEvent,
    ScrollWheelEvent, Window, canvas, div, prelude::*, px,
};
use hex_project::TrackId;
use std::collections::BTreeMap;

// Reuse the browser/workspace split's room budget. Mixer controls still fit
// at the minimum header width.
const MAX_HEADER_WIDTH: f32 = super::browser::DEFAULT_WIDTH;
const MIN_LANE_WIDTH: f32 = super::browser::MIN_WORKSPACE_WIDTH - TRACK_HEADER_WIDTH;
// Name and primary controls + separate 3px resize gutter and bottom border.
pub(super) const HEADER_ONLY_HEIGHT: f32 = super::track_header::NAME_HEIGHT + 4.0;

fn stepped_height(height: f32, maximum: f32) -> f32 {
    let max_steps = (maximum / HEADER_ONLY_HEIGHT).floor().max(1.0);
    (height / HEADER_ONLY_HEIGHT).round().clamp(1.0, max_steps) * HEADER_ONLY_HEIGHT
}

/// The pinned output row has layout identity without inventing a project TrackId.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Row {
    Track(TrackId),
    Main,
}
impl Row {
    fn track(self) -> Option<TrackId> {
        match self {
            Self::Track(id) => Some(id),
            Self::Main => None,
        }
    }
}

#[derive(Default)]
pub(super) struct TrackLayout {
    width: Option<f32>,
    pub(super) available_width: Option<f32>,
    heights: BTreeMap<Row, f32>,
    overview: Option<BTreeMap<Row, f32>>,
    drag: Option<ResizeDrag>,
    wheel_remainder: Option<(Row, f32)>,
    generation: u64,
    tracks: Vec<TrackId>,
    revision: Option<u64>,
    focus_ranges: std::rc::Rc<[(u64, u64)]>,
}
/// View-only custody of row heights and a stable scroll anchor for Zoom Back.
/// The layout owner alone captures/restores these private maps; no musical or
/// native playback resources are retained.
pub(super) struct VerticalFitSnapshot {
    generation: u64,
    heights: BTreeMap<Row, f32>,
    overview: Option<BTreeMap<Row, f32>>,
    anchor: Option<TrackId>,
    scroll: gpui::ListOffset,
}

impl TrackLayout {
    fn active_heights(&self) -> &BTreeMap<Row, f32> {
        self.overview.as_ref().unwrap_or(&self.heights)
    }
    fn active_heights_mut(&mut self) -> &mut BTreeMap<Row, f32> {
        self.overview.as_mut().unwrap_or(&mut self.heights)
    }
}
/// Empty trailing track space as a fraction of the actual scroll viewport.
pub(super) const BOTTOM_SCROLL_SPACE_RATIO: f32 = 0.2;

struct ResizeDrag {
    track: Option<Row>,
    pointer: f32,
    original: f32,
    generation: u64,
    revision: u64,
    previous_width: Option<f32>,
    previous_heights: Option<BTreeMap<Row, f32>>,
    maximum: f32,
    moved: bool,
}

/// Keep pan/autoscroll's pixel interface while GPUI owns variable-row layout.
#[derive(Clone)]
pub(super) struct TrackScroll {
    pub(super) list: ListState,
}
impl TrackScroll {
    pub(super) fn new() -> Self {
        Self {
            list: ListState::new(0, gpui::ListAlignment::Top, px(100.0)).measure_all(),
        }
    }
    pub(super) fn offset(&self) -> gpui::Point<gpui::Pixels> {
        self.list.scroll_px_offset_for_scrollbar()
    }
    pub(super) fn max_offset(&self) -> gpui::Size<gpui::Pixels> {
        let native = self.list.max_offset_for_scrollbar();
        let Some(last) = self
            .list
            .item_count()
            .checked_sub(1)
            .and_then(|index| self.list.bounds_for_item(index))
        else {
            return native;
        };
        // GPUI's wheel/set-offset path includes padding but its maximum query
        // omits it. Use its measured final row and actual offset, not a second
        // track-height inventory, so pan/autoscroll share the wheel's endpoint.
        let viewport = self.list.viewport_bounds();
        let content_height = last.bottom() - viewport.top() - self.offset().y;
        let maximum = (content_height + viewport.size.height * BOTTOM_SCROLL_SPACE_RATIO
            - viewport.size.height)
            .max(px(0.));
        gpui::size(native.width, maximum)
    }
    pub(super) fn set_offset(&self, offset: gpui::Point<gpui::Pixels>) {
        self.list.set_offset_from_scrollbar(offset);
    }
    pub(super) fn scroll_to_item(&self, index: usize, strategy: gpui::ScrollStrategy) {
        if strategy == gpui::ScrollStrategy::Top {
            self.list.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: px(0.0),
            });
        } else {
            self.list.scroll_to_reveal_item(index);
        }
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn logical_scroll_top_index(&self) -> usize {
        self.list.logical_scroll_top().item_ix
    }
    pub(super) fn remeasure(&self) {
        let top = self.list.logical_scroll_top();
        self.list.reset(self.list.item_count());
        self.list.scroll_to(top);
    }
    fn invalidate(&self, range: std::ops::Range<usize>, count: usize) {
        let top = self.list.logical_scroll_top();
        self.list.splice(range, count);
        self.list.scroll_to(top);
    }
}
/// A single canonical row move is a one-position rotation of the changed span.
/// Return the old source index and final insertion index, not an original gap.
fn single_row_move(old: &[TrackId], new: &[TrackId]) -> Option<(usize, usize)> {
    if old.len() != new.len() {
        return None;
    }
    let first = old.iter().zip(new).position(|(a, b)| a != b)?;
    let last = old.iter().zip(new).rposition(|(a, b)| a != b)?;
    if old[first] == new[last] && old[first + 1..=last] == new[first..last] {
        Some((first, last))
    } else if old[last] == new[first] && old[first..last] == new[first + 1..=last] {
        Some((last, first))
    } else {
        None
    }
}

/// A contiguous insertion or removal preserves measurements on either side.
fn row_count_change(old: &[TrackId], new: &[TrackId]) -> Option<(std::ops::Range<usize>, usize)> {
    let start = old
        .iter()
        .zip(new)
        .take_while(|(old, new)| old == new)
        .count();
    if new.len() > old.len() {
        let inserted = new.len() - old.len();
        (old[start..] == new[start + inserted..]).then_some((start..start, inserted))
    } else if old.len() > new.len() {
        let removed = old.len() - new.len();
        let end = start + removed;
        (old[end..] == new[start..]).then_some((start..end, 0))
    } else {
        None
    }
}

impl EditorView {
    /// Arrangement-only A/Shift+A/H/Q after child input. Text and capture retain ownership.
    pub(super) fn arrangement_view_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let reset = key == "h"
            && modifiers
                == (Modifiers {
                    platform: true,
                    ..Modifiers::default()
                });
        let reveal_automation = key == "a"
            && modifiers
                == (Modifiers {
                    shift: true,
                    ..Modifiers::default()
                });
        let plain = matches!(key, "a" | "h" | "q")
            && modifiers == Modifiers::default()
            && !self.computer_keyboard.enabled;
        if !(reset || plain || reveal_automation)
            || event.is_held
            || !(self.focus.is_focused(window)
                || self.track_header_focused(window)
                || self.section_focus_toggle_focused(window, cx))
            || !self.arrangement_view_change_available(cx)
        {
            return false;
        }
        if reset {
            self.track_layout.wheel_remainder = None;
            self.track_layout.heights.retain(|row, _| *row == Row::Main);
            self.track_layout.overview = None;
            self.track_scroll.remeasure();
            cx.notify();
            return true;
        }
        if reveal_automation {
            return self.reveal_arrangement_automation(cx);
        }
        if key == "a" {
            return self.toggle_automation_view(cx);
        }
        if key == "q" {
            self.toggle_arrangement_focus();
            cx.notify();
            return true;
        }
        self.track_layout.wheel_remainder = None;
        if self.track_layout.overview.take().is_none() {
            self.track_layout.overview = Some(BTreeMap::new());
        }
        self.track_scroll.remeasure();
        cx.notify();
        true
    }
    pub(super) fn arrangement_view_change_available(&self, cx: &Context<Self>) -> bool {
        self.mode == super::EditorMode::Arrangement && self.track_view_change_available(cx)
    }
    pub(super) fn track_view_change_available(&self, cx: &Context<Self>) -> bool {
        !(!self.drag_available()
            || self.edit_pending()
            || self.track_name.is_editing()
            || self.sections.naming()
            || self.sections.held()
            || self.section_install_pending()
            || self.tuning.open
            || self.theme_settings.open
            || self.changelog.is_open()
            || self.controller_input_is_open()
            || self.context_menu.is_some()
            || (self.clip_view_open
                && self.device_panel_active
                && self.device_panel.add_menu_is_open(self.state.as_deref()))
            || self.send_menu_track.is_some()
            || self.project_tempo_editing(cx)
            || self.gain_numeric_editing(cx)
            || self.sampler_parameter.numeric_draft_active()
            || self.track_gain_controls.has_pending_draft(cx)
            || super::device_panel::pending_send(self, cx).is_some()
            || self.gain_lane.pointer_is_active()
            || self.automation_view.devices.pointer_active()
            || self.clip_numeric_editing(cx)
            || self.project_open.busy()
            || self.pending_open_path.is_some()
            || self.export_dialog_open
            || self.save_dialog_open
            || self.preset_dialog_open
            || self.arrangement_pan.is_some()
            || self.pointer_down.is_some()
            || self.project_spectrum.resize_is_active()
            || self.clip_detail_resize.is_active()
            || self.device_panel.navigation.is_active()
            || self.clip_paint_gesture.is_some()
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.loop_rail_gesture.is_some()
            || self.scrub_active()
            || cx.has_active_drag()
            || self.state.is_none())
    }
    fn maximum_header_width(&self) -> f32 {
        self.track_layout
            .available_width
            .map_or(MAX_HEADER_WIDTH, |width| {
                (width - MIN_LANE_WIDTH).clamp(TRACK_HEADER_WIDTH, MAX_HEADER_WIDTH)
            })
    }
    pub(super) fn capture_vertical_fit(&mut self) -> Option<VerticalFitSnapshot> {
        let state = self.state.clone()?;
        self.sync_track_layout(state.project.tracks());
        if self.track_layout.generation != self.project_generation
            || !self
                .arrangement_bounds
                .is_some_and(|area| area.height.is_finite() && area.height > 0.0)
        {
            return None;
        }
        let scroll = self.track_scroll.list.logical_scroll_top();
        Some(VerticalFitSnapshot {
            generation: self.project_generation,
            heights: self.track_layout.heights.clone(),
            overview: self.track_layout.overview.clone(),
            anchor: self.track_layout.tracks.get(scroll.item_ix).copied(),
            scroll,
        })
    }

    /// Fit the inclusive visible row span, accounting for fixed automation lanes.
    /// Minimum operable row height wins when a large selection cannot fully fit.
    pub(super) fn fit_tracks_vertically(&mut self, members: &[TrackId]) -> bool {
        if self.track_resize_active()
            || self.track_layout.generation != self.project_generation
            || self.track_layout.revision != self.state.as_ref().map(|state| state.revision)
        {
            return false;
        }
        let Some(area) = self
            .arrangement_bounds
            .filter(|area| area.height.is_finite() && area.height > 0.0)
        else {
            return false;
        };
        let mut rows = self
            .track_layout
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, id)| members.contains(id))
            .map(|(index, _)| index);
        let Some(first) = rows.next() else {
            return false;
        };
        let last = rows.next_back().unwrap_or(first);
        let tracks = &self.track_layout.tracks[first..=last];
        let extra: f32 = tracks
            .iter()
            .map(|id| self.track_height(*id) - self.track_body_height(*id))
            .sum();
        let height = ((area.height - extra) / tracks.len() as f32)
            .clamp(HEADER_ONLY_HEIGHT, area.height.max(TRACK_ROW_HEIGHT));
        let scroll = self.track_scroll.list.logical_scroll_top();
        if tracks
            .iter()
            .all(|id| (self.track_body_height(*id) - height).abs() < 0.01)
            && scroll.item_ix == first
            && scroll.offset_in_item == px(0.0)
        {
            return false;
        }
        let layout = &mut self.track_layout;
        for id in &layout.tracks[first..=last] {
            layout
                .overview
                .as_mut()
                .unwrap_or(&mut layout.heights)
                .insert(Row::Track(*id), height);
        }
        self.track_scroll.remeasure();
        self.track_scroll
            .scroll_to_item(first, gpui::ScrollStrategy::Top);
        true
    }

    pub(super) fn restore_vertical_fit(&mut self, mut prior: VerticalFitSnapshot) {
        if prior.generation != self.project_generation {
            return;
        }
        let Some(state) = self.state.clone() else {
            return;
        };
        self.sync_track_layout(state.project.tracks());
        let maximum = self
            .arrangement_bounds
            .filter(|area| area.height.is_finite())
            .map(|area| area.height.max(TRACK_ROW_HEIGHT));
        let retain = |row: &Row, height: &mut f32| match row {
            Row::Main => true,
            Row::Track(id) => {
                if let Some(maximum) = maximum {
                    *height = height.clamp(HEADER_ONLY_HEIGHT, maximum);
                }
                state.project.track(*id).is_some()
            }
        };
        prior.heights.retain(retain);
        if let Some(overview) = prior.overview.as_mut() {
            overview.retain(retain);
        }
        self.track_layout.heights = prior.heights;
        self.track_layout.overview = prior.overview;
        self.track_scroll.remeasure();
        let index = prior
            .anchor
            .and_then(|id| self.track_layout.tracks.iter().position(|item| *item == id))
            .unwrap_or(
                prior
                    .scroll
                    .item_ix
                    .min(self.track_layout.tracks.len().saturating_sub(1)),
            );
        let height = self
            .track_layout
            .tracks
            .get(index)
            .map_or(0.0, |id| self.track_height(*id));
        self.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: index,
            offset_in_item: if f32::from(prior.scroll.offset_in_item).is_finite() {
                prior.scroll.offset_in_item.clamp(px(0.0), px(height))
            } else {
                px(0.0)
            },
        });
    }

    pub(super) fn track_header_width(&self) -> f32 {
        self.track_layout
            .width
            .unwrap_or(DEFAULT_TRACK_HEADER_WIDTH)
            .clamp(TRACK_HEADER_WIDTH, self.maximum_header_width())
    }
    pub(super) fn track_resize_active(&self) -> bool {
        self.track_layout.drag.is_some()
    }
    /// Option-only wheel is local sizing; ordinary scrolling keeps its platform delta.
    pub(super) fn row_size_wheel(event: &ScrollWheelEvent) -> bool {
        event.modifiers.alt
            && !event.modifiers.shift
            && !event.modifiers.control
            && !event.modifiers.platform
            && !event.modifiers.function
    }
    /// Bubbling dispatch lets nested controls own their wheel input first. Main
    /// shares normal/overview maps; Returns deliberately retain their fixed size.
    pub(super) fn resize_row_wheel(
        &mut self,
        track: Option<TrackId>,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        if !Self::row_size_wheel(event)
            || !self.arrangement_view_change_available(cx)
            || !f32::from(event.position.x).is_finite()
            || !f32::from(event.position.y).is_finite()
            || track.is_some_and(|id| {
                self.state
                    .as_ref()
                    .is_none_or(|s| s.project.track(id).is_none())
            })
        {
            return;
        }
        if !matches!(event.touch_phase, gpui::TouchPhase::Moved) {
            self.track_layout.wheel_remainder = None;
        }
        // One physical wheel tick is one height step. Collect small trackpad
        // deltas on the hovered row before applying a step.
        const WHEEL_PIXELS_PER_STEP: f32 = 64.0;
        let delta = event.physical_wheel_ticks().map_or_else(
            || {
                f32::from(
                    event
                        .physical_delta()
                        .pixel_delta(px(WHEEL_PIXELS_PER_STEP))
                        .y,
                ) / WHEEL_PIXELS_PER_STEP
            },
            |ticks| ticks.y,
        );
        if !delta.is_finite() || delta == 0. {
            return;
        }
        cx.stop_propagation();
        if event.is_momentum
            || matches!(
                event.touch_phase,
                gpui::TouchPhase::Ended | gpui::TouchPhase::Cancelled
            )
        {
            return;
        }
        let row = track.map_or(Row::Main, Row::Track);
        let remainder = self
            .track_layout
            .wheel_remainder
            .filter(|(owner, _)| *owner == row)
            .map_or(0.0, |(_, remainder)| remainder);
        let units = remainder + delta.clamp(-2.0, 2.0);
        let steps = units.trunc();
        self.track_layout.wheel_remainder = Some((row, units - steps));
        if steps == 0.0 {
            return;
        }
        let (original, maximum) = self.row_resize_geometry(row);
        let height = stepped_height(original + steps * HEADER_ONLY_HEIGHT, maximum);
        if (height - original) * steps <= 0.0 {
            return;
        }
        self.track_layout.active_heights_mut().insert(row, height);
        self.invalidate_track_size(track);
        if let Some(track) = track {
            self.center_resized_track(track);
        }
        cx.notify();
    }

    // Express the desired viewport top as a logical list position. Walking
    // backwards only across rows that fit above the target avoids maintaining a
    // second content-height inventory; GPUI clamps the final position at the end.
    fn center_resized_track(&self, track: TrackId) {
        let Some(mut index) = self.track_layout.tracks.iter().position(|id| *id == track) else {
            return;
        };
        let viewport_height = f32::from(self.track_scroll.list.viewport_bounds().size.height);
        let height = self.track_height(track);
        if !viewport_height.is_finite() || viewport_height <= 0.0 || !height.is_finite() {
            return;
        }
        let mut offset = (height - viewport_height) / 2.0;
        while offset < 0.0 && index > 0 {
            index -= 1;
            let previous = self.track_height(self.track_layout.tracks[index]);
            if !previous.is_finite() || previous <= 0.0 {
                return;
            }
            offset += previous;
        }
        if !offset.is_finite() {
            return;
        }
        self.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: index,
            offset_in_item: px(offset.max(0.0)),
        });
    }

    fn row_resize_geometry(&self, row: Row) -> (f32, f32) {
        let original = if row == Row::Main {
            // Flex may reduce Main after window resizing; its painted lane
            // excludes the one-pixel top border, just as edge resizing does.
            self.main_track_lane_bounds
                .filter(|lane| {
                    self.main_track_lane_epoch == self.drag_layout_generation
                        && lane.height.is_finite()
                        && lane.height > 0.
                })
                .map_or_else(|| self.main_track_height(), |lane| lane.height + 1.)
        } else {
            self.row_body_height(row)
        };
        let available = self
            .arrangement_bounds
            .filter(|body| body.height.is_finite())
            .map_or(TRACK_ROW_HEIGHT, |body| body.height.max(TRACK_ROW_HEIGHT));
        (
            original,
            if row == Row::Main {
                available.max(original)
            } else {
                available
            },
        )
    }
    pub(super) fn track_body_height(&self, track: TrackId) -> f32 {
        self.row_body_height(Row::Track(track))
    }
    pub(super) fn main_track_height(&self) -> f32 {
        self.row_body_height(Row::Main)
    }
    fn row_body_height(&self, row: Row) -> f32 {
        self.track_layout
            .active_heights()
            .get(&row)
            .copied()
            .unwrap_or(if self.track_layout.overview.is_some() {
                HEADER_ONLY_HEIGHT
            } else {
                TRACK_ROW_HEIGHT
            })
    }
    pub(super) fn track_height(&self, track: TrackId) -> f32 {
        self.track_body_height(track)
            + if self.automation_view.enabled {
                self.automation_view
                    .devices
                    .extra_height(super::device_panel::DevicePanelOwner::Track(track))
            } else {
                0.0
            }
    }
    /// Visibility can change with active-track selection without a project edit.
    pub(super) fn invalidate_arrangement_track_layout(&mut self) {
        self.track_layout.revision = None;
    }

    pub(super) fn sync_track_layout(&mut self, tracks: &[hex_project::Track]) {
        if self.track_layout.generation != self.project_generation {
            self.track_layout.heights.clear();
            self.track_layout.overview = None;
            self.track_layout.drag = None;
            self.track_layout.wheel_remainder = None;
            self.track_layout.generation = self.project_generation;
            self.track_layout.tracks.clear();
            self.track_scroll.list.reset(0);
            self.track_layout.revision = None;
        }
        let revision = self.state.as_ref().map(|state| state.revision);
        let focus_ranges = self.arrangement_focus_ranges();
        if self.track_layout.revision == revision && self.track_layout.focus_ranges == focus_ranges
        {
            return;
        }
        self.track_layout.revision = revision;
        self.track_layout.focus_ranges = focus_ranges;
        let ids = tracks
            .iter()
            .filter(|t| self.arrangement_track_visible(t.id()))
            .map(|t| t.id())
            .collect::<Vec<_>>();
        if self.track_layout.tracks != ids {
            if let Some(drag) = self.track_layout.drag.take() {
                self.restore_track_resize(drag);
            }
            self.track_layout.heights.retain(|row, _| {
                row.track()
                    .is_none_or(|id| tracks.iter().any(|t| t.id() == id))
            });
            if let Some(heights) = self.track_layout.overview.as_mut() {
                heights.retain(|row, _| {
                    row.track()
                        .is_none_or(|id| tracks.iter().any(|t| t.id() == id))
                });
            }
            let mut top = self.track_scroll.list.logical_scroll_top();
            let anchor = self.track_layout.tracks.get(top.item_ix).copied();
            top.item_ix = anchor
                .and_then(|id| ids.iter().position(|item| *item == id))
                .unwrap_or(top.item_ix.min(ids.len().saturating_sub(1)));
            if let Some((from, to)) = single_row_move(&self.track_layout.tracks, &ids) {
                // Cached sizes belong to stable TrackIds. Preserve every
                // unchanged row; only the inserted moved row loses its size.
                // GPUI still scans bounded metadata, but constructs elements
                // only for unmeasured/visible rows instead of all100+ tracks.
                self.track_scroll.list.splice(from..from + 1, 0);
                self.track_scroll.list.splice(to..to, 1);
                let _ = self.track_scroll.list.clone().measure_all();
            } else if let Some((removed, inserted)) =
                row_count_change(&self.track_layout.tracks, &ids)
            {
                self.track_scroll.list.splice(removed, inserted);
                let _ = self.track_scroll.list.clone().measure_all();
            } else {
                // Other structural changes keep the full measurement path.
                self.track_scroll.list.reset(ids.len());
            }
            self.track_scroll.list.scroll_to(top);
            self.track_layout.tracks = ids;
        }
        if let Some((id, offset)) = self.arrangement_focus.pending_restore.take()
            && let Some(item_ix) = self.track_layout.tracks.iter().position(|t| *t == id)
        {
            self.track_scroll.list.scroll_to(gpui::ListOffset {
                item_ix,
                offset_in_item: offset,
            });
        }
    }
    fn invalidate_track_size(&self, track: Option<TrackId>) {
        if let Some(id) = track
            && let Some(index) = self.track_layout.tracks.iter().position(|t| *t == id)
        {
            self.track_scroll.invalidate(index..index + 1, 1);
        }
    }
    fn capture_track_resize(
        &self,
        track: Option<Row>,
        revision: u64,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> bool {
        let generation = self.project_generation;
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            if track.is_some() {
                gpui::CursorStyle::ResizeRow
            } else {
                gpui::CursorStyle::ResizeColumn
            },
            move |event, window, cx| {
                let moved = event.downcast_ref::<MouseMoveEvent>();
                let released = event
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|e| e.button == MouseButton::Left);
                let Some(position) = moved
                    .map(|e| e.position)
                    .or_else(|| released.map(|e| e.position))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !view.track_layout.drag.as_ref().is_some_and(|d| {
                        d.track == track && d.generation == generation && d.revision == revision
                    }) {
                        return;
                    }
                    view.reconcile_track_resize(window, cx);
                    if view.track_layout.drag.is_none() {
                        return;
                    }
                    if !f32::from(position.x).is_finite() || !f32::from(position.y).is_finite() {
                        view.cancel_track_resize(cx);
                        return;
                    }
                    let all_tracks = moved
                        .map(|e| e.modifiers.alt)
                        .or_else(|| released.map(|e| e.modifiers.alt))
                        .unwrap_or(false);
                    view.update_track_resize(
                        f32::from(position.x),
                        f32::from(position.y),
                        all_tracks,
                        cx,
                    );
                    if released.is_some() {
                        view.track_layout.drag = None;
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.track_layout.drag.as_ref().is_some_and(|d| {
                        d.track == track && d.generation == generation && d.revision == revision
                    }) {
                        view.cancel_track_resize(cx);
                    }
                });
            },
            cx,
        )
    }
    fn begin_track_resize(
        &mut self,
        track: Option<Row>,
        pointer: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !pointer.is_finite()
            || self.mode != super::EditorMode::Arrangement
            || !self.drag_available()
            || self.track_resize_active()
            || self.edit_pending()
            || self.track_gain_controls.has_pending_draft(cx)
            || self.gain_numeric_editing(cx)
            || self.context_menu.is_some()
            || self.arrangement_pan.is_some()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.loop_rail_gesture.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.active_native_device_drag.is_some()
        {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if track
            .and_then(Row::track)
            .is_some_and(|id| state.project.track(id).is_none())
        {
            return;
        }
        let revision = state.revision;
        let (original, maximum) = track.map_or_else(
            || (self.track_header_width(), self.maximum_header_width()),
            |row| self.row_resize_geometry(row),
        );
        if !self.capture_track_resize(track, revision, window, cx) {
            return;
        }
        self.track_layout.wheel_remainder = None;
        self.track_layout.drag = Some(ResizeDrag {
            track,
            pointer,
            original,
            generation: self.project_generation,
            revision,
            previous_width: self.track_layout.width,
            // A modifier may change during capture: retain every custom height
            // once so all live updates have one complete cancellation owner.
            previous_heights: track.map(|_| self.track_layout.active_heights().clone()),
            maximum,
            moved: false,
        });
        self.focus.focus(window);
        cx.notify();
    }
    fn track_resize_current(&self, drag: &ResizeDrag) -> bool {
        self.mode == super::EditorMode::Arrangement
            && drag.generation == self.project_generation
            && self.state.as_ref().is_some_and(|state| {
                state.revision == drag.revision
                    && drag
                        .track
                        .and_then(Row::track)
                        .is_none_or(|id| state.project.track(id).is_some())
            })
    }
    pub(super) fn reconcile_track_resize(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self
            .track_layout
            .drag
            .as_ref()
            .is_some_and(|drag| !self.track_resize_current(drag) || !self.focus.is_focused(window))
        {
            self.cancel_track_resize(cx);
        }
    }
    fn update_track_resize(&mut self, x: f32, y: f32, all_tracks: bool, cx: &mut Context<Self>) {
        let Some(drag) = self.track_layout.drag.as_ref() else {
            return;
        };
        if !self.track_resize_current(drag) {
            self.cancel_track_resize(cx);
            return;
        }
        let pointer = if drag.track.is_some() { y } else { x };
        if !pointer.is_finite() {
            return;
        }
        let target = drag.track;
        if let Some(track) = target {
            if pointer == drag.pointer && !drag.moved {
                return;
            }
            let delta = if track == Row::Main {
                drag.pointer - pointer
            } else {
                pointer - drag.pointer
            };
            let height = stepped_height(drag.original + delta, drag.maximum);
            if let Some(drag) = self.track_layout.drag.as_mut() {
                drag.moved = true;
            }
            if all_tracks {
                if self
                    .track_layout
                    .tracks
                    .iter()
                    .all(|id| self.track_body_height(*id) == height)
                    && self.main_track_height() == height
                {
                    return;
                }
                let layout = &mut self.track_layout;
                let heights = layout.overview.as_mut().unwrap_or(&mut layout.heights);
                for id in &layout.tracks {
                    heights.insert(Row::Track(*id), height);
                }
                heights.insert(Row::Main, height);
                self.track_scroll.remeasure();
            } else {
                if height == self.row_body_height(track) {
                    return;
                }
                self.track_layout.active_heights_mut().insert(track, height);
                self.invalidate_track_size(track.track());
            }
        } else {
            let width = drag.original - (pointer - drag.pointer);
            let width = width.clamp(TRACK_HEADER_WIDTH, drag.maximum);
            if width == self.track_header_width() {
                return;
            }
            self.track_layout.width = Some(width);
        }
        cx.notify();
    }
    fn restore_track_resize(&mut self, drag: ResizeDrag) {
        if drag.generation != self.project_generation {
            return;
        }
        if let Some(previous) = drag.previous_heights {
            *self.track_layout.active_heights_mut() = previous;
            self.track_scroll.remeasure();
        } else {
            self.track_layout.width = drag.previous_width;
        }
    }
    pub(super) fn cancel_track_resize(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.track_layout.drag.take() else {
            return false;
        };
        self.restore_track_resize(drag);
        cx.notify();
        true
    }
    pub(super) fn track_width_resize_handle(
        &self,
        track: Option<TrackId>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let label = track.map_or_else(
            || "main-track-width-resize".to_owned(),
            |id| format!("track-width-resize-{}", id.value()),
        );
        div()
            .id(gpui::SharedString::from(label.clone()))
            .debug_selector(move || label.clone())
            .absolute()
            .left_0()
            .top_0()
            .bottom(px(if track.is_some() { 3.0 } else { 0.0 }))
            .w(px(5.0))
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.begin_track_resize(None, f32::from(event.position.x), window, cx);
                }),
            )
    }
    pub(super) fn track_resize_handles(
        &self,
        track: TrackId,
        cx: &mut Context<Self>,
    ) -> [gpui::Stateful<Div>; 2] {
        [
            self.track_width_resize_handle(Some(track), cx),
            self.row_height_resize_handle(Row::Track(track), cx),
        ]
    }
    pub(super) fn main_track_resize_handles(
        &self,
        cx: &mut Context<Self>,
    ) -> [gpui::Stateful<Div>; 2] {
        [
            self.track_width_resize_handle(None, cx),
            self.row_height_resize_handle(Row::Main, cx),
        ]
    }
    fn row_height_resize_handle(&self, row: Row, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let label = match row {
            Row::Track(id) => format!("track-height-resize-{}", id.value()),
            Row::Main => "main-track-height-resize".into(),
        };
        let handle = div()
            .id(gpui::SharedString::from(label.clone()))
            .debug_selector(move || label.clone())
            .absolute()
            .left_0()
            .right_0()
            .h(px(3.))
            .cursor_row_resize();
        let handle = if row == Row::Main {
            handle.top_0()
        } else {
            handle.bottom_0()
        };
        handle.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                cx.stop_propagation();
                view.begin_track_resize(Some(row), f32::from(event.position.y), window, cx);
            }),
        )
    }
    pub(super) fn track_resize_events(&self, cx: &mut Context<Self>) -> gpui::Canvas<()> {
        let entity = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, cx| {
                let view = entity.read(cx);
                if let Some(drag) = view.track_layout.drag.as_ref()
                    && view.track_resize_current(drag)
                    && view.focus.is_focused(window)
                {
                    window.renew_pointer_capture(
                        entity.entity_id(),
                        if drag.track.is_some() {
                            gpui::CursorStyle::ResizeRow
                        } else {
                            gpui::CursorStyle::ResizeColumn
                        },
                    );
                }
            },
        )
        .absolute()
        .inset_0()
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use crate::editor::device_automation::host_tests::{
        CMD, Slot, TestWave, dotted_lane_point, header_assign, lane_points, lane_value_point, pump,
        window_with_gain,
    };
    use gpui::{Modifiers, MouseButton, TestAppContext, px, size};
    use hex_lattice_contract::DeviceAutomationTarget;

    #[gpui::test]
    fn held_gain_point_prevents_new_sampler_text_draft(cx: &mut TestAppContext) {
        let source = TestWave::new();
        let (mut visual, editor, _, track, _) =
            window_with_gain(cx, "hex.utility", Slot::Track, false, Some(&source.0));
        let sampler = editor.read_with(&visual, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id()
        });
        visual.simulate_resize(size(px(1600.0), px(800.0)));
        visual.run_until_parked();
        let toggle = visual
            .debug_bounds("automation-view-toggle")
            .unwrap()
            .center();
        visual.simulate_click(toggle, gpui::Modifiers::default());
        editor.update(&mut visual, |view, cx| {
            view.gain_lane.track = Some(track);
            cx.notify();
        });
        visual.run_until_parked();
        let sampler_selector =
            Box::leak(format!("sampler-attack-{}-value", sampler.value()).into_boxed_str());
        let sampler_at = visual.debug_bounds(sampler_selector).unwrap().center();
        let plot_selector =
            Box::leak(format!("arrangement-automation-plot-{}", track.value()).into_boxed_str());
        let plot = visual.debug_bounds(plot_selector).unwrap();
        let at = gpui::point(plot.center().x, plot.top() + plot.size.height * 0.25);
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |view, _| assert!(view.gain_lane.draft.is_some()));
        visual.simulate_click(sampler_at, Modifiers::default());
        editor.read_with(&visual, |view, _| assert!(view.gain_lane.pending.is_some()));
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid");
        editor.read_with(&visual, |view, _| {
            assert!(
                !view.sampler_parameter.numeric_draft_active(),
                "sampler draft began while gain point was held"
            );
        });
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        pump(&mut visual, &editor);
        editor.read_with(&visual, |view, _| {
            assert!(view.gain_lane.pending.is_none());
            assert!(view.error.as_ref().is_some_and(|error| {
                error
                    .to_string()
                    .contains("sampler supports private offline export")
            }));
        });
        visual.run_until_parked();
        visual.simulate_click(sampler_at, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid");
        editor.read_with(&visual, |view, _| {
            assert!(view.sampler_parameter.numeric_draft_active())
        });
    }

    #[gpui::test]
    fn invalid_sampler_text_blocks_starting_an_automation_gesture(cx: &mut TestAppContext) {
        let source = TestWave::new();
        let (mut visual, editor, owner, track, device) =
            window_with_gain(cx, "hex.utility", Slot::Track, false, Some(&source.0));
        let sampler = editor.read_with(&visual, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id()
        });
        let target =
            DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(1));
        header_assign(&mut visual, &editor, owner, target);
        visual.simulate_resize(size(px(1600.0), px(800.0)));
        visual.run_until_parked();
        let selector =
            Box::leak(format!("sampler-attack-{}-value", sampler.value()).into_boxed_str());
        let position = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(position, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid");
        editor.read_with(&visual, |view, _| {
            assert!(
                view.sampler_parameter.numeric_draft_active(),
                "sampler text must be active before the automation press"
            )
        });
        let at = dotted_lane_point(&mut visual, device, target, 0.6);
        editor.read_with(&visual, |view, _| {
            assert!(
                view.sampler_parameter.numeric_draft_active(),
                "sampler text must survive a repaint"
            )
        });
        visual.simulate_mouse_down(at, MouseButton::Left, CMD);
        editor.read_with(&visual, |view, cx| {
            assert!(view.sampler_parameter.numeric_draft_active());
            assert_eq!(
                crate::editor::sampler_parameter::tests::sampler_text(view, cx),
                "invalid"
            );
            assert!(!view.automation_view.devices.point_draft_active());
        });
        visual.simulate_mouse_up(at, MouseButton::Left, CMD);
        editor.update(&mut visual, |view, cx| {
            assert!(view.cancel_sampler_drafts(cx));
        });
        visual.simulate_mouse_down(at, MouseButton::Left, CMD);
        visual.simulate_mouse_up(at, MouseButton::Left, CMD);
        visual.run_until_parked();
        pump(&mut visual, &editor);
        assert_eq!(lane_points(&visual, &editor, device, target).len(), 1);

        // The reverse order must also preserve the first draft. A press over
        // the sampler while automation owns the pointer cannot start text.
        let first_value = lane_points(&visual, &editor, device, target)[0].value.get();
        let next = lane_value_point(&mut visual, device, target, 0.3, first_value);
        let sampler_at = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_down(next, MouseButton::Left, CMD);
        editor.read_with(&visual, |view, _| {
            assert!(view.automation_view.devices.point_draft_active());
        });
        visual.simulate_mouse_down(sampler_at, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            assert!(view.automation_view.devices.point_draft_active());
            assert!(!view.sampler_parameter.numeric_draft_active());
        });
        visual.simulate_mouse_up(next, MouseButton::Left, CMD);
        pump(&mut visual, &editor);
        assert_eq!(lane_points(&visual, &editor, device, target).len(), 2);
    }
}

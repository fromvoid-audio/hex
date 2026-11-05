//! View-only track insertion and group-target feedback. Returns and Main remain pinned.
//!
//! Pointer motion reads bounded Arrangement row or painted Console page geometry;
//! it never clones the project, prepares audio or moves a track. Release sends one source-stamped worker request. The final insertion
//! line or group target stays owned by its exact receipt until canonical installation.
//! A name press remains a normal Click candidate until the existing drag threshold.
//! Only moved input claims the Window lease; pending receipt feedback never renews it.
use super::{
    EditorMode, EditorView,
    console_columns::ColumnFrame,
    console_track_page::TrackPage,
    pending_edit::{Disposition, Receipt},
};
use crate::theme;
use gpui::{
    Context, Div, MouseButton, MouseMoveEvent, MouseUpEvent, ScrollWheelEvent, Window, canvas, div,
    prelude::*, px, rgb,
};
use hex_project::{TrackDestination, TrackId, TrackSelectionIds};
use hex_session::service::{Event, Request};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

mod subtree;

#[derive(Default)]
pub(super) struct TrackReorder {
    gesture: Option<Gesture>,
    pending: Option<Pending>,
    painted_gap: Option<PaintedGap>,
    console_layout: Option<ConsoleLayout>,
}
#[derive(Clone, PartialEq)]
struct ConsoleLayout {
    frame: ColumnFrame,
    bounds: gpui::Bounds<gpui::Pixels>,
    page: Arc<TrackPage>,
    column_width: f32,
    scene_width: f32,
}
struct Gesture {
    mode: EditorMode,
    console: Option<ConsoleLayout>,
    pending_page: Option<usize>,
    generation: u64,
    revision: u64,
    track: TrackId,
    tracks: TrackSelectionIds,
    source_rows: subtree::SourceRows,
    origin: (f32, f32),
    pointer: (f32, f32),
    moved: bool,
    destination: Option<Destination>,
    last_scroll: Instant,
}
impl Gesture {
    fn crosses_threshold(&self, pointer: (f32, f32)) -> bool {
        (pointer.0 - self.origin.0)
            .abs()
            .max((pointer.1 - self.origin.1).abs())
            >= 4.0
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Destination {
    Gap(Option<TrackId>),
    Group(TrackId),
}
#[derive(Clone, PartialEq)]
struct PaintedGap {
    generation: u64,
    revision: u64,
    track: TrackId,
    destination: Destination,
    scroll: (usize, f32),
    console: Option<ConsoleLayout>,
}
struct Pending {
    mode: EditorMode,
    console: Option<ConsoleLayout>,
    generation: u64,
    revision: u64,
    tracks: TrackSelectionIds,
    destination: Destination,
    receipt: Receipt,
}
impl EditorView {
    pub(super) fn pending_console_reorder_page(&self) -> Option<Arc<TrackPage>> {
        self.track_reorder
            .pending
            .as_ref()?
            .console
            .as_ref()
            .map(|layout| layout.page.clone())
    }
    pub(super) fn track_reorder_active(&self) -> bool {
        self.track_reorder.gesture.is_some() || self.track_reorder.pending.is_some()
    }
    pub(super) fn begin_track_reorder(
        &mut self,
        track: TrackId,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.drag_available()
            || self.edit_pending()
            || self.track_reorder_active()
            || self.track_name.is_editing()
            || self.context_menu.is_some()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.time_range_move_gesture.is_some()
            || self.time_selection_gesture.is_some()
            || self.gain_lane.draft.is_some()
            || self.sections.naming()
            || self.tuning.open
            || self.theme_settings.open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.changelog.is_open()
        {
            return;
        }
        let console = if self.mode == EditorMode::Session {
            let Some(layout) = self
                .track_reorder
                .console_layout
                .as_ref()
                .filter(|layout| layout.frame.current(self, window))
            else {
                return;
            };
            if !layout.bounds.contains(&position)
                || !layout.page.columns.iter().any(|column| column.id == track)
            {
                return;
            }
            Some(layout.clone())
        } else {
            None
        };
        let Some(state) = &self.state else {
            return;
        };
        if state.project.track(track).is_none() {
            return;
        }
        let revision = state.revision;
        let pointer = (f32::from(position.x), f32::from(position.y));
        if !pointer.0.is_finite() || !pointer.1.is_finite() {
            return;
        }
        // A drag freezes current membership only when its anchor is selected.
        // Stationary clicks and modifier selection keep the existing header
        // Click owner; an unselected anchor still moves alone.
        let members = if self.track_selection.contains(track) {
            self.selected_track_ids()
        } else {
            vec![track]
        };
        let tracks = match TrackSelectionIds::new(&members) {
            Ok(tracks) => tracks,
            Err(error) => {
                self.error = Some(error.0.into());
                cx.notify();
                return;
            }
        };
        let source_rows = match subtree::SourceRows::capture(&state.project, &tracks) {
            Ok(rows) => rows,
            Err(error) => {
                self.error = Some(error.0.into());
                cx.notify();
                return;
            }
        };
        self.track_header_focus(track, cx).focus(window);
        self.track_reorder.painted_gap = None;
        self.track_reorder.gesture = Some(Gesture {
            mode: self.mode,
            console,
            pending_page: None,
            generation: self.project_generation,
            revision,
            track,
            tracks,
            source_rows,
            origin: pointer,
            pointer,
            moved: false,
            destination: None,
            last_scroll: Instant::now(),
        });
    }
    fn capture_track_reorder(&self, window: &mut Window, cx: &Context<Self>) -> bool {
        let Some(g) = self.track_reorder.gesture.as_ref() else {
            return false;
        };
        let generation = g.generation;
        let revision = g.revision;
        let track = g.track;
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            gpui::CursorStyle::ClosedHand,
            move |event, window, cx| {
                let moved = event.downcast_ref::<MouseMoveEvent>();
                let released = event
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|e| e.button == MouseButton::Left);
                let wheel = event.downcast_ref::<ScrollWheelEvent>();
                let position = moved
                    .map(|e| e.position)
                    .or_else(|| released.map(|e| e.position));
                if position.is_none() && wheel.is_none() {
                    return;
                }
                let _ = owner.update(cx, |view, cx| {
                    if !view.track_reorder.gesture.as_ref().is_some_and(|g| {
                        g.generation == generation && g.revision == revision && g.track == track
                    }) {
                        return;
                    }
                    if !view.reorder_header_focused(window) || !view.console_gesture_current(window)
                    {
                        view.cancel_track_reorder();
                    } else if let Some(wheel) = wheel {
                        if view.scroll_track_reorder(wheel) {
                            cx.notify();
                        }
                        return;
                    } else if let Some(position) = position {
                        if released.is_some() {
                            view.finish_track_reorder(position, cx);
                        } else {
                            view.update_track_reorder(position);
                        }
                    }
                    cx.notify();
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.track_reorder.gesture.as_ref().is_some_and(|g| {
                        g.generation == generation && g.revision == revision && g.track == track
                    }) {
                        view.cancel_track_reorder();
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    /// The normal header list wheel remains gesture-owned. Do not deliver it
    /// to underlying scalar controls or the timeline's unrelated zoom handler.
    fn scroll_track_reorder(&mut self, event: &ScrollWheelEvent) -> bool {
        let Some(g) = self.track_reorder.gesture.as_ref() else {
            return false;
        };
        if !self.track_reorder_current(g) {
            return self.cancel_track_reorder();
        }
        if g.mode != EditorMode::Arrangement {
            return false;
        }
        let Some(bounds) = self.arrangement_bounds else {
            return false;
        };
        let x = f32::from(event.position.x);
        let y = f32::from(event.position.y);
        let header_left = bounds.left + (bounds.width - self.track_header_width()).max(0.);
        if !g.moved
            || !x.is_finite()
            || !y.is_finite()
            || x < header_left
            || x > bounds.left + bounds.width
            || y < bounds.top
            || y > bounds.top + bounds.height
        {
            return false;
        }
        // Match GPUI List's normal logical wheel line unit. The existing
        // TrackScroll scrollbar endpoint retains variable-row mapping/clamps.
        let delta = f32::from(event.delta.pixel_delta(px(20.)).y);
        let offset = self.track_scroll.offset();
        let maximum = f32::from(self.track_scroll.max_offset().height);
        if !delta.is_finite()
            || !maximum.is_finite()
            || maximum < 0.
            || !f32::from(offset.y).is_finite()
        {
            return false;
        }
        let next = (f32::from(offset.y) + delta).clamp(-maximum, 0.);
        if next == f32::from(offset.y) {
            return false;
        }
        let pointer = g.pointer;
        self.track_scroll
            .set_offset(gpui::point(offset.x, px(next)));
        self.drag_layout_generation = self.drag_layout_generation.wrapping_add(1);
        self.update_track_reorder(gpui::point(px(pointer.0), px(pointer.1)));
        true
    }
    fn track_reorder_current(&self, g: &Gesture) -> bool {
        self.mode == g.mode
            && g.console.as_ref().is_none_or(|layout| {
                let frame_current = match g.pending_page {
                    Some(page) => layout.frame.current_transition_view(self, page),
                    None => layout.frame.current_view(self),
                };
                frame_current && self.track_reorder.console_layout.as_ref() == Some(layout)
            })
            && g.generation == self.project_generation
            && !self.edit_pending()
            // The immutable revision fences every frozen member; pointer
            // motion need not rescan the selected group.
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.revision == g.revision && s.project.track(g.track).is_some())
    }
    fn reorder_header_focused(&self, window: &Window) -> bool {
        matches!(
            self.focused_header_target(window),
            Some(super::track_name::Target::Track(_))
        )
    }
    fn console_gesture_current(&self, window: &Window) -> bool {
        self.track_reorder.gesture.as_ref().is_none_or(|g| {
            g.console
                .as_ref()
                .is_none_or(|layout| match g.pending_page {
                    Some(page) => layout.frame.current_transition(self, window, page),
                    None => layout.frame.current(self, window),
                })
        })
    }
    pub(super) fn cancel_track_reorder(&mut self) -> bool {
        self.track_reorder.gesture.take().is_some()
    }
    pub(super) fn reconcile_track_reorder(&mut self, window: &Window) {
        if self.track_reorder.gesture.as_ref().is_some_and(|g| {
            !self.track_reorder_current(g)
                || !self.reorder_header_focused(window)
                || !self.console_gesture_current(window)
        }) {
            self.cancel_track_reorder();
        } else if let Some(g) = &self.track_reorder.gesture
            && g.pending_page.is_none()
        {
            // Wheel/scrollbar motion can change list geometry without a pointer
            // move. The painted gap and release hit test share this mapping.
            self.update_track_reorder(gpui::point(px(g.pointer.0), px(g.pointer.1)));
        }
    }
    fn track_reorder_destination(
        &self,
        pointer: (f32, f32),
        source: &subtree::SourceRows,
    ) -> Option<Destination> {
        let bounds = self.arrangement_bounds?;
        let (x, y) = pointer;
        if !x.is_finite()
            || !y.is_finite()
            || x < bounds.left
            || x > bounds.left + bounds.width
            || y < bounds.top
            || y > bounds.top + bounds.height
        {
            return None;
        }
        let tracks = self.state.as_ref()?.project.tracks();
        let header_left = bounds.left + (bounds.width - self.track_header_width()).max(0.0);
        let top = self.track_scroll.list.logical_scroll_top();
        let mut row_y = bounds.top - f32::from(top.offset_in_item);
        let mut index = top.item_ix;
        for (canonical_index, track) in self
            .arrangement_tracks(&self.state.as_ref()?.project)
            .skip(index)
        {
            let height = self.track_height(track.id());
            if track.is_group()
                && x >= header_left
                && row_y >= bounds.top
                && row_y + height <= bounds.top + bounds.height
                && y >= row_y + 4.0
                && y < row_y + height * 0.75
            {
                return source
                    .permits_group(canonical_index, track.id())
                    .then_some(Destination::Group(track.id()));
            }
            if y < row_y + height / 2.0 {
                break;
            }
            row_y += height;
            index += 1;
        }
        let index = self.arrangement_track_index(index).unwrap_or(tracks.len());
        let destination_parent = tracks.get(index).and_then(|track| track.parent_group());
        if !source.permits(index, destination_parent) {
            return None;
        }
        if row_y < bounds.top || row_y > bounds.top + bounds.height {
            return None;
        }
        Some(Destination::Gap(tracks.get(index).map(|t| t.id())))
    }

    fn console_reorder_destination(
        &self,
        pointer: (f32, f32),
        source: &subtree::SourceRows,
        layout: &ConsoleLayout,
    ) -> Option<Destination> {
        let (x, y) = pointer;
        let left = f32::from(layout.bounds.origin.x);
        let top = f32::from(layout.bounds.origin.y);
        let width = f32::from(layout.bounds.size.width);
        let height = f32::from(layout.bounds.size.height);
        let end = left + (width - layout.scene_width).max(0.0);
        if !x.is_finite() || !y.is_finite() || x < left || x > end || y < top || y > top + height {
            return None;
        }
        let local = x - left;
        let index = (local / layout.column_width).floor() as usize;
        let project = &self.state.as_ref()?.project;
        if let Some(column) = layout.page.columns.get(index) {
            let track = project
                .tracks()
                .get(column.index)
                .filter(|t| t.id() == column.id)?;
            let within = local - index as f32 * layout.column_width;
            if track.is_group() && within >= 12.0 && within < layout.column_width - 12.0 {
                return source
                    .permits_group(column.index, column.id)
                    .then_some(Destination::Group(column.id));
            }
        }
        let boundary = if let Some(column) = layout.page.columns.get(index) {
            if local - index as f32 * layout.column_width < layout.column_width / 2.0 {
                Some(column.id)
            } else {
                layout
                    .page
                    .columns
                    .get(index + 1)
                    .map(|column| column.id)
                    .or(layout.page.next)
            }
        } else {
            layout.page.next
        };
        let canonical_index = match boundary {
            Some(id) => project.tracks().iter().position(|track| track.id() == id)?,
            None => project.tracks().len(),
        };
        let parent = project
            .tracks()
            .get(canonical_index)
            .and_then(|track| track.parent_group());
        source
            .permits(canonical_index, parent)
            .then_some(Destination::Gap(boundary))
    }

    /// Paint the exact Console target and capture its page/geometry in the same frame.
    pub(super) fn console_reorder_layer(
        &self,
        page: Arc<TrackPage>,
        frame: ColumnFrame,
        column_width: f32,
        scene_width: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let destination = self
            .track_reorder
            .pending
            .as_ref()
            .filter(|p| p.mode == EditorMode::Session)
            .map(|p| p.destination)
            .or_else(|| {
                self.track_reorder
                    .gesture
                    .as_ref()
                    .filter(|g| g.mode == EditorMode::Session)?
                    .destination
            });
        let drawn = destination.and_then(|destination| {
            let id = match destination {
                Destination::Gap(id) => id,
                Destination::Group(id) => Some(id),
            };
            let index = match id {
                Some(id) => page
                    .columns
                    .iter()
                    .position(|column| column.id == id)
                    .or_else(|| (page.next == Some(id)).then_some(page.columns.len()))?,
                None if page.next.is_none() => page.columns.len(),
                None => return None,
            };
            Some((destination, index))
        });
        let visual = drawn.map(|(destination, index)| {
            let base = div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(index as f32 * column_width));
            match destination {
                Destination::Gap(_) => base
                    .debug_selector(|| "console-track-insertion-line".into())
                    .w(px(2.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
                Destination::Group(_) => base
                    .debug_selector(|| "console-track-group-target".into())
                    .w(px(column_width))
                    .border_2()
                    .border_color(rgb(theme::color(theme::PaletteRole::Secondary))),
            }
        });
        let dragging = self
            .track_reorder
            .gesture
            .as_ref()
            .is_some_and(|g| g.mode == EditorMode::Session && g.moved);
        let previous = (dragging && page.range.start > 0).then(|| {
            div()
                .debug_selector(|| "console-track-page-previous-cue".into())
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(theme::color(theme::PaletteRole::Secondary)))
                .child(theme::text("‹"))
        });
        let next = (dragging
            && page.range.end < self.arrangement_focus.projection.console_indices.len())
        .then(|| {
            div()
                .debug_selector(|| "console-track-page-next-cue".into())
                .absolute()
                .right(px(scene_width))
                .top_0()
                .bottom_0()
                .w(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(theme::color(theme::PaletteRole::Secondary)))
                .child(theme::text("›"))
        });
        let entity = cx.entity();
        div()
            .absolute()
            .inset_0()
            .children(visual)
            .children(previous)
            .children(next)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        entity.update(cx, |view, cx| {
                            let layout = ConsoleLayout {
                                frame,
                                bounds,
                                page: page.clone(),
                                column_width,
                                scene_width,
                            };
                            let transition = view
                                .track_reorder
                                .gesture
                                .as_ref()
                                .and_then(|g| g.pending_page);
                            if let Some(expected_page) = transition {
                                let accepted = view
                                    .track_reorder
                                    .gesture
                                    .as_ref()
                                    .and_then(|g| g.console.as_ref())
                                    .is_some_and(|old| {
                                        old.frame.current_transition(view, window, expected_page)
                                            && frame.current(view, window)
                                            && old.bounds == bounds
                                            && old.column_width == column_width
                                            && old.scene_width == scene_width
                                    });
                                view.track_reorder.console_layout = Some(layout.clone());
                                view.track_reorder.painted_gap = None;
                                if !accepted {
                                    view.cancel_track_reorder();
                                } else if let Some(g) = view.track_reorder.gesture.as_mut() {
                                    g.console = Some(layout);
                                    g.pending_page = None;
                                    let pointer = g.pointer;
                                    view.update_track_reorder(gpui::point(
                                        px(pointer.0),
                                        px(pointer.1),
                                    ));
                                }
                                cx.notify();
                                return;
                            }
                            view.track_reorder.console_layout = Some(layout);
                            view.track_reorder.painted_gap = view
                                .track_reorder
                                .gesture
                                .as_ref()
                                .filter(|g| {
                                    g.mode == EditorMode::Session
                                        && drawn.is_some_and(|(destination, _)| {
                                            g.destination == Some(destination)
                                        })
                                })
                                .and_then(|g| view.track_gap_witness(g));
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
    }

    fn update_track_reorder(&mut self, position: gpui::Point<gpui::Pixels>) -> bool {
        let Some(mut g) = self.track_reorder.gesture.take() else {
            return false;
        };
        let pointer = (f32::from(position.x), f32::from(position.y));
        if !pointer.0.is_finite() || !pointer.1.is_finite() || !self.track_reorder_current(&g) {
            return true;
        }
        g.pointer = pointer;
        g.moved |= g.crosses_threshold(pointer);
        if g.pending_page.is_some() {
            g.destination = None;
            self.track_reorder.gesture = Some(g);
            return true;
        }
        g.destination = g
            .moved
            .then(|| {
                if let Some(layout) = &g.console {
                    self.console_reorder_destination(g.pointer, &g.source_rows, layout)
                } else {
                    self.track_reorder_destination(g.pointer, &g.source_rows)
                }
            })
            .flatten();
        self.track_reorder.gesture = Some(g);
        true
    }
    fn track_gap_witness(&self, g: &Gesture) -> Option<PaintedGap> {
        let scroll = if g.console.is_some() {
            (0, 0.0)
        } else {
            let top = self.track_scroll.list.logical_scroll_top();
            (top.item_ix, f32::from(top.offset_in_item))
        };
        if g.console.is_some() && self.track_reorder.console_layout != g.console {
            return None;
        }
        Some(PaintedGap {
            generation: g.generation,
            revision: g.revision,
            track: g.track,
            destination: g.destination?,
            scroll,
            console: g.console.clone(),
        })
    }
    pub(super) fn track_reorder_painted(&mut self) {
        self.track_reorder.painted_gap = self
            .track_reorder
            .gesture
            .as_ref()
            .and_then(|g| self.track_gap_witness(g));
    }
    pub(super) fn sidechain_drag(&self) -> Option<(TrackId, gpui::Point<gpui::Pixels>)> {
        let drag = self
            .track_reorder
            .gesture
            .as_ref()
            .filter(|drag| drag.moved)?;
        Some((
            drag.track,
            gpui::point(px(drag.pointer.0), px(drag.pointer.1)),
        ))
    }

    fn finish_track_reorder(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let was_captured = self.track_reorder.gesture.as_ref().is_some_and(|g| g.moved);
        self.update_track_reorder(position);
        let captured = was_captured || self.track_reorder.gesture.as_ref().is_some_and(|g| g.moved);
        let Some(g) = self.track_reorder.gesture.take() else {
            return captured;
        };
        if g.moved && super::device_panel::drop_track_key(self, g.track, position, cx) {
            return true;
        }
        // A wheel/scrollbar or final pointer event can arrive before a repaint.
        // Never commit a different gap than the user actually saw highlighted.
        if g.pending_page.is_some() || self.track_gap_witness(&g) != self.track_reorder.painted_gap
        {
            return captured;
        }
        if let Some(destination) = g.destination {
            self.track_reorder.pending = Some(Pending {
                mode: g.mode,
                console: g.console.clone(),
                generation: g.generation,
                revision: g.revision,
                tracks: g.tracks.clone(),
                destination,
                receipt: Receipt::new(g.generation),
            });
            let request = match destination {
                Destination::Gap(before) => Request::ReorderTracks {
                    project_generation: g.generation,
                    revision: g.revision,
                    tracks: g.tracks,
                    before,
                },
                Destination::Group(group) => Request::ReparentTracks {
                    project_generation: g.generation,
                    revision: g.revision,
                    tracks: g.tracks,
                    destination: TrackDestination {
                        parent_group: Some(group),
                        before: None,
                    },
                },
            };
            if !self.enqueue(request) {
                self.retire_pending_track_reorder();
            }
        }
        captured
    }
    pub(super) fn tick_track_reorder(&mut self, now: Instant) -> bool {
        let Some(g) = self.track_reorder.gesture.as_ref() else {
            return false;
        };
        if !self.track_reorder_current(g) {
            return self.cancel_track_reorder();
        }
        if g.mode == EditorMode::Session {
            return self.tick_console_reorder(now);
        }
        let Some(b) = self.arrangement_bounds else {
            return false;
        };
        let (x, y) = g.pointer;
        if !g.moved
            || x < b.left
            || x > b.left + b.width
            || y < b.top
            || y > b.top + b.height
            || now.saturating_duration_since(g.last_scroll) < Duration::from_millis(50)
        {
            return false;
        }
        let direction = if y < b.top + 24.0 {
            -1.0
        } else if y > b.top + b.height - 24.0 {
            1.0
        } else {
            return false;
        };
        let pointer = gpui::point(px(x), px(y));
        let offset = self.track_scroll.offset();
        let next_y = (f32::from(offset.y) - direction * 24.0)
            .clamp(-f32::from(self.track_scroll.max_offset().height), 0.0);
        if let Some(g) = self.track_reorder.gesture.as_mut() {
            g.last_scroll = now;
        }
        if next_y == f32::from(offset.y) {
            return false;
        }
        self.track_scroll
            .set_offset(gpui::point(offset.x, px(next_y)));
        self.drag_layout_generation = self.drag_layout_generation.wrapping_add(1);
        if let Some(g) = self.track_reorder.gesture.as_mut() {
            g.last_scroll = now;
        }
        self.update_track_reorder(pointer);
        true
    }
    /// Turn one Console page after a held edge; no destination exists until
    /// the new page and its matching target have both painted.
    fn tick_console_reorder(&mut self, now: Instant) -> bool {
        let Some(g) = self.track_reorder.gesture.as_ref() else {
            return false;
        };
        let Some(layout) = g.console.as_ref() else {
            return false;
        };
        if !g.moved
            || g.pending_page.is_some()
            || now.saturating_duration_since(g.last_scroll) < Duration::from_millis(450)
        {
            return false;
        }
        let left = f32::from(layout.bounds.origin.x);
        let top = f32::from(layout.bounds.origin.y);
        let end = left + (f32::from(layout.bounds.size.width) - layout.scene_width).max(0.0);
        let bottom = top + f32::from(layout.bounds.size.height);
        let (x, y) = g.pointer;
        if x < left || x > end || y < top || y > bottom {
            return false;
        }
        let size = self.session_visible_columns.max(1);
        let maximum = self
            .arrangement_focus
            .projection
            .console_indices
            .len()
            .saturating_sub(size);
        let current = layout.page.range.start;
        let next = if x <= left + 24.0 {
            current.saturating_sub(size)
        } else if x >= end - 24.0 {
            current.saturating_add(size).min(maximum)
        } else {
            return false;
        };
        if next == current {
            return false;
        }
        if let Some(g) = self.track_reorder.gesture.as_mut() {
            g.pending_page = Some(next);
            g.last_scroll = now;
            g.destination = None;
        }
        self.track_reorder.painted_gap = None;
        self.session_track_start = next;
        true
    }

    pub(super) fn track_reorder_line(&self) -> Option<Div> {
        let destination = self
            .track_reorder
            .pending
            .as_ref()
            .filter(|p| p.mode == EditorMode::Arrangement)
            .map(|p| p.destination)
            .or_else(|| {
                self.track_reorder
                    .gesture
                    .as_ref()
                    .filter(|g| g.mode == EditorMode::Arrangement)?
                    .destination
            })?;
        let bounds = self.arrangement_bounds?;
        let state = self.state.as_ref()?;
        let tracks = self.arrangement_track_indices();
        let target = match destination {
            Destination::Gap(before) => before,
            Destination::Group(group) => Some(group),
        };
        let index = match target {
            Some(id) => tracks
                .iter()
                .position(|i| state.project.tracks().get(*i).is_some_and(|t| t.id() == id))?,
            None => tracks.len(),
        };
        let top = self.track_scroll.list.logical_scroll_top();
        let mut y = -f32::from(top.offset_in_item);
        if index >= top.item_ix {
            for track in tracks.iter().skip(top.item_ix).take(index - top.item_ix) {
                y += self.track_height(state.project.tracks().get(*track)?.id());
            }
        } else {
            for track in tracks.iter().skip(index).take(top.item_ix - index) {
                y -= self.track_height(state.project.tracks().get(*track)?.id());
            }
        }
        if y < 0.0 || y > bounds.height {
            return None;
        }
        if let Destination::Group(group) = destination {
            return Some(
                div()
                    .debug_selector(|| "track-group-drop-target".into())
                    .absolute()
                    .right_0()
                    .w(px(self.track_header_width()))
                    .top(px(y))
                    .h(px(self.track_height(group)))
                    .border_2()
                    .border_color(rgb(theme::color(theme::PaletteRole::Secondary))),
            );
        }
        Some(
            div()
                .debug_selector(|| "track-insertion-line".into())
                .absolute()
                .left_0()
                .right_0()
                .top(px(y.max(1.0) - 1.0))
                .h(px(2.0))
                .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
        )
    }
    pub(super) fn track_reorder_events(&self, cx: &mut Context<Self>) -> gpui::Canvas<()> {
        let entity = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, cx| {
                let view = entity.read(cx);
                if view.reorder_header_focused(window)
                    && let Some(g) = view.track_reorder.gesture.as_ref()
                    && view.mode == g.mode
                    && view.console_gesture_current(window)
                    && g.moved
                    && g.generation == view.project_generation
                    && view
                        .state
                        .as_ref()
                        .is_some_and(|s| s.revision == g.revision)
                {
                    window.renew_pointer_capture(entity.entity_id(), gpui::CursorStyle::ClosedHand);
                }
                let moved = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() {
                        return;
                    }
                    moved.update(cx, |v, cx| {
                        let pointer = (f32::from(event.position.x), f32::from(event.position.y));
                        let activates = event.pressed_button == Some(MouseButton::Left)
                            && pointer.0.is_finite()
                            && pointer.1.is_finite()
                            && v.track_reorder
                                .gesture
                                .as_ref()
                                .is_some_and(|g| !g.moved && g.crosses_threshold(pointer));
                        if activates && !v.capture_track_reorder(window, cx) {
                            // Another owner retained the pointer; preserve its event.
                            if v.cancel_track_reorder() {
                                cx.notify();
                            }
                            return;
                        }
                        let changed = if !v.reorder_header_focused(window)
                            || !v.console_gesture_current(window)
                        {
                            v.cancel_track_reorder()
                        } else if event.pressed_button == Some(MouseButton::Left) {
                            v.update_track_reorder(event.position)
                        } else {
                            v.cancel_track_reorder()
                        };
                        if changed {
                            cx.notify();
                            cx.stop_propagation();
                        }
                    });
                });
                let up = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.capture() || event.button != MouseButton::Left {
                        return;
                    }
                    up.update(cx, |v, cx| {
                        let candidate = v.track_reorder.gesture.is_some();
                        let captured = if v.console_gesture_current(window) {
                            v.finish_track_reorder(event.position, cx)
                        } else {
                            v.cancel_track_reorder()
                        };
                        if candidate {
                            // Publish availability when the Click candidate ends,
                            // including when release falls outside its header.
                            cx.notify();
                        }
                        if captured {
                            cx.stop_propagation();
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0()
    }
    pub(super) fn pending_track_reorder_matches(&self, request: &Request) -> bool {
        let Some(p) = &self.track_reorder.pending else {
            return false;
        };
        if !p.receipt.is_queued() {
            return false;
        }
        match (request, p.destination) {
            (
                Request::ReorderTracks {
                    project_generation,
                    revision,
                    tracks,
                    before,
                },
                Destination::Gap(target),
            ) => {
                *project_generation == p.generation
                    && *revision == p.revision
                    && tracks == &p.tracks
                    && *before == target
            }
            (
                Request::ReparentTracks {
                    project_generation,
                    revision,
                    tracks,
                    destination,
                },
                Destination::Group(group),
            ) => {
                *project_generation == p.generation
                    && *revision == p.revision
                    && tracks == &p.tracks
                    && *destination
                        == TrackDestination {
                            parent_group: Some(group),
                            before: None,
                        }
            }
            _ => false,
        }
    }
    pub(super) fn track_reorder_submitted(&mut self) {
        if let Some(p) = self.track_reorder.pending.as_mut() {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn retire_pending_track_reorder(&mut self) -> bool {
        self.track_reorder.pending.take().is_some()
    }
    pub(super) fn track_reorder_complete(&mut self, event: &Event) {
        if self.track_reorder.pending.as_mut().is_some_and(|p| {
            p.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        }) {
            self.retire_pending_track_reorder();
        }
        if event.project_replaced
            || self
                .track_reorder
                .gesture
                .as_ref()
                .is_some_and(|g| g.revision != event.state.revision)
        {
            self.cancel_track_reorder();
        }
    }
}

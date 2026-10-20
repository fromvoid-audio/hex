//! Shared painted Arrangement destinations; catalog drops remain worker-owned edits.
use super::{
    EditorMode, EditorView,
    drag::{LaneBounds, NativeDeviceDrag, NativeDeviceDragSource},
};
use crate::theme;
use gpui::{Context, Div, Window, div, prelude::*, px, rgb};
use hex_session::service::{ArrangementDeviceDestination, Request};
use hex_session::snapshot_exports::SnapshotTransportError;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ArrangementDropTarget {
    ExistingTrack {
        track: hex_project::TrackId,
        lane: LaneBounds,
    },
    NewTrack {
        before_track: Option<hex_project::TrackId>,
        lane: LaneBounds,
        line: f32,
    },
}
#[derive(Clone, Copy, Debug)]
pub(super) struct ArrangementDevicePreview {
    pub(super) destination: ArrangementDeviceDestination,
    target: ArrangementDropTarget,
}
impl EditorView {
    pub(super) fn painted_arrangement_drop_target(
        &self,
        x: f32,
        y: f32,
    ) -> Option<ArrangementDropTarget> {
        self.painted_arrangement_drop_target_checked(x, y)
            .ok()
            .flatten()
    }
    fn painted_arrangement_drop_target_checked(
        &self,
        x: f32,
        y: f32,
    ) -> Result<Option<ArrangementDropTarget>, SnapshotTransportError> {
        if let Some((before_track, lane, line)) =
            self.painted_track_insertion_target_checked(x, y)?
        {
            return Ok(Some(ArrangementDropTarget::NewTrack {
                before_track,
                lane,
                line,
            }));
        }
        let Some((track, lane)) = self.hovered_drag_target(x, y) else {
            return Ok(None);
        };
        let Some(state) = self.state.as_ref() else {
            return Ok(None);
        };
        state.read_with(|snapshot| {
            snapshot
                .model()
                .track(track)
                .map(|_| ArrangementDropTarget::ExistingTrack { track, lane })
        })
    }
    pub(super) fn update_arrangement_device_drop(
        &mut self,
        drag: &NativeDeviceDrag,
        x: f32,
        y: f32,
    ) {
        if !self.arrangement_device_drag_owned(drag) {
            return;
        }
        let Ok(preview) = self.arrangement_device_drop_candidate(drag, x, y) else {
            return;
        };
        self.arrangement_device_preview = preview;
        if preview.is_some() {
            drag.destination.set(None);
        }
    }
    fn arrangement_device_drag_owned(&self, drag: &NativeDeviceDrag) -> bool {
        self.active_native_device_drag
            .as_ref()
            .is_some_and(|active| {
                std::rc::Rc::ptr_eq(&active.destination, &drag.destination)
                    && active.source == drag.source
                    && active.revision == drag.revision
                    && active.project_generation == drag.project_generation
            })
    }
    fn arrangement_device_drop_candidate(
        &self,
        drag: &NativeDeviceDrag,
        x: f32,
        y: f32,
    ) -> Result<Option<ArrangementDevicePreview>, SnapshotTransportError> {
        let owns = self.arrangement_device_drag_owned(drag);
        if !owns
            || !matches!(drag.source, NativeDeviceDragSource::Browser { .. })
            || self.mode != EditorMode::Arrangement
        {
            return Ok(None);
        }
        if !self.native_device_drag_current(drag)? {
            return Ok(None);
        }
        let Some(target) = self.painted_arrangement_drop_target_checked(x, y)? else {
            return Ok(None);
        };
        let destination = match target {
            ArrangementDropTarget::NewTrack { before_track, .. } => {
                ArrangementDeviceDestination::NewTrack { before_track }
            }
            ArrangementDropTarget::ExistingTrack { track, .. } => {
                let Some(state) = self.state.as_ref() else {
                    return Ok(None);
                };
                let instrument = state.read_with(|snapshot| {
                    snapshot
                        .model()
                        .track(track)
                        .and_then(|track| track.instrument())
                        .map(|device| device.id())
                })?;
                ArrangementDeviceDestination::ExistingTrack { track, instrument }
            }
        };
        Ok(Some(ArrangementDevicePreview {
            destination,
            target,
        }))
    }
    pub(super) fn drop_arrangement_device(
        &mut self,
        drag: &NativeDeviceDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.arrangement_device_drag_owned(drag) {
            return;
        }
        let captured = self.arrangement_device_preview;
        let pointer = window.mouse_position();
        let Ok(current) = self.arrangement_device_drop_candidate(
            drag,
            f32::from(pointer.x),
            f32::from(pointer.y),
        ) else {
            return;
        };
        self.arrangement_device_preview = current;
        let valid = captured.is_some_and(|preview| {
            current.is_some_and(|current| {
                current.destination == preview.destination && current.target == preview.target
            })
        }) && !self.save_dialog_open
            && !self.export_dialog_open
            && !self.export_pending
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
            && self.device_gesture_barriers.is_empty()
            && self.device_gesture.snapshot().is_none()
            && !self.device_panel.has_pending_draft(cx);
        let source = drag.source;
        self.clear_native_device_drag(drag);
        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
        if let (true, Some(preview), NativeDeviceDragSource::Browser { class, type_id }) =
            (valid, captured, source)
        {
            if self.enqueue_device_barrier(Request::DropBrowserDeviceOnArrangement {
                project_generation: drag.project_generation,
                revision: drag.revision,
                class,
                type_id,
                destination: preview.destination,
            }) {
                self.error = None;
                self.status = "adding device to arrangement".into();
            }
        } else {
            self.error =
                Some("device drop canceled · target or project changed, or editing is busy".into());
        }
        cx.notify();
    }
    pub(super) fn arrangement_device_drop_preview(&self) -> Option<gpui::Stateful<Div>> {
        let preview = self.arrangement_device_preview?;
        let drag = self.active_native_device_drag.as_ref()?;
        if !self.native_device_drag_is_current(drag) {
            return None;
        }
        let body = self.arrangement_bounds?;
        let (lane, line) = match preview.target {
            ArrangementDropTarget::ExistingTrack { lane, .. } => (lane, None),
            ArrangementDropTarget::NewTrack { lane, line, .. } => (lane, Some(line)),
        };
        let empty = self
            .state
            .as_ref()?
            .read_with(|snapshot| snapshot.model().tracks().len() == 0)
            .ok()?;
        if let Some(line) = line.filter(|_| !empty) {
            return Some(
                div()
                    .id("arrangement-device-drop-preview")
                    .debug_selector(|| "arrangement-device-drop-preview".into())
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(line - body.top))
                    .h(px(2.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
            );
        }

        let (top, height) = (lane.top - body.top, lane.height);
        Some(
            div()
                .id("arrangement-device-drop-preview")
                .debug_selector(|| "arrangement-device-drop-preview".into())
                .absolute()
                .left(px(lane.left - body.left))
                .top(px(top))
                .w(px(lane.width))
                .h(px(height))
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Secondary))),
        )
    }
}

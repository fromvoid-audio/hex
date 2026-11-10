//! Shared painted Arrangement destinations; catalog drops remain worker-owned edits.
use super::{
    EditorMode, EditorView,
    drag::{LaneBounds, NativeDeviceDrag, NativeDeviceDragSource},
};
use crate::theme;
use gpui::{Context, Div, Window, div, prelude::*, px, rgb};
use hex_session::service::{ArrangementDeviceDestination, Request};

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
        if let Some((before_track, lane, line)) = self.painted_track_insertion_target(x, y) {
            return Some(ArrangementDropTarget::NewTrack {
                before_track,
                lane,
                line,
            });
        }
        let (track, lane) = self.hovered_track_drop_target(x, y)?;
        self.state.as_ref()?.project.track(track)?;
        Some(ArrangementDropTarget::ExistingTrack { track, lane })
    }
    pub(super) fn update_arrangement_device_drop(
        &mut self,
        drag: &NativeDeviceDrag,
        x: f32,
        y: f32,
    ) {
        let owns = self
            .active_native_device_drag
            .as_ref()
            .is_some_and(|active| {
                std::rc::Rc::ptr_eq(&active.destination, &drag.destination)
                    && active.source == drag.source
                    && active.revision == drag.revision
                    && active.project_generation == drag.project_generation
            });
        if !owns {
            return;
        }
        self.arrangement_device_preview = None;
        // Existing-device moves retain their rail/card contract. Plugin
        // effects can target a track, while creating a track requires a
        // separate atomic plugin-preparation contract.
        let plugin_effect = match &drag.source {
            NativeDeviceDragSource::Browser { .. } => false,
            NativeDeviceDragSource::PluginEffect(selection)
                if selection.role == hex_plugin_catalog::PluginRole::Effect =>
            {
                true
            }
            _ => return,
        };
        if !self.native_device_drag_is_current(drag) || self.mode != EditorMode::Arrangement {
            return;
        }
        let Some(target) = self.painted_arrangement_drop_target(x, y) else {
            return;
        };
        if plugin_effect && matches!(target, ArrangementDropTarget::NewTrack { .. }) {
            return;
        }
        let destination = match target {
            ArrangementDropTarget::NewTrack { before_track, .. } => {
                ArrangementDeviceDestination::NewTrack { before_track }
            }
            ArrangementDropTarget::ExistingTrack { track, .. } => {
                let instrument = self
                    .state
                    .as_ref()
                    .and_then(|state| state.project.track(track))
                    .and_then(|track| track.instrument())
                    .map(hex_project::Device::id);
                ArrangementDeviceDestination::ExistingTrack { track, instrument }
            }
        };
        self.arrangement_device_preview = Some(ArrangementDevicePreview {
            destination,
            target,
        });
        // Rail/card destinations are distinct from captured Arrangement intent.
        drag.destination.set(None);
    }
    pub(super) fn drop_arrangement_device(
        &mut self,
        drag: &NativeDeviceDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owns = self
            .active_native_device_drag
            .as_ref()
            .is_some_and(|active| {
                std::rc::Rc::ptr_eq(&active.destination, &drag.destination)
                    && active.source == drag.source
                    && active.revision == drag.revision
                    && active.project_generation == drag.project_generation
            });
        let captured = self.arrangement_device_preview;
        let pointer = window.mouse_position();
        let unsupported_new_track = owns
            && self.native_device_drag_is_current(drag)
            && matches!(drag.source, NativeDeviceDragSource::PluginEffect(_))
            && matches!(
                self.painted_arrangement_drop_target(f32::from(pointer.x), f32::from(pointer.y)),
                Some(ArrangementDropTarget::NewTrack { .. })
            );
        self.update_arrangement_device_drop(drag, f32::from(pointer.x), f32::from(pointer.y));
        let current = self.arrangement_device_preview;
        let valid = owns
            && self.native_device_drag_is_current(drag)
            && captured.is_some_and(|preview| {
                current.is_some_and(|current| {
                    current.destination == preview.destination && current.target == preview.target
                })
            })
            && !self.save_dialog_open
            && !self.export_dialog_open
            && !self.export_pending
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
            && self.device_gesture_barriers.is_empty()
            && self.device_gesture.snapshot().is_none()
            && !self.device_panel.has_pending_draft(cx);
        let source = &drag.source;
        self.clear_native_device_drag(drag);
        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
        if let (true, Some(preview), NativeDeviceDragSource::PluginEffect(selection)) =
            (valid, captured, source)
            && let ArrangementDeviceDestination::ExistingTrack { track, .. } = preview.destination
        {
            self.insert_plugin_effect(
                selection,
                hex_session::service::PluginEffectDestination::Track(track),
                None,
                cx,
            );
        } else if let (true, Some(preview), NativeDeviceDragSource::Browser { class, type_id }) =
            (valid, captured, source)
        {
            let gain = match self.device_track_gain(preview.destination) {
                Ok(gain) => gain,
                Err(error) => {
                    self.error = Some(error.into());
                    cx.notify();
                    return;
                }
            };
            if self.enqueue_device_barrier(Request::DropBrowserDeviceOnArrangement {
                project_generation: drag.project_generation,
                revision: drag.revision,
                class: *class,
                type_id: *type_id,
                destination: preview.destination,
                new_track_gain: gain,
            }) {
                self.error = None;
                self.status = "adding device to arrangement".into();
            }
        } else {
            self.error = Some(
                if unsupported_new_track {
                    "creating a track with a vst3 is not supported yet"
                } else {
                    "device drop canceled · target or project changed, or editing is busy"
                }
                .into(),
            );
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
        let empty = self.state.as_ref()?.project.tracks().is_empty();
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
